//! Phase 18 Stage 2A — Automated tests for competitive `retakes` (Rust -> WASM) plugin,
//! `configs/plugins.toml` registration, and `configs/lang/gamemodes.toml` bilingual catalog.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use cs_assets::bsp::Entity;
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_mod_api::services::SharedPlatformServices;
use cs_mod_api::{AdminFlags, ArmorKind, HookAction, HookId, HookPhase, TeamKind};
use cs_mod_host::natives::{ActiveEvent, ClientSnap, EngineOp, EngineSnapshot, EntitySnap};
use cs_mod_host::runtime::{ModExecContext, ModInstance, ModRuntime, WasmiRuntime};
use cs_mod_host::{ModStatus, ModdedGame, ServerModHost, load_package_dir};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_transport::{LoopbackHub, PeerAddr};

const RETAKES_TEST_FUEL: u64 = 2_000_000;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn load_retakes_instance(
    services: SharedPlatformServices,
) -> (Box<dyn ModInstance>, BTreeMap<String, String>) {
    let pkg = load_package_dir(&repo_root().join("mods/gamemodes/retakes"))
        .expect("load mods/gamemodes/retakes");
    assert_eq!(pkg.manifest.id, "retakes");
    let wasm = pkg.server_wasm.expect("retakes server.wasm built");
    let rt = WasmiRuntime::new();
    let mut inst = rt
        .instantiate(
            &pkg.manifest.id,
            &wasm,
            pkg.manifest.permissions,
            pkg.manifest.max_memory_bytes,
        )
        .expect("instantiate retakes wasmi");
    inst.configure_v2(services, pkg.manifest.http_hosts, None);

    let snap = EngineSnapshot::default();
    let ev = ActiveEvent::default();
    let mut storage = BTreeMap::new();
    let out = inst
        .call_init(&ModExecContext {
            snapshot: &snap,
            event: &ev,
            storage: &storage,
            fuel_budget: RETAKES_TEST_FUEL,
        })
        .expect("retakes init");
    apply_storage(&mut storage, &out.storage_writes);
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

fn make_7p_snapshot(map_name: &str, streak_cvar: Option<&str>) -> EngineSnapshot {
    let mut entities = BTreeMap::new();
    let mut clients = Vec::new();
    for slot in 0..7u8 {
        let is_t = slot < 3;
        entities.insert(
            u16::from(slot) + 1,
            EntitySnap {
                index: u16::from(slot) + 1,
                classname: "player".to_string(),
                origin: [0.0, 0.0, 64.0],
                velocity: [0.0, 0.0, 0.0],
                health: 100.0,
                flags: 1 << 9,
            },
        );
        clients.push(ClientSnap {
            slot,
            active: true,
            name: format!("Player{slot}"),
            health: 100.0,
            max_health: 100.0,
            team: if is_t { 1 } else { 2 },
            alive: true,
            steamid: format!("STEAM_0:0:70{slot}"),
            admin_flags: if slot == 0 {
                AdminFlags::KICK.bits() | AdminFlags::IMMUNITY.bits()
            } else {
                AdminFlags::USER.bits()
            },
            lang: if slot == 0 { "ru" } else { "en" }.to_string(),
            ..ClientSnap::default()
        });
    }
    let mut cvars = BTreeMap::new();
    if let Some(sc) = streak_cvar {
        cvars.insert("retakes_scramble_streak".to_string(), sc.to_string());
    }
    EngineSnapshot {
        time_secs: 10.0,
        frametime: 0.01,
        max_clients: 32,
        map_name: map_name.to_string(),
        entities,
        clients,
        cvars,
        ..EngineSnapshot::default()
    }
}

fn make_services_7p() -> SharedPlatformServices {
    let services = SharedPlatformServices::new();
    {
        let mut svc = services.lock();
        svc.lang
            .update_player_from_userinfo(0, Some("STEAM_0:0:700"), Some("ru"), None);
        for slot in 1..7u8 {
            let sid = format!("STEAM_0:0:70{slot}");
            svc.lang
                .update_player_from_userinfo(slot, Some(&sid), Some("en"), None);
        }
        svc.admins.set_slot_flags(0, AdminFlags::parse_str("ac"));
    }
    services
}

fn activate_map(
    inst: &mut Box<dyn ModInstance>,
    snap: &EngineSnapshot,
    storage: &mut BTreeMap<String, String>,
    map: &str,
) {
    let ev = ActiveEvent {
        i32_args: [32, 0, 0, 0, 0, 0],
        str_args: [map.to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let out = inst
        .call_hook(
            HookId::ServerActivate,
            HookPhase::Post,
            &ModExecContext {
                snapshot: snap,
                event: &ev,
                storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .expect("server_activate");
    apply_storage(storage, &out.storage_writes);
}

#[test]
fn retakes_wasm_cycles_pistol_force_fullbuy_with_bombsite_callouts_and_defusers() {
    let services = make_services_7p();
    let (mut inst, mut storage) = load_retakes_instance(services);
    let snap = make_7p_snapshot("de_dust2", None);
    activate_map(&mut inst, &snap, &mut storage, "de_dust2");

    // --- Round 1: Pistol Round on Bombsite A (3T vs 4CT) ---
    let ev1 = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let out1 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ev1,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .expect("round 1 start");
    apply_storage(&mut storage, &out1.storage_writes);

    // Crisp amber DHUD banner in both EN and RU
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::DhudMessage(spec)
            if spec.slot == Some(1)
                && spec.rgb == [255, 176, 0]
                && spec.text.contains("RETAKES — BOMBSITE A [3T vs 4CT]")
                && spec.text.contains("PISTOL")
    )));
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::DhudMessage(spec)
            if spec.slot == Some(0)
                && spec.text.contains("РЕТЕЙК — ТОЧКА А")
                && spec.text.contains("ПИСТОЛЕТНЫЙ")
    )));

    // All 7 players positioned around de_dust2 Bombsite A
    let origin_ops: Vec<u16> = out1
        .ops
        .iter()
        .filter_map(|op| match op {
            EngineOp::SetOrigin { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(
        origin_ops.len(),
        7,
        "all 7 players teleported to site spawns"
    );

    // Pistol round armor = Kevlar (vest only), C4 on T0, defusers distributed to CTs
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::SetPlayerArmor {
            slot: 3,
            armor: 100,
            kind: ArmorKind::Kevlar
        }
    )));
    assert!(out1.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 0, item } if item == "weapon_c4"
    )));
    let pistol_defusers: Vec<u8> = out1
        .ops
        .iter()
        .filter_map(|op| match op {
            EngineOp::SetUserDefuser { slot, has: true } => Some(*slot),
            _ => None,
        })
        .collect();
    assert!(
        !pistol_defusers.is_empty() && pistol_defusers.iter().all(|&s| s >= 3),
        "CT defusers distributed on pistol round: {pistol_defusers:?}"
    );
    assert!(
        !out1.ops.iter().any(|op| matches!(
            op,
            EngineOp::GiveItem { item, .. } if item == "weapon_m4a1" || item == "weapon_ak47" || item == "weapon_awp"
        )),
        "pistol round must not grant rifles or AWP"
    );

    // --- Round 2: Force-Buy on Bombsite B (tactical red DHUD, FAMAS/Galil + VestHelm + CT defusers) ---
    let ev2 = ActiveEvent {
        i32_args: [2, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let out2 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ev2,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .expect("round 2 start");
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::DhudMessage(spec)
            if spec.slot == Some(1)
                && spec.rgb == [255, 72, 64]
                && spec.text.contains("RETAKES — BOMBSITE B")
                && spec.text.contains("FORCE-BUY")
    )));
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::DhudMessage(spec)
            if spec.slot == Some(0)
                && spec.text.contains("РЕТЕЙК — ТОЧКА Б")
                && spec.text.contains("ФОРС-БАЙ")
    )));
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 3, item } if item == "weapon_famas"
    )));
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 0, item } if item == "weapon_galil"
    )));
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::SetPlayerArmor {
            slot: 3,
            armor: 100,
            kind: ArmorKind::VestHelm
        }
    )));
    let force_ct_defusers = out2
        .ops
        .iter()
        .filter(|op| matches!(op, EngineOp::SetUserDefuser { slot, has: true } if *slot >= 3))
        .count();
    assert_eq!(
        force_ct_defusers, 4,
        "all 4 CTs receive defuser on Force-Buy"
    );

    // --- Round 3: Full-Buy on Bombsite A (M4A1 / AK-47 + Full Nades + VestHelm) ---
    let ev3 = ActiveEvent {
        i32_args: [3, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let out3 = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ev3,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .expect("round 3 start");
    assert!(out3.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 3, item } if item == "weapon_m4a1"
    )));
    assert!(out3.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 0, item } if item == "weapon_ak47"
    )));
    assert!(out3.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot: 3, item } if item == "weapon_smokegrenade"
    )));
}

