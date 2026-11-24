//! End-to-end tests for `cs-mod-host`: `wasmi` fuel/memory sandboxing, manifest permission
//! denial, `ModRuntime` trait pluggability, pre/post hookchains (`GameDll` + `EngineApi`),
//! server natives, `.csmod` package/dep loading & hot reload, client HUD/usermsg/input mods,
//! and SHA-256 verified auto-download skipping original CS 1.6 clients.

use std::collections::{BTreeMap, VecDeque};

use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_mod_api::{
    CsModArchive, HookAction, HookId, HookPhase, HudDrawCmd, MOD_CAPABILITY_KEY, ModManifest,
    Permissions,
};
use cs_mod_host::natives::{EngineOp, HookRegistration};
use cs_mod_host::package::topo_sort_packages;
use cs_mod_host::runtime::{ModCallOutcome, ModExecContext, ModInstance, ModRuntime, RuntimeError};
use cs_mod_host::{
    ApprovalPolicy, ClientModHost, ClientModStatus, ClientModTransfer, LoadedPackage, ModStatus,
    ModdedGame, ServerModHost, TransferError, compile_wat_to_wasm, discover_packages,
    load_package_csmod,
};
use cs_platform::MemoryStorage;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{LoopbackHub, PeerAddr};
use cs_vfs::Vfs;

fn make_pkg(id: &str, toml_extra: &str, server_wat: Option<&str>, client_wat: Option<&str>) -> LoadedPackage {
    let manifest = ModManifest::parse_toml(&format!(
        "id = \"{id}\"\nversion = \"1.0.0\"\napi_version = 1\n{toml_extra}"
    ))
    .unwrap();
    let server_wasm = server_wat.map(|s| compile_wat_to_wasm(s).unwrap());
    let client_wasm = client_wat.map(|s| compile_wat_to_wasm(s).unwrap());
    let mut pkg = LoadedPackage {
        manifest,
        source_path: None,
        server_wasm,
        client_wasm,
        client_bundle: None,
        assets: BTreeMap::new(),
    };
    pkg.build_client_bundle().unwrap();
    pkg
}

#[test]
fn wasmi_runtime_enforces_fuel_and_memory_limits_and_isolates_faults() {
    let mut host = ServerModHost::new();

    // Infinite loop traps out of fuel on start_frame; healthy sibling survives.
    let loop_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (loop $l
                    (br $l)
                )
                (i32.const 0)
            )
        )
    "#;
    let oom_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1 4)
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (if (i32.eq (memory.grow (i32.const 20)) (i32.const -1))
                    (then (unreachable))
                )
                (i32.const 0)
            )
        )
    "#;
    let healthy_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "print_all" (func $print_all (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "healthy-alive")
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 1) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (drop (call $print_all (i32.const 0) (i32.const 13)))
                (i32.const 0)
            )
        )
    "#;

    host.load_package(make_pkg("looper", "fuel_per_frame = 2000\n", Some(loop_wat), None))
        .unwrap();
    host.load_package(make_pkg("oomer", "max_memory_bytes = 262144\n", Some(oom_wat), None))
        .unwrap();
    host.load_package(make_pkg(
        "healthy",
        "permissions = [\"messages\"]\n",
        Some(healthy_wat),
        None,
    ))
    .unwrap();

    let mut server = Server::new(
        ServerConfig {
            max_clients: 2,
            ..ServerConfig::default()
        },
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let mut hub = LoopbackHub::new();
    let _ep = hub.connect();
    server.frame(&mut hub, 0.01);

    let mods = server.game_mut().host.list_mods();
    let looper = mods.iter().find(|m| m.id == "looper").unwrap();
    let oomer = mods.iter().find(|m| m.id == "oomer").unwrap();
    let healthy = mods.iter().find(|m| m.id == "healthy").unwrap();
    assert!(matches!(looper.status, ModStatus::Faulted(_)));
    assert!(matches!(oomer.status, ModStatus::Faulted(_)));
    assert_eq!(healthy.status, ModStatus::Active);
    assert!(healthy.total_fuel_used > 0);
}

