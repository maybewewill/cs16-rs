//! Phase 18 Stage 2A — Automated test suite for `mods/gamemodes/multi_1v1.py`
//! (Zero-Compile Python 3 Multi-1v1 Arena Ladder):
//! - Fuel budget & fast hot-reload verification
//! - Arena 1..N pairing, spatial isolation & cross-arena damage guard
//! - Winner-UP / Loser-DOWN ladder promotion & Arena 1 Server Champion crown defense
//! - Odd-player waiting queue rotation
//! - GoldSrc `/guns` (`!guns`) menu resolving Rifle, Pistol (`100 HP + Vest`), and AWP duels
//! - Persistent Elo rating + `/top1v1` & `!rank` bilingual (`en` + `ru`) leaderboard
//! - `ServerModHost` + `attach_python_runtime` live loading.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use cs_mod_api::services::SharedPlatformServices;
use cs_mod_api::{
    ArmorKind, DEFAULT_FUEL_PER_FRAME, DEFAULT_MEMORY_BYTES, HookAction, HookId, HookPhase, VfxOp,
};
use cs_mod_host::natives::{ActiveEvent, ClientSnap, EngineOp, EngineSnapshot, EntitySnap};
use cs_mod_host::runtime::{ModExecContext, ModInstance};
use cs_mod_host::{ModStatus, ServerModHost};
use cs_mod_py::{
    PyModInstance, PySourceSyntaxCheck, attach_python_runtime, default_python_permissions,
    synthesize_manifest_from_py,
};

fn multi_1v1_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("mods")
        .join("gamemodes")
        .join("multi_1v1.py")
}

fn load_gamemodes_lang() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("configs")
        .join("lang")
        .join("gamemodes.toml");
    std::fs::read_to_string(path).expect("read configs/lang/gamemodes.toml")
}