#[test]
fn retakes_guns_menu_persists_preferences_and_caps_awp_lottery_to_one_per_team() {
    let services = make_services_7p();
    let (mut inst, mut storage) = load_retakes_instance(services.clone());
    let snap = make_7p_snapshot("de_inferno", None);
    activate_map(&mut inst, &snap, &mut storage, "de_inferno");

    // Open /guns via PlayerSay Pre -> supercedes chat & displays GoldSrc PaginatedMenu
    let say_ev = ActiveEvent {
        i32_args: [3, 0, 0, 0, 0, 0],
        str_args: ["/guns".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let say_out = inst
        .call_hook(
            HookId::PlayerSay,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &say_ev,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .expect("/guns say");
    assert_eq!(say_out.action, HookAction::Supercede);
    let menu_id = say_out
        .ops
        .iter()
        .find_map(|op| match op {
            EngineOp::MenuDisplay(spec) if spec.slot == 3 => Some(spec.menu_id),
            _ => None,
        })
        .expect("MenuDisplay emitted for slot 3");
    {
        let mut svc = services.lock();
        let rendered = svc
            .menus
            .display(3, menu_id, 0, 25)
            .expect("render paginated guns menu");
        assert!(rendered.text.contains("\\yRetakes Loadout Allocator\\w"));
        assert!(rendered.text.contains("M4A1"));
        assert!(rendered.text.contains("AK-47"));
        assert!(rendered.text.contains("AWP Lottery"));
    }

    // Two CTs (slots 3, 4) and two Ts (slots 0, 1) configure guns + opt into AWP lottery
    let picks: &[(u8, &str)] = &[
        (3, "ct:aug"),
        (3, "pistol:deagle"),
        (3, "awp:toggle"),
        (4, "ct:aug"),
        (4, "pistol:deagle"),
        (4, "awp:toggle"),
        (0, "t:galil"),
        (0, "pistol:p228"),
        (0, "awp:toggle"),
        (1, "t:galil"),
        (1, "pistol:p228"),
        (1, "awp:toggle"),
    ];
    for &(slot, info) in picks {
        let ev = ActiveEvent {
            str_args: [info.to_string(), String::new(), String::new()],
            ..Default::default()
        };
        let out = inst
            .call_menu(
                1,
                slot,
                menu_id,
                0,
                &ModExecContext {
                    snapshot: &snap,
                    event: &ev,
                    storage: &storage,
                    fuel_budget: RETAKES_TEST_FUEL,
                },
            )
            .expect("menu pick");
        apply_storage(&mut storage, &out.storage_writes);
    }

    assert!(
        storage.contains_key("retakes_pref_STEAM_0_0_703"),
        "preferences persisted in storage: {storage:?}"
    );

    // Reload a FRESH WASM instance using only persisted storage -> preferences survive reload!
    let (mut fresh_inst, _) = load_retakes_instance(services);
    activate_map(&mut fresh_inst, &snap, &mut storage, "de_inferno");

    // Full-Buy Round (round 3): verify max 1 AWP per team & persisted AUG/Galil/Deagle/P228!
    let full_ev = ActiveEvent {
        i32_args: [3, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let full_out = fresh_inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &full_ev,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .expect("full buy round");

    let t_awps: Vec<u8> = full_out
        .ops
        .iter()
        .filter_map(|op| match op {
            EngineOp::GiveItem { slot, item } if item == "weapon_awp" && *slot < 3 => Some(*slot),
            _ => None,
        })
        .collect();
    let ct_awps: Vec<u8> = full_out
        .ops
        .iter()
        .filter_map(|op| match op {
            EngineOp::GiveItem { slot, item } if item == "weapon_awp" && *slot >= 3 => Some(*slot),
            _ => None,
        })
        .collect();
    assert_eq!(t_awps.len(), 1, "max 1 AWP on T team: {t_awps:?}");
    assert_eq!(ct_awps.len(), 1, "max 1 AWP on CT team: {ct_awps:?}");

    // Non-AWP opted CT (slot 4) receives persisted AUG + Deagle; non-AWP T (slot 1) receives Galil + P228
    let ct_non_awp = if ct_awps[0] == 3 { 4 } else { 3 };
    let t_non_awp = if t_awps[0] == 0 { 1 } else { 0 };
    assert!(full_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot, item } if *slot == ct_non_awp && item == "weapon_aug"
    )));
    assert!(full_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot, item } if *slot == ct_non_awp && item == "weapon_deagle"
    )));
    assert!(full_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot, item } if *slot == t_non_awp && item == "weapon_galil"
    )));
    assert!(full_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::GiveItem { slot, item } if *slot == t_non_awp && item == "weapon_p228"
    )));
}

