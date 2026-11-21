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
//!   player, which touches nothing, leaves the zone the same way.
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
//! Deviations and what waits for later tasks:
//! - Weapons, grenades and the shield do not exist yet (2D): buying one, by a menu or an alias,
//!   goes through `CanPlayerBuy` and the alias's team check of the original and is then refused
//!   with `#Cannot_Buy_This`, nothing taken. The original also checks the price (and tells
//!   `#Not_Enough_Money`) before it hands out the item. Ammo is bought for the weapons a player
//!   carries, and a player carries none, so the ammo commands only run `CanPlayerBuy`, as the
//!   original does with empty slots. `cl_autobuy`, `cl_rebuy` and their setters (they buy
//!   weapons) are not ported.
//! - The defuser (`MENU_SLOT_ITEM_DEFUSEKIT`) is sold to a counter-terrorist on a map with a
//!   bomb target only; the bomb targets are 2E, so no map has one yet and the purchase is
//!   refused silently, as the original does on a map without one. The menu then has no defuser
//!   key either. The terrorists' `defuser` alias says `#Alias_Not_Avail` as the original.
//! - `BuyZoneIcon_Clear` is also called by `CBasePlayer::Disappear`, which only the escape
//!   zone (`func_escapezone`, `triggers.cpp:2069`, 2E) calls; it has no caller here yet and
//!   the 2E escape port must add the call.
//! - `HintMessage("#Hint_use_nightvision")` (first night vision purchase) has no hint queue
//!   behind it and is not sent, like the hints of [`crate::money`].
//! - `m_bCTCantBuy`/`m_bTCantBuy` are never set by a vanilla multiplayer game and the VIP does
//!   not exist yet, so the checks on them are left out. The tutor is not ported.
//! - A `func_buyzone`'s `angles` (one-way touch, `SetMovedir`) only matter to the generic
//!   trigger code, not to the buy zone, and are cleared without computing a `movedir`.

use cs_engine::consts::{MOVETYPE_NONE, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::chat::client_print;
use crate::entity::GameEntity;
use crate::menus::{
    self, HUD_PRINTCENTER, MENU_KEY_0, MENU_KEY_1, MENU_KEY_2, MENU_KEY_3, MENU_KEY_4, MENU_KEY_5,
    MENU_KEY_6, MENU_KEY_7, MENU_KEY_8, Menu, VGUI_MENU_BUY_ITEM,
};
use crate::money;
use crate::player::{ATTN_NORM, ArmorType, CHAN_ITEM, DEAD_NO, EF_NODRAW, PITCH_NORM, Team, player_slot};

/// `SIGNAL_BUY` (`unisignals.h`): the player stands in a buy zone.
pub const SIGNAL_BUY: u8 = 1 << 0;
/// `MIN_BUY_TIME` (`player.h`, vanilla): the least `mp_buytime` allows, in seconds.
pub const MIN_BUY_TIME: i32 = 15;
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
/// `ITEM_STATUS_NIGHTVISION` (`cdll_dll.h`).
const ITEM_STATUS_NIGHTVISION: u8 = 1 << 0;

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

/// `gmsgStatusIcon` of `BuyZoneIcon_Set`: the `buyzone` icon, shown, in green.
fn buy_zone_icon_set(eng: &mut EngineApi<'_>, slot: u8) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::StatusIcon {
            status: 1,
            sprite_name: "buyzone".to_string(),
            color: Some([0, 160, 0]),
        },
    );
}

