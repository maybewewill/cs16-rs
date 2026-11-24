//! Buy zones and the purchase of equipment, ported from ReGameDLL_CS (`triggers.cpp`
//! `CBuyZone`, `player.cpp` `HandleSignals`/`OLD_CheckBuyZone`/`BuyZoneIcon_Set`/
//! `BuyZoneIcon_Clear`/`CanPlayerBuy`/`SendItemStatus`, `client.cpp` `BuyItem`/`BuyAmmo`/
//! `HandleBuyAliasCommands`/the `buy`, `buyequip`, `client_buy_open` and `menuselect` commands,
//! `items.cpp` `CItemKevlar`/`CItemAssaultSuit`, `weapontype.cpp` `g_weaponBuyAliasInfo`, read
//! 2026-11-22), the vanilla `mp.dll` behaviour (not `REGAMEDLL_FIXES`/`REGAMEDLL_ADD`: no
//! `CanPlayerBuy` in the zone touch, no `mp_buytime 0`, no `buy_anywhere`).
//!
//! - A player is in a buy zone when a `func_buyzone` of its team (or of no team, `team 0`)
//!   touches it ([`BuyZone`]); a map with no `func_buyzone` lets the players buy within 200
//!   units of a spawn point of their team (`OLD_CheckBuyZone`). The touch only raises a signal
//!   ([`Signals`], `CUnifiedSignals`): every half second the player's client data update
//!   (`UpdateClientData`, [`handle_signals_when_due`]) turns the signals raised since the last
//!   time into the state, and the `buyzone` status icon follows the changes of it. A dead
//!   player, which touches nothing, leaves the zone the same way. The signals of the objective
//!   zones ([`crate::zones`]: bomb site, hostage rescue, escape, VIP safety) go the same way, with
//!   the icons `c4`, `rescue`, `escape` and `vipsafety`, and the legacy `info_bomb_target` and
//!   `info_hostage_rescue` points for a map that has no such zone entity.
//! - [`can_player_buy`] is `CBasePlayer::CanPlayerBuy`: alive, in a zone, and within
//!   `mp_buytime` minutes (at least 15 seconds, written back to the cvar) of the start of the
//!   round (`m_fRoundStartTime`, which restarts when the freeze period ends). The `#Cant_buy`
//!   message tells the player the seconds.
//! - `buy` and `buyequip` open the buy menus (`ShowVGUIMenu`: the VGUI panel for a client with
//!   `_vgui_menus 1`, else the text menu, answered with `menuselect`, [`menuselect`]);
//!   `client_buy_open` and `client_buy_close` track the client's own menu, which is closed with
//!   `BuyClose` when the zone is left or the player is not in one.
//! - [`buy_item`] is `BuyItem` for the equipment: the vest (650), the vest with helmet (1000, or
//!   the 350 / 650 that make up what is missing), the night vision goggles (1250). The armor
//!   items work as the items `item_kevlar` and `item_assaultsuit` do when touched: 100 armor, the
//!   pickup sound, `ItemPickup`, `ArmorType`; the `Battery` message follows with the next client
//!   data update. Not enough money is `#Not_Enough_Money` and `BlinkAccount`.
//!
//! Weapons (2D): [`buy_weapon_by_id`] is `BuyWeaponByWeaponID` (`client.cpp` 1595): `CanPlayerBuy`,
//! `CanBuyThis` (a weapon the player's class of the map cannot have is `#Cannot_Buy_This`, one it
//! already carries `#Cstrike_Already_Own_Weapon`), the price (`#Not_Enough_Money` and the account
//! blinks), then the weapon the player holds in that slot is dropped (`DropPrimary`,
//! `DropSecondary`) and the new one is given (`GiveNamedItem`) for its price. The ammo is bought
//! a clip at a time ([`buy_ammo`]: `BuyAmmo`, `BuyGunAmmo`) and the grenades through the equipment
//! menu ([`buy_item`]).
//!
//! Deviations and what waits for later tasks:
//! - The shield (`SLOT_ITEM_SHIELD`, $2200) is sold to counter-terrorists (unless carrying Elite),
//!   dropping any primary weapon (`inventory::give_shield`).
//! - `autobuy`, `cl_autobuy`, `cl_setautobuy`, `rebuy`, `cl_rebuy`, `cl_setrebuy` buy equipment
//!   from the player's priority/rebuy lists.
//! - The ammo clip is given as the `ammo_*` entity of the original does it
//!   (`CBasePlayerAmmo::AddAmmo`: `GiveAmmo` of `buyClipSize`, then `items/9mmclip1.wav`); the sound
//!   comes from the player's edict, not from an entity of its own (assumption: the same place).
//! - The defuser (`MENU_SLOT_ITEM_DEFUSEKIT`, price 200) is sold to a counter-terrorist on a map
//!   with a bomb target. The terrorists' `defuser` alias says `#Alias_Not_Avail` as the original.
//! - `HintMessage("#Hint_use_nightvision")` (first night vision purchase) goes through
//!   [`crate::hint`].
//! - `CanPlayerBuy` also refuses the VIP and the team `info_map_parameters` (`buying`) keeps out
//!   of the buy menus ([`crate::zones`]). The tutor is not ported.
//! - A `func_buyzone`'s `angles` (one-way touch, `SetMovedir`) only matter to the generic
//!   trigger code, not to the buy zone, and are cleared without computing a `movedir`.

use cs_engine::consts::{MOVETYPE_NONE, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::chat::client_print;
use crate::entity::GameEntity;
use crate::hint;
use crate::menus::{
    self, HUD_PRINTCENTER, MENU_KEY_0, MENU_KEY_1, MENU_KEY_2, MENU_KEY_3, MENU_KEY_4, MENU_KEY_5,
    MENU_KEY_6, MENU_KEY_7, MENU_KEY_8, Menu, VGUI_MENU_BUY_ITEM,
};
use crate::inventory::{self, PISTOL_SLOT, PRIMARY_WEAPON_SLOT};
use crate::money;
use crate::player::{ATTN_NORM, ArmorType, CHAN_ITEM, DEAD_NO, EF_NODRAW, PITCH_NORM, Team, player_slot};
use crate::weapons::{
    AmmoType, FLASHBANG_PRICE, HEGRENADE_PRICE, SMOKEGRENADE_PRICE, WeaponId, ammo_info_by_name,
    is_primary_weapon, is_secondary_weapon, max_ammo_carry_for_weapon, weapon_info,
};

/// `SIGNAL_BUY` (`unisignals.h`): the player stands in a buy zone.
pub const SIGNAL_BUY: u8 = 1 << 0;
/// `SIGNAL_BOMB`: the player stands in a bomb site (`func_bomb_target`, carrier only).
pub const SIGNAL_BOMB: u8 = 1 << 1;
/// `SIGNAL_RESCUE`: the player stands in a hostage rescue zone.
pub const SIGNAL_RESCUE: u8 = 1 << 2;
/// `SIGNAL_ESCAPE`: the player stands in an escape zone.
pub const SIGNAL_ESCAPE: u8 = 1 << 3;
/// `SIGNAL_VIPSAFETY`: the player stands in a VIP safety zone.
pub const SIGNAL_VIPSAFETY: u8 = 1 << 4;
/// `MIN_BUY_TIME` (`player.h`, vanilla): the least `mp_buytime` allows, in seconds.
pub const MIN_BUY_TIME: i32 = 15;
/// `DEFUSEKIT_PRICE` (`weapontype.h`).
pub const DEFUSEKIT_PRICE: i32 = 200;
/// `SHIELDGUN_PRICE` (`weapontype.h`).
pub const SHIELDGUN_PRICE: i32 = 2200;
/// `KEVLAR_PRICE`, `HELMET_PRICE`, `ASSAULTSUIT_PRICE`, `NVG_PRICE` (`weapontype.h`).
pub const KEVLAR_PRICE: i32 = 650;
pub const HELMET_PRICE: i32 = 350;
pub const ASSAULTSUIT_PRICE: i32 = 1000;
pub const NVG_PRICE: i32 = 1250;
/// `MAX_NORMAL_BATTERY`: the armor a vest gives.
pub const MAX_NORMAL_BATTERY: f32 = 100.0;
/// The distance from a spawn point within which a map without buy zones lets a player buy
/// (`OLD_CheckBuyZone`).
const OLD_BUY_ZONE_RADIUS: f32 = 200.0;
/// Seconds between two runs of `HandleSignals`.
const HANDLE_SIGNALS_INTERVAL: f64 = 0.5;
/// The default `numBlinks` of `BlinkAccount` (`client.h:128`), which `BuyItem` calls it with
/// (`client.cpp:1572`, `1617`). `MONEY_BLINK_AMOUNT` (30) is the tutor's.
const BUY_BLINK_AMOUNT: u8 = 2;
/// `ITEM_STATUS_NIGHTVISION` and `ITEM_STATUS_DEFUSER` (`cdll_dll.h`).
const ITEM_STATUS_NIGHTVISION: u8 = 1 << 0;
const ITEM_STATUS_DEFUSER: u8 = 1 << 1;

/// `VGUI_Menu_Buy`, `VGUI_Menu_Buy_Pistol`, `VGUI_Menu_Buy_ShotGun`, `VGUI_Menu_Buy_Rifle`,
/// `VGUI_Menu_Buy_SubMachineGun`, `VGUI_Menu_Buy_MachineGun` (`cdll_dll.h`); the item menu is
/// [`VGUI_MENU_BUY_ITEM`].
pub const VGUI_MENU_BUY: u8 = 28;
pub const VGUI_MENU_BUY_PISTOL: u8 = 29;
pub const VGUI_MENU_BUY_SHOTGUN: u8 = 30;
pub const VGUI_MENU_BUY_RIFLE: u8 = 31;
pub const VGUI_MENU_BUY_SUBMACHINEGUN: u8 = 32;
pub const VGUI_MENU_BUY_MACHINEGUN: u8 = 33;

/// `VGUI_MenuSlot_Buy_*` (`cdll_dll.h`): the keys of the first buy menu.
const SLOT_BUY_PISTOL: i32 = 1;
const SLOT_BUY_SHOTGUN: i32 = 2;
const SLOT_BUY_SUBMACHINEGUN: i32 = 3;
const SLOT_BUY_RIFLE: i32 = 4;
const SLOT_BUY_MACHINEGUN: i32 = 5;
const SLOT_BUY_PRIMARY_AMMO: i32 = 6;
const SLOT_BUY_SECONDARY_AMMO: i32 = 7;
const SLOT_BUY_ITEM: i32 = 8;

/// `BuyItemMenuSlot` (`client.h`): the keys of the equipment menu.
const SLOT_ITEM_VEST: i32 = 1;
const SLOT_ITEM_VESTHELM: i32 = 2;
const SLOT_ITEM_FLASHGREN: i32 = 3;
const SLOT_ITEM_HEGREN: i32 = 4;
const SLOT_ITEM_SMOKEGREN: i32 = 5;
const SLOT_ITEM_NVG: i32 = 6;
const SLOT_ITEM_DEFUSEKIT: i32 = 7;
const SLOT_ITEM_SHIELD: i32 = 8;

/// `CUnifiedSignals` (`unisignals.h`): the zone signals of a player. A touch raises a signal; the
/// next [`update`](Self::update) makes the raised signals the state and clears them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Signals {
    signal: u8,
    state: u8,
}

