//! End-to-end integration tests for Phase 15 Task 7:
//! - "Black-Box" Time-Travel Plugin Crash Recorder (`ModFaultDump`, `FaultDumpStore`)
//! - Live fault isolation on `ModdedGame<CsGame>` (divide-by-zero, `OutOfFuel`, timer trap, panic)
//!   keeping the 100 Hz server & healthy sibling plugins alive
//! - Bounded JSON serialization (`<= 256 KiB`), 32-entry ring buffer eviction on disk & `ConfigStorage`
//! - Deterministic offline replay (`replay_fault_dump`, `replay_fault_dump_from_path`) reproducing
//!   the exact `FAULT` on buggy WASM / AMXX binaries and returning `PASS` with exact `EngineOp`s
//!   and storage mutations on fixed binaries.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use cs_amxx::amxxfile::build_test_amxx_plugin;
use cs_amxx::vm::op::*;
use cs_assets::bsp::Entity;
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_mod_api::{HookAction, HookId, HookPhase, ModManifest, Permissions, sha256_hex};
use cs_mod_host::crash_dump::{
    DEFAULT_RNG_SEED, FAULT_DUMP_SCHEMA_VERSION, InvocationTrigger, MAX_FAULT_DUMP_BYTES,
    MAX_STORED_FAULTS, ModFaultDump, PluginLanguage, ReplayOutcome, ReplayTarget, StorageMutation,
    default_fault_dump_dir, replay_fault_dump, replay_fault_dump_from_path,
};
use cs_mod_host::natives::{ActiveEvent, ClientSnap, EngineOp, EngineSnapshot, HookRegistration};
use cs_mod_host::runtime::{ModCallOutcome, ModExecContext, ModInstance, ModRuntime, RuntimeError};
use cs_mod_host::{
    LoadedPackage, ModStatus, ModdedGame, ServerModHost, compile_wat_to_wasm,
};
use cs_platform::MemoryStorage;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{LoopbackHub, PeerAddr};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDirGuard(PathBuf);

