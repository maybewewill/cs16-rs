//! Zero-dependency pure-`proc_macro` attribute macros (`#![forbid(unsafe_code)]`) for
//! `cs16-rs` ABI v2 WASM mod authors.
//!
//! Supported attributes:
//! - `#[cs_mod_init]`
//! - `#[cs_mod_hook(PlayerTakeDamage, pre)]` (accepts both `PascalCase` and `snake_case`,
//!   plus optional priority `#[cs_mod_hook(PlayerTakeDamage, pre, 10)]`)
//! - `#[cs_mod_chat("/guns", "!guns")]`
//! - `#[cs_mod_cmd("amx_kick")]`
//! - `#[cs_mod_hud]`

use proc_macro::{TokenStream, TokenTree};

fn compile_error(msg: &str) -> TokenStream {
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    format!("compile_error!(\"{escaped}\");")
        .parse()
        .unwrap_or_default()
}

fn escape_rust_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
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

/// Normalizes any ABI v1/v2 hook identifier (`PascalCase` or `snake_case`) into its canonical
/// `cs_mod_api::HookId::as_name()` string.
fn canonical_hook_name(raw: &str) -> Option<&'static str> {
    let compact: String = raw
        .trim()
        .chars()
        .filter(|c| *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match compact.as_str() {
        // GameDll hooks (v1)
        "serveractivate" => Some("server_activate"),
        "serverdeactivate" => Some("server_deactivate"),
        "spawnentity" => Some("spawn_entity"),
        "clientconnect" => Some("client_connect"),
        "clientputinserver" => Some("client_put_in_server"),
        "clientdisconnect" => Some("client_disconnect"),
        "clientcommand" => Some("client_command"),
        "clientuserinfochanged" => Some("client_userinfo_changed"),
        "runusercmd" => Some("run_usercmd"),
        "postusercmd" => Some("post_usercmd"),
        "startframe" => Some("start_frame"),
        "think" => Some("think"),
        "touch" => Some("touch"),
        "blocked" => Some("blocked"),
        "updateclientdata" => Some("update_client_data"),
        "getweapondata" => Some("get_weapon_data"),
        "servercommand" => Some("server_command"),
        "addtofullpack" => Some("add_to_full_pack"),
        // Gameplay & ReAPI hooks (v1 + v2)
        "playerdeath" => Some("player_death"),
        "roundstart" => Some("round_start"),
        "playerspawn" => Some("player_spawn"),
        "playertakedamage" | "takedamage" => Some("player_take_damage"),
        "playerkilled" => Some("player_killed"),
        "playerprethink" => Some("player_pre_think"),
        "playerpostthink" => Some("player_post_think"),
        "playerjump" => Some("player_jump"),
        "playersshouldcollide" | "shouldcollide" => Some("players_should_collide"),
        "canbuyitem" => Some("can_buy_item"),
        "canhaveplayeritem" => Some("can_have_player_item"),
        "weapondeploy" => Some("weapon_deploy"),
        "weaponprimaryattack" => Some("weapon_primary_attack"),
        "weaponreload" => Some("weapon_reload"),
        "playersay" => Some("player_say"),
        "menuselect" => Some("menu_select"),
        "freezetimeend" => Some("freeze_time_end"),
        "roundend" => Some("round_end"),
        "bombplanting" => Some("bomb_planting"),
        "bombplanted" => Some("bomb_planted"),
        "bombdefusing" => Some("bomb_defusing"),
        "bombdefused" => Some("bomb_defused"),
        "bombexploded" => Some("bomb_exploded"),
        "hostagetouched" => Some("hostage_touched"),
        "hostagerescued" => Some("hostage_rescued"),
        "grenadethrow" => Some("grenade_throw"),
        // EngineApi hooks (v1)
        "engemitsound" => Some("eng_emit_sound"),
        "engsendusermsg" => Some("eng_send_usermsg"),
        "engclientprint" => Some("eng_client_print"),
        "engcenterprint" => Some("eng_center_print"),
        "engcreateentity" => Some("eng_create_entity"),
        "engremoveentity" => Some("eng_remove_entity"),
        "engkick" => Some("eng_kick"),
        "engchangelevel" => Some("eng_change_level"),
        _ => None,
    }
}

fn canonical_phase(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "pre" => Some("pre"),
        "post" => Some("post"),
        _ => None,
    }
}

fn parse_string_literal(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() < 2 || !trimmed.starts_with('"') || !trimmed.ends_with('"') {
        return None;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => out.push(other),
            }
        } else {
            out.push(ch);
        }
    }
    Some(out)
}