impl Signals {
    /// `Signal`: raises `flags`.
    pub fn signal(&mut self, flags: u8) {
        self.signal |= flags;
    }

    /// `Update`: what was raised is now the state.
    pub fn update(&mut self) {
        self.state = self.signal;
        self.signal = 0;
    }

    /// `GetSignal`: what was raised since the last update.
    pub fn pending(&self) -> u8 {
        self.signal
    }

    /// `GetState`: the zones the player was in at the last update.
    pub fn state(&self) -> u8 {
        self.state
    }
}

/// `func_buyzone` (`CBuyZone`): a trigger brush in which the players of its team (any team for
/// `team 0`) may buy.
#[derive(Debug, Default)]
pub struct BuyZone {
    /// `pev->team`: 0 any, 1 terrorists, 2 counter-terrorists.
    team: i32,
}

impl GameEntity for BuyZone {
    fn classname(&self) -> &'static str {
        "func_buyzone"
    }

    /// `CBuyZone::Spawn`: `InitTrigger` (a non-solid trigger of the brush model, invisible unless
    /// `showtriggers` is set), then the team is checked: a number other than 0 to 2 in the map is
    /// reported and means any team.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        // The engine hands the entity's keys on; `team` is the one the zone needs.
        self.team = keys
            .iter()
            .rev()
            .find(|(k, _)| k == "team")
            .map_or(0, |(_, v)| menus::atoi(v));
        let showtriggers = eng.cvar_f32("showtriggers") != 0.0;
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        // `InitTrigger`: angles are for one-way touches (see the module documentation).
        v.angles = Vec3::ZERO;
        v.solid = SOLID_TRIGGER;
        v.movetype = MOVETYPE_NONE;
        let (model, origin) = (v.model.clone(), v.origin);
        if !showtriggers {
            v.effects |= EF_NODRAW;
        }
        if model.is_empty() {
            eng.set_origin(id, origin);
        } else if !eng.set_model(id, &model) {
            tracing::warn!(model = %model, "func_buyzone model could not be set");
        }
        if !(0..=2).contains(&self.team) {
            tracing::warn!(team = self.team, "Bad team number in func_buyzone");
            self.team = 0;
        }
        true
    }

    /// `CBuyZone::BuyTouch`: a player of the zone's team raises [`SIGNAL_BUY`].
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, _id: EdictId, other: EdictId) {
        let Some(slot) = player_slot(g, eng, other) else {
            return;
        };
        if let Some(p) = g.players[slot].as_mut()
            && (self.team == 0 || self.team == i32::from(p.team.id()))
        {
            p.signals.signal(SIGNAL_BUY);
        }
    }
}

// ---- the signals of a player ----

/// `STATUSICON_HIDE`, `STATUSICON_SHOW`, `STATUSICON_FLASH` (`cdll_dll.h`).
const STATUSICON_HIDE: u8 = 0;
const STATUSICON_SHOW: u8 = 1;
const STATUSICON_FLASH: u8 = 2;
/// The colour of every icon the zones show (`WRITE_BYTE(0); WRITE_BYTE(160); WRITE_BYTE(0)`).
const ICON_GREEN: [u8; 3] = [0, 160, 0];
/// `MAX_HOSTAGES_RESCUE_RADIUS` (`hostage.h`): how close to an `info_hostage_rescue` a player
/// is in a rescue zone on a map that has no `func_hostage_rescue`.
const MAX_HOSTAGES_RESCUE_RADIUS: f32 = 256.0;
/// The distance from an `info_bomb_target` within which a map that has no `func_bomb_target`
/// puts a player in a bomb site (`OLD_CheckBombTarget`, `<=`).
const OLD_BOMB_TARGET_RADIUS: f32 = 256.0;

/// `gmsgStatusIcon` of the `*Icon_Set` functions: the `sprite` icon, shown, in green.
fn zone_icon_set(eng: &mut EngineApi<'_>, slot: u8, sprite: &str) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::StatusIcon {
            status: STATUSICON_SHOW,
            sprite_name: sprite.to_string(),
            color: Some(ICON_GREEN),
        },
    );
}

/// The `*Icon_Clear` functions (`BuyZoneIcon_Clear`, `RescueZoneIcon_Clear`,
/// `EscapeZoneIcon_Clear`, `VIP_SafetyZoneIcon_Clear`): the `sprite` icon is hidden. A buy menu
/// the player has open is closed with it: a text or VGUI menu with the `slot10` command (the
/// client's "close"), the client's own menu with `BuyClose`. (The four functions are the same
/// but for the sprite in vanilla; the `REGAMEDLL_FIXES` `BuyClose` for a VGUI menu is not
/// ported.)
fn zone_icon_clear(g: &GameState, eng: &mut EngineApi<'_>, slot: u8, sprite: &str) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::StatusIcon {
            status: STATUSICON_HIDE,
            sprite_name: sprite.to_string(),
            color: None,
        },
    );
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    match p.menu {
        Menu::Buy
        | Menu::BuyPistol
        | Menu::BuyRifle
        | Menu::BuyMachineGun
        | Menu::BuyShotgun
        | Menu::BuySubMachineGun
        | Menu::BuyItem => eng.stufftext(slot, "slot10\n"),
        Menu::ClientBuy => {
            eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::BuyClose);
        }
        Menu::Off
        | Menu::ChooseTeam
        | Menu::ChooseAppearance
        | Menu::Radio1
        | Menu::Radio2
        | Menu::Radio3 => {}
    }
}

/// `BuyZoneIcon_Clear`: the `buyzone` icon is hidden (see [`zone_icon_clear`]).
pub(crate) fn buy_zone_icon_clear(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    zone_icon_clear(g, eng, slot, "buyzone");
}