/// `BuyZoneIcon_Clear`: the `buyzone` icon is hidden. A buy menu the player has open is closed
/// with it: a text or VGUI menu with the `slot10` command (the client's "close"), the client's own
/// menu with `BuyClose`.
pub(crate) fn buy_zone_icon_clear(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::StatusIcon {
            status: 0,
            sprite_name: "buyzone".to_string(),
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

/// `OLD_CheckBuyZone`: on a map with no `func_buyzone`, a player of a team that spawns is in a
/// buy zone within 200 units of one of its team's spawn points (the origin of the point, whether
/// or not the player is alive).
fn old_check_buy_zone(g: &mut GameState, eng: &EngineApi<'_>, slot: u8) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let class = match p.team {
        Team::Terrorist => crate::world::T_SPAWN_CLASS,
        Team::Ct => crate::world::CT_SPAWN_CLASS,
        Team::Unassigned | Team::Spectator => return,
    };
    let Some(origin) = eng.ent(eng.client_edict(slot)).map(|v| v.origin) else {
        return;
    };
    let mut near = false;
    let mut spot = None;
    while let Some(found) = eng.find_by_classname(spot, class) {
        spot = Some(found);
        if eng
            .ent(found)
            .is_some_and(|v| (v.origin - origin).length() < OLD_BUY_ZONE_RADIUS)
        {
            near = true;
        }
    }
    if near && let Some(p) = g.players[usize::from(slot)].as_mut() {
        p.signals.signal(SIGNAL_BUY);
    }
}

/// `HandleSignals` (the buy part; the bomb, rescue, escape and VIP zones are 2E): on a map
/// without buy zones the spawn points count ([`old_check_buy_zone`]), then the raised signals
/// become the state and a change of the buy signal shows or hides the icon.
fn handle_signals(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if !g.rules.map_has_buy_zone() {
        old_check_buy_zone(g, eng, slot);
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
    if changed & SIGNAL_BUY != 0 {
        if signal & SIGNAL_BUY != 0 {
            buy_zone_icon_set(eng, slot);
        } else {
            buy_zone_icon_clear(g, eng, slot);
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

/// `SendItemStatus`: what the player carries that the HUD shows (the goggles, when
/// `has_night_vision`; the defuser is 2E).
pub(crate) fn send_item_status(eng: &mut EngineApi<'_>, slot: u8, has_night_vision: bool) {
    let mut status = 0;
    if has_night_vision {
        status |= ITEM_STATUS_NIGHTVISION;
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
enum ArmorItem {
    /// `item_kevlar` (`CItemKevlar::MyTouch`).
    Kevlar,
    /// `item_assaultsuit` (`CItemAssaultSuit::MyTouch`).
    AssaultSuit,
}

/// `GiveNamedItem("item_kevlar")` / `("item_assaultsuit")`: the item is touched by the player at
/// once. The vest sets the armor type only when the player has none (so a player with a helmet
/// keeps it) and always tells the client `ArmorType` 0; the assault suit sets the helmet and
/// tells 1. Both fill the armor to 100.
fn give_armor(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, item: ArmorItem) {
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

/// `BuyItem` (ReGameDLL `client.cpp`) for the equipment menu slot `item_slot`. Weapons (the
/// grenades and the shield) are refused until they exist ([`refuse_weapon`]).
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
                purchase = Some((ArmorItem::Kevlar, KEVLAR_PRICE));
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
                    purchase = Some((ArmorItem::AssaultSuit, HELMET_PRICE));
                }
            } else if has_helmet {
                if account >= KEVLAR_PRICE {
                    say(eng, slot, "#Already_Have_Helmet_Bought_Kevlar");
                    purchase = Some((ArmorItem::AssaultSuit, KEVLAR_PRICE));
                }
            } else if account >= ASSAULTSUIT_PRICE {
                purchase = Some((ArmorItem::AssaultSuit, ASSAULTSUIT_PRICE));
            }
        }
        SLOT_ITEM_FLASHGREN | SLOT_ITEM_HEGREN | SLOT_ITEM_SMOKEGREN | SLOT_ITEM_SHIELD => {
            say(eng, slot, "#Cannot_Buy_This");
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
                item_sound(eng, slot, "items/equip_nvg.wav");
                if let Some(p) = g.players[usize::from(slot)].as_mut() {
                    p.has_night_vision = true;
                }
                money::add_account(g, eng, slot, -NVG_PRICE, true);
                send_item_status(eng, slot, true);
                return;
            }
        }
        SLOT_ITEM_DEFUSEKIT => {
            if team != Team::Ct || !g.rules.map_has_bomb_target() {
                return;
            }
            // The defuser is sold on a bomb map; the bomb targets are 2E (see the module
            // documentation), so there is no such map yet.
            return;
        }
        _ => return,
    }
    let Some((item, price)) = purchase else {
        not_enough_money(eng, slot);
        return;
    };
    give_armor(g, eng, slot, item);
    money::add_account(g, eng, slot, -price, true);
}

/// A weapon is bought (`BuyWeaponByWeaponID`): the original takes the money and gives the item;
/// there are no weapons yet (2D), so a purchase that passes `CanPlayerBuy` is refused with
/// `#Cannot_Buy_This`, no money taken.
fn refuse_weapon(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if can_player_buy(g, eng, slot, true) {
        say(eng, slot, "#Cannot_Buy_This");
    }
}

/// `BuyAmmo`: ammo for the weapons in the primary (or secondary) slot. A player has no weapon in
/// them yet, so this only asks `CanPlayerBuy`, which tells a player out of buy time.
fn buy_ammo(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    can_player_buy(g, eng, slot, true);
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
/// weapon (no map here has VIP safety zones, so the plain menus), the ammo, or the equipment.
fn choose_buy_group(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, choice: i32) {
    let ct = team_of(g, slot) == Team::Ct;
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
            let keys = MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_0;
            show(
                g,
                eng,
                slot,
                Menu::BuyShotgun,
                VGUI_MENU_BUY_SHOTGUN,
                keys,
                "#BuyShotgun",
            );
        }
        SLOT_BUY_SUBMACHINEGUN => {
            let text = if ct {
                "#CT_BuySubMachineGun"
            } else {
                "#T_BuySubMachineGun"
            };
            let keys = MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_0;
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
            let text = if ct { "#CT_BuyRifle" } else { "#T_BuyRifle" };
            show(
                g,
                eng,
                slot,
                Menu::BuyRifle,
                VGUI_MENU_BUY_RIFLE,
                KEYS_1_TO_6 | MENU_KEY_0,
                text,
            );
        }
        SLOT_BUY_MACHINEGUN => {
            let keys = MENU_KEY_1 | MENU_KEY_0;
            show(
                g,
                eng,
                slot,
                Menu::BuyMachineGun,
                VGUI_MENU_BUY_MACHINEGUN,
                keys,
                "#BuyMachineGun",
            );
        }
        SLOT_BUY_PRIMARY_AMMO | SLOT_BUY_SECONDARY_AMMO if in_zone(g) => buy_ammo(g, eng, slot),
        SLOT_BUY_ITEM if in_zone(g) => show_item_menu(g, eng, slot),
        _ => {}
    }
}

/// The `menuselect` of a text menu client in a buy menu (`menu` is the one the player is in).
/// The weapon menus' keys 1 to their last one are weapons, refused ([`refuse_weapon`]); any
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
    // `client.cpp:1177-1285`. Task 2D replaces the refusal with the purchase.
    let weapon_keys = match menu {
        Menu::Buy => {
            choose_buy_group(g, eng, slot, choice);
            return;
        }
        Menu::BuyItem => {
            buy_item(g, eng, slot, choice);
            return;
        }
        Menu::BuyPistol => 5,
        Menu::BuyShotgun => 2,
        Menu::BuySubMachineGun => 4,
        Menu::BuyRifle => 6,
        Menu::BuyMachineGun => 1,
        Menu::Off
        | Menu::ChooseTeam
        | Menu::ChooseAppearance
        | Menu::Radio1
        | Menu::Radio2
        | Menu::Radio3
        | Menu::ClientBuy => return,
    };
    if (1..=weapon_keys).contains(&choice) {
        refuse_weapon(g, eng, slot);
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

/// Whether `name` is a command of this module: the buy menus' commands, the buy aliases and the
/// weapon aliases.
pub(crate) fn is_buy_command(name: &str) -> bool {
    matches!(
        name,
        "buy" | "buyequip" | "buyammo1" | "buyammo2" | "client_buy_open" | "client_buy_close"
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
        "buyammo1" | "buyammo2" => {
            if in_zone {
                buy_ammo(g, eng, slot);
            }
        }
        alias => alias_command(g, eng, slot, alias),
    }
}

/// `HandleBuyAliasCommands`: the commands that buy one thing, for the players whose scripts
/// buy. Only a living player gets here: `ClientCommand_` calls it inside its `DEAD_NO` branch,
/// so a dead player is told nothing, not even `#Alias_Not_Avail` (checked 2026-11-22).
fn alias_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, command: &str) {
    let team = team_of(g, slot);
    if let Some(&(_, fail_name, teams)) = weapon_alias(command) {
        let allowed = match teams {
            'T' => team == Team::Terrorist,
            'C' => team == Team::Ct,
            _ => matches!(team, Team::Terrorist | Team::Ct),
        };
        if allowed {
            refuse_weapon(g, eng, slot);
        } else {
            client_print(eng, slot, HUD_PRINTCENTER, "#Alias_Not_Avail", &[fail_name]);
        }
        return;
    }
    match command {
        "primammo" | "secammo" => buy_ammo(g, eng, slot),
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