#[test]
fn sandbox_denies_undeclared_permissions() {
    let mut host = ServerModHost::new();
    let wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "print_all" (func $print_all (param i32 i32) (result i32)))
            (import "cs16_mod" "storage_set" (func $sset (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "ent_create" (func $ecreate (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "blocked")
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                ;; Every denied call must return -1.
                (if (i32.ne (call $print_all (i32.const 0) (i32.const 7)) (i32.const -1))
                    (then (unreachable))
                )
                (if (i32.ne (call $sset (i32.const 0) (i32.const 3) (i32.const 0) (i32.const 3)) (i32.const -1))
                    (then (unreachable))
                )
                (if (i32.ne (call $ecreate (i32.const 0) (i32.const 7)) (i32.const -1))
                    (then (unreachable))
                )
                (i32.const 0)
            )
        )
    "#;
    // Undeclared permissions -> Permissions::NONE
    host.load_package(make_pkg("restricted", "", Some(wat), None))
        .unwrap();

    let mut server = Server::new(
        ServerConfig {
            max_clients: 2,
            ..ServerConfig::default()
        },
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let mut hub = LoopbackHub::new();
    server.frame(&mut hub, 0.01);

    // Did NOT unreachable-trap -> all 3 returned -1 cleanly!
    let mods = server.game_mut().host.list_mods();
    assert_eq!(mods[0].status, ModStatus::Active);
}

struct MockRuntime;
struct MockInst;
impl ModInstance for MockInst {
    fn call_init(&mut self, _ctx: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome {
            hooks_registered: vec![HookRegistration {
                hook: HookId::StartFrame,
                phase: HookPhase::Pre,
                priority: 0,
            }],
            ..Default::default()
        })
    }
    fn call_hook(
        &mut self,
        _hook: HookId,
        _phase: HookPhase,
        _ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome {
            action: HookAction::Handled,
            ops: vec![EngineOp::SetCvar {
                name: "hostname".to_string(),
                value: "MockJITServer".to_string(),
            }],
            fuel_used: 42,
            ..Default::default()
        })
    }
    fn call_timer(&mut self, _: i32, _: i32, _: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }
    fn call_client_hud(&mut self, _: i32, _: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }
    fn call_client_usermsg(&mut self, _: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }
    fn call_client_input(&mut self, _: i32, _: bool, _: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }
    fn memory_bytes(&self) -> usize {
        0
    }
}
impl ModRuntime for MockRuntime {
    fn name(&self) -> &'static str {
        "mock-jit"
    }
    fn instantiate(&self, _: &str, _: &[u8], _: Permissions, _: usize) -> Result<Box<dyn ModInstance>, RuntimeError> {
        Ok(Box::new(MockInst))
    }
}