/// `CBasePlayer::SetBombIcon(bFlash)` (`player.cpp` 1935-1956, read 2026-11-23): the `c4` icon
/// is shown (flashing with `flash`) for the player that carries the bomb, else hidden; then
/// `SetScoreboardAttributes`.
pub(crate) fn set_bomb_icon(g: &GameState, eng: &mut EngineApi<'_>, slot: u8, flash: bool) {
    let has_c4 = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.has_c4);
    let msg = if has_c4 {
        CsUserMsg::StatusIcon {
            status: if flash { STATUSICON_FLASH } else { STATUSICON_SHOW },
            sprite_name: "c4".to_string(),
            color: Some(ICON_GREEN),
        }
    } else {
        CsUserMsg::StatusIcon {
            status: STATUSICON_HIDE,
            sprite_name: "c4".to_string(),
            color: None,
        }
    };
    eng.send_usermsg(MsgDest::One(slot), &msg);
    crate::messages::set_scoreboard_attributes(g, eng, usize::from(slot));
}

/// `BombTargetFlash_Set`: the bomb carrier is told once that it is in a target zone, and its
/// `c4` icon flashes.
fn bomb_target_flash_set(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let carries = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.has_c4);
    if carries && hint::first_time(g, slot, hint::dhf::IN_TARGET_ZONE) {
        hint::hint(g, eng, slot, "#Hint_you_are_in_targetzone");
    }
    set_bomb_icon(g, eng, slot, true);
}

/// `RescueZoneIcon_Set`: the `rescue` icon; a counter-terrorist is told once what the zone is.
fn rescue_zone_icon_set(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    zone_icon_set(eng, slot, "rescue");
    if team_of(g, slot) == Team::Ct && hint::first_time(g, slot, hint::dhf::IN_RESCUE_ZONE) {
        hint::hint(g, eng, slot, "#Hint_hostage_rescue_zone");
    }
}

/// `EscapeZoneIcon_Set`: the `escape` icon; a counter-terrorist is told once what the zone is.
fn escape_zone_icon_set(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    zone_icon_set(eng, slot, "escape");
    if team_of(g, slot) == Team::Ct && hint::first_time(g, slot, hint::dhf::IN_ESCAPE_ZONE) {
        hint::hint(g, eng, slot, "#Hint_terrorist_escape_zone");
    }
}

/// `VIP_SafetyZoneIcon_Set`: the `vipsafety` icon; a player of either team is told once what
/// the zone is (`HintMessage(text, TRUE)`: shown to the dead too, but not past `_ah 0`).
fn vip_safety_zone_icon_set(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    zone_icon_set(eng, slot, "vipsafety");
    let text = match team_of(g, slot) {
        Team::Ct => "#Hint_ct_vip_zone",
        Team::Terrorist => "#Hint_terrorist_vip_zone",
        _ => return,
    };
    if hint::first_time(g, slot, hint::dhf::IN_VIPSAFETY_ZONE) {
        hint::hint_message(g, eng, slot, text, true, false);
    }
}

/// Whether the player of `slot` is at a distance `within` accepts from any entity of `class`.
fn near_any(eng: &EngineApi<'_>, slot: u8, class: &str, within: impl Fn(f32) -> bool) -> bool {
    let Some(origin) = eng.ent(eng.client_edict(slot)).map(|v| v.origin) else {
        return false;
    };
    let mut near = false;
    let mut spot = None;
    while let Some(found) = eng.find_by_classname(spot, class) {
        spot = Some(found);
        if eng
            .ent(found)
            .is_some_and(|v| within((v.origin - origin).length()))
        {
            near = true;
        }
    }
    near
}

/// `OLD_CheckBuyZone`: on a map with no `func_buyzone`, a player of a team that spawns is in a
/// buy zone within 200 units of one of its team's spawn points (the origin of the point, whether
/// or not the player is alive).
fn old_check_buy_zone(g: &mut GameState, eng: &EngineApi<'_>, slot: u8) {
    let class = match team_of(g, slot) {
        Team::Terrorist => crate::world::T_SPAWN_CLASS,
        Team::Ct => crate::world::CT_SPAWN_CLASS,
        _ => return,
    };
    if near_any(eng, slot, class, |d| d < OLD_BUY_ZONE_RADIUS)
        && let Some(p) = g.players[usize::from(slot)].as_mut()
    {
        p.signals.signal(SIGNAL_BUY);
    }
}

/// `OLD_CheckBombTarget` (`player.cpp` 7132-7144, read 2026-11-23): a player within 256 units
/// of an `info_bomb_target` is in a bomb site (any player, not only the carrier).
fn old_check_bomb_target(g: &mut GameState, eng: &EngineApi<'_>, slot: u8) {
    if near_any(eng, slot, "info_bomb_target", |d| d <= OLD_BOMB_TARGET_RADIUS)
        && let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
    {
        p.signals.signal(SIGNAL_BOMB);
    }
}

/// `OLD_CheckRescueZone` (`player.cpp` 7147-7160, read 2026-11-23): a player within
/// [`MAX_HOSTAGES_RESCUE_RADIUS`] of an `info_hostage_rescue` is in a rescue zone.
fn old_check_rescue_zone(g: &mut GameState, eng: &EngineApi<'_>, slot: u8) {
    if near_any(eng, slot, "info_hostage_rescue", |d| d <= MAX_HOSTAGES_RESCUE_RADIUS)
        && let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
    {
        p.signals.signal(SIGNAL_RESCUE);
    }
}

/// `CBasePlayer::HandleSignals` (`player.cpp` 7162-7260, read 2026-11-23, vanilla: no
/// `buy_anywhere`, `plant_c4_anywhere` or `buytime 0`): a map without a `func_buyzone`, a
/// `func_bomb_target` or a `func_hostage_rescue` lets the players use the legacy points
/// (`OLD_CheckBuyZone`, `OLD_CheckBombTarget`, `OLD_CheckRescueZone`), then the raised signals
/// become the state and each change of a signal sets or clears its icon, in the order buy, bomb,
/// rescue, escape, VIP safety.
///
/// The state lags the touch by up to half a second in both directions (the zones touch every
/// frame, this runs every half second): `CC4::PrimaryAttack` reads the state.
fn handle_signals(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if !g.rules.map_has_buy_zone() {
        old_check_buy_zone(g, eng, slot);
    }
    if !g.rules.map_has_bomb_zone() {
        old_check_bomb_target(g, eng, slot);
    }
    if !g.rules.map_has_rescue_zone() {
        old_check_rescue_zone(g, eng, slot);
    }
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let signal = p.signals.pending();
    let changed = p.signals.state() ^ signal;
    p.signals.update();
    let on = |bit: u8| signal & bit != 0;
    if changed & SIGNAL_BUY != 0 {
        if on(SIGNAL_BUY) {
            zone_icon_set(eng, slot, "buyzone");
        } else {
            buy_zone_icon_clear(g, eng, slot);
        }
    }
    if changed & SIGNAL_BOMB != 0 {
        if on(SIGNAL_BOMB) {
            bomb_target_flash_set(g, eng, slot);
        } else {
            // `BombTargetFlash_Clear`.
            set_bomb_icon(g, eng, slot, false);
        }
    }
    if changed & SIGNAL_RESCUE != 0 {
        if on(SIGNAL_RESCUE) {
            rescue_zone_icon_set(g, eng, slot);
        } else {
            zone_icon_clear(g, eng, slot, "rescue");
        }
    }
    if changed & SIGNAL_ESCAPE != 0 {
        if on(SIGNAL_ESCAPE) {
            escape_zone_icon_set(g, eng, slot);
        } else {
            zone_icon_clear(g, eng, slot, "escape");
        }
    }
    if changed & SIGNAL_VIPSAFETY != 0 {
        if on(SIGNAL_VIPSAFETY) {
            vip_safety_zone_icon_set(g, eng, slot);
        } else {
            zone_icon_clear(g, eng, slot, "vipsafety");
        }
    }
}

/// The call `UpdateClientData` makes at its end: `HandleSignals` when `m_tmHandleSignals` has
/// passed, which is every half second.
pub(crate) fn handle_signals_when_due(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if now > p.next_handle_signals {
        p.next_handle_signals = now + HANDLE_SIGNALS_INTERVAL;
        handle_signals(g, eng, slot);
    }
}

// ---- what the player may buy ----