impl TempDirGuard {
    fn new(label: &str) -> Self {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "cs16-p15-t7-{label}-{}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        Self(dir)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
fn player_take_damage_divide_by_zero_dump_and_replay_fix_end_to_end() {
    let tmp = TempDirGuard::new("divzero");
    let fault_dir = default_fault_dump_dir(tmp.path());

    // Buggy plugin: on ClientCommand `seed_cfg`, persists storage `"scale" = "3"`;
    // on `test_dmg`, deals 75.0 damage to trigger `PlayerTakeDamage Pre`;
    // in `PlayerTakeDamage Pre`, computes `i32.div_s(dmg_int, 0)` -> WASM divide-by-zero trap!
    let buggy_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_i32" (func $arg_i32 (param i32) (result i32)))
            (import "cs16_mod" "hook_arg_f32" (func $arg_f32 (param i32) (result f32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "player_damage" (func $dmg_player (param i32 i32 f32 i32) (result i32)))
            (import "cs16_mod" "storage_set" (func $sset (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "scale")
            (data (i32.const 16) "3")
            (func (export "cs_mod_init") (result i32)
                ;; ClientCommand Pre (7, 0)
                (drop (call $reg (i32.const 7) (i32.const 0) (i32.const 0)))
                ;; PlayerTakeDamage Pre (33, 0)
                (drop (call $reg (i32.const 33) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $slot i32)
                (local $len i32)
                (local $dmg i32)
                (local.set $slot (call $arg_i32 (i32.const 0)))
                (if (i32.eq (local.get $hid) (i32.const 7))
                    (then
                        (local.set $len (call $arg_str (i32.const 0) (i32.const 128) (i32.const 32)))
                        ;; "seed_cfg" starts with 's' (115), len 8
                        (if (i32.and (i32.eq (local.get $len) (i32.const 8)) (i32.eq (i32.load8_u (i32.const 128)) (i32.const 115)))
                            (then
                                (drop (call $sset (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 1)))
                                (return (i32.const 1))
                            )
                        )
                        ;; "test_dmg" starts with 't' (116), len 8
                        (if (i32.and (i32.eq (local.get $len) (i32.const 8)) (i32.eq (i32.load8_u (i32.const 128)) (i32.const 116)))
                            (then
                                (drop (call $dmg_player (local.get $slot) (i32.const -1) (f32.const 75.0) (i32.const 0)))
                                (return (i32.const 1))
                            )
                        )
                        (return (i32.const 0))
                    )
                )
                (if (i32.eq (local.get $hid) (i32.const 33))
                    (then
                        ;; Intentional bug: integer divide by zero when damage >= 50
                        (local.set $dmg (i32.trunc_f32_s (call $arg_f32 (i32.const 0))))
                        (drop (i32.div_s (local.get $dmg) (i32.const 0)))
                        (return (i32.const 0))
                    )
                )
                (i32.const 0)
            )
        )
    "#;

    // Fixed plugin: reads hydrated `storage_cache["scale"]` ("3"), reads `hook_arg_f32(0)` (75.0),
    // divides 75 / 3 = 25.0 safely, overrides damage to 25.0, emits SayText + SetPlayerMoney,
    // and writes storage `"last_scaled" = "25"`!
    let fixed_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_i32" (func $arg_i32 (param i32) (result i32)))
            (import "cs16_mod" "hook_arg_f32" (func $arg_f32 (param i32) (result f32)))
            (import "cs16_mod" "hook_set_override_f32" (func $ovr_f32 (param f32) (result i32)))
            (import "cs16_mod" "storage_get" (func $sget (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "storage_set" (func $sset (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "say_text" (func $say (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "player_set_money" (func $set_money (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "scale")
            (data (i32.const 16) "last_scaled")
            (data (i32.const 32) "25")
            (data (i32.const 48) "[fixed] damage scaled safely")
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 33) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $slot i32)
                (local $div i32)
                (local $len i32)
                (local $scaled f32)
                (if (i32.eq (local.get $hid) (i32.const 33))
                    (then
                        (local.set $slot (call $arg_i32 (i32.const 0)))
                        ;; Read "scale" from hydrated storage_cache into offset 200.
                        (local.set $len (call $sget (i32.const 0) (i32.const 5) (i32.const 200) (i32.const 8)))
                        (if (i32.gt_s (local.get $len) (i32.const 0))
                            (then
                                (local.set $div (i32.sub (i32.load8_u (i32.const 200)) (i32.const 48)))
                            )
                            (else
                                (local.set $div (i32.const 1))
                            )
                        )
                        ;; Guard against zero divisor.
                        (if (i32.le_s (local.get $div) (i32.const 0))
                            (then (local.set $div (i32.const 1)))
                        )
                        (local.set $scaled (f32.div (call $arg_f32 (i32.const 0)) (f32.convert_i32_s (local.get $div))))
                        (drop (call $ovr_f32 (local.get $scaled)))
                        (drop (call $say (local.get $slot) (i32.const 48) (i32.const 28)))
                        (drop (call $set_money (local.get $slot) (i32.const 5555) (i32.const 1)))
                        (drop (call $sset (i32.const 16) (i32.const 11) (i32.const 32) (i32.const 2)))
                        (return (i32.const 2)) ;; HookAction::Override
                    )
                )
                (i32.const 0)
            )
        )
    "#;

    // Healthy sibling plugin proves the server keeps running other plugins unaffected.
    let healthy_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 1) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (i32.const 0)
            )
        )
    "#;

    let buggy_wasm = compile_wat_to_wasm(buggy_wat).unwrap();
    let fixed_wasm = compile_wat_to_wasm(fixed_wat).unwrap();
    let buggy_sha = sha256_hex(&buggy_wasm);

    let mut host = ServerModHost::new();
    host.set_fault_dump_dir(&fault_dir);
    host.load_package(make_v2_pkg(
        "damage_calc",
        "permissions = [\"entities\", \"messages\", \"storage\"]\n",
        buggy_wat,
    ))
    .unwrap();
    host.load_package(make_v2_pkg("healthy_watch", "", healthy_wat))
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

    // Populate storage_cache before triggering the fatal PlayerTakeDamage event.
    c0.send_stringcmd("seed_cfg");
    for _ in 0..10 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }

    // Trigger PlayerTakeDamage Pre -> divide by zero inside `damage_calc`!
    c0.send_stringcmd("test_dmg");
    for _ in 0..30 {
        now += 0.01;
        c0.pump(now);
        server.frame(&mut hub, now);
    }

    // 1. Server stays alive and healthy sibling keeps ticking at 100 Hz!
    let mods = server.game().host.list_mods();
    let buggy_info = mods.iter().find(|m| m.id == "damage_calc").unwrap();
    let healthy_info = mods.iter().find(|m| m.id == "healthy_watch").unwrap();
    assert!(
        matches!(buggy_info.status, ModStatus::Faulted(ref r) if r.to_ascii_lowercase().contains("divide")),
        "expected divide-by-zero fault, got {:?}",
        buggy_info.status
    );
    assert_eq!(healthy_info.status, ModStatus::Active);
    assert!(healthy_info.total_fuel_used > 0);

    // 2. Verify captured ModFaultDump in FaultDumpStore and on disk under `logs/mod_faults/`.
    let faults = server.game().host.list_faults();
    assert_eq!(faults.len(), 1);
    let summary = &faults[0];
    assert_eq!(summary.mod_id, "damage_calc");
    assert_eq!(summary.language, PluginLanguage::Wasm);
    assert!(summary.file_name.ends_with(".json"));
    assert!(summary.byte_len <= MAX_FAULT_DUMP_BYTES);

    let raw_json = server
        .game()
        .host
        .get_fault_json(&summary.id)
        .expect("dump JSON present by ID");
    assert!(raw_json.len() <= MAX_FAULT_DUMP_BYTES);
    assert_eq!(
        server.game().host.get_fault_json(&summary.file_name).as_deref(),
        Some(raw_json.as_str())
    );

    let disk_dump_path = fault_dir.join(&summary.file_name);
    assert!(disk_dump_path.is_file(), "expected {}", disk_dump_path.display());
    let disk_json = std::fs::read_to_string(&disk_dump_path).unwrap();
    assert_eq!(disk_json, raw_json);

    let dump = ModFaultDump::from_json(&disk_json).expect("parse captured black-box dump");
    assert_eq!(dump.mod_id, "damage_calc");
    assert_eq!(dump.version, "2.0.0");
    assert_eq!(dump.binary_sha256, buggy_sha);
    assert_eq!(
        dump.trigger,
        InvocationTrigger::Hook {
            hook: HookId::PlayerTakeDamage,
            phase: HookPhase::Pre
        }
    );
    assert_eq!(dump.event.i32_args[0], 0);
    assert!((dump.event.f32_args[0] - 75.0).abs() < 0.01);
    assert_eq!(dump.snapshot.map_name, "de_dust2");
    assert_eq!(
        dump.storage_cache.get("scale").map(String::as_str),
        Some("3")
    );
    let hero = dump.snapshot.client(0).expect("client slot 0 captured");
    assert_eq!(hero.name, "Hero");
    assert!(hero.active);

    // 3. Replay against BUGGY WASM reproduces exact FAULT offline!
    let buggy_report = replay_fault_dump(&dump, ReplayTarget::WasmBytes(&buggy_wasm));
    assert!(
        matches!(buggy_report.outcome, ReplayOutcome::Fault(ref e) if e.to_ascii_lowercase().contains("divide")),
        "expected reproduced divide-by-zero fault, got {:?}",
        buggy_report.outcome
    );
    assert!(!buggy_report.stack_trace.is_empty());
    let buggy_display = buggy_report.to_string();
    assert!(buggy_display.contains("FAULT:"));

    // 4. Replay the EXACT SAME dump file against FIXED WASM -> PASS + exact EngineOps!
    let fixed_wasm_path = tmp.path().join("fixed_damage_calc.wasm");
    std::fs::write(&fixed_wasm_path, &fixed_wasm).unwrap();
    let fixed_report = replay_fault_dump_from_path(&disk_dump_path, Some(&fixed_wasm_path))
        .expect("replay from disk dump + override path");
    assert_eq!(fixed_report.outcome, ReplayOutcome::Pass(HookAction::Override));
    assert!(fixed_report.fuel_used > 0);
    assert_eq!(fixed_report.override_f32, Some(25.0));
    assert_eq!(
        fixed_report.emitted_ops,
        vec![
            EngineOp::SayText {
                slot: Some(0),
                text: "[fixed] damage scaled safely".to_string(),
            },
            EngineOp::SetPlayerMoney {
                slot: 0,
                amount: 5555,
                flash: true,
            },
        ]
    );
    assert_eq!(
        fixed_report.storage_mutations,
        vec![StorageMutation::Set {
            key: "last_scaled".to_string(),
            value: "25".to_string(),
        }]
    );
    let fixed_display = fixed_report.to_string();
    assert!(fixed_display.contains("PASS: HookAction::Override"));
    assert!(fixed_display.contains("SetPlayerMoney"));
    assert!(fixed_display.contains("SET last_scaled = \"25\""));
}

#[test]
fn out_of_fuel_trap_captures_fuel_consumed_and_replays_to_pass_when_fixed() {
    let buggy_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (loop $spin
                    (br $spin)
                )
                (i32.const 0)
            )
        )
    "#;
    let fixed_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "print_all" (func $print_all (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "frame-ok")
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 11) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (drop (call $print_all (i32.const 0) (i32.const 8)))
                (i32.const 0)
            )
        )
    "#;

    let mut host = ServerModHost::new();
    host.load_package(make_v2_pkg(
        "spinner",
        "permissions = [\"messages\"]\nfuel_per_frame = 4000\n",
        buggy_wat,
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
    server.frame(&mut hub, 0.01);

    let dump = server
        .game()
        .host
        .latest_fault()
        .expect("OutOfFuel dump captured");
    assert_eq!(dump.mod_id, "spinner");
    assert_eq!(dump.fuel_budget, 4000);
    assert_eq!(dump.fuel_consumed_before_trap, 4000);
    assert_eq!(
        dump.trigger,
        InvocationTrigger::Hook {
            hook: HookId::StartFrame,
            phase: HookPhase::Pre
        }
    );

    let buggy_wasm = compile_wat_to_wasm(buggy_wat).unwrap();
    let fixed_wasm = compile_wat_to_wasm(fixed_wat).unwrap();

    let buggy_rep = replay_fault_dump(&dump, ReplayTarget::WasmBytes(&buggy_wasm));
    assert!(buggy_rep.outcome.is_fault());
    assert_eq!(buggy_rep.fuel_used, 4000);

    let fixed_rep = replay_fault_dump(&dump, ReplayTarget::WasmBytes(&fixed_wasm));
    assert_eq!(fixed_rep.outcome, ReplayOutcome::Pass(HookAction::Continue));
    assert!(fixed_rep.fuel_used > 0 && fixed_rep.fuel_used < 4000);
    assert_eq!(
        fixed_rep.emitted_ops,
        vec![EngineOp::PrintAll {
            text: "frame-ok".to_string()
        }]
    );
}

struct PanickingRuntime;
struct PanickingInst;
struct FixedInst;

impl ModInstance for PanickingInst {
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
        panic!("unexpected nil deref in custom script runtime");
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

impl ModInstance for FixedInst {
    fn call_init(&mut self, _ctx: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
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
                name: "hostname".into(),
                value: "Recovered".into(),
            }],
            fuel_used: 19,
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

impl ModRuntime for PanickingRuntime {
    fn name(&self) -> &'static str {
        "panicking-test-rt"
    }
    fn instantiate(&self, _: &str, _: &[u8], _: Permissions, _: usize) -> Result<Box<dyn ModInstance>, RuntimeError> {
        Ok(Box::new(PanickingInst))
    }
}

#[test]
fn timer_trap_and_panicking_runtime_are_isolated_and_replayable() {
    // 1. Timer callback trap (`InvocationTrigger::Timer`).
    let buggy_timer_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "timer_after" (func $timer (param i32 i32 i32) (result i32)))
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 1) (i32.const 1) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param i32 i32) (result i32)
                (drop (call $timer (i32.const 10) (i32.const 7) (i32.const 99)))
                (i32.const 0)
            )
            (func (export "cs_mod_timer") (param $cb i32) (param $arg i32) (result i32)
                (unreachable)
            )
        )
    "#;
    let fixed_timer_wat = r#"
        (module
            (import "cs16_mod" "print_all" (func $print_all (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "timer-fixed")
            (func (export "cs_mod_timer") (param $cb i32) (param $arg i32) (result i32)
                (if (i32.and (i32.eq (local.get $cb) (i32.const 7)) (i32.eq (local.get $arg) (i32.const 99)))
                    (then
                        (drop (call $print_all (i32.const 0) (i32.const 11)))
                    )
                )
                (i32.const 0)
            )
        )
    "#;

    let mut host = ServerModHost::new();
    host.load_package(make_v2_pkg(
        "timer_bug",
        "permissions = [\"timers\", \"messages\"]\n",
        buggy_timer_wat,
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
    for i in 1..=5 {
        server.frame(&mut hub, (i as f64) * 0.01);
    }

    let timer_dump = server
        .game()
        .host
        .latest_fault()
        .expect("timer fault dump captured");
    assert_eq!(
        timer_dump.trigger,
        InvocationTrigger::Timer { cb_id: 7, arg: 99 }
    );
    let fixed_rep = replay_fault_dump(
        &timer_dump,
        ReplayTarget::WasmBytes(&compile_wat_to_wasm(fixed_timer_wat).unwrap()),
    );
    assert_eq!(fixed_rep.outcome, ReplayOutcome::Pass(HookAction::Continue));
    assert_eq!(
        fixed_rep.emitted_ops,
        vec![EngineOp::PrintAll {
            text: "timer-fixed".into()
        }]
    );

    // 2. Rust panic inside a custom ModInstance is caught cleanly without crashing the server.
    let mut panic_host =
        ServerModHost::with_runtime(Box::new(PanickingRuntime), Box::new(MemoryStorage::default()));
    let pkg = LoadedPackage {
        manifest: ModManifest::parse_toml("id = \"script_panic\"\nversion = \"1.0\"\n").unwrap(),
        source_path: None,
        server_wasm: Some(vec![0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]),
        client_wasm: None,
        client_bundle: None,
        assets: BTreeMap::new(),
    };
    panic_host.load_package(pkg).unwrap();

    let mut srv2 = Server::new(
        ServerConfig::default(),
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), panic_host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    srv2.frame(&mut hub, 0.01);

    let panic_dump = srv2
        .game()
        .host
        .latest_fault()
        .expect("panic fault dump captured");
    assert!(panic_dump.error.contains("plugin panicked"));
    assert!(panic_dump.error.contains("unexpected nil deref"));

    let mut buggy_inst = PanickingInst;
    let rep_bug = replay_fault_dump(&panic_dump, ReplayTarget::Instance(&mut buggy_inst));
    assert!(rep_bug.outcome.is_fault());

    let mut fixed_inst = FixedInst;
    let rep_fix = replay_fault_dump(&panic_dump, ReplayTarget::Instance(&mut fixed_inst));
    assert_eq!(rep_fix.outcome, ReplayOutcome::Pass(HookAction::Handled));
    assert_eq!(rep_fix.fuel_used, 19);
    assert_eq!(
        rep_fix.emitted_ops,
        vec![EngineOp::SetCvar {
            name: "hostname".into(),
            value: "Recovered".into(),
        }]
    );
}

#[test]
fn ring_buffer_evicts_oldest_on_disk_and_amxx_fault_replays_cleanly() {
    let tmp = TempDirGuard::new("ring_amxx");
    let fault_dir = default_fault_dump_dir(tmp.path());

    let mut host = ServerModHost::new();
    host.set_fault_dump_dir(&fault_dir);

    for i in 0..36u64 {
        let trigger = InvocationTrigger::Hook {
            hook: HookId::PlayerTakeDamage,
            phase: HookPhase::Pre,
        };
        let ts = 1_750_000_000_000 + i;
        let dump = ModFaultDump {
            schema_version: FAULT_DUMP_SCHEMA_VERSION,
            dump_id: ModFaultDump::make_dump_id(ts, &format!("mod_{i}"), &trigger),
            timestamp_unix_ms: ts,
            mod_id: format!("mod_{i}"),
            version: "1.0.0".into(),
            language: PluginLanguage::Python,
            source_path: None,
            binary_sha256: "b".repeat(64),
            permissions_mask: 0,
            http_hosts: Vec::new(),
            fuel_budget: 100_000,
            max_memory_bytes: 1_048_576,
            trigger,
            event: ActiveEvent::default(),
            snapshot: EngineSnapshot::default(),
            storage_cache: BTreeMap::new(),
            rng_seed: DEFAULT_RNG_SEED,
            fuel_consumed_before_trap: 123,
            error: format!("ZeroDivisionError #{i}"),
            stack_trace: vec![format!("line {}: x / 0", i + 1)],
            truncated: false,
        };
        host.record_external_fault(dump);
    }

    assert_eq!(host.list_faults().len(), MAX_STORED_FAULTS);
    let json_list = host.list_faults_json();
    assert!(json_list.contains("mod_35"));
    assert!(!json_list.contains("\"mod_0\""));

    let disk_count = std::fs::read_dir(&fault_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .count();
    assert_eq!(disk_count, MAX_STORED_FAULTS);

    // Hydrate a brand-new host from disk directory.
    let mut restored_host = ServerModHost::new();
    assert_eq!(
        restored_host.load_fault_dumps_from_dir(&fault_dir).unwrap(),
        MAX_STORED_FAULTS
    );
    assert_eq!(restored_host.list_faults().len(), MAX_STORED_FAULTS);

    // Offline `.amxx` Pawn 3.2 fault replay (buggy divide-by-zero vs fixed health heal).
    let buggy_amxx_code = vec![
        OP_HALT,
        0,
        OP_PROC,
        OP_CONST_PRI,
        100,
        OP_CONST_ALT,
        0,
        OP_SDIV,
        OP_RETN,
    ];
    let buggy_amxx = build_test_amxx_plugin(
        &[("on_damage", 8)],
        &[],
        &[],
        &buggy_amxx_code,
        &[0u8; 64],
        2048,
        0,
    );

    let mut msg_data = Vec::new();
    for &b in b"amxx-fixed" {
        msg_data.extend_from_slice(&i32::from(b).to_le_bytes());
    }
    msg_data.extend_from_slice(&0i32.to_le_bytes());
    let fixed_amxx_code = vec![
        OP_HALT,
        0,
        OP_PROC,
        // set_user_health(1, 125)
        OP_PUSH_C,
        125,
        OP_PUSH_C,
        1,
        OP_PUSH_C,
        8,
        OP_SYSREQ_C,
        0,
        OP_STACK,
        12,
        // server_print("amxx-fixed")
        OP_PUSH_C,
        0,
        OP_PUSH_C,
        4,
        OP_SYSREQ_C,
        1,
        OP_STACK,
        8,
        OP_CONST_PRI,
        0,
        OP_RETN,
    ];
    let fixed_amxx = build_test_amxx_plugin(
        &[("on_damage", 8)],
        &["set_user_health", "server_print"],
        &[],
        &fixed_amxx_code,
        &msg_data,
        2048,
        0,
    );

    let amxx_dump = ModFaultDump {
        schema_version: FAULT_DUMP_SCHEMA_VERSION,
        dump_id: "1750000000099_legacy_amxx_on_damage".into(),
        timestamp_unix_ms: 1_750_000_000_099,
        mod_id: "legacy.amxx".into(),
        version: "1.0".into(),
        language: PluginLanguage::Amxx,
        source_path: None,
        binary_sha256: sha256_hex(&buggy_amxx),
        permissions_mask: 0,
        http_hosts: Vec::new(),
        fuel_budget: 100_000,
        max_memory_bytes: 1_048_576,
        trigger: InvocationTrigger::AmxxPublic {
            function: "on_damage".into(),
            args: vec![1, 75],
        },
        event: ActiveEvent::default(),
        snapshot: EngineSnapshot {
            max_clients: 16,
            map_name: "de_inferno".into(),
            clients: vec![ClientSnap {
                slot: 0,
                active: true,
                name: "Player1".into(),
                health: 40.0,
                ..ClientSnap::default()
            }],
            ..EngineSnapshot::default()
        },
        storage_cache: BTreeMap::new(),
        rng_seed: DEFAULT_RNG_SEED,
        fuel_consumed_before_trap: 4,
        error: "divide by zero".into(),
        stack_trace: vec!["amxx::on_damage() -> divide by zero".into()],
        truncated: false,
    };

    let rep_buggy = replay_fault_dump(&amxx_dump, ReplayTarget::AmxxBytes(&buggy_amxx));
    assert!(
        matches!(rep_buggy.outcome, ReplayOutcome::Fault(ref e) if e.contains("divide by zero")),
        "got {:?}",
        rep_buggy.outcome
    );

    let rep_fixed = replay_fault_dump(&amxx_dump, ReplayTarget::AmxxBytes(&fixed_amxx));
    assert_eq!(rep_fixed.outcome, ReplayOutcome::Pass(HookAction::Continue));
    assert_eq!(
        rep_fixed.emitted_ops,
        vec![
            EngineOp::SetPlayerHealth {
                slot: 0,
                health: 125.0,
            },
            EngineOp::PrintAll {
                text: "amxx-fixed".into(),
            },
        ]
    );
}
