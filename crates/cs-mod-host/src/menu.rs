//! Host-managed paginated GoldSrc `ShowMenu` manager (`1..7`, `8. Back`, `9. More`, `0. Exit`,
//! authentic CS 1.6 `\y`/`\w`/`\r`/`\d`/`\R` styling) with multi-part wire chunking so menus
//! never truncate on the 175-byte `ShowMenu` packet limit.

use cs_engine::{EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;

pub use cs_mod_api::services::{
    ManagedMenu, MenuManager, MenuSelectOutcome, RenderedMenu, mprop,
};

/// Maximum UTF-8 bytes sent in a single `ShowMenu` chunk (matching AMX Mod X `show_menu` /
/// HLSDK `ShowMenu` 175-byte safety ceiling so 255-byte usermsg limit is never exceeded).
const MAX_SHOW_MENU_CHUNK_BYTES: usize = 175;

/// Emits a [`RenderedMenu`] to its target slot using authentic GoldSrc multi-part `ShowMenu`
/// packets when the body exceeds 175 bytes.
pub fn send_rendered_menu(eng: &mut EngineApi<'_>, rendered: &RenderedMenu) {
    let text = rendered.text.as_str();
    if text.len() <= MAX_SHOW_MENU_CHUNK_BYTES {
        let _ = eng.send_usermsg(
            MsgDest::One(rendered.slot),
            &CsUserMsg::ShowMenu {
                keys_bitmask: rendered.keys_bitmask,
                display_time: rendered.time_sec,
                multipart: false,
                text: text.to_string(),
            },
        );
        return;
    }

    let mut start = 0usize;
    while start < text.len() {
        let mut end = (start + MAX_SHOW_MENU_CHUNK_BYTES).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            break;
        }
        let multipart = end < text.len();
        let _ = eng.send_usermsg(
            MsgDest::One(rendered.slot),
            &CsUserMsg::ShowMenu {
                keys_bitmask: rendered.keys_bitmask,
                display_time: rendered.time_sec,
                multipart,
                text: text[start..end].to_string(),
            },
        );
        start = end;
    }
}

/// Closes any open text menu on `slot` by sending an empty `ShowMenu`.
pub fn close_client_menu(eng: &mut EngineApi<'_>, slot: u8) {
    let _ = eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::ShowMenu {
            keys_bitmask: 0,
            display_time: 0,
            multipart: false,
            text: String::new(),
        },
    );
}