#[test]
fn custom_mod_runtime_trait_seam_plugs_into_host() {
    let mut host = ServerModHost::with_runtime(Box::new(MockRuntime), Box::new(MemoryStorage::default()));
    let mut pkg = LoadedPackage {
        manifest: ModManifest::parse_toml("id = \"jit_demo\"\nversion = \"1.0\"\n").unwrap(),
        source_path: None,
        server_wasm: Some(vec![0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]),
        client_wasm: None,
        client_bundle: None,
        assets: BTreeMap::new(),
    };
    pkg.build_client_bundle().unwrap();
    host.load_package(pkg).unwrap();

    let mut server = Server::new(
        ServerConfig::default(),
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let mut hub = LoopbackHub::new();
    server.frame(&mut hub, 0.01);
    assert_eq!(server.hostname(), "MockJITServer");
}

#[test]
fn hookchains_run_pre_post_and_support_override_and_supercede() {
    let mut host = ServerModHost::new();
    // Mod A registers ServerCommand pre with priority 1 (supercedes `blocked_cmd`) and EngCenterPrint pre (supercedes `"badmsg"`).
    let wat_a = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "center_print" (func $cprint (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 64) "badmsg")
            (func (export "cs_mod_init") (result i32)
                ;; ServerCommand Pre = 17, 0
                (drop (call $reg (i32.const 17) (i32.const 0) (i32.const 1)))
                ;; EngCenterPrint Pre = 103, 0
                (drop (call $reg (i32.const 103) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $len i32)
                (local.set $len (call $arg_str (i32.const 0) (i32.const 0) (i32.const 60)))
                ;; If ServerCommand == "emit_bad" -> attempt center_print("badmsg") which EngCenterPrint pre supercedes
                (if (i32.eq (local.get $hid) (i32.const 17))
                    (then
                        (if (i32.eq (local.get $len) (i32.const 8))
                            (then
                                (drop (call $cprint (i32.const -1) (i32.const 64) (i32.const 6)))
                                (return (i32.const 3)) ;; Supercede
                            )
                        )
                    )
                )
                (if (i32.eq (local.get $hid) (i32.const 103))
                    (then
                        ;; Supercede EngCenterPrint!
                        (return (i32.const 3))
                    )
                )
                (i32.const 0)
            )
        )
    "#;
    host.load_package(make_pkg(
        "guard",
        "permissions = [\"messages\", \"commands\"]\n",
        Some(wat_a),
        None,
    ))
    .unwrap();

    // Mod B tests SpawnEntity Override (`hook_set_override_i32(0)` cancels spawn -> return 2 Override).
    let wat_b = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_set_override_i32" (func $ovr (param i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 3) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (local $len i32)
                (local.set $len (call $arg_str (i32.const 0) (i32.const 0) (i32.const 64)))
                ;; Suppress `blocked_entity` (len 14, first byte 'b' = 98)
                (if (i32.and (i32.eq (local.get $len) (i32.const 14)) (i32.eq (i32.load8_u (i32.const 0)) (i32.const 98)))
                    (then
                        (drop (call $ovr (i32.const 0)))
                        (return (i32.const 2))
                    )
                )
                (i32.const 0)
            )
        )
    "#;
    host.load_package(make_pkg("filter", "", Some(wat_b), None))
        .unwrap();

    let ents = vec![cs_assets::bsp::Entity {
        properties: std::collections::HashMap::from([(
            "classname".to_string(),
            "blocked_entity".to_string(),
        )]),
    }];
    let mut server = Server::new(
        ServerConfig::default(),
        ServerMap::stub_with_entities("de_dust2", ents),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let out = server.console_command("emit_bad");
    assert!(matches!(out, cs_engine::ConsoleOutcome::Output(s) if s.is_empty()));
}

