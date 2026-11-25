//! Safe high-level Rust SDK (`#![forbid(unsafe_code)]`) for authoring `cs16-rs` ABI v2 server
//! and client WASM mods.
//!
//! - On native targets (`#[cfg(not(target_arch = "wasm32"))]`), [`MockHostBridge`] lets every
//!   Rust mod run fast deterministic `cargo test` suites in milliseconds.
//! - On `wasm32-unknown-unknown`, guest `cdylib` mod crates invoke
//!   [`cs16_wasm_host_bridge!`] to bind every `"cs16_mod"` ABI v1 + v2 import cleanly without
//!   compromising the workspace-wide `#![forbid(unsafe_code)]` rule.

pub mod bridge;
#[cfg(not(target_arch = "wasm32"))]
pub mod mock;
pub mod player;
pub mod template;
pub mod ui;
pub mod vfx;
pub mod wasm_bridge;

pub use bridge::{
    HostBridge, encode_custom_weapon_spec, encode_lang_args, pack_rgb, pack_rgba,
    parse_leaderboard_rank_row, parse_leaderboard_top_rows,
};
pub use cs_mod_api::*;
pub use cs_mod_macros::{cs_mod_chat, cs_mod_cmd, cs_mod_hook, cs_mod_hud, cs_mod_init};
#[cfg(not(target_arch = "wasm32"))]
pub use mock::{MockEntity, MockHostBridge, MockPaginatedMenu, MockPlayer, MockTimer};
pub use player::{Player, color, render_fx, render_mode, weapon_id};
pub use ui::{CustomWeaponBuilder, HudCanvas, MENU_EXIT, MenuBuilder, PaginatedMenu, mprop, vgui};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[cs_mod_init]
    fn on_init() -> i32 {
        0
    }

    #[cs_mod_hook(start_frame, pre)]
    fn on_frame() -> HookAction {
        HookAction::Continue
    }

    #[cs_mod_hook(PlayerTakeDamage, pre, -5)]
    fn on_damage() -> HookAction {
        HookAction::Override
    }

    #[cs_mod_chat("/guns", "!guns")]
    fn on_guns_chat() -> HookAction {
        HookAction::Handled
    }

    #[cs_mod_cmd("amx_kick")]
    fn on_kick_cmd() -> HookAction {
        HookAction::Handled
    }

    #[cs_mod_hud]
    fn on_hud(canvas: &mut HudCanvas) {
        canvas.vgui_box(0.02, 0.2, 0.22, 0.12, "SDK");
        canvas.vgui_progress_bar(0.03, 0.28, 0.20, 0.02, 0.75, vgui::HUD_AMBER);
        canvas.amber_text(0.03, 0.24, 14.0, "READY");
    }

    #[test]
    fn macros_and_sdk_builders_compile_and_scaffold_valid_manifest() {
        assert_eq!(CS_MOD_INIT_SYMBOL, "on_init");
        assert_eq!(CS_MOD_HOOK_ON_FRAME, ("start_frame", "pre"));
        assert_eq!(CS_MOD_HOOK_ON_DAMAGE, ("player_take_damage", "pre"));
        assert_eq!(CS_MOD_HOOK_ON_DAMAGE_SPEC, ("player_take_damage", "pre", -5));
        assert_eq!(CS_MOD_CHAT_ON_GUNS_CHAT, &["/guns", "!guns"]);
        assert!(on_guns_chat_matches_chat("/guns ak47"));
        assert!(on_guns_chat_matches_chat("  !GUNS  "));
        assert!(!on_guns_chat_matches_chat("/rs"));
        assert_eq!(CS_MOD_CMD_ON_KICK_CMD, &["amx_kick"]);
        assert!(on_kick_cmd_matches_cmd("AMX_KICK"));
        assert!(!on_kick_cmd_matches_cmd("amx_ban"));
        assert_eq!(CS_MOD_HUD_SYMBOL, "on_hud");
        assert_eq!(on_init(), 0);
        assert_eq!(on_frame(), HookAction::Continue);
        assert_eq!(on_damage(), HookAction::Override);
        assert_eq!(on_guns_chat(), HookAction::Handled);
        assert_eq!(on_kick_cmd(), HookAction::Handled);

        let mut bridge = MockHostBridge::default();
        bridge.add_player(0, "Alice");
        bridge.add_player(1, "Ivan");
        bridge.set_player_lang(1, "ru");
        bridge.set_player_admin_flags(0, AdminFlags::parse_str("abct"));
        bridge.lang_load_toml(
            "[en]\nGREET = \"Hello {name}!\"\n[ru]\nGREET = \"Привет, {name}!\"\n",
        );

        let alice = Player::new(0);
        let ivan = Player::new(1);
        assert_eq!(alice.tr(&bridge, "GREET", &[("name", "Alice")]), "Hello Alice!");
        assert_eq!(ivan.tr(&bridge, "GREET", &[("name", "Ivan")]), "Привет, Ivan!");
        assert!(alice.has_flag(&bridge, 't'));
        assert!(!ivan.has_flag(&bridge, 't'));

        alice.set_health(&mut bridge, 125.0);
        alice.set_armor(&mut bridge, 100, ArmorKind::VestHelm);
        alice.add_money(&mut bridge, 2000, true);
        alice.give_weapon(&mut bridge, "ak47");
        alice.set_glow(&mut bridge, [255, 160, 0], 16);
        assert_eq!(alice.health(&bridge), 125.0);
        assert_eq!(alice.armor(&bridge), 100);
        assert_eq!(alice.armor_type(&bridge), ArmorKind::VestHelm);
        assert_eq!(alice.money(&bridge), 2800);
        assert!(bridge.players[&0].items.iter().any(|w| w == "weapon_ak47"));

        // 1-line VFX builder
        vfx::shockwave_ring(&mut bridge, [1.0, 2.0, 3.0], 200.0, [0, 255, 255, 200], 8, 10, None);
        vfx::laser_beam(&mut bridge, [0.0; 3], [100.0, 0.0, 0.0], [255, 0, 0, 255], 5, 4, 0, None);
        vfx::aura_light(&mut bridge, [10.0, 20.0, 30.0], 24, [255, 200, 50], 10, 5, Some(0));
        assert_eq!(bridge.vfx_ops.len(), 3);

        // Classic raw MenuBuilder + ABI v2 PaginatedMenu
        MenuBuilder::new("Admin")
            .item(1, "Kick")
            .item(2, "Slay")
            .send_to(&mut bridge, 0);
        assert_eq!(bridge.menus.len(), 1);
        assert_eq!(bridge.menus[0].1 & 0b11, 0b11);

        let mid = PaginatedMenu::new("Weapons", 7)
            .item("AK-47", "weapon_ak47")
            .item("M4A1", "weapon_m4a1")
            .blank()
            .text("VIP perks enabled")
            .per_page(7)
            .display_to(&mut bridge, 0, 0, 15)
            .expect("menu created");
        assert_eq!(bridge.paginated_menus[&mid].items.len(), 4);
        assert_eq!(bridge.menu_displays, vec![(0, mid, 0, 15)]);

        // Declarative CustomWeaponBuilder
        let spec = CustomWeaponBuilder::new("golden_ak47", weapon_id::AK47)
            .display_name("Golden AK-47")
            .v_model("models/v_golden_ak47.mdl")
            .damage_mult(1.5)
            .clip_size(35)
            .knockback(2.0)
            .cost(5000)
            .build();
        let encoded = encode_custom_weapon_spec(&spec);
        assert!(encoded.contains("id=golden_ak47"));
        assert!(encoded.contains("base_weapon=28"));
        assert!(encoded.contains("damage_mult=1.5"));
        assert_eq!(bridge.register_custom_weapon(&spec), 0);
        alice.give_custom_weapon(&mut bridge, "golden_ak47");
        assert_eq!(bridge.custom_weapons.len(), 1);
        assert_eq!(bridge.given_custom_weapons, vec![(0, "golden_ak47".to_string())]);

        // VGUI2 HudCanvas
        let mut canvas = HudCanvas::new();
        on_hud(&mut canvas);
        canvas.flush(&mut bridge);
        let cmds = canvas.finish();
        assert!(cmds.len() >= 7);
        assert_eq!(bridge.hud_cmds.len(), cmds.len());

        // Leaderboards + Webhooks + NVault
        bridge.leaderboard_upsert("stats", "STEAM_0:0:1", 42.0, "{\"k\":42}");
        bridge.leaderboard_upsert("stats", "STEAM_0:0:2", 99.0, "{\"k\":99}");
        let (rank_entry, total) = bridge.leaderboard_rank("stats", "STEAM_0:0:1").unwrap();
        assert_eq!(rank_entry.rank, 2);
        assert_eq!(total, 2);
        let top = bridge.leaderboard_top("stats", 0, 10);
        assert_eq!(top[0].key, "STEAM_0:0:2");

        // Scaffold validation
        let files = template::scaffold_mod_project("my_mod", "My Mod", ModSide::Both);
        let parsed = ModManifest::parse_toml(&files["mod.toml"]).unwrap();
        assert_eq!(parsed.id, "my_mod");
        assert_eq!(parsed.api_version, ABI_VERSION);
        assert_eq!(parsed.side, ModSide::Both);
        assert!(files.contains_key("src/lib.rs"));
        assert!(files.contains_key("lang.toml"));
    }

    #[test]
    fn wire_helpers_pack_and_parse_exact_host_formats() {
        assert_eq!(pack_rgba([0x12, 0x34, 0x56, 0x78]), 0x1234_5678_u32 as i32);
        assert_eq!(pack_rgb([0xAA, 0xBB, 0xCC]), 0x00AA_BBCC);
        assert_eq!(encode_lang_args(&[("weapon", "AK-47"), ("cost", "2500")]), "weapon=AK-47\ncost=2500");

        let (entry, total) = parse_leaderboard_rank_row("STEAM_1", "2\t15\t120.5\t{\"k\":10}").unwrap();
        assert_eq!(entry.rank, 2);
        assert_eq!(total, 15);
        assert_eq!(entry.score, 120.5);

        let rows = parse_leaderboard_top_rows("1\tSTEAM_2\t200\t{}\n2\tSTEAM_1\t120.5\t{\"k\":10}\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "STEAM_2");
        assert_eq!(rows[1].rank, 2);
    }
}