/// `CBasePlayer::CanPlayerBuy`: the player is alive, in a buy zone and within the buy time. With
/// `display`, a player out of time is told so (`#Cant_buy`, with the seconds of the buy time).
pub(crate) fn can_player_buy(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    display: bool,
) -> bool {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    // Is the player alive?
    if eng
        .ent(eng.client_edict(slot))
        .is_none_or(|v| v.deadflag != DEAD_NO)
    {
        return false;
    }
    // Is the player in a buy zone?
    if p.signals.state() & SIGNAL_BUY == 0 {
        return false;
    }
    let mut buy_time = (eng.cvar_f32("mp_buytime") * 60.0) as i32;
    if buy_time < MIN_BUY_TIME {
        buy_time = MIN_BUY_TIME;
        eng.set_cvar("mp_buytime", &(MIN_BUY_TIME as f32 / 60.0).to_string());
    }
    if eng.time() - g.rules.round_start_time() > f64::from(buy_time) {
        if display {
            client_print(
                eng,
                slot,
                HUD_PRINTCENTER,
                "#Cant_buy",
                &[&buy_time.to_string()],
            );
        }
        return false;
    }
    // The VIP buys nothing, and a team the map's `info_map_parameters` keeps out of the buy
    // menus (`buying`) cannot buy (`player.cpp` 4693-4721, read 2026-11-23).
    if p.is_vip {
        if display {
            client_print(eng, slot, HUD_PRINTCENTER, "#VIP_cant_buy", &[]);
        }
        return false;
    }
    if g.rules.ct_cant_buy() && p.team == Team::Ct {
        if display {
            client_print(eng, slot, HUD_PRINTCENTER, "#CT_cant_buy", &[]);
        }
        return false;
    }
    if g.rules.t_cant_buy() && p.team == Team::Terrorist {
        if display {
            client_print(eng, slot, HUD_PRINTCENTER, "#Terrorist_cant_buy", &[]);
        }
        return false;
    }
    true
}

/// `ClientPrint(pev, HUD_PRINTCENTER, text)`.
fn say(eng: &mut EngineApi<'_>, slot: u8, text: &str) {
    client_print(eng, slot, HUD_PRINTCENTER, text, &[]);
}

/// The player is not given what it asked for: `#Not_Enough_Money` and the account blinks.
fn not_enough_money(eng: &mut EngineApi<'_>, slot: u8) {
    say(eng, slot, "#Not_Enough_Money");
    money::blink_account(eng, slot, BUY_BLINK_AMOUNT);
}

/// `SendItemStatus`: what the player carries that the HUD shows (night vision and defuser).
pub(crate) fn send_item_status(
    eng: &mut EngineApi<'_>,
    slot: u8,
    has_night_vision: bool,
    has_defuser: bool,
) {
    let mut status = 0;
    if has_night_vision {
        status |= ITEM_STATUS_NIGHTVISION;
    }
    if has_defuser {
        status |= ITEM_STATUS_DEFUSER;
    }
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::ItemStatus(status));
}

/// `EMIT_SOUND(ENT(pev), CHAN_ITEM, sample, VOL_NORM, ATTN_NORM)`.
fn item_sound(eng: &mut EngineApi<'_>, slot: u8, sample: &str) {
    let id = eng.client_edict(slot);
    eng.emit_sound(id, CHAN_ITEM, sample, 1.0, ATTN_NORM, PITCH_NORM);
}

/// The items a player buys by giving it the class of an item to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArmorItem {
    /// `item_kevlar` (`CItemKevlar::MyTouch`).
    Kevlar,
    /// `item_assaultsuit` (`CItemAssaultSuit::MyTouch`).
    AssaultSuit,
}

/// `GiveNamedItem("item_kevlar")` / `("item_assaultsuit")`: the item is touched by the player at
/// once. The vest sets the armor type only when the player has none (so a player with a helmet
/// keeps it) and always tells the client `ArmorType` 0; the assault suit sets the helmet and
/// tells 1. Both fill the armor to 100.
pub(crate) fn give_armor_item(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    item: ArmorItem,
) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let (class, armor_type) = match item {
        ArmorItem::Kevlar => {
            if p.kevlar == ArmorType::None {
                p.kevlar = ArmorType::Vest;
            }
            ("item_kevlar", 0)
        }
        ArmorItem::AssaultSuit => {
            p.kevlar = ArmorType::VestHelm;
            ("item_assaultsuit", 1)
        }
    };
    p.armor = MAX_NORMAL_BATTERY;
    item_sound(eng, slot, "items/ammopickup2.wav");
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::ItemPickup(class.to_string()),
    );
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::ArmorType(armor_type));
}

