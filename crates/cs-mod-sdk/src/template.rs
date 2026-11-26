//! Starter project generator for authoring ABI v2 WASM mods in Rust with `cs-mod-sdk`.

use std::collections::BTreeMap;

use cs_mod_api::{ABI_VERSION, ModSide};

/// Generates a starter Rust WASM mod repository layout (`Cargo.toml`, `mod.toml`, `lang.toml`,
/// `src/lib.rs`, `README.md`) with native `MockHostBridge` tests and `wasm32` exports.
pub fn scaffold_mod_project(id: &str, name: &str, side: ModSide) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();

    let side_str = side.as_str();
    let entry_server = if side.has_server() {
        "entry_server = \"server.wasm\"\n"
    } else {
        ""
    };
    let entry_client = if side.has_client() {
        "entry_client = \"client.wasm\"\n"
    } else {
        ""
    };

    files.insert(
        "mod.toml".to_string(),
        format!(
            "[mod]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\napi_version = {ABI_VERSION}\nside = \"{side_str}\"\n{entry_server}{entry_client}permissions = [\"messages\", \"entities\", \"menus\", \"vfx\", \"commands\", \"storage\"]\nfuel_per_frame = 500000\nmax_memory_bytes = 4194304\n"
        ),
    );

    files.insert(
        "lang.toml".to_string(),
        format!(
            "[en]\nWELCOME = \"\\x04[{name}]\\x01 Welcome, {{player}}! Type /hello for a shockwave.\"\nHELLO_REPLY = \"\\x04[{name}]\\x01 Hello {{player}}!\"\n\n[ru]\nWELCOME = \"\\x04[{name}]\\x01 Добро пожаловать, {{player}}! Введите /hello.\"\nHELLO_REPLY = \"\\x04[{name}]\\x01 Привет, {{player}}!\"\n"
        ),
    );

    files.insert(
        "Cargo.toml".to_string(),
        format!(
            "[package]\nname = \"{id}\"\nversion = \"0.1.0\"\nedition = \"2024\"\nlicense = \"GPL-3.0-or-later\"\n\n[lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n\n[dependencies]\ncs-mod-sdk = {{ path = \"../../crates/cs-mod-sdk\" }}\n"
        ),
    );

    files.insert(
        "src/lib.rs".to_string(),
        r#"use cs_mod_sdk::{
    HookAction, HookId, HookPhase, HostBridge, Player, cs_mod_chat, cs_mod_hook, cs_mod_init, vfx,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);
    host.hook_register(HookId::PlayerTakeDamage, HookPhase::Pre, 0);
    host.hook_register(HookId::PlayerSay, HookPhase::Pre, 0);
    0
}

#[cs_mod_hook(PlayerTakeDamage, pre)]
pub fn on_take_damage(host: &mut impl HostBridge) -> HookAction {
    let damage = host.hook_arg_f32(0);
    if damage > 150.0 {
        host.hook_set_override_f32(150.0);
        return HookAction::Override;
    }
    HookAction::Continue
}

#[cs_mod_chat("/hello", "!hello")]
pub fn on_hello_chat(host: &mut impl HostBridge, slot: u8) -> HookAction {
    let p = Player::new(slot);
    let name = p.name(host);
    p.say_tr(host, "HELLO_REPLY", &[("player", &name)]);
    if let Some(origin) = p.origin(host) {
        vfx::shockwave_ring(host, origin, 160.0, [0, 220, 255, 220], 6, 8, None);
    }
    HookAction::Handled
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::PlayerTakeDamage, HookPhase::Pre) => on_take_damage(host),
        (HookId::PlayerSay, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let msg = host.hook_arg_str(0).unwrap_or_default();
            if on_hello_chat_matches_chat(&msg) {
                return on_hello_chat(host, slot);
            }
            HookAction::Continue
        }
        _ => HookAction::Continue,
    }
}

cs_mod_sdk::cs16_wasm_exports! {
    init: init_mod,
    hook: dispatch_hook,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_mod_sdk::MockHostBridge;

    #[test]
    fn hello_chat_translates_and_caps_damage() {
        let mut host = MockHostBridge::default();
        host.add_player(0, "Neo");
        host.set_player_lang(0, "ru");
        assert_eq!(init_mod(&mut host), 0);

        host.active_f32_args[0] = 200.0;
        assert_eq!(dispatch_hook(&mut host, HookId::PlayerTakeDamage, HookPhase::Pre), HookAction::Override);
        assert_eq!(host.override_f32, Some(150.0));

        host.active_i32_args[0] = 0;
        host.active_str_args[0] = "/hello".to_string();
        assert_eq!(dispatch_hook(&mut host, HookId::PlayerSay, HookPhase::Pre), HookAction::Handled);
        assert!(host.say_texts[0].1.contains("Привет, Neo"));
    }
}
"#
        .to_string(),
    );

    files.insert(
        "README.md".to_string(),
        format!(
            "# {name}\n\nBuild and package with `cargo xtask mod build` or run unit tests with `cargo test`.\n"
        ),
    );

    files
}
