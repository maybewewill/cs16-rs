//! The menus the server shows (`ShowMenu`, `ShowVGUIMenu`, `ResetMenu`) and the `menuselect`
//! command that answers the text version of them. Ported from ReGameDLL_CS (`client.cpp`,
//! `player.cpp`, `cdll_dll.h`, read 2026-11-22), the vanilla `mp.dll` behaviour.
//!
//! A player is in at most one menu, [`Menu`] (`m_iMenu`); the game answers `menuselect N`
//! according to it. A client with `_vgui_menus 1` (the default) gets the VGUI menu from
//! `VGUIMenu` and answers with `jointeam`/`joinclass`; one with `_vgui_menus 0` gets the text
//! menu from `ShowMenu` and answers with `menuselect`.
//!
//! The menus of the buy and radio commands (`Menu_Buy` ... `Menu_ClientBuy`, `Menu_Radio1` ...)
//! come with the economy and the radio; [`Menu`] has only the ones the team selection uses.

use cs_engine::{EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;

use crate::GameState;
use crate::teams::{self, JoinState};

/// `_Menu` (`player.h`): the menu a player is looking at (`Menu_IGChooseTeam` is never set by
/// the game and is left out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Menu {
    #[default]
    Off,
    ChooseTeam,
    ChooseAppearance,
}

/// `VGUI_Menu_Team`, `VGUI_Menu_Class_T`, `VGUI_Menu_Class_CT` (`cdll_dll.h`): the ids the
/// client's `VGUIMenu` handler opens a panel by.
pub const VGUI_MENU_TEAM: u8 = 2;
pub const VGUI_MENU_CLASS_T: u8 = 26;
pub const VGUI_MENU_CLASS_CT: u8 = 27;
/// `VGUI_Menu_Buy_Item`, the last of the menus a `_vgui_menus 0` client may see as text:
/// every menu above it is VGUI only.
pub const VGUI_MENU_BUY_ITEM: u8 = 34;

/// `MENU_KEY_1` to `MENU_KEY_6` and `MENU_KEY_0` (`cdll_dll.h`): the keys a menu accepts.
pub const MENU_KEY_1: u16 = 1 << 0;
pub const MENU_KEY_2: u16 = 1 << 1;
pub const MENU_KEY_3: u16 = 1 << 2;
pub const MENU_KEY_4: u16 = 1 << 3;
pub const MENU_KEY_5: u16 = 1 << 4;
pub const MENU_KEY_6: u16 = 1 << 5;
pub const MENU_KEY_0: u16 = 1 << 9;

pub(crate) use crate::chat::{HUD_PRINTCENTER, HUD_PRINTCONSOLE, HUD_PRINTNOTIFY};

/// `atoi` as the C library does it: leading white space, an optional sign, then digits; the
/// rest is ignored and a string with no number is 0. A number too large for `i32` saturates
/// (the C behaviour is undefined).
pub(crate) fn atoi(s: &str) -> i32 {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut value: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        value = (value * 10 + i64::from(b - b'0')).min(i64::from(i32::MAX) + 1);
    }
    let value = if negative { -value } else { value };
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// `ShowMenu`: the text menu `text` (a localization key such as `#Team_Select`) with the keys
/// of `keys`, to `slot`. `display_time` is in seconds, -1 for no limit.
pub(crate) fn show_menu(
    eng: &mut EngineApi<'_>,
    slot: u8,
    keys: u16,
    display_time: i8,
    text: &str,
) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::ShowMenu {
            keys_bitmask: keys,
            display_time,
            multipart: false,
            text: text.to_string(),
        },
    );
}

/// `ShowVGUIMenu`: the VGUI menu `menu_id` to a client that wants them (`_vgui_menus`, and every
/// menu above [`VGUI_MENU_BUY_ITEM`]), else the text menu `old_menu`.
pub(crate) fn show_vgui_menu(
    eng: &mut EngineApi<'_>,
    slot: u8,
    vgui: bool,
    menu_id: u8,
    keys: u16,
    old_menu: &str,
) {
    if vgui || menu_id > VGUI_MENU_BUY_ITEM {
        eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::VguiMenu {
                menu_id,
                keys_bitmask: keys,
                time: -1,
                multipart: false,
                name: " ".to_string(),
            },
        );
    } else {
        show_menu(eng, slot, keys, -1, old_menu);
    }
}