#[test]
fn retakes_win_streak_scramble_and_admin_commands_work() {
    let services = make_services_7p();
    let (mut inst, mut storage) = load_retakes_instance(services);
    let snap = make_7p_snapshot("de_nuke", Some("4"));
    activate_map(&mut inst, &snap, &mut storage, "de_nuke");

    // Start round 1
    let rs = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let _ = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &rs,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .unwrap();

    // 3 CT retake wins -> no scramble yet
    let ct_win = ActiveEvent {
        i32_args: [2, 7, 3250, 0, 0, 0],
        ..Default::default()
    };
    for streak in 1..=3 {
        let out = inst
            .call_hook(
                HookId::RoundEnd,
                HookPhase::Post,
                &ModExecContext {
                    snapshot: &snap,
                    event: &ct_win,
                    storage: &storage,
                    fuel_budget: RETAKES_TEST_FUEL,
                },
            )
            .unwrap();
        assert!(
            !out.ops
                .iter()
                .any(|op| matches!(op, EngineOp::SetPlayerTeam { .. })),
            "must not scramble on streak {streak}/4"
        );
    }

    // 4th consecutive CT win -> rotates & scrambles teams + localized EN/RU announcement
    let out4 = inst
        .call_hook(
            HookId::RoundEnd,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &ct_win,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .unwrap();
    let team_changes: Vec<(u8, TeamKind)> = out4
        .ops
        .iter()
        .filter_map(|op| match op {
            EngineOp::SetPlayerTeam { slot, team, .. } => Some((*slot, *team)),
            _ => None,
        })
        .collect();
    assert_eq!(team_changes.len(), 7, "all 7 players rotated on scramble");
    let new_t_count = team_changes
        .iter()
        .filter(|(_, t)| *t == TeamKind::Terrorist)
        .count();
    let new_ct_count = team_changes
        .iter()
        .filter(|(_, t)| *t == TeamKind::Ct)
        .count();
    assert_eq!((new_t_count, new_ct_count), (3, 4), "preserves 3T vs 4CT");
    // Previous T0 (slot 0) must now be CT (genuine side rotation!)
    assert!(
        team_changes.contains(&(0, TeamKind::Ct)),
        "former T0 rotated to CT: {team_changes:?}"
    );
    assert!(out4.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(1), text } if text.contains("4-round streak")
    )));
    assert!(out4.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text } if text.contains("4 раундов подряд")
    )));

    // Non-admin slot 2 denied amx_retakes_site
    let non_admin = ActiveEvent {
        i32_args: [2, 2, 0, 0, 0, 0],
        str_args: [
            "amx_retakes_site".to_string(),
            "b".to_string(),
            "amx_retakes_site b".to_string(),
        ],
        ..Default::default()
    };
    let denied = inst
        .call_hook(
            HookId::ClientCommand,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &non_admin,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .unwrap();
    assert!(denied.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(2), text } if text.contains("Admin privileges required")
    )));

    // Admin slot 0 forces Site B + Full-Buy for next round
    let force_site = ActiveEvent {
        i32_args: [0, 2, 0, 0, 0, 0],
        str_args: [
            "amx_retakes_site".to_string(),
            "b".to_string(),
            "amx_retakes_site b".to_string(),
        ],
        ..Default::default()
    };
    let _ = inst
        .call_hook(
            HookId::ClientCommand,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &force_site,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .unwrap();
    let force_round = ActiveEvent {
        i32_args: [0, 2, 0, 0, 0, 0],
        str_args: [
            "amx_retakes_round".to_string(),
            "full".to_string(),
            "amx_retakes_round full".to_string(),
        ],
        ..Default::default()
    };
    let _ = inst
        .call_hook(
            HookId::ClientCommand,
            HookPhase::Pre,
            &ModExecContext {
                snapshot: &snap,
                event: &force_round,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .unwrap();

    // Even on Round 1 (normally Pistol Site A), forced overrides yield Site B + Full-Buy!
    let forced_rs = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let forced_out = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &snap,
                event: &forced_rs,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .unwrap();
    assert!(forced_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::DhudMessage(spec)
            if spec.slot == Some(1)
                && spec.text.contains("BOMBSITE B")
                && spec.text.contains("FULL-BUY")
    )));
}