/// What `BuyItem` gives for the money: an armor item, or a weapon class (a grenade).
#[derive(Debug, Clone, Copy)]
enum Purchase {
    Armor(ArmorItem),
    Weapon(&'static str),
}

/// `BuyItem` (ReGameDLL `client.cpp`) for the equipment menu slot `item_slot`. The shield is
/// refused (it does not exist yet, see the module documentation).
pub(crate) fn buy_item(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, item_slot: i32) {
    if !can_player_buy(g, eng, slot, true) {
        return;
    }
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let max_slot = if p.team == Team::Ct { 8 } else { 6 };
    if !(1..=max_slot).contains(&item_slot) {
        return;
    }
    let full_armor = p.armor >= MAX_NORMAL_BATTERY;
    let has_helmet = p.kevlar == ArmorType::VestHelm;
    let (account, team) = (p.money, p.team);

    // What is bought, for how much; `None` is not enough money.
    let mut purchase = None;
    match item_slot {
        SLOT_ITEM_VEST => {
            if full_armor {
                say(eng, slot, "#Already_Have_Kevlar");
                return;
            }
            if account >= KEVLAR_PRICE {
                if has_helmet {
                    say(eng, slot, "#Already_Have_Helmet_Bought_Kevlar");
                }
                purchase = Some((Purchase::Armor(ArmorItem::Kevlar), KEVLAR_PRICE));
            }
        }
        SLOT_ITEM_VESTHELM => {
            if full_armor {
                if has_helmet {
                    say(eng, slot, "#Already_Have_Kevlar_Helmet");
                    return;
                }
                if account >= HELMET_PRICE {
                    say(eng, slot, "#Already_Have_Kevlar_Bought_Helmet");
                    purchase = Some((Purchase::Armor(ArmorItem::AssaultSuit), HELMET_PRICE));
                }
            } else if has_helmet {
                if account >= KEVLAR_PRICE {
                    say(eng, slot, "#Already_Have_Helmet_Bought_Kevlar");
                    purchase = Some((Purchase::Armor(ArmorItem::AssaultSuit), KEVLAR_PRICE));
                }
            } else if account >= ASSAULTSUIT_PRICE {
                purchase = Some((Purchase::Armor(ArmorItem::AssaultSuit), ASSAULTSUIT_PRICE));
            }
        }
        SLOT_ITEM_FLASHGREN | SLOT_ITEM_HEGREN | SLOT_ITEM_SMOKEGREN => {
            // `AmmoInventory(AMMO_*) >= MaxAmmoCarry(WEAPON_*)`: no room for another.
            let (ammo, weapon, class, price) = match item_slot {
                SLOT_ITEM_FLASHGREN => (
                    AmmoType::Flashbang,
                    WeaponId::Flashbang,
                    "weapon_flashbang",
                    FLASHBANG_PRICE,
                ),
                SLOT_ITEM_HEGREN => (
                    AmmoType::HeGrenade,
                    WeaponId::HeGrenade,
                    "weapon_hegrenade",
                    HEGRENADE_PRICE,
                ),
                _ => (
                    AmmoType::SmokeGrenade,
                    WeaponId::SmokeGrenade,
                    "weapon_smokegrenade",
                    SMOKEGRENADE_PRICE,
                ),
            };
            let held = g.players[usize::from(slot)]
                .as_ref()
                .and_then(|p| p.ammo.get(ammo as usize).copied())
                .unwrap_or(0);
            if held >= max_ammo_carry_for_weapon(weapon) {
                say(eng, slot, "#Cannot_Carry_Anymore");
                return;
            }
            if account >= price {
                purchase = Some((Purchase::Weapon(class), price));
            }
        }
        SLOT_ITEM_SHIELD => {
            if !can_buy_weapon_by_maptype(team, WeaponId::ShieldGun, g.rules.map_has_vip_safety_zone()) {
                say(eng, slot, "#Cannot_Buy_This");
                return;
            }
            let (owns_shield, has_elite) = g.players[usize::from(slot)]
                .as_ref()
                .map_or((false, false), |p| {
                    (p.owns_shield, inventory::has_player_item(p, WeaponId::Elite))
                });
            if owns_shield {
                say(eng, slot, "#Already_Have_One");
                return;
            }
            if has_elite {
                say(eng, slot, "#Cannot_Buy_This");
                return;
            }
            if account < SHIELDGUN_PRICE {
                not_enough_money(eng, slot);
                return;
            }
            inventory::drop_primary(g, eng, slot);
            inventory::give_shield(g, eng, slot, true);
            money::add_account(g, eng, slot, -SHIELDGUN_PRICE, true);
            snapshot_rebuy(g, slot);
            return;
        }
        SLOT_ITEM_NVG => {
            if g.players[usize::from(slot)]
                .as_ref()
                .is_some_and(|p| p.has_night_vision)
            {
                say(eng, slot, "#Already_Have_One");
                return;
            }
            if account >= NVG_PRICE {
                // The first purchase comes with a hint on how to use the goggles
                // (client.cpp:1494, read 2026-11-22).
                if crate::hint::first_time(g, slot, crate::hint::dhf::NIGHTVISION) {
                    crate::hint::hint(g, eng, slot, "#Hint_use_nightvision");
                }
                item_sound(eng, slot, "items/equip_nvg.wav");
                let has_defuser = if let Some(p) = g.players[usize::from(slot)].as_mut() {
                    p.has_night_vision = true;
                    p.has_defuser
                } else {
                    false
                };
                money::add_account(g, eng, slot, -NVG_PRICE, true);
                send_item_status(eng, slot, true, has_defuser);
                snapshot_rebuy(g, slot);
                return;
            }
        }
        SLOT_ITEM_DEFUSEKIT => {
            if team != Team::Ct || !g.rules.map_has_bomb_target() {
                return;
            }
            if g.players[usize::from(slot)]
                .as_ref()
                .is_some_and(|p| p.has_defuser)
            {
                say(eng, slot, "#Already_Have_One");
                return;
            }
            if account >= DEFUSEKIT_PRICE {
                crate::player::give_defuser(g, eng, slot);
                money::add_account(g, eng, slot, -DEFUSEKIT_PRICE, true);
                // Vanilla (`!REGAMEDLL_FIXES`): `CHAN_ITEM` rather than `CHAN_VOICE`.
                item_sound(eng, slot, "items/kevlar.wav");
                snapshot_rebuy(g, slot);
                return;
            }
        }
        _ => return,
    }
    let Some((item, price)) = purchase else {
        not_enough_money(eng, slot);
        return;
    };
    match item {
        Purchase::Armor(armor) => give_armor_item(g, eng, slot, armor),
        Purchase::Weapon(class) => {
            inventory::give_named_item(g, eng, slot, class);
        }
    }
    money::add_account(g, eng, slot, -price, true);
    snapshot_rebuy(g, slot);
}

/// `CanBuyWeaponByMaptype` (`weapontype.cpp` 596-680): the weapons a team can buy, taking
/// assassination (`as_*`, `m_bMapHasVIPSafetyZone`) restrictions into account.
fn can_buy_weapon_by_maptype(team: Team, id: WeaponId, assassination: bool) -> bool {
    use WeaponId::*;
    if assassination {
        return match team {
            Team::Ct => matches!(
                id,
                P228 | Xm1014 | Aug | FiveSeven | Ump45 | Sg550 | Famas | Usp | Glock18 | Mp5n
                    | M249 | M3 | M4a1 | Tmp | Deagle | P90 | ShieldGun
            ),
            Team::Terrorist => matches!(
                id,
                P228 | Mac10 | Elite | Ump45 | Galil | Usp | Glock18 | Awp | Deagle | Ak47
            ),
            _ => false,
        };
    }
    match team {
        Team::Ct => matches!(
            id,
            P228 | Scout | Xm1014 | Aug | FiveSeven | Ump45 | Sg550 | Famas | Usp | Glock18 | Awp
                | Mp5n | M249 | M3 | M4a1 | Tmp | Deagle | P90 | ShieldGun
        ),
        Team::Terrorist => matches!(
            id,
            P228 | Scout | Xm1014 | Mac10 | Elite | Ump45 | Galil | Usp | Glock18 | Awp | Mp5n
                | M249 | M3 | G3sg1 | Deagle | Sg552 | Ak47 | P90
        ),
        _ => false,
    }
}

/// `CanBuyThis` (`client.cpp` 1133): a weapon the player already holds in its slot is
/// `#Cstrike_Already_Own_Weapon`, one the map's rules keep from its team `#Cannot_Buy_This`.
fn can_buy_this(g: &GameState, eng: &mut EngineApi<'_>, slot: u8, id: WeaponId) -> bool {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    if p.owns_shield && id == WeaponId::ShieldGun {
        say(eng, slot, "#Already_Have_One");
        return false;
    }
    if p.owns_shield && id == WeaponId::Elite {
        say(eng, slot, "#Cannot_Buy_This");
        return false;
    }
    if id == WeaponId::ShieldGun && inventory::has_player_item(p, WeaponId::Elite) {
        say(eng, slot, "#Cannot_Buy_This");
        return false;
    }
    let owns = |slot_index: usize| p.items[slot_index].first().is_some_and(|i| i.weapon.id == id);
    if owns(PRIMARY_WEAPON_SLOT) || owns(PISTOL_SLOT) {
        say(eng, slot, "#Cstrike_Already_Own_Weapon");
        return false;
    }
    if !can_buy_weapon_by_maptype(p.team, id, g.rules.map_has_vip_safety_zone()) {
        say(eng, slot, "#Cannot_Buy_This");
        return false;
    }
    true
}

/// `BuyWeaponByWeaponID` (`client.cpp` 1595, vanilla): the weapon `id` is bought when the player
/// may buy at all, may have it and can pay; the weapon held in the slot is dropped first. True
/// when it was bought.
pub(crate) fn buy_weapon_by_id(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, id: WeaponId) -> bool {
    if !can_player_buy(g, eng, slot, true) {
        return false;
    }
    if !can_buy_this(g, eng, slot, id) {
        return false;
    }
    if id == WeaponId::ShieldGun {
        let account = g
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.money);
        if account < SHIELDGUN_PRICE {
            not_enough_money(eng, slot);
            return false;
        }
        inventory::drop_primary(g, eng, slot);
        inventory::give_shield(g, eng, slot, true);
        money::add_account(g, eng, slot, -SHIELDGUN_PRICE, true);
        snapshot_rebuy(g, slot);
        return true;
    }
    let Some(info) = weapon_info(id).filter(|i| !i.entity_name.is_empty()) else {
        return false;
    };
    let account = g.players.get(usize::from(slot)).and_then(Option::as_ref).map_or(0, |p| p.money);
    if account < info.cost {
        not_enough_money(eng, slot);
        return false;
    }
    if is_primary_weapon(id) {
        inventory::drop_primary(g, eng, slot);
    } else {
        inventory::drop_secondary(g, eng, slot);
    }
    inventory::give_named_item(g, eng, slot, info.entity_name);
    money::add_account(g, eng, slot, -info.cost, true);
    snapshot_rebuy(g, slot);
    true
}

/// Snapshots the player's currently carried loadout into `m_rebuyStruct` (`rebuy_record`) so
/// future `rebuy` / `cl_rebuy` commands restore it.
pub(crate) fn snapshot_rebuy(g: &mut GameState, slot: u8) {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    p.rebuy_record = crate::player::RebuyRecord {
        primary: p.items[PRIMARY_WEAPON_SLOT].first().map(|i| i.weapon.id),
        secondary: p.items[PISTOL_SLOT].first().map(|i| i.weapon.id),
        armor: if p.armor > 0.0 { p.kevlar } else { ArmorType::None },
        defuser: p.has_defuser,
        nvg: p.has_night_vision,
        he_grenade: p.ammo.get(AmmoType::HeGrenade as usize).copied().unwrap_or(0) > 0,
        flashbangs: p.ammo.get(AmmoType::Flashbang as usize).copied().unwrap_or(0),
        smoke_grenade: p.ammo.get(AmmoType::SmokeGrenade as usize).copied().unwrap_or(0) > 0,
        shield: p.owns_shield,
    };
}

/// `BuyGunAmmo` (`client.cpp` 2328, vanilla): one clip of the ammo of the weapon `id` the player
/// carries, when the player can buy, uses ammo, is not full and can pay; with `blink_money` a
/// player that cannot pay is told. The clip is given as the ammo entity of the original gives
/// it (`CBasePlayerAmmo::AddAmmo`).
fn buy_gun_ammo(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, id: WeaponId, blink_money: bool) -> bool {
    if !can_player_buy(g, eng, slot, true) {
        return false;
    }
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    let Some((s, i)) = p.find_item(id) else {
        return false;
    };
    // Ensure that the weapon uses ammo.
    let ammo_index = p.items[s][i].weapon.primary_ammo_type;
    let Ok(index) = usize::try_from(ammo_index) else {
        return false;
    };
    // Can only buy if the player does not already have full ammo.
    let max = crate::weapons::item_info(id).map_or(0, |i| i.max_ammo1);
    if p.ammo.get(index).is_none_or(|&have| have >= max) {
        return false;
    }
    let Some(info) = weapon_info(id) else {
        return false;
    };
    // Purchase the ammo if the player has enough money.
    if p.money >= info.clip_cost {
        let Some(ammo) = ammo_info_by_name(info.ammo_name1) else {
            return false;
        };
        if crate::weapons::core::give_ammo(g, eng, slot, ammo.buy_clip_size, Some(ammo.ammo_name2), -1) != -1 {
            item_sound(eng, slot, "items/9mmclip1.wav");
        }
        money::add_account(g, eng, slot, -info.clip_cost, true);
        return true;
    }
    if blink_money {
        // Not enough money.. let the player know.
        not_enough_money(eng, slot);
    }
    false
}