fn load_multi_1v1(services: SharedPlatformServices) -> (PyModInstance, BTreeMap<String, String>) {
    let src = std::fs::read_to_string(multi_1v1_path()).expect("read multi_1v1.py");
    let mut inst = PyModInstance::from_source(
        "multi_1v1",
        "multi_1v1.py",
        &src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .expect("load multi_1v1.py");
    inst.configure_v2(services, Vec::new(), None);
    let snap = EngineSnapshot::default();
    let ev = ActiveEvent::default();
    let mut storage = BTreeMap::new();
    let init = inst
        .call_init(&ModExecContext {
            snapshot: &snap,
            event: &ev,
            storage: &storage,
            fuel_budget: DEFAULT_FUEL_PER_FRAME,
        })
        .expect("multi_1v1 init");
    apply_storage(&mut storage, &init.storage_writes);
    (inst, storage)
}

fn apply_storage(storage: &mut BTreeMap<String, String>, writes: &[(String, Option<String>)]) {
    for (k, v) in writes {
        match v {
            Some(val) => {
                storage.insert(k.clone(), val.clone());
            }
            None => {
                storage.remove(k);
            }
        }
    }
}

fn make_arena_snapshot(players: &[(&str, &str, &str)]) -> (EngineSnapshot, SharedPlatformServices) {
    let services = SharedPlatformServices::new();
    let mut entities = BTreeMap::new();
    let mut clients = Vec::new();
    {
        let mut svc = services.lock();
        svc.lang.load_toml_str(&load_gamemodes_lang());
        for (idx, &(name, sid, lang)) in players.iter().enumerate() {
            let slot = idx as u8;
            svc.lang
                .update_player_from_userinfo(slot, Some(sid), Some(lang), None);
            entities.insert(
                u16::from(slot) + 1,
                EntitySnap {
                    index: u16::from(slot) + 1,
                    classname: "player".to_string(),
                    origin: [0.0, 0.0, 64.0],
                    velocity: [0.0; 3],
                    health: 100.0,
                    flags: 1 << 9,
                },
            );
            clients.push(ClientSnap {
                slot,
                active: true,
                name: name.to_string(),
                health: 100.0,
                max_health: 100.0,
                team: if idx % 2 == 0 { 1 } else { 2 },
                alive: true,
                steamid: sid.to_string(),
                lang: lang.to_string(),
                ..ClientSnap::default()
            });
        }
    }

    let snap = EngineSnapshot {
        time_secs: 15.0,
        frametime: 0.01,
        max_clients: 32,
        map_name: "aim_map".to_string(),
        entities,
        clients,
        ..EngineSnapshot::default()
    };
    (snap, services)
}

#[test]
fn multi_1v1_syntax_manifest_fuel_and_hot_reload() {
    let src = std::fs::read_to_string(multi_1v1_path()).expect("read multi_1v1.py");
    let report = PySourceSyntaxCheck::check("multi_1v1.py", &src).expect("syntax check");
    assert_eq!(report.meta.id.as_deref(), Some("multi_1v1"));
    assert!(report.hook_count >= 5);
    assert!(report.chat_command_count >= 3);

    let manifest = synthesize_manifest_from_py("multi_1v1", "multi_1v1.py", &src)
        .expect("synthesize manifest");
    assert_eq!(manifest.id, "multi_1v1");

    let (snap, services) = make_arena_snapshot(&[
        ("Alice", "STEAM_0:0:1", "en"),
        ("Ivan", "STEAM_0:0:2", "ru"),
    ]);
    let (mut inst, mut storage) = load_multi_1v1(services);

    let rs_ev = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let out = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &rs_ev,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .expect("round start within fuel budget");
    apply_storage(&mut storage, &out.storage_writes);
    assert!(
        out.fuel_used < DEFAULT_FUEL_PER_FRAME / 2,
        "fuel_used={} should fit comfortably within frame budget",
        out.fuel_used
    );

    let t0 = Instant::now();
    inst.reload_source(&src).expect("hot reload multi_1v1.py");
    let elapsed_ms = t0.elapsed().as_millis();
    assert!(
        elapsed_ms < 250,
        "hot reload should be fast (was {elapsed_ms}ms)"
    );
}

#[test]
fn multi_1v1_arena_pairing_isolation_and_winner_up_loser_down_promotion() {
    let (snap, services) = make_arena_snapshot(&[
        ("Alice", "STEAM_0:0:101", "en"),
        ("Ivan", "STEAM_0:0:102", "ru"),
        ("Carol", "STEAM_0:0:103", "en"),
        ("Dmitry", "STEAM_0:0:104", "ru"),
    ]);
    let (mut inst, mut storage) = load_multi_1v1(services);

    // Round 1: Arena 1 (Alice vs Ivan), Arena 2 (Carol vs Dmitry)
    let rs1 = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let out1 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &rs1,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .expect("round 1");
    apply_storage(&mut storage, &out1.storage_writes);

    // Bilingual Arena 1 Champion & Arena 2 announcements
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text }
            if text.contains("[Arena 1 - Server Champion]") && text.contains("Alice vs Ivan")
    )));
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(1), text }
            if text.contains("[Арена 1 - Чемпион сервера]") && text.contains("Alice против Ivan")
    )));
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(2), text }
            if text.contains("[Arena 2]") && text.contains("Carol vs Dmitry")
    )));
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(3), text }
            if text.contains("[Арена 2]") && text.contains("Carol против Dmitry")
    )));

    // Spatial isolation: Arena 1 and Arena 2 teleported to distinct separated coordinates
    let origin_of = |ops: &[EngineOp], edict: u16| -> [f32; 3] {
        ops.iter()
            .find_map(|op| match op {
                EngineOp::SetOrigin { index, origin } if *index == edict => Some(*origin),
                _ => None,
            })
            .expect("origin op")
    };
    let a1_p1 = origin_of(&out1.ops, 1);
    let a1_p2 = origin_of(&out1.ops, 2);
    let a2_p1 = origin_of(&out1.ops, 3);
    assert!((a1_p2[0] - a1_p1[0]).abs() >= 500.0, "duelists face off");
    assert!(
        (a2_p1[0] - a1_p1[0]).abs() >= 1000.0,
        "Arena 2 isolated far from Arena 1: {a1_p1:?} vs {a2_p1:?}"
    );

    // Cross-arena damage guard: Alice (Arena 1) cannot damage Carol (Arena 2)
    let cross_dmg = ActiveEvent {
        i32_args: [2, 0, 1, 28, 0, 0],
        f32_args: [35.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let cross_out = inst
        .call_hook(
            HookId::PlayerTakeDamage,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &cross_dmg,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert_eq!(cross_out.action, HookAction::Supercede);

    // Same-arena damage allowed: Alice (0) vs Ivan (1)
    let same_dmg = ActiveEvent {
        i32_args: [1, 0, 1, 28, 0, 0],
        f32_args: [35.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let same_out = inst
        .call_hook(
            HookId::PlayerTakeDamage,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &same_dmg,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert_eq!(same_out.action, HookAction::Continue);

    // Duel 1 outcome: Alice (0) defeats Ivan (1) in Arena 1 -> Crown defense + golden aura
    let kill1 = ActiveEvent {
        i32_args: [1, 0, 28, 1, 1, 0],
        str_args: ["ak47".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let k1_out = inst
        .call_hook(
            HookId::PlayerKilled,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &kill1,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    apply_storage(&mut storage, &k1_out.storage_writes);
    assert!(k1_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::EmitVfx(VfxOp::AuraLight {
            rgb: [255, 200, 0],
            ..
        })
    )));
    assert!(k1_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text } if text.contains("defended the Server Champion crown (1 defenses)")
    )));

    // Duel 2 outcome: Carol (2) defeats Dmitry (3) in Arena 2 -> triggers ladder promotion/relegation!
    let kill2 = ActiveEvent {
        i32_args: [3, 2, 28, 1, 0, 0],
        str_args: ["ak47".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let k2_out = inst
        .call_hook(
            HookId::PlayerKilled,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &kill2,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    apply_storage(&mut storage, &k2_out.storage_writes);
    assert!(k2_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(2), text } if text.contains("Promoted UP to Arena 1")
    )));
    assert!(k2_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(1), text } if text.contains("Понижение на Арену 2")
    )));

    // Round 2: Arena 1 MUST now be Alice vs Carol; Arena 2 MUST now be Ivan vs Dmitry!
    let rs2 = ActiveEvent {
        i32_args: [2, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let out2 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &rs2,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text }
            if text.contains("[Arena 1 - Server Champion]") && text.contains("Alice vs Carol")
    )));
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(1), text }
            if text.contains("[Арена 2]") && text.contains("Ivan против Dmitry")
    )));
}

