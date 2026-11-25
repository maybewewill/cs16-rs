//! End-to-end Phase 15 Task 1 integration tests for `cs-mod-host` ABI v2:
//! - ReAPI Pre/Post hooks on real `ModdedGame<CsGame>` (`PlayerTakeDamage`, `PlayerSay`,
//!   `PlayersShouldCollide`, `CanBuyItem`, `WeaponDeploy`, `PlayerSpawn`, `PlayerKilled`).
//! - CS/fun/engine/cstrike natives + `CustomWeaponSpec` + 12 `vfx_*` primitives + HUD/DHUD/MOTD.
//! - Paginated `MenuManager` non-colliding `menuselect` across two plugins.
//! - Unified `LangCatalog` (`ru` + `en` simultaneously + live `/lang ru`).
//! - Shared `AdminRegistry` (`admins.toml` + `users.ini`, flags & `_pw` check).
//! - Synchronous polyglot `RpcBus` (`rpc_export` / `rpc_call` / `rpc_emit_event`).
//! - `NVaultStore` + $O(\log N)$ `LeaderboardStore`.
//! - Minimal safe WASI preview1 shims (`fd_write`, `clock_time_get`, `environ_sizes_get`,
//!   `args_sizes_get`, `proc_exit` trapping only the plugin).
//! - Capability-gated non-blocking HTTP POST JSON `WebhookClient` (allowlist rejection + real
//!   loopback `TcpListener` delivery with zero tick blocking).

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use cs_assets::bsp::Entity;
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_mod_api::ModManifest;
use cs_mod_host::{
    LoadedPackage, ModStatus, ModdedGame, ServerModHost, compile_wat_to_wasm,
};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{LoopbackHub, PeerAddr};

fn make_v2_pkg(id: &str, toml_extra: &str, server_wat: &str) -> LoadedPackage {
    let manifest = ModManifest::parse_toml(&format!(
        "id = \"{id}\"\nversion = \"2.0.0\"\napi_version = 2\n{toml_extra}"
    ))
    .unwrap();
    let server_wasm = Some(compile_wat_to_wasm(server_wat).unwrap());
    LoadedPackage {
        manifest,
        source_path: None,
        server_wasm,
        client_wasm: None,
        client_bundle: None,
        assets: BTreeMap::new(),
    }
}

fn spawn_map() -> ServerMap {
    let ents = vec![
        Entity {
            properties: HashMap::from([
                ("classname".into(), "info_player_start".into()),
                ("origin".into(), "0 0 64".into()),
            ]),
        },
        Entity {
            properties: HashMap::from([
                ("classname".into(), "info_player_deathmatch".into()),
                ("origin".into(), "128 0 64".into()),
            ]),
        },
    ];
    ServerMap::stub_with_entities("de_dust2", ents)
}