/// `BuyAmmo` (`client.cpp` 2380): one clip for each weapon in the inventory slot `n_slot` (1
/// primary, 2 pistol), from the head of the list, while each purchase works. True when every
/// weapon of the slot got its clip (so a slot with no weapon is false).
fn buy_ammo(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, n_slot: usize, blink_money: bool) -> bool {
    if !can_player_buy(g, eng, slot, true) {
        return false;
    }
    if !(PRIMARY_WEAPON_SLOT..=PISTOL_SLOT).contains(&n_slot) {
        return false;
    }
    // Buy one ammo clip for all weapons in the given slot.
    let ids: Vec<WeaponId> = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| p.items[n_slot].iter().map(|i| i.weapon.id).collect())
        .unwrap_or_default();
    for id in ids {
        if !buy_gun_ammo(g, eng, slot, id, blink_money) {
            return false;
        }
    }
    // The loop ends with `pItem == nullptr` only for a non-empty slot.
    g.players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| !p.items[n_slot].is_empty())
}

/// What `primammo`, `secammo` and the ammo keys of the buy menu do: "buy as much ammo as
/// possible; blink money only if the player doesn't have enough for the first clip".
fn buy_ammo_all(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, n_slot: usize) {
    if buy_ammo(g, eng, slot, n_slot, true) {
        // Bounded by the ammo the slot can hold: every round of the loop buys a clip or stops.
        for _ in 0..256 {
            if !buy_ammo(g, eng, slot, n_slot, false) {
                break;
            }
        }
    }
}

/// `BuyPistol`, `BuyShotgun`, `BuySubMachineGun`, `BuyRifle` and `BuyMachineGun`
/// (`client.cpp` 1177-1290): the weapon a key of a weapon menu stands for, by team.
fn menu_weapon(menu: Menu, team: Team, key: i32) -> Option<WeaponId> {
    use WeaponId::*;
    let ct = team == Team::Ct;
    Some(match (menu, key) {
        (Menu::BuyPistol, 1) => Glock18,
        (Menu::BuyPistol, 2) => Usp,
        (Menu::BuyPistol, 3) => P228,
        (Menu::BuyPistol, 4) => Deagle,
        (Menu::BuyPistol, 5) => if ct { FiveSeven } else { Elite },
        (Menu::BuyShotgun, 1) => M3,
        (Menu::BuyShotgun, 2) => Xm1014,
        (Menu::BuySubMachineGun, 1) => if ct { Tmp } else { Mac10 },
        (Menu::BuySubMachineGun, 2) => Mp5n,
        (Menu::BuySubMachineGun, 3) => Ump45,
        (Menu::BuySubMachineGun, 4) => P90,
        (Menu::BuyRifle, 1) => if ct { Famas } else { Galil },
        (Menu::BuyRifle, 2) => if ct { Scout } else { Ak47 },
        (Menu::BuyRifle, 3) => if ct { M4a1 } else { Scout },
        (Menu::BuyRifle, 4) => if ct { Aug } else { Sg552 },
        (Menu::BuyRifle, 5) => if ct { Sg550 } else { Awp },
        (Menu::BuyRifle, 6) => if ct { Awp } else { G3sg1 },
        (Menu::BuyMachineGun, 1) => M249,
        _ => return Option::None,
    })
}

// ---- the menus ----

/// The team of the player of `slot`.
fn team_of(g: &GameState, slot: u8) -> Team {
    g.players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map_or(Team::Unassigned, |p| p.team)
}

/// Shows a menu of the buy group: the VGUI panel to a client that wants them, else the text menu
/// `old_menu`; the player is then in `menu`.
fn show(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    menu: Menu,
    menu_id: u8,
    keys: u16,
    old_menu: &str,
) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.menu = menu;
    let vgui = p.vgui_menus;
    menus::show_vgui_menu(eng, slot, vgui, menu_id, keys, old_menu);
}

const KEYS_1_TO_6: u16 =
    MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_5 | MENU_KEY_6;

/// The first buy menu (`buy`).
fn show_buy_menu(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let keys = KEYS_1_TO_6 | MENU_KEY_7 | MENU_KEY_8 | MENU_KEY_0;
    show(g, eng, slot, Menu::Buy, VGUI_MENU_BUY, keys, "#Buy");
}

/// The equipment menu (`buyequip`, or the item key of the first menu): a counter-terrorist's has
/// the defuser key on a map with a bomb target and always the shield.
fn show_item_menu(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let ct = team_of(g, slot) == Team::Ct;
    let (keys, text) = match (g.rules.map_has_bomb_target(), ct) {
        (true, true) => (
            KEYS_1_TO_6 | MENU_KEY_7 | MENU_KEY_8 | MENU_KEY_0,
            "#DCT_BuyItem",
        ),
        (true, false) => (KEYS_1_TO_6 | MENU_KEY_0, "#DT_BuyItem"),
        (false, true) => (KEYS_1_TO_6 | MENU_KEY_8 | MENU_KEY_0, "#CT_BuyItem"),
        (false, false) => (KEYS_1_TO_6 | MENU_KEY_0, "#T_BuyItem"),
    };
    show(g, eng, slot, Menu::BuyItem, VGUI_MENU_BUY_ITEM, keys, text);
}

/// The `Menu_Buy` choice `choice` (`menuselect` of a text menu client): the menu of a kind of
/// weapon (including `#AS_*` variants on maps with a VIP safety zone), the ammo, or the equipment.
fn choose_buy_group(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, choice: i32) {
    let ct = team_of(g, slot) == Team::Ct;
    let as_map = g.rules.map_has_vip_safety_zone();
    let in_zone = |g: &GameState| {
        g.players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .is_some_and(|p| p.signals.state() & SIGNAL_BUY != 0)
    };
    match choice {
        SLOT_BUY_PISTOL => {
            let text = if ct { "#CT_BuyPistol" } else { "#T_BuyPistol" };
            let keys = MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_5 | MENU_KEY_0;
            show(
                g,
                eng,
                slot,
                Menu::BuyPistol,
                VGUI_MENU_BUY_PISTOL,
                keys,
                text,
            );
        }
        SLOT_BUY_SHOTGUN => {
            let (keys, text) = if as_map && !ct {
                (MENU_KEY_0, "#AS_BuyShotgun")
            } else {
                (MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_0, "#BuyShotgun")
            };
            show(
                g,
                eng,
                slot,
                Menu::BuyShotgun,
                VGUI_MENU_BUY_SHOTGUN,
                keys,
                text,
            );
        }
        SLOT_BUY_SUBMACHINEGUN => {
            let (keys, text) = match (as_map, ct) {
                (true, true) => (
                    MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_0,
                    "#AS_CT_BuySubMachineGun",
                ),
                (true, false) => (
                    MENU_KEY_1 | MENU_KEY_3 | MENU_KEY_0,
                    "#AS_T_BuySubMachineGun",
                ),
                (false, true) => (
                    MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_0,
                    "#CT_BuySubMachineGun",
                ),
                (false, false) => (
                    MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_0,
                    "#T_BuySubMachineGun",
                ),
            };
            show(
                g,
                eng,
                slot,
                Menu::BuySubMachineGun,
                VGUI_MENU_BUY_SUBMACHINEGUN,
                keys,
                text,
            );
        }
        SLOT_BUY_RIFLE => {
            let (keys, text) = match (as_map, ct) {
                (true, true) => (
                    MENU_KEY_1 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_5 | MENU_KEY_0,
                    "#AS_CT_BuyRifle",
                ),
                (true, false) => (
                    MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5 | MENU_KEY_0,
                    "#AS_T_BuyRifle",
                ),
                (false, true) => (KEYS_1_TO_6 | MENU_KEY_0, "#CT_BuyRifle"),
                (false, false) => (KEYS_1_TO_6 | MENU_KEY_0, "#T_BuyRifle"),
            };
            show(
                g,
                eng,
                slot,
                Menu::BuyRifle,
                VGUI_MENU_BUY_RIFLE,
                keys,
                text,
            );
        }
        SLOT_BUY_MACHINEGUN => {
            let (keys, text) = if as_map && !ct {
                (MENU_KEY_0, "#AS_T_BuyMachineGun")
            } else {
                (MENU_KEY_1 | MENU_KEY_0, "#BuyMachineGun")
            };
            show(
                g,
                eng,
                slot,
                Menu::BuyMachineGun,
                VGUI_MENU_BUY_MACHINEGUN,
                keys,
                text,
            );
        }
        SLOT_BUY_PRIMARY_AMMO if in_zone(g) => buy_ammo_all(g, eng, slot, PRIMARY_WEAPON_SLOT),
        SLOT_BUY_SECONDARY_AMMO if in_zone(g) => buy_ammo_all(g, eng, slot, PISTOL_SLOT),
        SLOT_BUY_ITEM if in_zone(g) => show_item_menu(g, eng, slot),
        _ => {}
    }
}