#[test]
fn multi_1v1_guns_menu_resolves_rifle_pistol_and_awp_duels() {
    let (snap, services) = make_arena_snapshot(&[
        ("Alice", "STEAM_0:0:201", "en"),
        ("Bob", "STEAM_0:0:202", "en"),
    ]);
    let (mut inst, mut storage) = load_multi_1v1(services.clone());

    // Open /guns for Alice (slot 0)
    let guns_say = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["/guns".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let guns_out = inst
        .call_hook(
            HookId::PlayerSay,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &guns_say,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert_eq!(guns_out.action, HookAction::Supercede);
    let menu_id = guns_out
        .ops
        .iter()
        .find_map(|op| match op {
            EngineOp::MenuDisplay(spec) if spec.slot == 0 => Some(spec.menu_id),
            _ => None,
        })
        .expect("guns MenuDisplay");
    let cb_id = {
        let mut svc = services.lock();
        let rendered = svc.menus.display(0, menu_id, 0, 25).expect("render menu");
        assert!(rendered.text.contains("\\yMulti-1v1 Arena Weapons\\w"));
        assert!(rendered.text.contains("AWP Rounds Opt-In"));
        match svc.menus.handle_menuselect(0, 1) {
            cs_mod_api::services::MenuSelectOutcome::Selected { callback_id, .. } => callback_id,
            other => panic!("expected menu select, got {other:?}"),
        }
    };

    let invoke_pick =
        |inst: &mut PyModInstance, storage: &mut BTreeMap<String, String>, slot: u8, info: &str| {
            let ev = ActiveEvent {
                str_args: [info.to_string(), String::new(), String::new()],
                ..Default::default()
            };
            let out = inst
                .call_menu(
                    cb_id,
                    slot,
                    menu_id,
                    0,
                    &ModExecContext {
                        snapshot: &snap,
                        event: &ev,
                        storage,
                        fuel_budget: DEFAULT_FUEL_PER_FRAME,
                    },
                )
                .unwrap();
            apply_storage(storage, &out.storage_writes);
        };

    // 1. Rifle duel with Alice switching AK-47 -> M4A1
    invoke_pick(&mut inst, &mut storage, 0, "toggle:rifle");
    let r1 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ActiveEvent::default(),
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert!(r1.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 0, item } if item == "weapon_m4a1"
    )));
    assert!(r1.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 1, item } if item == "weapon_ak47"
    )));
    assert!(r1.ops.iter().any(|op| matches!(
        op,
        EngineOp::SetPlayerArmor {
            slot: 0,
            armor: 100,
            kind: ArmorKind::VestHelm
        }
    )));

    // 2. Pistol duel when Alice disables Rifle rounds (intersection = ["pistol"]) -> 100 HP + Vest only!
    invoke_pick(&mut inst, &mut storage, 0, "toggle:pistol"); // Deagle -> USP
    invoke_pick(&mut inst, &mut storage, 0, "toggle:allow_rifle"); // Rifle OFF
    let r2 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ActiveEvent::default(),
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert!(r2.ops.iter().any(|op| matches!(
        op,
        EngineOp::SetPlayerArmor {
            slot: 0,
            armor: 100,
            kind: ArmorKind::Kevlar
        }
    )));
    assert!(r2.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 0, item } if item == "weapon_usp"
    )));
    assert!(
        !r2.ops.iter().any(|op| matches!(
            op,
            EngineOp::GiveItem { item, .. } if item == "weapon_m4a1" || item == "weapon_ak47" || item == "weapon_awp"
        )),
        "pistol duel must not grant rifles or AWP"
    );

    // 3. AWP opt-in: Alice alone opts in -> still Pistol; once Bob ALSO opts in -> AWP duel!
    invoke_pick(&mut inst, &mut storage, 0, "toggle:allow_rifle"); // Rifle back ON
    invoke_pick(&mut inst, &mut storage, 0, "toggle:allow_awp"); // Alice AWP ON
    let r3_one_awp = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ActiveEvent::default(),
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert!(
        !r3_one_awp
            .ops
            .iter()
            .any(|op| matches!(op, EngineOp::GiveItem { item, .. } if item == "weapon_awp")),
        "single-sided AWP opt-in must not force AWP duel"
    );

    invoke_pick(&mut inst, &mut storage, 1, "toggle:allow_awp"); // Bob AWP ON too!
    let r4_awp = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ActiveEvent::default(),
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert!(r4_awp.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 0, item } if item == "weapon_awp"
    )));
    assert!(r4_awp.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 1, item } if item == "weapon_awp"
    )));
}