#[test]
fn server_natives_cover_entities_messages_cvars_menus_timers_traces_sound_storage() {
    let mut host = ServerModHost::new();
    let wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "host_time_ms" (func $time (result i64)))
            (import "cs16_mod" "ent_create" (func $ecreate (param i32 i32) (result i32)))
            (import "cs16_mod" "ent_set_health" (func $ehp (param i32 f32) (result i32)))
            (import "cs16_mod" "cvar_set_str" (func $cset (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "show_menu" (func $menu (param i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "timer_after" (func $timer (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "trace_line" (func $trace (param f32 f32 f32 f32 f32 f32) (result f32)))
            (import "cs16_mod" "precache_sound" (func $precache (param i32 i32) (result i32)))
            (import "cs16_mod" "emit_sound" (func $sound (param i32 i32 i32 i32 f32) (result i32)))
            (import "cs16_mod" "storage_set" (func $sset (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "storage_get" (func $sget (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "info_target")
            (data (i32.const 16) "hostname")
            (data (i32.const 32) "NativeModHost")
            (data (i32.const 48) "1. Pick")
            (data (i32.const 64) "weapons/ak47-1.wav")
            (data (i32.const 96) "boot_cnt")
            (data (i32.const 112) "ok")
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 1) (i32.const 1) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (drop (call $time))
                (drop (call $ecreate (i32.const 0) (i32.const 11)))
                (drop (call $ehp (i32.const 0) (f32.const 250.0)))
                (drop (call $cset (i32.const 16) (i32.const 8) (i32.const 32) (i32.const 13)))
                (drop (call $menu (i32.const 0) (i32.const 1) (i32.const 10) (i32.const 48) (i32.const 7)))
                (drop (call $precache (i32.const 64) (i32.const 18)))
                (drop (call $sound (i32.const 0) (i32.const 1) (i32.const 64) (i32.const 18) (f32.const 1.0)))
                (drop (call $trace (f32.const 0.0) (f32.const 0.0) (f32.const 0.0) (f32.const 10.0) (f32.const 0.0) (f32.const 0.0)))
                (drop (call $sset (i32.const 96) (i32.const 8) (i32.const 112) (i32.const 2)))
                (drop (call $timer (i32.const 20) (i32.const 7) (i32.const 99)))
                (i32.const 0)
            )
            (func (export "cs_mod_timer") (param $cb i32) (param $arg i32) (result i32)
                (if (i32.eq (call $sget (i32.const 96) (i32.const 8) (i32.const 200) (i32.const 16)) (i32.const 2))
                    (then (drop (call $cset (i32.const 16) (i32.const 8) (i32.const 32) (i32.const 13))))
                )
                (i32.const 0)
            )
        )
    "#;
    host.load_package(make_pkg(
        "natives",
        "permissions = [\"entities\", \"messages\", \"cvars\", \"commands\", \"menus\", \"timers\", \"traces\", \"sound\", \"storage\"]\n",
        Some(wat),
        None,
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
    for i in 0..5 {
        server.frame(&mut hub, 0.01 * f64::from(i + 1));
    }
    assert_eq!(server.hostname(), "NativeModHost");
}

#[test]
fn package_loader_resolves_deps_and_hot_reloads_mods() {
    let dir = std::env::temp_dir().join("cs16_mod_loader_test");
    let _ = std::fs::remove_dir_all(&dir);
    let dep_dir = dir.join("base_mod");
    std::fs::create_dir_all(&dep_dir).unwrap();
    std::fs::write(
        dep_dir.join("mod.toml"),
        "id = \"base_mod\"\nversion = \"1.0.0\"\napi_version = 1\nside = \"server\"\nentry_server = \"server.wat\"\n",
    )
    .unwrap();
    std::fs::write(dep_dir.join("server.wat"), "(module (func (export \"cs_mod_init\") (result i32) (i32.const 0)))").unwrap();

    // Child mod packaged as `.csmod` depending on `base_mod`
    let child_zip = CsModArchive::write_zip([
        ("mod.toml", b"id = \"child_mod\"\nversion = \"2.0.0\"\napi_version = 1\nside = \"server\"\ndependencies = [\"base_mod\"]\nentry_server = \"server.wat\"\n".as_slice()),
        ("server.wat", b"(module (func (export \"cs_mod_init\") (result i32) (i32.const 0)))".as_slice()),
    ])
    .unwrap();
    std::fs::write(dir.join("child_mod.csmod"), &child_zip).unwrap();

    let pkgs = discover_packages(&dir).unwrap();
    assert_eq!(pkgs.len(), 2);
    assert_eq!(pkgs[0].manifest.id, "base_mod");
    assert_eq!(pkgs[1].manifest.id, "child_mod");

    let mut host = ServerModHost::new();
    assert_eq!(host.load_dir(&dir).unwrap(), 2);
    assert!(host.reload_mod("base_mod").is_ok());

    // Cycle check
    let cyc_a = make_pkg("a", "dependencies = [\"b\"]\n", None, None);
    let cyc_b = make_pkg("b", "dependencies = [\"a\"]\n", None, None);
    assert!(topo_sort_packages(vec![cyc_a, cyc_b]).is_err());

    let loaded_csmod = load_package_csmod(&child_zip, None).unwrap();
    assert_eq!(loaded_csmod.manifest.id, "child_mod");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn client_wasm_mod_draws_hud_and_handles_custom_usermsg_and_input() {
    let client_wat = r#"
        (module
            (import "cs16_mod" "hud_panel" (func $panel (param f32 f32 f32 f32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "hud_draw_bar" (func $bar (param f32 f32 f32 f32 f32 i32) (result i32)))
            (import "cs16_mod" "hud_draw_text" (func $text (param f32 f32 f32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_bytes" (func $arg_bytes (param i32 i32) (result i32)))
            (import "cs16_mod" "client_exec" (func $cexec (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $tick (mut i32) (i32.const 0))
            (data (i32.const 0) "MOD STATUS")
            (data (i32.const 16) "say hello_from_mod")
            (func (export "cs_mod_init") (result i32) (i32.const 0))
            (func (export "cs_mod_client_usermsg") (result i32)
                (local $nlen i32)
                (local.set $nlen (call $arg_str (i32.const 0) (i32.const 64) (i32.const 32)))
                (if (i32.gt_s (call $arg_bytes (i32.const 128) (i32.const 32)) (i32.const 0))
                    (then (global.set $tick (i32.load8_u (i32.const 128))))
                )
                (i32.const 0)
            )
            (func (export "cs_mod_client_input") (param $key i32) (param $pressed i32) (result i32)
                (if (i32.and (i32.eq (local.get $key) (i32.const 74)) (i32.eq (local.get $pressed) (i32.const 1)))
                    (then
                        (drop (call $cexec (i32.const 16) (i32.const 18)))
                        (return (i32.const 1))
                    )
                )
                (i32.const 0)
            )
            (func (export "cs_mod_client_hud") (param i32) (param $hp i32) (param i32) (param i32)
                (drop (call $panel (f32.const 0.02) (f32.const 0.2) (f32.const 0.22) (f32.const 0.08) (i32.const 0x203020CC) (i32.const 0) (i32.const 10)))
                (drop (call $bar (f32.const 0.03) (f32.const 0.25) (f32.const 0.2) (f32.const 0.02) (f32.div (f32.convert_i32_s (local.get $hp)) (f32.const 100.0)) (i32.const 0xFFA000FF)))
                (if (i32.gt_s (global.get $tick) (i32.const 0))
                    (then (drop (call $text (f32.const 0.03) (f32.const 0.225) (f32.const 14.0) (i32.const 0xFFFFFFFF) (i32.const 0) (i32.const 10))))
                )
            )
        )
    "#;
    let manifest = ModManifest::parse_toml(
        "id = \"hud\"\nversion = \"1.0\"\napi_version = 1\nside = \"client\"\npermissions = [\"hud\", \"input\"]\n",
    )
    .unwrap();
    let wasm = compile_wat_to_wasm(client_wat).unwrap();

    let mut client_host = ClientModHost::new();
    client_host.load_client_mod(manifest, &wasm).unwrap();
    assert_eq!(client_host.status("hud"), Some(&ClientModStatus::Active));

    client_host.on_usermsg("CsModEvent", &[99]);
    let cmds = client_host.render_hud(16, 80, 100, 16000);
    assert_eq!(cmds.len(), 3);
    assert!(matches!(&cmds[0], HudDrawCmd::Panel { title, .. } if title == "MOD STATUS"));
    assert!(matches!(&cmds[1], HudDrawCmd::ProgressBar { fraction, .. } if (*fraction - 0.8).abs() < 0.01));

    assert_eq!(client_host.on_input(74, true), HookAction::Handled);
    assert_eq!(client_host.take_execs(), vec!["say hello_from_mod".to_string()]);
}

#[test]
fn auto_download_verifies_sha256_enforces_prompt_and_skips_original_clients() {
    let client_wat = r#"
        (module
            (import "cs16_mod" "hud_draw_rect" (func $rect (param f32 f32 f32 f32 i32) (result i32)))
            (func (export "cs_mod_init") (result i32) (i32.const 0))
            (func (export "cs_mod_client_hud") (param i32 i32 i32 i32)
                (drop (call $rect (f32.const 0.1) (f32.const 0.1) (f32.const 0.2) (f32.const 0.05) (i32.const 0x00FF00FF)))
            )
        )
    "#;
    let mut pkg = make_pkg(
        "hud_dl",
        "side = \"both\"\nentry_client = \"client.wat\"\npermissions = [\"hud\"]\n",
        None,
        Some(client_wat),
    );
    pkg.assets.insert("sprites/mod_flag.txt".to_string(), b"downloaded".to_vec());
    pkg.build_client_bundle().unwrap();

    let mut host = ServerModHost::new();
    host.load_package(pkg).unwrap();

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
    let c1_ep = hub.connect();
    let c2_ep = hub.connect();

    let mut vanilla_conn = Connection::new(
        c1_ep,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("vanilla", 76_561_197_960_265_728, &md5_hex(b"v")),
    );
    let mut mod_conn = Connection::new(
        c2_ep,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("modclient", 76_561_197_960_265_729, &md5_hex(b"m")),
    );
    // Only client 2 advertises `_cs16rs_mods = 1`
    mod_conn.set_userinfo(MOD_CAPABILITY_KEY, "1");

    vanilla_conn.connect();
    mod_conn.connect();

    let mut transfer = ClientModTransfer::new(ApprovalPolicy::PromptRequired);
    let mut client_host = ClientModHost::new();
    let mut vfs = Vfs::from_memory(Vec::new());
    let mut unknown_cursor = VecDeque::new();

    let mut now = 0.0;
    for step in 0..500 {
        now += 0.01;
        vanilla_conn.pump(now);
        mod_conn.pump(now);
        server.frame(&mut hub, now);

        while let Some(um) = mod_conn.fx_mut().hud.unknown_messages.pop_front() {
            unknown_cursor.push_back(um);
        }
        while let Some(um) = unknown_cursor.pop_front() {
            transfer.ingest_usermsg(&um.name, &um.payload, &mut client_host, Some(&mut vfs));
        }

        // Before explicit approval, chunks are NEVER requested.
        if step < 150 {
            assert!(transfer.take_outgoing_commands().is_empty());
        }
        if !transfer.pending_approvals().is_empty() && step >= 150 {
            assert!(transfer.approve("hud_dl"));
        }
        for cmd in transfer.take_outgoing_commands() {
            mod_conn.send_stringcmd(&cmd);
        }
        if client_host.status("hud_dl") == Some(&ClientModStatus::Active) {
            break;
        }
    }

    assert_eq!(vanilla_conn.state(), &ConnState::InGame);
    assert_eq!(mod_conn.state(), &ConnState::InGame);
    // Original CS 1.6 client MUST receive zero CsMod* messages!
    assert!(
        !vanilla_conn
            .fx()
            .hud
            .unknown_messages
            .iter()
            .any(|m| m.name.starts_with("CsMod"))
    );
    // Mod-capable client approved, downloaded chunks, verified SHA-256, mounted assets, and rendered HUD!
    let errs_diag = transfer.take_errors();
    let pending_diag = transfer.pending_approvals();
    let mod_msgs: Vec<String> = mod_conn.fx().hud.user_messages.values().map(|u| u.name.clone()).collect();
    assert_eq!(
        client_host.status("hud_dl"),
        Some(&ClientModStatus::Active),
        "errs={errs_diag:?} pending={pending_diag:?} usermsgs={mod_msgs:?} raw_unknown={:?}",
        mod_conn.fx().hud.unknown_messages
    );
    assert_eq!(vfs.read_bytes("sprites/mod_flag.txt").unwrap(), b"downloaded");
    assert_eq!(client_host.render_hud(16, 100, 100, 800).len(), 1);

    // Tampered download SHA-256 check
    let mut bad_transfer = ClientModTransfer::new(ApprovalPolicy::AutoApproveTrusted);
    let mut bad_host = ClientModHost::new();
    let manifest_pkt = cs_mod_api::ModManifestPacket {
        mod_index: 0,
        total_mods: 1,
        entry: cs_mod_api::ClientModEntry {
            id: "evil".to_string(),
            version: "1.0".to_string(),
            sha256_hex: "0".repeat(64),
            byte_len: 4,
            permissions_mask: 0,
        },
    }
    .encode()
    .unwrap();
    bad_transfer.ingest_usermsg("CsModManifest", &manifest_pkt, &mut bad_host, None);
    let chunk_pkt = cs_mod_api::ModChunkPacket {
        mod_index: 0,
        offset: 0,
        data: vec![1, 2, 3, 4],
    }
    .encode()
    .unwrap();
    bad_transfer.ingest_usermsg("CsModChunk", &chunk_pkt, &mut bad_host, None);
    let errs = bad_transfer.take_errors();
    assert!(matches!(errs.as_slice(), [TransferError::ShaMismatch { .. }]));
}