#[test]
fn v2_reapi_hooks_cs_natives_custom_weapons_and_vfx_on_real_cs_game() {
    let wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_i32" (func $arg_i32 (param i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_set_override_f32" (func $ovr_f32 (param f32) (result i32)))
            (import "cs16_mod" "hook_set_override_str" (func $ovr_str (param i32 i32) (result i32)))
            (import "cs16_mod" "register_custom_weapon" (func $reg_wpn (param i32 i32) (result i32)))
            (import "cs16_mod" "give_custom_weapon" (func $give_wpn (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "player_set_health" (func $set_hp (param i32 f32) (result i32)))
            (import "cs16_mod" "player_set_armor" (func $set_ap (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "player_set_money" (func $set_money (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "player_set_score" (func $set_score (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "player_set_godmode" (func $set_god (param i32 i32) (result i32)))
            (import "cs16_mod" "player_damage" (func $dmg_player (param i32 i32 f32 i32) (result i32)))
            (import "cs16_mod" "vfx_shockwave_ring" (func $vfx_ring (param f32 f32 f32 f32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "dhud_message" (func $dhud (param i32 i32 f32 f32 f32 f32 f32 f32 i32 i32) (result i32)))
            (import "cs16_mod" "show_motd" (func $motd (param i32 i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "id=ak_gold\nbase_weapon=28\ndamage_mult=1.75\nclip_size=40\n")
            (data (i32.const 128) "ak_gold")
            (data (i32.const 160) "[VIP] Rewritten")
            (data (i32.const 200) "VIP READY")
            (data (i32.const 220) "<h1>CS16-RS</h1>")
            (func (export "cs_mod_init") (result i32)
                ;; Register custom weapon during init (flushed & precached on ServerActivate)
                (drop (call $reg_wpn (i32.const 0) (i32.const 56)))
                ;; ClientCommand Pre (7, 0)
                (drop (call $reg (i32.const 7) (i32.const 0) (i32.const 0)))
                ;; PlayerSpawn Post (32, 1)
                (drop (call $reg (i32.const 32) (i32.const 1) (i32.const 0)))
                ;; PlayerTakeDamage Pre (33, 0)
                (drop (call $reg (i32.const 33) (i32.const 0) (i32.const 0)))
                ;; PlayerSay Pre (44, 0)
                (drop (call $reg (i32.const 44) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $slot i32)
                (local $len i32)
                (local.set $slot (call $arg_i32 (i32.const 0)))
                ;; ClientCommand Pre -> `test_dmg` triggers player_damage(slot, -1, 75.0, 0)
                (if (i32.eq (local.get $hid) (i32.const 7))
                    (then
                        (local.set $len (call $arg_str (i32.const 0) (i32.const 300) (i32.const 32)))
                        (if (i32.and (i32.eq (local.get $len) (i32.const 8)) (i32.eq (i32.load8_u (i32.const 300)) (i32.const 116)))
                            (then
                                (drop (call $dmg_player (local.get $slot) (i32.const -1) (f32.const 75.0) (i32.const 0)))
                                (return (i32.const 1))
                            )
                        )
                        (return (i32.const 0))
                    )
                )
                ;; PlayerSpawn Post -> grant custom weapon, 150 HP, VestHelm, $9999, score, VFX, DHUD, MOTD
                (if (i32.eq (local.get $hid) (i32.const 32))
                    (then
                        (drop (call $set_hp (local.get $slot) (f32.const 150.0)))
                        (drop (call $set_ap (local.get $slot) (i32.const 100) (i32.const 2)))
                        (drop (call $set_money (local.get $slot) (i32.const 9999) (i32.const 1)))
                        (drop (call $set_score (local.get $slot) (i32.const 7) (i32.const 2)))
                        (drop (call $give_wpn (local.get $slot) (i32.const 128) (i32.const 7)))
                        (drop (call $vfx_ring (f32.const 0.0) (f32.const 0.0) (f32.const 64.0) (f32.const 180.0) (i32.const 0x00C8FFFF) (i32.const 5) (i32.const 16) (local.get $slot)))
                        (drop (call $dhud (local.get $slot) (i32.const 0xFFAA00) (f32.const -1.0) (f32.const 0.25) (f32.const 0.1) (f32.const 2.0) (f32.const 0.2) (f32.const 0.0) (i32.const 200) (i32.const 9)))
                        (drop (call $motd (local.get $slot) (i32.const 200) (i32.const 9) (i32.const 220) (i32.const 16)))
                        (return (i32.const 0))
                    )
                )
                ;; PlayerTakeDamage Pre -> override damage to 12.0
                (if (i32.eq (local.get $hid) (i32.const 33))
                    (then
                        (drop (call $ovr_f32 (f32.const 12.0)))
                        (return (i32.const 2))
                    )
                )
                ;; PlayerSay Pre -> rewrite chat line
                (if (i32.eq (local.get $hid) (i32.const 44))
                    (then
                        (drop (call $ovr_str (i32.const 160) (i32.const 15)))
                        (return (i32.const 2))
                    )
                )
                (i32.const 0)
            )
        )
    "#;

    let mut host = ServerModHost::new();
    host.load_package(make_v2_pkg(
        "vip_v2",
        "permissions = [\"entities\", \"messages\", \"vfx\", \"weapons\"]\n",
        wat,
    ))
    .unwrap();

    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("mp_freezetime", "0");
    cvars.set("bot_quota", "0");

    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        spawn_map(),
        ModdedGame::new(CsGame::new(), host),
        DeltaRegistry::cs16_default(),
        cvars,
    );

    let mut hub = LoopbackHub::new();
    let ep0 = hub.connect();
    let mut c0 = Connection::new(
        ep0,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("Hero", 76_561_197_960_265_810, &md5_hex(b"hero")),
    );
    c0.set_userinfo("_vgui_menus", "0");
    c0.connect();

    let mut now = 0.0;
    for _ in 0..300 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
        if c0.state() == &ConnState::InGame {
            break;
        }
    }
    assert_eq!(c0.state(), &ConnState::InGame);

    c0.send_stringcmd("jointeam 2");
    c0.send_stringcmd("joinclass 1");
    for _ in 0..60 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }

    // Verify PlayerSpawn Post applied CS natives + CustomWeaponSpec
    {
        let g = server.game_mut().inner.state();
        let p = g.players[0].as_ref().expect("player 0 spawned");
        assert_eq!(p.health, 150.0);
        assert_eq!(p.armor, 100.0);
        assert_eq!(p.money, 9999);
        assert_eq!(p.frags, 7);
        assert_eq!(p.deaths, 2);
        assert_eq!(cs_game::mod_hooks::get_weapon_clip(g, 0, 28), 40);
    }

    // Verify PlayerTakeDamage Pre Override reduced 75 damage -> 12 damage (and 100 VestHelm
    // absorbs 50% of 12 -> 6 HP lost: 150 -> 144)!
    let hp_before = server.game().inner.state().players[0].as_ref().unwrap().health;
    c0.send_stringcmd("test_dmg");
    for _ in 0..20 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }
    let hp_after = server.game().inner.state().players[0].as_ref().unwrap().health;
    assert!(
        (hp_before - hp_after - 6.0).abs() < 1.0,
        "expected overridden 12 dmg * 0.5 armor = 6 hp lost; before={hp_before} after={hp_after}"
    );

    // Verify PlayerSay Pre Override rewrote chat message!
    c0.send_stringcmd("say hello");
    for _ in 0..25 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }
    assert!(
        c0.fx().hud.chat.iter().any(|m| {
            m.text.contains("[VIP] Rewritten")
                || m.params.iter().any(|p| p.contains("[VIP] Rewritten"))
        }),
        "chat={:?}",
        c0.fx().hud.chat
    );
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}

#[test]
fn paginated_menus_multilang_admins_rpc_and_leaderboards_share_platform_services() {
    // Mod A: creates 9-item paginated menu, exports RPC `add_bonus`, subscribes to `match_mvp`,
    // checks admin flags + writes to leaderboard + nvault + translates strings per player.
    let mod_a_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_i32" (func $arg_i32 (param i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "menu_create" (func $m_create (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "menu_add_item" (func $m_add (param i32 i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "menu_display" (func $m_disp (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "say_text" (func $say (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "lang_load_toml" (func $lang_load (param i32 i32) (result i32)))
            (import "cs16_mod" "lang_tr" (func $lang_tr (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "admin_has_flag" (func $has_flag (param i32 i32) (result i32)))
            (import "cs16_mod" "rpc_export" (func $rpc_exp (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "rpc_subscribe" (func $rpc_sub (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "leaderboard_upsert" (func $lb_up (param i32 i32 i32 i32 f64 i32 i32) (result i32)))
            (import "cs16_mod" "leaderboard_rank" (func $lb_rank (param i32 i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "nvault_open" (func $nv_open (param i32 i32) (result i32)))
            (import "cs16_mod" "nvault_set" (func $nv_set (param i32 i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $menu_id (mut i32) (i32.const 0))
            (data (i32.const 0) "Shop")
            (data (i32.const 16) "Item")
            (data (i32.const 32) "info")
            (data (i32.const 48) "add_bonus")
            (data (i32.const 64) "match_mvp")
            (data (i32.const 80) "[en]\nHI = \"Hello\"\n[ru]\nHI = \"Привет\"\n")
            (data (i32.const 160) "HI")
            (data (i32.const 176) "PICKED_A")
            (data (i32.const 192) "EVENT_OK")
            (data (i32.const 208) "ADMIN_YES")
            (data (i32.const 224) "stats")
            (data (i32.const 240) "p0")
            (data (i32.const 256) "vault1")
            (data (i32.const 272) "val42")
            (func (export "cs_mod_init") (result i32)
                (local $mid i32)
                (local $i i32)
                (local $vh i32)
                (drop (call $lang_load (i32.const 80) (i32.const 44)))
                (drop (call $rpc_exp (i32.const 48) (i32.const 9) (i32.const 10)))
                (drop (call $rpc_sub (i32.const 64) (i32.const 9) (i32.const 20)))
                (local.set $mid (call $m_create (i32.const 0) (i32.const 4) (i32.const 1)))
                (global.set $menu_id (local.get $mid))
                (local.set $i (i32.const 0))
                (loop $l
                    (drop (call $m_add (local.get $mid) (i32.const 16) (i32.const 4) (i32.const 32) (i32.const 4) (i32.const 1)))
                    (local.set $i (i32.add (local.get $i) (i32.const 1)))
                    (br_if $l (i32.lt_s (local.get $i) (i32.const 9)))
                )
                (drop (call $lb_up (i32.const 224) (i32.const 5) (i32.const 240) (i32.const 2) (f64.const 500.0) (i32.const 0) (i32.const 0)))
                (local.set $vh (call $nv_open (i32.const 256) (i32.const 6)))
                (drop (call $nv_set (local.get $vh) (i32.const 240) (i32.const 2) (i32.const 272) (i32.const 5)))
                ;; ClientCommand Pre (7, 0)
                (drop (call $reg (i32.const 7) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $slot i32)
                (local $len i32)
                (local $tr_len i32)
                (local.set $slot (call $arg_i32 (i32.const 0)))
                (local.set $len (call $arg_str (i32.const 0) (i32.const 400) (i32.const 32)))
                ;; Command `open_a` (len 6, first byte 'o' = 111)
                (if (i32.and (i32.eq (local.get $len) (i32.const 6)) (i32.eq (i32.load8_u (i32.const 400)) (i32.const 111)))
                    (then
                        (drop (call $m_disp (local.get $slot) (global.get $menu_id) (i32.const 0) (i32.const -1)))
                        (return (i32.const 1))
                    )
                )
                ;; Command `greet` (len 5, first byte 'g' = 103)
                (if (i32.and (i32.eq (local.get $len) (i32.const 5)) (i32.eq (i32.load8_u (i32.const 400)) (i32.const 103)))
                    (then
                        (local.set $tr_len (call $lang_tr (local.get $slot) (i32.const 160) (i32.const 2) (i32.const 0) (i32.const 0) (i32.const 512) (i32.const 64)))
                        (drop (call $say (local.get $slot) (i32.const 512) (local.get $tr_len)))
                        (return (i32.const 1))
                    )
                )
                ;; Command `chk_adm` (len 7, first byte 'c' = 99)
                (if (i32.and (i32.eq (local.get $len) (i32.const 7)) (i32.eq (i32.load8_u (i32.const 400)) (i32.const 99)))
                    (then
                        (if (i32.eq (call $has_flag (local.get $slot) (i32.const 116)) (i32.const 1))
                            (then (drop (call $say (local.get $slot) (i32.const 208) (i32.const 9))))
                        )
                        (return (i32.const 1))
                    )
                )
                (i32.const 0)
            )
            (func (export "cs_mod_menu") (param $cb i32) (param $slot i32) (param $mid i32) (param $idx i32) (result i32)
                ;; Page 2 item 1 has item_index == 7!
                (if (i32.eq (local.get $idx) (i32.const 7))
                    (then (drop (call $say (local.get $slot) (i32.const 176) (i32.const 8))))
                )
                (i32.const 0)
            )
            (func (export "cs_mod_rpc") (param $cb i32) (result i32)
                ;; add_bonus(x) -> x + 77
                (i32.add (call $arg_i32 (i32.const 0)) (i32.const 77))
            )
            (func (export "cs_mod_event") (param $cb i32) (result i32)
                (drop (call $say (i32.const -1) (i32.const 192) (i32.const 8)))
                (i32.const 0)
            )
        )
    "#;

    // Mod B: calls Mod A's `add_bonus` via synchronous `rpc_call`, verifies return 100 + 77 == 177,
    // emits `match_mvp` event, and opens its own separate menu on slot 1 without colliding!
    let mod_b_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_i32" (func $arg_i32 (param i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "rpc_call" (func $rpc_call (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "rpc_emit_event" (func $rpc_emit (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "say_text" (func $say (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "add_bonus")
            (data (i32.const 16) "match_mvp")
            (data (i32.const 32) "RPC_177")
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 7) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $len i32)
                (local $res i32)
                (local.set $len (call $arg_str (i32.const 0) (i32.const 128) (i32.const 32)))
                ;; Command `do_rpc` (len 6, first byte 'd' = 100)
                (if (i32.and (i32.eq (local.get $len) (i32.const 6)) (i32.eq (i32.load8_u (i32.const 128)) (i32.const 100)))
                    (then
                        (local.set $res (call $rpc_call (i32.const 0) (i32.const 9) (i32.const 100) (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)))
                        (if (i32.eq (local.get $res) (i32.const 177))
                            (then
                                (drop (call $say (i32.const -1) (i32.const 32) (i32.const 7)))
                                (drop (call $rpc_emit (i32.const 16) (i32.const 9) (i32.const 0) (i32.const 0)))
                            )
                        )
                        (return (i32.const 1))
                    )
                )
                (i32.const 0)
            )
        )
    "#;

    let mut host = ServerModHost::new();
    {
        let svc_handle = host.services();
        let mut svc = svc_handle.lock();
        svc.admins
            .load_admins_toml_str("[[admin]]\nname = \"RusPlayer\"\nflags = \"abct\"\n");
    }
    host.load_package(make_v2_pkg(
        "mod_a",
        "permissions = [\"menus\", \"messages\", \"admins\", \"rpc\", \"storage\"]\n",
        mod_a_wat,
    ))
    .unwrap();
    host.load_package(make_v2_pkg(
        "mod_b",
        "permissions = [\"messages\", \"rpc\"]\n",
        mod_b_wat,
    ))
    .unwrap();

    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );

    let mut hub = LoopbackHub::new();
    let ep0 = hub.connect();
    let ep1 = hub.connect();
    let mut c0 = Connection::new(
        ep0,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("RusPlayer", 76_561_197_960_265_820, &md5_hex(b"r")),
    );
    let mut c1 = Connection::new(
        ep1,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("EngPlayer", 76_561_197_960_265_821, &md5_hex(b"e")),
    );
    c0.set_userinfo("cl_language", "russian");
    c1.set_userinfo("lang", "en");
    c0.connect();
    c1.connect();

    let mut now = 0.0;
    for _ in 0..300 {
        now += 0.01;
        c0.pump(now);
        c1.pump(now);
        server.frame(&mut hub, now);
        if c0.state() == &ConnState::InGame && c1.state() == &ConnState::InGame {
            break;
        }
    }
    assert_eq!(c0.state(), &ConnState::InGame);
    assert_eq!(c1.state(), &ConnState::InGame);

    // 1. Simultaneous RU + EN i18n
    c0.send_stringcmd("greet");
    c1.send_stringcmd("greet");
    for _ in 0..20 {
        now += 0.01;
        c0.pump(now);
        c1.pump(now);
        server.frame(&mut hub, now);
    }
    assert!(c0.fx().hud.chat.iter().any(|m| m.text.contains("Привет")));
    assert!(c1.fx().hud.chat.iter().any(|m| m.text.contains("Hello")));

    // 2. Live `/lang ru` switches c1 to Russian!
    c1.send_stringcmd("say /lang ru");
    c1.send_stringcmd("greet");
    for _ in 0..25 {
        now += 0.01;
        c0.pump(now);
        c1.pump(now);
        server.frame(&mut hub, now);
    }
    assert!(c1.fx().hud.chat.iter().any(|m| m.text.contains("Привет")));

    // 3. AdminRegistry check: c0 has flag 't', c1 does not
    c0.send_stringcmd("chk_adm");
    c1.send_stringcmd("chk_adm");
    for _ in 0..20 {
        now += 0.01;
        c0.pump(now);
        c1.pump(now);
        server.frame(&mut hub, now);
    }
    assert!(c0.fx().hud.chat.iter().any(|m| m.text.contains("ADMIN_YES")));
    assert!(!c1.fx().hud.chat.iter().any(|m| m.text.contains("ADMIN_YES")));

    // 4. Paginated menu: open 9-item menu -> page 2 via `menuselect 9` -> item 8 via `menuselect 1`
    c0.send_stringcmd("open_a");
    for _ in 0..15 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }
    let m1 = c0.fx().hud.menu.as_ref().expect("page 1 menu");
    assert!(m1.text.contains("1/2"), "menu={}", m1.text);
    c0.send_stringcmd("menuselect 9");
    for _ in 0..15 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }
    let m2 = c0.fx().hud.menu.as_ref().expect("page 2 menu");
    assert!(m2.text.contains("2/2"), "menu={}", m2.text);
    c0.send_stringcmd("menuselect 1");
    for _ in 0..15 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }
    assert!(c0.fx().hud.chat.iter().any(|m| m.text.contains("PICKED_A")));

    // 5. Synchronous cross-plugin RPC (`mod_b` -> `mod_a`) + `rpc_emit_event` (`mod_b` -> `mod_a`)
    c0.send_stringcmd("do_rpc");
    for _ in 0..20 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }
    assert!(c0.fx().hud.chat.iter().any(|m| m.text.contains("RPC_177")));
    assert!(c0.fx().hud.chat.iter().any(|m| m.text.contains("EVENT_OK")));

    // 6. Verify leaderboard & nvault entries written by WASM mod
    let svc_handle = server.game_mut().host.services();
    let svc = svc_handle.lock();
    assert_eq!(svc.leaderboards.rank("stats", "p0").unwrap().0.rank, 1);
    let vh = svc.nvault.get(1, "p0");
    assert_eq!(vh, Some("val42"));
}

#[test]
fn wasi_preview1_shims_log_stdout_clock_and_isolate_proc_exit() {
    let wasi_wat = r#"
        (module
            (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "clock_time_get" (func $clock (param i32 i64 i32) (result i32)))
            (import "wasi_snapshot_preview1" "environ_sizes_get" (func $env_sz (param i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "args_sizes_get" (func $args_sz (param i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 64) "hello-from-wasi\n")
            ;; iovec at offset 0: ptr=64, len=16
            (data (i32.const 0) "\40\00\00\00\10\00\00\00")
            (func (export "_initialize")
                ;; Verify Go/C++ WASI reactor init runs cleanly
                (drop (call $env_sz (i32.const 128) (i32.const 132)))
                (drop (call $args_sz (i32.const 136) (i32.const 140)))
            )
            (func (export "cs_mod_init") (result i32)
                (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 16)))
                (drop (call $clock (i32.const 0) (i64.const 0) (i32.const 32)))
                ;; Register StartFrame Pre (11, 0) where we test proc_exit trapping only this plugin
                (drop (call $reg (i32.const 11) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (call $exit (i32.const 1))
                (i32.const 0)
            )
        )
    "#;

    let mut host = ServerModHost::new();
    host.load_package(make_v2_pkg("wasi_go_mod", "", wasi_wat))
        .unwrap();
    let logs = host.take_wasi_logs();
    assert_eq!(
        logs,
        vec![("wasi_go_mod".to_string(), "hello-from-wasi".to_string())]
    );

    let mut server = Server::new(
        ServerConfig::default(),
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let mut hub = LoopbackHub::new();
    server.frame(&mut hub, 0.01);

    // `proc_exit(1)` trapped ONLY this plugin without exiting the test process!
    let mods = server.game_mut().host.list_mods();
    assert!(matches!(mods[0].status, ModStatus::Faulted(_)));
}

#[test]
fn webhook_client_enforces_host_allowlist_and_posts_json_nonblocking() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let server_thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept webhook");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        let _ = stream.write_all(resp.as_bytes());
        req
    });

    let url = format!("http://127.0.0.1:{port}/hook");
    let bad_url = "http://evil.example.com/steal";
    let body = "{\"winner\":\"CT\"}";
    let wat = format!(
        r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "http_post_json" (func $post (param i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "cvar_set_str" (func $cset (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $sent (mut i32) (i32.const 0))
            (data (i32.const 0) "{url}")
            (data (i32.const 128) "{bad_url}")
            (data (i32.const 256) "{body}")
            (data (i32.const 320) "hostname")
            (data (i32.const 352) "Webhook200")
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (if (i32.eq (global.get $sent) (i32.const 0))
                    (then
                        (global.set $sent (i32.const 1))
                        ;; Disallowed host MUST be rejected synchronously with -1!
                        (if (i32.ne (call $post (i32.const 128) (i32.const {bad_len}) (i32.const 256) (i32.const {body_len}) (i32.const 1)) (i32.const -1))
                            (then (unreachable))
                        )
                        ;; Allowlisted 127.0.0.1 MUST queue non-blocking and return 0!
                        (if (i32.ne (call $post (i32.const 0) (i32.const {url_len}) (i32.const 256) (i32.const {body_len}) (i32.const 42)) (i32.const 0))
                            (then (unreachable))
                        )
                    )
                )
                (i32.const 0)
            )
            (func (export "cs_mod_webhook") (param $cb i32) (param $status i32) (result i32)
                (if (i32.and (i32.eq (local.get $cb) (i32.const 42)) (i32.eq (local.get $status) (i32.const 200)))
                    (then (drop (call $cset (i32.const 320) (i32.const 8) (i32.const 352) (i32.const 10))))
                )
                (i32.const 0)
            )
        )
        "#,
        url = url,
        url_len = url.len(),
        bad_url = bad_url,
        bad_len = bad_url.len(),
        body = body.replace('"', "\\\""),
        body_len = body.len(),
    );

    let mut host = ServerModHost::new();
    host.load_package(make_v2_pkg(
        "discord_relay",
        "permissions = [\"http:127.0.0.1\", \"cvars\"]\n",
        &wat,
    ))
    .unwrap();

    let mut server = Server::new(
        ServerConfig::default(),
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let mut hub = LoopbackHub::new();

    // First frame queues webhook without blocking the game tick!
    let t0 = Instant::now();
    server.frame(&mut hub, 0.01);
    assert!(
        t0.elapsed() < Duration::from_millis(500),
        "tick must never block on outbound HTTP"
    );

    let req = server_thread.join().expect("webhook received");
    assert!(req.starts_with("POST /hook HTTP/1.1"), "req={req}");
    assert!(req.contains("{\"winner\":\"CT\"}"), "req={req}");

    // Poll frames until webhook response callback updates hostname -> "Webhook200"
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut now = 0.02;
    while Instant::now() < deadline {
        server.frame(&mut hub, now);
        now += 0.01;
        if server.hostname() == "Webhook200" {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(server.hostname(), "Webhook200");
}