#[test]
fn multi_1v1_elo_leaderboard_top1v1_rank_and_server_host_load() {
    let (snap, services) = make_arena_snapshot(&[
        ("Alice", "STEAM_0:0:301", "en"),
        ("Ivan", "STEAM_0:0:302", "ru"),
        ("Carol", "STEAM_0:0:303", "en"),
    ]);
    let (mut inst, mut storage) = load_multi_1v1(services);

    // Round 1 with 3 players: Alice vs Ivan in Arena 1, Carol in Wait Queue
    let r1 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ActiveEvent::default(),
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    apply_storage(&mut storage, &r1.storage_writes);
    assert!(r1.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(2), text } if text.contains("[Arena Wait]")
    )));

    // Alice defeats Ivan in Arena 1 -> Elo rises above 1000, Ivan drops below 1000
    let kill = ActiveEvent {
        i32_args: [1, 0, 28, 1, 1, 0],
        ..Default::default()
    };
    let k_out = inst
        .call_hook(
            HookId::PlayerKilled,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &kill,
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    apply_storage(&mut storage, &k_out.storage_writes);

    // Query /top1v1 in EN (slot 0) and RU (slot 1)
    let top_en = inst
        .call_hook(
            HookId::PlayerSay,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &ActiveEvent {
                    i32_args: [0, 0, 0, 0, 0, 0],
                    str_args: ["/top1v1".to_string(), String::new(), String::new()],
                    ..Default::default()
                },
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert_eq!(top_en.action, HookAction::Supercede);
    assert!(top_en.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text }
            if text.contains("#1 Alice") && text.contains("1016 Elo") && text.contains("Arena 1 defenses: 1")
    )));

    let rank_ru = inst
        .call_hook(
            HookId::PlayerSay,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &ActiveEvent {
                    i32_args: [1, 0, 0, 0, 0, 0],
                    str_args: ["!rank".to_string(), String::new(), String::new()],
                    ..Default::default()
                },
                storage: &storage,
                fuel_budget: DEFAULT_FUEL_PER_FRAME,
            },
        )
        .unwrap();
    assert_eq!(rank_ru.action, HookAction::Supercede);
    assert!(rank_ru.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(1), text }
            if text.contains("Ранг #") && text.contains("984 Elo")
    )));

    // Verify ServerModHost + attach_python_runtime loads multi_1v1.py cleanly
    let mut host = ServerModHost::new();
    attach_python_runtime(&mut host);
    let loaded = host
        .load_python_file(&multi_1v1_path())
        .expect("ServerModHost load multi_1v1.py");
    assert_eq!(loaded, "multi_1v1");
    assert!(
        host.list_mods()
            .iter()
            .any(|m| m.id == "multi_1v1" && m.status == ModStatus::Active)
    );
}