/// The `menuselect` of a text menu client in a buy menu (`menu` is the one the player is in).
/// The weapon menus' keys 1 to their last one buy the weapon of the key ([`menu_weapon`]); any
/// other key does nothing.
pub(crate) fn menuselect(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    menu: Menu,
    choice: i32,
) {
    // The key counts are those of `BuyPistol` (1 to 5), `BuyShotgun` (1 to 2),
    // `BuySubMachineGun` (1 to 4), `BuyRifle` (1 to 6) and `BuyMachineGun` (only 1) in
    // `client.cpp:1177-1285`.
    match menu {
        Menu::Buy => {
            choose_buy_group(g, eng, slot, choice);
        }
        Menu::BuyItem => {
            buy_item(g, eng, slot, choice);
        }
        Menu::BuyPistol
        | Menu::BuyShotgun
        | Menu::BuySubMachineGun
        | Menu::BuyRifle
        | Menu::BuyMachineGun => {
            let team = team_of(g, slot);
            if let Some(id) = menu_weapon(menu, team, choice) {
                buy_weapon_by_id(g, eng, slot, id);
            }
        }
        Menu::Off
        | Menu::ChooseTeam
        | Menu::ChooseAppearance
        | Menu::Radio1
        | Menu::Radio2
        | Menu::Radio3
        | Menu::ClientBuy => {}
    }
}

// ---- the commands ----

/// The weapon aliases of `g_weaponBuyAliasInfo` (`weapontype.cpp`): the command, the text the
/// original tells a team that cannot buy it (`#Alias_Not_Avail`), and the teams that can
/// (`CanBuyWeaponByMaptype`: `T` terrorists, `C` counter-terrorists, `B` both).
const WEAPON_ALIASES: &[(&str, &str, char)] = &[
    ("galil", "#Galil", 'T'),
    ("defender", "#Galil", 'T'),
    ("ak47", "#AK47", 'T'),
    ("cv47", "#AK47", 'T'),
    ("scout", "#Scout", 'B'),
    ("sg552", "#SG552", 'T'),
    ("krieg552", "#SG552", 'T'),
    ("awp", "#ArcticWarfareMagnum", 'B'),
    ("magnum", "#ArcticWarfareMagnum", 'B'),
    ("g3sg1", "#G3SG1", 'T'),
    ("d3au1", "#G3SG1", 'T'),
    ("famas", "#Famas", 'C'),
    ("clarion", "#Famas", 'C'),
    ("m4a1", "#M4A1", 'C'),
    ("aug", "#Aug", 'C'),
    ("bullpup", "#Aug", 'C'),
    ("sg550", "#SG550", 'C'),
    ("krieg550", "#SG550", 'C'),
    ("glock", "#Glock18", 'B'),
    ("9x19mm", "#Glock18", 'B'),
    ("usp", "#USP45", 'B'),
    ("km45", "#USP45", 'B'),
    ("p228", "#P228", 'B'),
    ("228compact", "#P228", 'B'),
    ("deagle", "#DesertEagle", 'B'),
    ("nighthaw", "#DesertEagle", 'B'),
    ("elites", "#Beretta96G", 'T'),
    ("fn57", "#FiveSeven", 'C'),
    ("fiveseven", "#FiveSeven", 'C'),
    ("m3", "#Super90", 'B'),
    ("12gauge", "#Super90", 'B'),
    ("xm1014", "#XM1014", 'B'),
    ("autoshotgun", "#XM1014", 'B'),
    ("mac10", "#Mac10", 'T'),
    ("tmp", "#tmp", 'C'),
    ("mp", "#tmp", 'C'),
    ("mp5", "#mp5navy", 'B'),
    ("smg", "#mp5navy", 'B'),
    ("ump45", "#UMP45", 'B'),
    ("p90", "#FNP90", 'B'),
    ("c90", "#FNP90", 'B'),
    ("m249", "#M249", 'B'),
];

/// The entry of [`WEAPON_ALIASES`] for the command `name` (`Q_stricmp`: any case).
fn weapon_alias(name: &str) -> Option<&'static (&'static str, &'static str, char)> {
    WEAPON_ALIASES
        .iter()
        .find(|(alias, _, _)| alias.eq_ignore_ascii_case(name))
}

/// The commands of the other buy aliases, which `HandleBuyAliasCommands` takes exactly.
const EQUIPMENT_ALIASES: &[&str] = &[
    "primammo", "secammo", "vest", "vesthelm", "flash", "hegren", "sgren", "nvgs", "defuser",
    "shield",
];

/// Maximum number of tokens accepted by `cl_setautobuy` / `cl_setrebuy`.
const MAX_BUY_SCRIPT_TOKENS: usize = 32;
/// Maximum byte length of a single token in `cl_setautobuy` / `cl_setrebuy`.
const MAX_BUY_SCRIPT_TOKEN_LEN: usize = 32;

/// Whether `name` is a command of this module: the buy menus' commands, the buy aliases, the
/// weapon aliases, and `autobuy` / `rebuy` commands.
pub(crate) fn is_buy_command(name: &str) -> bool {
    matches!(
        name,
        "buy"
            | "buyequip"
            | "buyammo1"
            | "buyammo2"
            | "client_buy_open"
            | "client_buy_close"
            | "autobuy"
            | "cl_autobuy"
            | "cl_setautobuy"
            | "rebuy"
            | "cl_rebuy"
            | "cl_setrebuy"
    ) || EQUIPMENT_ALIASES.contains(&name)
        || weapon_alias(name).is_some()
}

/// The buy commands of `ClientCommand_` (ReGameDLL `client.cpp`). `client_buy_open` and
/// `client_buy_close` work for anyone; the rest only for a living player (a dead one's commands
/// fall through to nothing).
pub(crate) fn client_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
) {
    let Some(command) = argv.first().map(String::as_str) else {
        return;
    };
    let in_zone = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| p.signals.state() & SIGNAL_BUY != 0);
    let Some(in_zone) = in_zone else {
        return;
    };
    match command {
        "client_buy_open" => {
            if let Some(p) = g.players[usize::from(slot)].as_mut()
                && p.menu == Menu::Off
            {
                p.menu = Menu::ClientBuy;
            }
            if !in_zone {
                eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::BuyClose);
            }
            return;
        }
        "client_buy_close" => {
            if let Some(p) = g.players[usize::from(slot)].as_mut()
                && p.menu == Menu::ClientBuy
            {
                p.menu = Menu::Off;
            }
            return;
        }
        _ => {}
    }
    // The rest is for the living.
    let alive = eng
        .ent(eng.client_edict(slot))
        .is_some_and(|v| v.deadflag == DEAD_NO);
    if !alive {
        return;
    }
    match command {
        "buy" => {
            if in_zone {
                show_buy_menu(g, eng, slot);
            }
        }
        "buyequip" => {
            if in_zone {
                show_item_menu(g, eng, slot);
            }
        }
        "buyammo1" => {
            if in_zone {
                buy_ammo(g, eng, slot, PRIMARY_WEAPON_SLOT, true);
            }
        }
        "buyammo2" => {
            if in_zone {
                buy_ammo(g, eng, slot, PISTOL_SLOT, true);
            }
        }
        "cl_setautobuy" => parse_buy_script_tokens(&argv[1..], g, slot, true),
        "cl_setrebuy" => parse_buy_script_tokens(&argv[1..], g, slot, false),
        "autobuy" | "cl_autobuy" => handle_autobuy(g, eng, slot),
        "rebuy" | "cl_rebuy" => handle_rebuy(g, eng, slot),
        alias => alias_command(g, eng, slot, alias),
    }
}