/// `CBasePlayer::ResetMenu` as the vanilla build has it: closes the text menu on the client
/// (an empty `ShowMenu`). It leaves `m_iMenu` alone.
pub(crate) fn reset_menu_message(eng: &mut EngineApi<'_>, slot: u8) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::ShowMenu {
            keys_bitmask: 0,
            display_time: 0,
            multipart: false,
            text: String::new(),
        },
    );
}

/// `ClientPrint(HUD_PRINTCENTER, text)`: `text` (a localization key) in the middle of the
/// screen of `slot`.
pub(crate) fn print_center(eng: &mut EngineApi<'_>, slot: u8, text: &str) {
    crate::chat::client_print(eng, slot, HUD_PRINTCENTER, text, &[]);
}

/// `UTIL_ClientPrintAll(HUD_PRINTNOTIFY, text, param)`: the notice line of everyone's console
/// and chat.
pub(crate) fn print_all_notify(eng: &mut EngineApi<'_>, text: &str, param: &str) {
    crate::chat::client_print_all(eng, HUD_PRINTNOTIFY, text, &[param]);
}

/// `UTIL_ClientPrintAll(dest, text, params...)`: `text` (a localization key) with its parameters
/// to everyone.
pub(crate) fn print_all(eng: &mut EngineApi<'_>, dest: u8, text: &str, params: &[&str]) {
    crate::chat::client_print_all(eng, dest, text, params);
}

/// `UTIL_ClientPrintAll(HUD_PRINTCENTER, text)`.
pub(crate) fn print_center_all(eng: &mut EngineApi<'_>, text: &str) {
    print_all(eng, HUD_PRINTCENTER, text, &[]);
}

/// `UTIL_ClientPrintAll(HUD_PRINTCONSOLE, text)`.
pub(crate) fn print_console_all(eng: &mut EngineApi<'_>, text: &str) {
    print_all(eng, HUD_PRINTCONSOLE, text, &[]);
}

/// The `menuselect` branch of `ClientCommand_` (ReGameDLL `client.cpp`): `menuselect N` is
/// answered according to the player's [`Menu`], and only when the client uses text menus
/// (`_vgui_menus 0`: a VGUI client answers with `jointeam` and `joinclass`). Key 10 (`0`)
/// closes a menu the player is not still choosing a team or appearance in.
pub(crate) fn menuselect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
    let choice = atoi(argv.get(1).map_or("", String::as_str));
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if (p.joining == JoinState::Joined
        || (p.menu != Menu::ChooseAppearance && p.menu != Menu::ChooseTeam))
        && choice == 10
    {
        p.menu = Menu::Off;
    }
    let text_menus = !p.vgui_menus;
    match p.menu {
        Menu::Off => {}
        Menu::ChooseTeam => {
            if text_menus && !teams::handle_menu_choose_team(g, eng, slot, choice) {
                teams::show_team_menu_again(g, eng, slot);
            }
        }
        Menu::ChooseAppearance => {
            if text_menus {
                teams::handle_menu_choose_appearance(g, eng, slot, choice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoi_reads_like_the_c_library() {
        assert_eq!(atoi("7"), 7);
        assert_eq!(atoi("  -12abc"), -12);
        assert_eq!(atoi("+3"), 3);
        assert_eq!(atoi(""), 0);
        assert_eq!(atoi("abc"), 0);
        assert_eq!(atoi("-"), 0);
        assert_eq!(atoi("99999999999999999999"), i32::MAX);
        assert_eq!(atoi("-99999999999999999999"), i32::MIN);
        assert_eq!(atoi(&"9".repeat(4096)), i32::MAX);
    }

    #[test]
    fn menu_ids_and_keys_are_the_clients() {
        assert_eq!(
            (VGUI_MENU_TEAM, VGUI_MENU_CLASS_T, VGUI_MENU_CLASS_CT),
            (2, 26, 27)
        );
        assert_eq!(MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5, 0b1_0011);
        assert_eq!(MENU_KEY_0, 0x200);
    }
}