#[test]
fn retakes_all_five_competitive_map_presets_and_custom_map_fallback() {
    let services = make_services_7p();
    let (mut inst, mut storage) = load_retakes_instance(services);

    for map in ["de_dust2", "de_inferno", "de_nuke", "de_mirage", "de_train"] {
        let snap = make_7p_snapshot(map, None);
        activate_map(&mut inst, &snap, &mut storage, map);
        let rs = ActiveEvent {
            i32_args: [1, 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let out = inst
            .call_hook(
                HookId::RoundStart,
                HookPhase::Post,
                &ModExecContext {
                    snapshot: &snap,
                    event: &rs,
                    storage: &storage,
                    fuel_budget: RETAKES_TEST_FUEL,
                },
            )
            .unwrap();
        let moved = out
            .ops
            .iter()
            .filter(|op| matches!(op, EngineOp::SetOrigin { .. }))
            .count();
        assert_eq!(moved, 7, "map {map} must position all 7 players");
    }

    // Custom map with func_bomb_target fallback
    let mut custom_snap = make_7p_snapshot("de_custom_arena", None);
    custom_snap.entities.insert(
        20,
        EntitySnap {
            index: 20,
            classname: "func_bomb_target".to_string(),
            origin: [500.0, -500.0, 128.0],
            velocity: [0.0; 3],
            health: 0.0,
            flags: 0,
        },
    );
    activate_map(&mut inst, &custom_snap, &mut storage, "de_custom_arena");
    let rs = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let out = inst
        .call_hook(
            HookId::RoundStart,
            HookPhase::Post,
            &ModExecContext {
                snapshot: &custom_snap,
                event: &rs,
                storage: &storage,
                fuel_budget: RETAKES_TEST_FUEL,
            },
        )
        .unwrap();
    let t0_origin = out
        .ops
        .iter()
        .find_map(|op| match op {
            EngineOp::SetOrigin { index: 1, origin } => Some(*origin),
            _ => None,
        })
        .expect("T0 placed near custom func_bomb_target");
    assert!(
        (t0_origin[0] - 500.0).abs() < 200.0 && (t0_origin[1] + 500.0).abs() < 200.0,
        "T0 spawned near custom bomb target: {t0_origin:?}"
    );
}

#[test]
fn retakes_live_server_integration_and_configs_registration() {
    // 1. Verify configs/plugins.toml registers retakes & multi_1v1 disabled by default
    let plugins_toml = std::fs::read_to_string(repo_root().join("configs/plugins.toml"))
        .expect("read configs/plugins.toml");
    assert!(plugins_toml.contains("../mods/gamemodes/retakes"));
    assert!(plugins_toml.contains("../mods/gamemodes/multi_1v1.py"));

    // 2. Verify configs/lang/gamemodes.toml loads all EN & RU Retakes + Multi-1v1 keys
    let lang_toml = std::fs::read_to_string(repo_root().join("configs/lang/gamemodes.toml"))
        .expect("read configs/lang/gamemodes.toml");
    let services = SharedPlatformServices::new();
    {
        let mut svc = services.lock();
        let loaded = svc.lang.load_toml_str(&lang_toml);
        assert!(
            loaded >= 80,
            "expected full bilingual gamemodes catalog, got {loaded}"
        );
        svc.lang
            .update_player_from_userinfo(0, Some("RU_USER"), Some("ru"), None);
        svc.lang
            .update_player_from_userinfo(1, Some("EN_USER"), Some("en"), None);
        assert!(
            svc.lang
                .tr(
                    Some(1),
                    "RETAKES_BANNER",
                    &[("site", "A"), ("t", "3"), ("ct", "4"), ("round", "PISTOL")]
                )
                .contains("RETAKES — BOMBSITE A")
        );
        assert!(
            svc.lang
                .tr(
                    Some(0),
                    "RETAKES_BANNER",
                    &[
                        ("site", "А"),
                        ("t", "3"),
                        ("ct", "4"),
                        ("round", "ПИСТОЛЕТНЫЙ")
                    ]
                )
                .contains("РЕТЕЙК — ТОЧКА А")
        );
        assert!(
            svc.lang
                .tr(Some(1), "M1V1_CHAMP_TAG", &[])
                .contains("[Arena 1")
        );
        assert!(
            svc.lang
                .tr(Some(0), "M1V1_CHAMP_TAG", &[])
                .contains("[Арена 1")
        );
    }

    // 3. Live ModdedGame<CsGame> integration with retakes WASM package + /guns menu select
    let pkg = load_package_dir(&repo_root().join("mods/gamemodes/retakes")).unwrap();
    let mut host = ServerModHost::new();
    host.load_package(pkg).unwrap();

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
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("mp_freezetime", "0");
    cvars.set("bot_quota", "0");

    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        ServerMap::stub_with_entities("de_dust2", ents),
        ModdedGame::new(CsGame::new(), host),
        DeltaRegistry::cs16_default(),
        cvars,
    );

    let mut hub = LoopbackHub::new();
    let ep = hub.connect();
    let mut conn = Connection::new(
        ep,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("Retaker", 76_561_197_960_265_901, &md5_hex(b"retakes")),
    );
    conn.set_userinfo("_vgui_menus", "0");
    conn.connect();

    let mut now = 0.0;
    for _ in 0..300 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut hub, now);
        if conn.state() == &ConnState::InGame {
            break;
        }
    }
    assert_eq!(conn.state(), &ConnState::InGame);
    conn.send_stringcmd("jointeam 2");
    conn.send_stringcmd("joinclass 1");
    for _ in 0..50 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut hub, now);
    }

    conn.send_stringcmd("guns");
    for _ in 0..20 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut hub, now);
    }
    let menu = conn
        .fx()
        .hud
        .menu
        .as_ref()
        .expect("retakes /guns menu opened");
    assert!(
        menu.text.contains("Retakes Loadout Allocator"),
        "{}",
        menu.text
    );

    conn.send_stringcmd("menuselect 3"); // CT Rifle: AUG Bullpup
    for _ in 0..20 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut hub, now);
    }
    assert!(
        conn.fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("[Retakes]") && c.text.contains("AUG")),
        "chat={:?}",
        conn.fx().hud.chat
    );
    assert_eq!(
        server.game_mut().host.list_mods()[0].status,
        ModStatus::Active
    );
}