/// Parses bounded, ASCII-safe tokens from `cl_setautobuy` / `cl_setrebuy` into the player's list.
fn parse_buy_script_tokens(args: &[String], g: &mut GameState, slot: u8, is_autobuy: bool) {
    let mut tokens = Vec::new();
    for arg in args {
        for raw in arg.split_ascii_whitespace() {
            if tokens.len() >= MAX_BUY_SCRIPT_TOKENS {
                break;
            }
            if !raw.is_ascii() || raw.len() > MAX_BUY_SCRIPT_TOKEN_LEN {
                continue;
            }
            let lower = raw.to_ascii_lowercase();
            if !lower.is_empty() {
                tokens.push(lower);
            }
        }
        if tokens.len() >= MAX_BUY_SCRIPT_TOKENS {
            break;
        }
    }
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        if is_autobuy {
            p.autobuy_list = tokens;
        } else {
            p.rebuy_order = tokens;
        }
    }
}

/// `AutoBuy` (`client.cpp`): iterates `p.autobuy_list`, skipping opposite-team items and duplicate
/// primary/secondary weapons once filled, buying the first affordable items from the list.
fn handle_autobuy(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if !can_player_buy(g, eng, slot, true) {
        return;
    }
    let Some(list) = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| p.autobuy_list.clone())
    else {
        return;
    };
    let assassination = g.rules.map_has_vip_safety_zone();
    for token in &list {
        let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
            return;
        };
        let team = p.team;
        let has_primary = p.has_primary || p.owns_shield;
        let lower = token.to_ascii_lowercase();
        if let Some(id) = crate::weapons::buy_alias_to_weapon_id(&lower) {
            if !can_buy_weapon_by_maptype(team, id, assassination) {
                continue;
            }
            if (is_primary_weapon(id) || id == WeaponId::ShieldGun) && has_primary {
                continue;
            }
            if is_secondary_weapon(id)
                && p.items[PISTOL_SLOT]
                    .first()
                    .is_some_and(|i| i.weapon.id == id)
            {
                continue;
            }
            buy_weapon_by_id(g, eng, slot, id);
            continue;
        }
        match lower.as_str() {
            "primammo" | "primaryammo" => {
                buy_ammo_all(g, eng, slot, PRIMARY_WEAPON_SLOT);
            }
            "secammo" | "secondaryammo" => {
                buy_ammo_all(g, eng, slot, PISTOL_SLOT);
            }
            "vest" => buy_item(g, eng, slot, SLOT_ITEM_VEST),
            "vesthelm" => buy_item(g, eng, slot, SLOT_ITEM_VESTHELM),
            "flash" | "flashbang" => buy_item(g, eng, slot, SLOT_ITEM_FLASHGREN),
            "hegren" | "hegrenade" => buy_item(g, eng, slot, SLOT_ITEM_HEGREN),
            "sgren" | "smokegrenade" => buy_item(g, eng, slot, SLOT_ITEM_SMOKEGREN),
            "nvgs" | "nightvision" => buy_item(g, eng, slot, SLOT_ITEM_NVG),
            "defuser" if team == Team::Ct => buy_item(g, eng, slot, SLOT_ITEM_DEFUSEKIT),
            "shield" if team == Team::Ct && !has_primary => {
                buy_item(g, eng, slot, SLOT_ITEM_SHIELD);
            }
            _ => {}
        }
    }
}

/// `Rebuy` (`client.cpp`): restores equipment recorded in `p.rebuy_record` in `p.rebuy_order`.
fn handle_rebuy(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if !can_player_buy(g, eng, slot, true) {
        return;
    }
    let Some((order, record, team)) = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| (p.rebuy_order.clone(), p.rebuy_record.clone(), p.team))
    else {
        return;
    };
    for token in &order {
        let key = token.to_ascii_lowercase();
        match key.as_str() {
            "primaryweapon" => {
                if let Some(id) = record.primary {
                    buy_weapon_by_id(g, eng, slot, id);
                }
            }
            "primaryammo" => buy_ammo_all(g, eng, slot, PRIMARY_WEAPON_SLOT),
            "secondaryweapon" => {
                if let Some(id) = record.secondary {
                    buy_weapon_by_id(g, eng, slot, id);
                }
            }
            "secondaryammo" => buy_ammo_all(g, eng, slot, PISTOL_SLOT),
            "armor" => match record.armor {
                ArmorType::VestHelm => buy_item(g, eng, slot, SLOT_ITEM_VESTHELM),
                ArmorType::Vest => buy_item(g, eng, slot, SLOT_ITEM_VEST),
                ArmorType::None => {}
            },
            "defuser" => {
                if record.defuser && team == Team::Ct {
                    buy_item(g, eng, slot, SLOT_ITEM_DEFUSEKIT);
                }
            }
            "hegrenade" | "hegren" => {
                if record.he_grenade {
                    buy_item(g, eng, slot, SLOT_ITEM_HEGREN);
                }
            }
            "flashbang" | "flash" => {
                for _ in 0..record.flashbangs.clamp(0, 2) {
                    buy_item(g, eng, slot, SLOT_ITEM_FLASHGREN);
                }
            }
            "smokegrenade" | "sgren" => {
                if record.smoke_grenade {
                    buy_item(g, eng, slot, SLOT_ITEM_SMOKEGREN);
                }
            }
            "nightvision" | "nvgs" => {
                if record.nvg {
                    buy_item(g, eng, slot, SLOT_ITEM_NVG);
                }
            }
            "tacticalshield" | "shield" => {
                if record.shield && team == Team::Ct {
                    buy_item(g, eng, slot, SLOT_ITEM_SHIELD);
                }
            }
            _ => {}
        }
    }
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.rebuy_record = record;
    }
}

/// `HandleBuyAliasCommands`: the commands that buy one thing, for the players whose scripts
/// buy. Only a living player gets here: `ClientCommand_` calls it inside its `DEAD_NO` branch,
/// so a dead player is told nothing, not even `#Alias_Not_Avail` (checked 2026-11-22).
fn alias_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, command: &str) {
    let team = team_of(g, slot);
    if let Some(&(_, fail_name, _)) = weapon_alias(command) {
        if let Some(id) = crate::weapons::buy_alias_to_weapon_id(command) {
            if can_buy_weapon_by_maptype(team, id, g.rules.map_has_vip_safety_zone()) {
                buy_weapon_by_id(g, eng, slot, id);
            } else {
                client_print(eng, slot, HUD_PRINTCENTER, "#Alias_Not_Avail", &[fail_name]);
            }
        }
        return;
    }
    match command {
        "primammo" => buy_ammo_all(g, eng, slot, PRIMARY_WEAPON_SLOT),
        "secammo" => buy_ammo_all(g, eng, slot, PISTOL_SLOT),
        "vest" => buy_item(g, eng, slot, SLOT_ITEM_VEST),
        "vesthelm" => buy_item(g, eng, slot, SLOT_ITEM_VESTHELM),
        "flash" => buy_item(g, eng, slot, SLOT_ITEM_FLASHGREN),
        "hegren" => buy_item(g, eng, slot, SLOT_ITEM_HEGREN),
        "sgren" => buy_item(g, eng, slot, SLOT_ITEM_SMOKEGREN),
        "nvgs" => buy_item(g, eng, slot, SLOT_ITEM_NVG),
        "defuser" => {
            if team == Team::Ct {
                buy_item(g, eng, slot, SLOT_ITEM_DEFUSEKIT);
            } else {
                client_print(
                    eng,
                    slot,
                    HUD_PRINTCENTER,
                    "#Alias_Not_Avail",
                    &["#Bomb_Defusal_Kit"],
                );
            }
        }
        "shield" => {
            if team == Team::Ct {
                buy_item(g, eng, slot, SLOT_ITEM_SHIELD);
            } else {
                client_print(
                    eng,
                    slot,
                    HUD_PRINTCENTER,
                    "#Alias_Not_Avail",
                    &["#TactShield_Desc"],
                );
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_become_the_state_at_the_update() {
        let mut s = Signals::default();
        assert_eq!((s.pending(), s.state()), (0, 0));
        s.signal(SIGNAL_BUY);
        assert_eq!((s.pending(), s.state()), (SIGNAL_BUY, 0));
        s.update();
        assert_eq!((s.pending(), s.state()), (0, SIGNAL_BUY));
        s.update();
        assert_eq!(s.state(), 0);
    }

    #[test]
    fn aliases_match_like_the_original() {
        // The weapon aliases ignore case, the others do not.
        assert!(is_buy_command("AK47"));
        assert!(is_buy_command("vest"));
        assert!(!is_buy_command("VEST"));
        assert!(!is_buy_command("kill"));
        assert_eq!(weapon_alias("m4a1").map(|a| a.2), Some('C'));
        assert_eq!(weapon_alias("ak47").map(|a| a.2), Some('T'));
        assert_eq!(weapon_alias("awp").map(|a| a.2), Some('B'));
        assert_eq!(WEAPON_ALIASES.len(), 42);
    }
}
