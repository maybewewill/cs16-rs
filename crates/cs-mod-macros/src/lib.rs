//! Zero-dependency pure-`proc_macro` attribute macros (`#![forbid(unsafe_code)]`) for
//! `cs16-rs` WASM mod authors.

use proc_macro::{TokenStream, TokenTree};

fn compile_error(msg: &str) -> TokenStream {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    format!("compile_error!(\"{escaped}\");")
        .parse()
        .unwrap_or_default()
}

fn extract_fn_name(item: &TokenStream) -> Option<String> {
    let mut saw_fn = false;
    for tt in item.clone() {
        if let TokenTree::Ident(ident) = tt {
            let s = ident.to_string();
            if saw_fn {
                return Some(s);
            }
            if s == "fn" {
                saw_fn = true;
            }
        }
    }
    None
}

fn is_valid_hook_name(name: &str) -> bool {
    matches!(
        name,
        "server_activate"
            | "server_deactivate"
            | "spawn_entity"
            | "client_connect"
            | "client_put_in_server"
            | "client_disconnect"
            | "client_command"
            | "client_userinfo_changed"
            | "run_usercmd"
            | "post_usercmd"
            | "start_frame"
            | "think"
            | "touch"
            | "blocked"
            | "update_client_data"
            | "get_weapon_data"
            | "server_command"
            | "add_to_full_pack"
            | "player_death"
            | "round_start"
            | "eng_emit_sound"
            | "eng_send_usermsg"
            | "eng_client_print"
            | "eng_center_print"
            | "eng_create_entity"
            | "eng_remove_entity"
            | "eng_kick"
            | "eng_change_level"
    )
}

/// Marks a mod initialization entrypoint and validates it sits on a `fn`.
#[proc_macro_attribute]
pub fn cs_mod_init(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let Some(fn_name) = extract_fn_name(&item) else {
        return compile_error("#[cs_mod_init] must be placed on a `fn`");
    };
    let meta: TokenStream = format!("pub const CS_MOD_INIT_SYMBOL: &str = \"{fn_name}\";\n")
        .parse()
        .unwrap_or_default();
    let mut out = item;
    out.extend(meta);
    out
}

/// Annotates a pre/post hook handler and validates the hook name and phase at compile time.
///
/// Usage: `#[cs_mod_hook(start_frame, pre)]`
#[proc_macro_attribute]
pub fn cs_mod_hook(attr: TokenStream, item: TokenStream) -> TokenStream {
    let Some(fn_name) = extract_fn_name(&item) else {
        return compile_error("#[cs_mod_hook] must be placed on a `fn`");
    };
    let mut idents = Vec::new();
    for tt in attr {
        if let TokenTree::Ident(id) = tt {
            idents.push(id.to_string());
        }
    }
    if idents.len() != 2 {
        return compile_error("expected `#[cs_mod_hook(<hook_name>, pre|post)]`");
    }
    let hook = idents[0].as_str();
    let phase = idents[1].as_str();
    if !is_valid_hook_name(hook) {
        return compile_error(&format!("unknown hook `{hook}` in #[cs_mod_hook]"));
    }
    if phase != "pre" && phase != "post" {
        return compile_error(&format!("invalid phase `{phase}` (expected `pre` or `post`)"));
    }
    let upper = fn_name.to_ascii_uppercase();
    let meta: TokenStream =
        format!("pub const CS_MOD_HOOK_{upper}: (&str, &str) = (\"{hook}\", \"{phase}\");\n")
            .parse()
            .unwrap_or_default();
    let mut out = item;
    out.extend(meta);
    out
}

/// Marks a client WASM HUD render callback.
#[proc_macro_attribute]
pub fn cs_mod_hud(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let Some(fn_name) = extract_fn_name(&item) else {
        return compile_error("#[cs_mod_hud] must be placed on a `fn`");
    };
    let meta: TokenStream = format!("pub const CS_MOD_HUD_SYMBOL: &str = \"{fn_name}\";\n")
        .parse()
        .unwrap_or_default();
    let mut out = item;
    out.extend(meta);
    out
}