fn parse_string_literal_list(attr: TokenStream, macro_name: &str) -> Result<Vec<String>, String> {
    let mut items = Vec::new();
    let mut expect_comma = false;
    for tt in attr {
        match tt {
            TokenTree::Punct(p) if p.as_char() == ',' => {
                if !expect_comma {
                    return Err(format!("unexpected `,` in #[{macro_name}(...)]"));
                }
                expect_comma = false;
            }
            TokenTree::Literal(lit) => {
                if expect_comma {
                    return Err(format!("expected `,` between literals in #[{macro_name}(...)]"));
                }
                let raw = lit.to_string();
                let parsed = parse_string_literal(&raw).ok_or_else(|| {
                    format!("expected string literal in #[{macro_name}(...)], found `{raw}`")
                })?;
                if parsed.trim().is_empty() {
                    return Err(format!(
                        "empty string literal is not allowed in #[{macro_name}(...)]"
                    ));
                }
                items.push(parsed);
                expect_comma = true;
            }
            other => {
                return Err(format!(
                    "expected string literal in #[{macro_name}(...)], found `{other}`"
                ));
            }
        }
    }
    if items.is_empty() {
        return Err(format!(
            "#[{macro_name}(...)] requires at least one string literal"
        ));
    }
    Ok(items)
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

/// Annotates a pre/post hook handler and validates the hook name, phase, and optional priority
/// at compile time.
///
/// Usage:
/// - `#[cs_mod_hook(PlayerTakeDamage, pre)]`
/// - `#[cs_mod_hook(start_frame, post)]`
/// - `#[cs_mod_hook(PlayerSay, pre, -10)]`
#[proc_macro_attribute]
pub fn cs_mod_hook(attr: TokenStream, item: TokenStream) -> TokenStream {
    let Some(fn_name) = extract_fn_name(&item) else {
        return compile_error("#[cs_mod_hook] must be placed on a `fn`");
    };

    // Split top-level comma-separated argument groups so negative priorities (`-10`) parse cleanly.
    let mut groups: Vec<Vec<TokenTree>> = vec![Vec::new()];
    for tt in attr {
        if let TokenTree::Punct(ref p) = tt
            && p.as_char() == ','
        {
            groups.push(Vec::new());
            continue;
        }
        if let Some(last) = groups.last_mut() {
            last.push(tt);
        }
    }
    while groups.last().is_some_and(Vec::is_empty) {
        groups.pop();
    }

    if !(2..=3).contains(&groups.len()) {
        return compile_error(
            "expected `#[cs_mod_hook(<HookId>, pre|post)]` or `#[cs_mod_hook(<HookId>, pre|post, <priority>)]`",
        );
    }

    let hook_raw = match groups[0].as_slice() {
        [TokenTree::Ident(id)] => id.to_string(),
        _ => return compile_error("first argument of #[cs_mod_hook] must be a hook identifier"),
    };
    let phase_raw = match groups[1].as_slice() {
        [TokenTree::Ident(id)] => id.to_string(),
        _ => {
            return compile_error(
                "second argument of #[cs_mod_hook] must be `pre` or `post`",
            );
        }
    };
    let priority: i32 = if groups.len() == 3 {
        let joined: String = groups[2].iter().map(ToString::to_string).collect();
        match joined.parse::<i32>() {
            Ok(v) => v,
            Err(_) => {
                return compile_error(&format!(
                    "invalid priority `{joined}` in #[cs_mod_hook] (expected i32 integer)"
                ));
            }
        }
    } else {
        0
    };

    let Some(hook) = canonical_hook_name(&hook_raw) else {
        return compile_error(&format!("unknown hook `{hook_raw}` in #[cs_mod_hook]"));
    };
    let Some(phase) = canonical_phase(&phase_raw) else {
        return compile_error(&format!(
            "invalid phase `{phase_raw}` in #[cs_mod_hook] (expected `pre` or `post`)"
        ));
    };

    let upper = fn_name.to_ascii_uppercase();
    let meta: TokenStream = format!(
        "pub const CS_MOD_HOOK_{upper}: (&str, &str) = (\"{hook}\", \"{phase}\");\n\
         pub const CS_MOD_HOOK_{upper}_SPEC: (&str, &str, i32) = (\"{hook}\", \"{phase}\", {priority});\n"
    )
    .parse()
    .unwrap_or_default();
    let mut out = item;
    out.extend(meta);
    out
}

/// Declares one or more chat triggers handled by a `fn` (e.g. `#[cs_mod_chat("/guns", "!guns")]`).
///
/// Emits `CS_MOD_CHAT_<UPPER>: &[&str]` and a `<fn_name>_matches_chat(message: &str) -> bool` helper.
#[proc_macro_attribute]
pub fn cs_mod_chat(attr: TokenStream, item: TokenStream) -> TokenStream {
    let Some(fn_name) = extract_fn_name(&item) else {
        return compile_error("#[cs_mod_chat] must be placed on a `fn`");
    };
    let triggers = match parse_string_literal_list(attr, "cs_mod_chat") {
        Ok(list) => list,
        Err(msg) => return compile_error(&msg),
    };
    let upper = fn_name.to_ascii_uppercase();
    let elems = triggers
        .iter()
        .map(|s| format!("\"{}\"", escape_rust_str(s)))
        .collect::<Vec<_>>()
        .join(", ");
    let meta: TokenStream = format!(
        "pub const CS_MOD_CHAT_{upper}: &[&str] = &[{elems}];\n\
         pub fn {fn_name}_matches_chat(message: &str) -> bool {{\n\
             let trimmed = message.trim();\n\
             let first = trimmed.split_whitespace().next().unwrap_or(\"\");\n\
             CS_MOD_CHAT_{upper}.iter().any(|t| first.eq_ignore_ascii_case(t))\n\
         }}\n"
    )
    .parse()
    .unwrap_or_default();
    let mut out = item;
    out.extend(meta);
    out
}

/// Declares one or more console commands handled by a `fn` (e.g. `#[cs_mod_cmd("amx_kick")]`).
///
/// Emits `CS_MOD_CMD_<UPPER>: &[&str]` and a `<fn_name>_matches_cmd(cmd: &str) -> bool` helper.
#[proc_macro_attribute]
pub fn cs_mod_cmd(attr: TokenStream, item: TokenStream) -> TokenStream {
    let Some(fn_name) = extract_fn_name(&item) else {
        return compile_error("#[cs_mod_cmd] must be placed on a `fn`");
    };
    let cmds = match parse_string_literal_list(attr, "cs_mod_cmd") {
        Ok(list) => list,
        Err(msg) => return compile_error(&msg),
    };
    let upper = fn_name.to_ascii_uppercase();
    let elems = cmds
        .iter()
        .map(|s| format!("\"{}\"", escape_rust_str(s)))
        .collect::<Vec<_>>()
        .join(", ");
    let meta: TokenStream = format!(
        "pub const CS_MOD_CMD_{upper}: &[&str] = &[{elems}];\n\
         pub fn {fn_name}_matches_cmd(cmd: &str) -> bool {{\n\
             let trimmed = cmd.trim();\n\
             CS_MOD_CMD_{upper}.iter().any(|c| trimmed.eq_ignore_ascii_case(c))\n\
         }}\n"
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hook_accepts_pascal_and_snake_case_for_all_v2_hooks() {
        assert_eq!(canonical_hook_name("PlayerTakeDamage"), Some("player_take_damage"));
        assert_eq!(canonical_hook_name("player_take_damage"), Some("player_take_damage"));
        assert_eq!(canonical_hook_name("take_damage"), Some("player_take_damage"));
        assert_eq!(canonical_hook_name("PlayersShouldCollide"), Some("players_should_collide"));
        assert_eq!(canonical_hook_name("CanBuyItem"), Some("can_buy_item"));
        assert_eq!(canonical_hook_name("CanHavePlayerItem"), Some("can_have_player_item"));
        assert_eq!(canonical_hook_name("WeaponDeploy"), Some("weapon_deploy"));
        assert_eq!(canonical_hook_name("WeaponPrimaryAttack"), Some("weapon_primary_attack"));
        assert_eq!(canonical_hook_name("WeaponReload"), Some("weapon_reload"));
        assert_eq!(canonical_hook_name("PlayerSay"), Some("player_say"));
        assert_eq!(canonical_hook_name("MenuSelect"), Some("menu_select"));
        assert_eq!(canonical_hook_name("FreezeTimeEnd"), Some("freeze_time_end"));
        assert_eq!(canonical_hook_name("RoundStart"), Some("round_start"));
        assert_eq!(canonical_hook_name("RoundEnd"), Some("round_end"));
        assert_eq!(canonical_hook_name("BombPlanted"), Some("bomb_planted"));
        assert_eq!(canonical_hook_name("BombDefused"), Some("bomb_defused"));
        assert_eq!(canonical_hook_name("HostageRescued"), Some("hostage_rescued"));
        assert_eq!(canonical_hook_name("GrenadeThrow"), Some("grenade_throw"));
        assert_eq!(canonical_hook_name("start_frame"), Some("start_frame"));
        assert_eq!(canonical_hook_name("eng_emit_sound"), Some("eng_emit_sound"));
        assert_eq!(canonical_hook_name("nonexistent_hook"), None);
    }

    #[test]
    fn string_literal_parser_handles_escapes() {
        assert_eq!(parse_string_literal("\"/guns\"").as_deref(), Some("/guns"));
        assert_eq!(parse_string_literal("\"amx_kick\"").as_deref(), Some("amx_kick"));
        assert_eq!(parse_string_literal("\"a\\\"b\"").as_deref(), Some("a\"b"));
        assert_eq!(parse_string_literal("123"), None);
    }
}
