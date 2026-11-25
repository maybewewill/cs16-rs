//! Comprehensive test suite for `cs-mod-py`:
//! - Python 3 language features (functions, closures, classes, comprehensions, f-strings,
//!   slicing, try/except/finally, math/random/json, builtins)
//! - Decorators & hooks (`PlayerTakeDamage` override + supercede, chat/console commands, timers)
//! - Deterministic `OutOfFuel` cutoff on `while True:` with Python line stack trace (uncatchable)
//! - Simultaneous multilingual `player.tr(...)` (`en` + `ru`)
//! - 1-line `mod.vfx.*` / `vfx.*` temp-entity emission
//! - `mod.storage` preservation across hot-reload & `PyHotReloader` mtime watching
//! - Menus, leaderboards, custom weapons, cross-plugin RPC, allowlisted HTTP webhooks
//! - Full end-to-end execution of all 5 community plugins:
//!   `resetscore.py`, `vip_system.py`, `afk_control.py`, `damager_and_parachute.py`,
//!   `antiflood_admanager.py`

use std::collections::BTreeMap;
use std::sync::Arc;

use cs_mod_api::services::SharedPlatformServices;
use cs_mod_api::{
    AdminFlags, ArmorKind, DEFAULT_FUEL_PER_FRAME, DEFAULT_MEMORY_BYTES, HookAction, HookId,
    HookPhase, Permission, Permissions, RpcValue, TeamKind, VfxOp,
};
use cs_mod_host::natives::{ActiveEvent, ClientSnap, EngineOp, EngineSnapshot, EntitySnap};
use cs_mod_host::runtime::{ModExecContext, ModInstance, ModRuntime};
use cs_mod_py::{
    PyModInstance, PyModRuntime, PySourceSyntaxCheck, default_python_permissions,
    discover_python_mods, synthesize_manifest_from_py,
};

fn make_sample_snapshot() -> EngineSnapshot {
    let mut entities = BTreeMap::new();
    entities.insert(
        1,
        EntitySnap {
            index: 1,
            classname: "player".to_string(),
            origin: [100.0, 200.0, 64.0],
            velocity: [0.0, 0.0, -320.0],
            health: 100.0,
            flags: 0, // airborne (FL_ONGROUND not set)
        },
    );
    entities.insert(
        2,
        EntitySnap {
            index: 2,
            classname: "player".to_string(),
            origin: [150.0, 200.0, 64.0],
            velocity: [180.0, 0.0, 0.0],
            health: 100.0,
            flags: 1 << 9, // FL_ONGROUND
        },
    );

    let clients = vec![
        ClientSnap {
            slot: 0,
            active: true,
            name: "Ivan".to_string(),
            health: 100.0,
            max_health: 100.0,
            armor: 0,
            armor_kind: 0,
            money: 800,
            frags: 5,
            deaths: 9,
            team: 1, // Terrorist
            alive: true,
            active_weapon: 6, // C4
            steamid: "STEAM_0:0:1001".to_string(),
            ip: "10.0.0.1".to_string(),
            admin_flags: AdminFlags::LEVEL_H.bits(), // 't' VIP
            lang: "ru".to_string(),
            ..ClientSnap::default()
        },
        ClientSnap {
            slot: 1,
            active: true,
            name: "Alice".to_string(),
            health: 100.0,
            max_health: 100.0,
            armor: 100,
            armor_kind: 2,
            money: 4000,
            frags: 12,
            deaths: 3,
            team: 1, // Terrorist
            alive: true,
            active_weapon: 28, // AK47
            steamid: "STEAM_0:0:1002".to_string(),
            ip: "10.0.0.2".to_string(),
            admin_flags: AdminFlags::USER.bits(),
            lang: "en".to_string(),
            ..ClientSnap::default()
        },
    ];

    let mut cvars = BTreeMap::new();
    cvars.insert("sv_parachute".to_string(), "1".to_string());
    cvars.insert("mp_freezetime".to_string(), "3".to_string());

    EngineSnapshot {
        time_secs: 42.5,
        frametime: 0.01,
        max_clients: 32,
        map_name: "de_dust2".to_string(),
        entities,
        clients,
        cvars,
        ..EngineSnapshot::default()
    }
}

fn make_services_ru_en() -> SharedPlatformServices {
    let services = SharedPlatformServices::new();
    {
        let mut svc = services.lock();
        svc.lang.load_toml_str(
            r#"
            [en]
            RS_DONE = "Your score has been reset, {name}!"
            VIP_BOUGHT = "You received VIP kit: {weapon}!"
            VIP_NOT_ALLOWED = "VIP access required!"
            FLOOD_WARN = "Stop flooding the chat!"
            AD_1 = "Welcome to cs16-rs!"
            AD_2 = "Type /rs to reset your score."
            [ru]
            RS_DONE = "Ваш счёт обнулён, {name}!"
            VIP_BOUGHT = "Вы получили VIP набор: {weapon}!"
            VIP_NOT_ALLOWED = "Требуется статус VIP!"
            FLOOD_WARN = "Не флудите в чат!"
            AD_1 = "Добро пожаловать на сервер cs16-rs!"
            AD_2 = "Введите /rs чтобы обнулить счёт."
            "#,
        );
        svc.lang
            .update_player_from_userinfo(0, Some("STEAM_0:0:1001"), None, Some("russian"));
        svc.lang
            .update_player_from_userinfo(1, Some("STEAM_0:0:1002"), Some("en"), None);
        svc.admins.set_slot_flags(0, AdminFlags::LEVEL_H);
        svc.admins.set_slot_flags(1, AdminFlags::USER);
    }
    services
}

fn exec_ctx<'a>(
    snapshot: &'a EngineSnapshot,
    event: &'a ActiveEvent,
    storage: &'a BTreeMap<String, String>,
) -> ModExecContext<'a> {
    ModExecContext {
        snapshot,
        event,
        storage,
        fuel_budget: DEFAULT_FUEL_PER_FRAME,
    }
}

#[test]
fn language_features_stdlib_classes_comprehensions_and_fstrings() {
    let src = r#"
import math
import json
import random
from cs16 import mod, Color

class Counter:
    def __init__(self, start=0):
        self.value = start

    def bump(self, delta=1):
        self.value += delta
        return self.value

class DoubleCounter(Counter):
    def bump(self, delta=1):
        self.value += delta * 2
        return self.value

def compute(nums, scale=2, *extra, **opts):
    total = sum(x * scale for x in nums if x % 2 == 0)
    total += sum(extra)
    if opts.get("bonus"):
        total += opts["bonus"]
    return total

@mod.console_command("py_selftest")
def cmd_selftest(player, args):
    c = DoubleCounter(10)
    c.bump(3)
    squares = [x ** 2 for x in range(1, 6)]
    rev = squares[::-1]
    lookup = {k: k * 10 for k in range(3)}
    s = {1, 2, 2, 3} | {3, 4}
    assert len(s) == 4
    assert 0 <= c.value < 20
    assert isinstance(c, Counter) and isinstance(c, DoubleCounter)
    assert any(x > 20 for x in squares) and all(x > 0 for x in squares)
    pairs = list(enumerate(["a", "b"], 1))
    assert pairs == [(1, "a"), (2, "b")]
    ordered = sorted(["bb", "a", "ccc"], key=lambda w: len(w), reverse=True)
    assert ordered == ["ccc", "bb", "a"]

    caught = None
    try:
        _ = 1 / 0
    except ZeroDivisionError as e:
        caught = e.kind
    finally:
        c.bump(1)

    random.seed(12345)
    r1 = random.randint(1, 100)
    random.seed(12345)
    r2 = random.randint(1, 100)
    assert r1 == r2

    dist = math.dist((0, 0, 0), (3, 4, 0))
    payload = json.loads(json.dumps({"dist": dist, "rev0": rev[0], "k2": lookup[2], "caught": caught}))
    res = compute([1, 2, 3, 4], 3, 5, bonus=7)
    return f"val={c.value};res={res};dist={payload['dist']:.1f};rev0={payload['rev0']};caught={payload['caught']}"
"#;

    let mut inst = PyModInstance::from_source(
        "selftest",
        "selftest.py",
        src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .expect("load selftest");
    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();
    let init_ev = ActiveEvent::default();
    let init_out = inst.call_init(&exec_ctx(&snap, &init_ev, &storage)).unwrap();
    assert!(init_out
        .ops
        .iter()
        .any(|op| matches!(op, EngineOp::RegisterCommand { name } if name == "py_selftest")));

    let ev = ActiveEvent {
        str_args: [
            "py_selftest".to_string(),
            String::new(),
            "py_selftest".to_string(),
        ],
        ..Default::default()
    };
    let out = inst
        .call_hook(
            HookId::ServerCommand,
            HookPhase::Pre,
            &exec_ctx(&snap, &ev, &storage),
        )
        .unwrap();
    assert_eq!(out.action, HookAction::Handled);
    assert_eq!(
        out.override_str.as_deref(),
        Some("val=18;res=30;dist=5.0;rev0=25;caught=ZeroDivisionError")
    );
}

#[test]
fn player_take_damage_override_and_supercede() {
    let src = r#"
from cs16 import mod, HookAction

@mod.hook("player_take_damage", phase="pre")
def on_damage(ev):
    if ev.team_attack:
        return HookAction.SUPERCEDE
    if ev.attacker is not None and ev.attacker.has_flag("t"):
        ev.damage = ev.damage * 1.5
        ev.hitgroup = 1
        return HookAction.OVERRIDE
    return HookAction.CONTINUE
"#;
    let mut inst = PyModInstance::from_source(
        "dmg_mod",
        "dmg_mod.py",
        src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    inst.configure_v2(make_services_ru_en(), Vec::new(), None);
    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();

    let init_out = inst
        .call_init(&exec_ctx(&snap, &ActiveEvent::default(), &storage))
        .unwrap();
    assert_eq!(init_out.hooks_registered.len(), 1);
    assert_eq!(init_out.hooks_registered[0].hook, HookId::PlayerTakeDamage);
    assert_eq!(init_out.hooks_registered[0].phase, HookPhase::Pre);

    // 1. VIP attacker (slot 0) deals 1.5x damage + hitgroup override
    let dmg_ev = ActiveEvent {
        i32_args: [1, 0, 2, 28, 0, 0],
        f32_args: [40.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let out = inst
        .call_hook(
            HookId::PlayerTakeDamage,
            HookPhase::Pre,
            &exec_ctx(&snap, &dmg_ev, &storage),
        )
        .unwrap();
    assert_eq!(out.action, HookAction::Override);
    assert_eq!(out.override_f32, Some(60.0));
    assert_eq!(out.override_i32, Some(1));

    // 2. Team attack -> SUPERCEDE (cancel damage)
    let tk_ev = ActiveEvent {
        i32_args: [1, 0, 2, 28, 0, 1],
        f32_args: [40.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let tk_out = inst
        .call_hook(
            HookId::PlayerTakeDamage,
            HookPhase::Pre,
            &exec_ctx(&snap, &tk_ev, &storage),
        )
        .unwrap();
    assert_eq!(tk_out.action, HookAction::Supercede);
}

#[test]
fn infinite_loop_halts_with_out_of_fuel_and_line_traceback_uncatchable() {
    let src = r#"
from cs16 import mod

def spin_forever():
    i = 0
    try:
        while True:
            i += 1
    except Exception:
        return 999

@mod.hook("round_start", phase="post")
def on_round(ev):
    spin_forever()
"#;
    let mut inst = PyModInstance::from_source(
        "bad_loop",
        "bad_loop.py",
        src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();
    let ev = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let ctx = ModExecContext {
        snapshot: &snap,
        event: &ev,
        storage: &storage,
        fuel_budget: 2_000,
    };
    let err = inst
        .call_hook(HookId::RoundStart, HookPhase::Post, &ctx)
        .expect_err("infinite loop must trigger OutOfFuel");
    let msg = err.to_string();
    assert!(msg.contains("OutOfFuel"), "expected OutOfFuel in: {msg}");
    assert!(msg.contains("bad_loop.py"), "expected file name in traceback: {msg}");
    assert!(msg.contains("in on_round"), "expected caller frame in traceback: {msg}");
    assert!(msg.contains("in spin_forever"), "expected inner frame in traceback: {msg}");
    assert!(msg.contains("line 7") || msg.contains("line 8"), "expected loop line number: {msg}");
    assert!(inst.last_error().is_some_and(|e| e.is_out_of_fuel()));
}

#[test]
fn vfx_leaderboard_menu_custom_weapon_rpc_and_webhook_bindings() {
    let src = r#"
from cs16 import mod, vfx, Menu, CustomWeapon, WeaponId

@mod.info(
    id="showcase",
    name="Showcase",
    version="2.0.0",
    permissions=["entities", "messages", "vfx", "menus", "storage", "weapons", "rpc", "http:hooks.example.com"],
)
def _meta():
    pass

mod.register_custom_weapon(CustomWeapon(
    "golden_ak",
    WeaponId.AK47,
    "Golden AK-47",
    v_model="models/v_golden_ak.mdl",
    damage_mult=1.5,
    clip_size=40,
    cost=5000,
))

@mod.export_fn("showcase.double")
def rpc_double(x):
    return x * 2

def on_menu_pick(player, idx, info):
    player.give_custom_weapon(info)
    vfx.shockwave_ring(player.origin, radius=240.0, rgba=(0, 220, 255, 255), life=12, width=18)
    mod.vfx.laser_beam((0, 0, 0), (100, 0, 0), rgba=(255, 0, 0, 255))

@mod.chat_command("/shop")
def cmd_shop(player, args):
    mod.leaderboard.upsert("shop_visits", player.steamid, 100.0, {"name": player.name})
    rank_info = mod.leaderboard.rank("shop_visits", player.steamid)
    assert rank_info["rank"] == 1
    m = Menu("Extra Shop", callback=on_menu_pick)
    m.add_item("Golden AK-47", "golden_ak")
    m.show(player)
    host_val = mod.call_fn("host.ping", 21)
    mod.emit_event("shop.opened", {"slot": player.slot, "host_val": host_val})
    mod.http_post_json("http://hooks.example.com/notify", {"user": player.name})
"#;

    let manifest = synthesize_manifest_from_py("fallback", "showcase.py", src).unwrap();
    assert_eq!(manifest.id, "showcase");
    assert_eq!(manifest.version, "2.0.0");
    assert!(manifest.permissions.contains(Permission::Http));
    assert_eq!(manifest.http_hosts, vec!["hooks.example.com".to_string()]);

    let mut inst = PyModInstance::from_source(
        &manifest.id,
        "showcase.py",
        src,
        manifest.permissions,
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    let services = make_services_ru_en();
    services
        .lock()
        .rpc
        .register_host_fn("host.ping", |args| match args.first() {
            Some(RpcValue::Int(n)) => RpcValue::Int(n + 1),
            _ => RpcValue::Int(0),
        });
    inst.configure_v2(services.clone(), manifest.http_hosts.clone(), None);

    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();
    let init_out = inst
        .call_init(&exec_ctx(&snap, &ActiveEvent::default(), &storage))
        .unwrap();
    assert!(init_out
        .ops
        .iter()
        .any(|op| matches!(op, EngineOp::RegisterCustomWeapon(spec) if spec.id_name == "golden_ak")));

    // Exported RPC callable synchronously
    let rpc_entry = services.lock().rpc.resolve_fn("showcase.double").cloned().unwrap();
    let rpc_ev = ActiveEvent {
        i32_args: [21, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let (rc, _) = inst
        .call_rpc(rpc_entry.callback_id, &exec_ctx(&snap, &rpc_ev, &storage))
        .unwrap();
    assert_eq!(rc, 42);

    // Chat command `/shop` opens paginated menu, upserts leaderboard, emits RPC event & webhook
    let say_ev = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["/shop".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let out = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &say_ev, &storage))
        .unwrap();
    assert_eq!(out.action, HookAction::Supercede);
    let menu_id = out
        .ops
        .iter()
        .find_map(|op| match op {
            EngineOp::MenuDisplay(spec) => Some(spec.menu_id),
            _ => None,
        })
        .expect("MenuDisplay emitted");
    assert!(out.ops.iter().any(|op| matches!(op, EngineOp::RpcEmitEvent(e) if e.event_name == "shop.opened" && e.payload_json.contains("22"))));
    assert!(out.ops.iter().any(|op| matches!(op, EngineOp::HttpPostJson(h) if h.url == "http://hooks.example.com/notify")));

    // Simulate selecting item 0 on the menu -> gives custom weapon + emits VFX
    let cb_id = services.lock().menus.menu(menu_id).unwrap().callback_id;
    let pick_ev = ActiveEvent {
        i32_args: [0, menu_id as i32, 0, cb_id, 0, 0],
        str_args: ["golden_ak".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let pick_out = inst
        .call_menu(cb_id, 0, menu_id, 0, &exec_ctx(&snap, &pick_ev, &storage))
        .unwrap();
    assert!(pick_out.ops.iter().any(|op| matches!(op, EngineOp::GiveCustomWeapon { slot: 0, id_name } if id_name == "golden_ak")));
    assert!(pick_out
        .ops
        .iter()
        .any(|op| matches!(op, EngineOp::EmitVfx(VfxOp::ShockwaveRing { .. }))));
    assert!(pick_out
        .ops
        .iter()
        .any(|op| matches!(op, EngineOp::EmitVfx(VfxOp::LaserBeam { .. }))));
}

#[test]
fn storage_preserved_across_hot_reload_and_discovery() {
    let v1 = r#"
from cs16 import mod

@mod.chat_command("/count")
def cmd_count(player, args):
    n = int(mod.storage.get("hits", "0")) + 1
    mod.storage["hits"] = str(n)
    player.chat(f"v1 hits={n}")
"#;
    let v2 = r#"
from cs16 import mod

@mod.chat_command("/count")
def cmd_count(player, args):
    n = int(mod.storage.get("hits", "0")) + 10
    mod.storage.set("hits", str(n))
    player.chat(f"v2 hits={n}")
"#;
    let mut inst = PyModInstance::from_source(
        "counter",
        "counter.py",
        v1,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    let snap = make_sample_snapshot();
    let mut storage = BTreeMap::new();
    let say_ev = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["/count".to_string(), String::new(), String::new()],
        ..Default::default()
    };

    // Call v1 twice
    for expected in ["v1 hits=1", "v1 hits=2"] {
        let out = inst
            .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &say_ev, &storage))
            .unwrap();
        for (k, v) in out.storage_writes {
            match v {
                Some(val) => {
                    storage.insert(k, val);
                }
                None => {
                    storage.remove(&k);
                }
            }
        }
        assert!(out
            .ops
            .iter()
            .any(|op| matches!(op, EngineOp::SayText { text, .. } if text == expected)));
    }

    // Hot-reload to v2 — persistent `storage` ("hits" == "2") is preserved!
    inst.reload_source(v2).unwrap();
    let out_v2 = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &say_ev, &storage))
        .unwrap();
    assert!(out_v2
        .ops
        .iter()
        .any(|op| matches!(op, EngineOp::SayText { text, .. } if text == "v2 hits=12")));

    // Discovery + mtime hot-reloader smoke test on temp directory
    let tmp = std::env::temp_dir().join(format!("cs16_py_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("pkg_mod")).unwrap();
    let single_py = tmp.join("hello.py");
    let main_py = tmp.join("pkg_mod").join("main.py");
    std::fs::write(&single_py, v1).unwrap();
    std::fs::write(&main_py, v2).unwrap();

    let discovered = discover_python_mods(&tmp).unwrap();
    assert_eq!(discovered.len(), 2);

    let mut watcher = cs_mod_py::PyHotReloader::default();
    watcher.watch("hello", &single_py);
    assert!(watcher.poll_changes(true).is_empty());
    // Change file length to trigger immediate change detection regardless of filesystem mtime granularity
    std::fs::write(&single_py, format!("{v2}\n# updated")).unwrap();
    let changed = watcher.poll_changes(true);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0, "hello");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn community_plugin_1_resetscore_en_and_ru() {
    let src = r#"
from cs16 import mod, Color, HookAction

@mod.info(id="resetscore", name="ResetScore", version="1.0.0")
def _meta():
    pass

@mod.chat_command("/rs", "/resetscore", "!rs")
def cmd_resetscore(player, args):
    player.set_score(0, 0)
    msg = player.tr("RS_DONE", name=player.name)
    player.chat(f"{Color.GREEN}[RS]{Color.DEFAULT} {msg}")
    return HookAction.SUPERCEDE
"#;

    let mut inst = PyModInstance::from_source(
        "resetscore",
        "resetscore.py",
        src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    inst.configure_v2(make_services_ru_en(), Vec::new(), None);
    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();

    // Russian player (slot 0) types `/rs`
    let ev_ru = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["/rs".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let out_ru = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &ev_ru, &storage))
        .unwrap();
    assert_eq!(out_ru.action, HookAction::Supercede);
    assert!(out_ru.ops.contains(&EngineOp::SetPlayerScore {
        slot: 0,
        frags: 0,
        deaths: 0
    }));
    assert!(out_ru.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text }
        if text == "\x04[RS]\x01 Ваш счёт обнулён, Ivan!"
    )));

    // English player (slot 1) types `!rs`
    let ev_en = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        str_args: ["!rs".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let out_en = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &ev_en, &storage))
        .unwrap();
    assert!(out_en.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(1), text }
        if text == "\x04[RS]\x01 Your score has been reset, Alice!"
    )));
}

#[test]
fn community_plugin_2_vip_system() {
    let src = r#"
from cs16 import mod, vfx, Color, HookAction, Armor, Team, Menu

round_num = 0
used_this_round = set()

@mod.hook("round_start", phase="post")
def on_round_start(ev):
    global round_num
    round_num = ev.round_num
    used_this_round.clear()

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    if not p.has_flag("t"):
        return
    p.set_armor(100, Armor.VESTHELM)
    p.give_item("weapon_hegrenade")
    if p.team == Team.CT:
        p.set_defuser(True)

@mod.hook("player_killed", phase="post")
def on_kill(ev):
    k = ev.killer
    if k is None or not k.has_flag("t"):
        return
    bonus_money = 500 if ev.headshot else 300
    bonus_hp = 20 if ev.headshot else 10
    k.add_money(bonus_money, flash=True)
    k.set_health(min(k.health + bonus_hp, k.max_health))
    vfx.aura_light(k.origin, radius=18, rgb=(255, 215, 0), life=8, decay=15)

def on_vip_pick(player, info):
    if player.slot in used_this_round:
        return
    used_this_round.add(player.slot)
    player.give_item(info)
    player.give_item("weapon_deagle")
    player.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} {player.tr('VIP_BOUGHT', weapon=info)}")

@mod.chat_command("/vip", "/vipmenu")
def cmd_vip(player, args):
    if not player.has_flag("t"):
        player.chat(player.tr("VIP_NOT_ALLOWED"))
        return HookAction.SUPERCEDE
    if round_num < 2:
        return HookAction.SUPERCEDE
    menu = Menu("VIP Arsenal", callback=on_vip_pick)
    menu.add_item("AK-47 + Deagle", "weapon_ak47")
    menu.add_item("M4A1 + Deagle", "weapon_m4a1")
    menu.add_item("AWP + Deagle", "weapon_awp")
    menu.show(player)
    return HookAction.SUPERCEDE

@mod.hook("player_say", phase="pre")
def on_vip_chat(ev):
    if ev.player.has_flag("t") and not ev.message.startswith("/"):
        ev.message = f"{Color.GREEN}[VIP] {Color.TEAM}{ev.player.name}{Color.DEFAULT}: {ev.message}"
        return HookAction.OVERRIDE
"#;

    let mut inst = PyModInstance::from_source(
        "vip_system",
        "vip_system.py",
        src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    let services = make_services_ru_en();
    inst.configure_v2(services.clone(), Vec::new(), None);
    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();

    // Round 2 starts
    let rs_ev = ActiveEvent {
        i32_args: [2, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    inst.call_hook(HookId::RoundStart, HookPhase::Post, &exec_ctx(&snap, &rs_ev, &storage))
        .unwrap();

    // VIP (slot 0) spawns -> vesthelm + hegrenade
    let sp_ev = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        ..Default::default()
    };
    let sp_out = inst
        .call_hook(HookId::PlayerSpawn, HookPhase::Post, &exec_ctx(&snap, &sp_ev, &storage))
        .unwrap();
    assert!(sp_out.ops.contains(&EngineOp::SetPlayerArmor {
        slot: 0,
        armor: 100,
        kind: ArmorKind::VestHelm
    }));
    assert!(sp_out.ops.contains(&EngineOp::GiveItem {
        slot: 0,
        item: "weapon_hegrenade".to_string()
    }));

    // VIP kills enemy with headshot -> +$500 + AuraLight VFX
    let kill_ev = ActiveEvent {
        i32_args: [1, 0, 28, 1, 1, 0],
        str_args: ["ak47".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let kill_out = inst
        .call_hook(HookId::PlayerKilled, HookPhase::Post, &exec_ctx(&snap, &kill_ev, &storage))
        .unwrap();
    assert!(kill_out.ops.contains(&EngineOp::AddPlayerMoney {
        slot: 0,
        delta: 500,
        flash: true
    }));
    assert!(kill_out
        .ops
        .iter()
        .any(|op| matches!(op, EngineOp::EmitVfx(VfxOp::AuraLight { .. }))));

    // VIP chat tag rewrite
    let chat_ev = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["rush B!".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let chat_out = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &chat_ev, &storage))
        .unwrap();
    assert_eq!(chat_out.action, HookAction::Override);
    assert_eq!(
        chat_out.override_str.as_deref(),
        Some("\x04[VIP] \x03Ivan\x01: rush B!")
    );

    // VIP opens /vipmenu and picks AK-47
    let menu_ev = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["/vipmenu".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let menu_out = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &menu_ev, &storage))
        .unwrap();
    let mid = menu_out
        .ops
        .iter()
        .find_map(|op| match op {
            EngineOp::MenuDisplay(spec) => Some(spec.menu_id),
            _ => None,
        })
        .unwrap();
    let cb_id = services.lock().menus.menu(mid).unwrap().callback_id;
    let pick_ev = ActiveEvent {
        i32_args: [0, mid as i32, 0, cb_id, 0, 0],
        str_args: ["weapon_ak47".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let pick_out = inst
        .call_menu(cb_id, 0, mid, 0, &exec_ctx(&snap, &pick_ev, &storage))
        .unwrap();
    assert!(pick_out.ops.contains(&EngineOp::GiveItem {
        slot: 0,
        item: "weapon_ak47".to_string()
    }));
    assert!(pick_out.ops.contains(&EngineOp::GiveItem {
        slot: 0,
        item: "weapon_deagle".to_string()
    }));
}

#[test]
fn community_plugin_3_afk_control() {
    let src = r#"
import math
from cs16 import mod, Team, WeaponId

last_pos = {}
afk_secs = {}

@mod.timer(1.0, repeat=True)
def check_afk():
    players = mod.players()
    for p in players:
        if not p.alive or p.team == Team.SPECTATOR:
            afk_secs[p.slot] = 0.0
            continue
        pos = p.origin
        prev = last_pos.get(p.slot)
        if prev is not None and math.dist(pos, prev) < 4.0:
            afk_secs[p.slot] = afk_secs.get(p.slot, 0.0) + 1.0
        else:
            afk_secs[p.slot] = 0.0
            last_pos[p.slot] = pos

        idle = afk_secs[p.slot]
        if idle >= 15.0 and p.team == Team.TERRORIST and p.has_weapon(WeaponId.C4):
            mates = [m for m in players if m.slot != p.slot and m.alive and m.team == Team.TERRORIST and afk_secs.get(m.slot, 0.0) < 5.0]
            if mates:
                nearest = min(mates, key=lambda m: math.dist(m.origin, pos))
                p.drop_slot(5)
                nearest.give_item("weapon_c4")
        if idle >= 45.0:
            p.slay()
            p.set_team(Team.SPECTATOR)
"#;

    let mut inst = PyModInstance::from_source(
        "afk_control",
        "afk_control.py",
        src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    let mut snap = make_sample_snapshot();
    let storage = BTreeMap::new();
    let init_out = inst
        .call_init(&exec_ctx(&snap, &ActiveEvent::default(), &storage))
        .unwrap();
    assert_eq!(init_out.timers.len(), 1);
    let cb_id = init_out.timers[0].cb_id;

    // Tick 16 times: slot 0 stays still with C4, slot 1 moves each tick -> C4 transfers at 15s!
    let mut transferred = false;
    for tick in 0..16 {
        snap.entities.get_mut(&2).unwrap().origin[0] = 150.0 + (tick as f32) * 20.0;
        let out = inst
            .call_timer(cb_id, 0, &exec_ctx(&snap, &ActiveEvent::default(), &storage))
            .unwrap();
        if out.ops.contains(&EngineOp::GiveItem {
            slot: 1,
            item: "weapon_c4".to_string(),
        }) {
            assert!(out.ops.contains(&EngineOp::DropSlot {
                slot: 0,
                inv_slot: 5
            }));
            transferred = true;
        }
    }
    assert!(transferred, "C4 must transfer from AFK T to nearest moving teammate at 15s");

    // Continue ticking to 45s -> slot 0 is moved to Spectator
    let mut moved_to_spec = false;
    for tick in 16..47 {
        snap.entities.get_mut(&2).unwrap().origin[0] = 150.0 + (tick as f32) * 20.0;
        let out = inst
            .call_timer(cb_id, 0, &exec_ctx(&snap, &ActiveEvent::default(), &storage))
            .unwrap();
        if out.ops.contains(&EngineOp::SetPlayerTeam {
            slot: 0,
            team: TeamKind::Spectator,
            model_class: None,
        }) {
            moved_to_spec = true;
        }
    }
    assert!(moved_to_spec, "45s AFK player must be moved to Spectator");
}

#[test]
fn community_plugin_4_damager_and_parachute() {
    let src = r#"
from cs16 import mod, Button, HookAction

damager_enabled = {}

@mod.chat_command("/damager")
def cmd_damager(player, args):
    cur = damager_enabled.get(player.slot, True)
    damager_enabled[player.slot] = not cur
    return HookAction.SUPERCEDE

@mod.hook("player_take_damage", phase="post")
def on_damage_post(ev):
    att = ev.attacker
    if att is None or att.slot == ev.victim.slot:
        return
    if damager_enabled.get(att.slot, True):
        att.hud(f"{int(round(ev.damage))}", x=-1.0, y=0.55, channel=2, rgba=(0, 180, 255, 255), hold=0.8)

@mod.hook("player_pre_think", phase="pre")
def on_pre_think(ev):
    p = ev.player
    if not p.alive or p.on_ground:
        return
    if (ev.buttons & Button.USE) != 0:
        vx, vy, vz = p.velocity
        if vz < -100.0:
            p.set_velocity((vx, vy, -100.0))
            p.set_gravity(0.1)
"#;

    let mut inst = PyModInstance::from_source(
        "damager_and_parachute",
        "damager_and_parachute.py",
        src,
        default_python_permissions(),
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();

    // 1. Damage post shows HUD damage number to attacker
    let dmg_ev = ActiveEvent {
        i32_args: [1, 0, 2, 28, 0, 0],
        f32_args: [27.4, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let dmg_out = inst
        .call_hook(
            HookId::PlayerTakeDamage,
            HookPhase::Post,
            &exec_ctx(&snap, &dmg_ev, &storage),
        )
        .unwrap();
    assert!(dmg_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::HudMessage(spec) if spec.slot == Some(0) && spec.text == "27" && spec.channel == 2
    )));

    // 2. Holding +use while falling clamps Z velocity to -100 and sets low gravity
    let think_ev = ActiveEvent {
        i32_args: [0, 1 << 5, 0, 0, 0, 0], // Button.USE
        ..Default::default()
    };
    let think_out = inst
        .call_hook(
            HookId::PlayerPreThink,
            HookPhase::Pre,
            &exec_ctx(&snap, &think_ev, &storage),
        )
        .unwrap();
    assert!(think_out.ops.contains(&EngineOp::SetVelocity {
        index: 1,
        velocity: [0.0, 0.0, -100.0]
    }));
    assert!(think_out.ops.contains(&EngineOp::SetPlayerGravity {
        slot: 0,
        gravity: 0.1
    }));
}

#[test]
fn community_plugin_5_antiflood_admanager() {
    let src = r#"
import json
from cs16 import mod, HookAction

@mod.info(
    id="antiflood_admanager",
    version="1.0.0",
    permissions=["entities", "messages", "timers", "http:discord.com"],
)
def _meta():
    pass

last_chat_time = {}
ad_keys = ["AD_1", "AD_2"]
ad_cursor = 0

@mod.hook("player_say", phase="pre")
def on_say(ev):
    if ev.message.startswith("/report"):
        return HookAction.CONTINUE
    now = mod.time
    prev = last_chat_time.get(ev.slot, -999.0)
    if now - prev < 0.75:
        ev.player.chat(ev.player.tr("FLOOD_WARN"))
        return HookAction.SUPERCEDE
    last_chat_time[ev.slot] = now
    return HookAction.CONTINUE

@mod.timer(60.0, repeat=True)
def broadcast_ad():
    global ad_cursor
    key = ad_keys[ad_cursor % len(ad_keys)]
    ad_cursor += 1
    for p in mod.players():
        p.chat(p.tr(key))

@mod.chat_command("/report")
def cmd_report(player, args):
    parts = args.split(" ", 1)
    target = parts[0] if len(parts) > 0 else "unknown"
    reason = parts[1] if len(parts) > 1 else "no reason"
    payload = json.dumps({
        "reporter": player.name,
        "target": target,
        "reason": reason,
        "map": mod.map_name,
    })
    mod.http_post_json("https://discord.com/api/webhooks/test", payload)
    return HookAction.SUPERCEDE
"#;

    let manifest = synthesize_manifest_from_py("antiflood", "antiflood_admanager.py", src).unwrap();
    let mut inst = PyModInstance::from_source(
        &manifest.id,
        "antiflood_admanager.py",
        src,
        manifest.permissions,
        DEFAULT_MEMORY_BYTES,
    )
    .unwrap();
    inst.configure_v2(make_services_ru_en(), manifest.http_hosts, None);
    let mut snap = make_sample_snapshot();
    let storage = BTreeMap::new();
    let init_out = inst
        .call_init(&exec_ctx(&snap, &ActiveEvent::default(), &storage))
        .unwrap();
    let ad_cb = init_out.timers[0].cb_id;

    // 1. First chat allowed, second chat 0.2s later blocked with localized RU warning
    let say_ev = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["hello".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let out1 = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &say_ev, &storage))
        .unwrap();
    assert_eq!(out1.action, HookAction::Continue);

    snap.time_secs += 0.2;
    let out2 = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &say_ev, &storage))
        .unwrap();
    assert_eq!(out2.action, HookAction::Supercede);
    assert!(out2.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text } if text == "Не флудите в чат!"
    )));

    // 2. Rotating ad timer sends Russian to slot 0 and English to slot 1 simultaneously!
    let ad_out = inst
        .call_timer(ad_cb, 0, &exec_ctx(&snap, &ActiveEvent::default(), &storage))
        .unwrap();
    assert!(ad_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(0), text } if text == "Добро пожаловать на сервер cs16-rs!"
    )));
    assert!(ad_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::SayText { slot: Some(1), text } if text == "Welcome to cs16-rs!"
    )));

    // 3. `/report Bob aimbot` enqueues non-blocking JSON webhook
    let rep_ev = ActiveEvent {
        i32_args: [1, 0, 0, 0, 0, 0],
        str_args: ["/report Bob aimbot".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let rep_out = inst
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &rep_ev, &storage))
        .unwrap();
    assert_eq!(rep_out.action, HookAction::Supercede);
    assert!(rep_out.ops.iter().any(|op| matches!(
        op,
        EngineOp::HttpPostJson(spec)
        if spec.url == "https://discord.com/api/webhooks/test"
            && spec.body.contains("\"target\": \"Bob\"")
            && spec.body.contains("\"reason\": \"aimbot\"")
    )));
}

#[test]
fn syntax_check_and_mod_runtime_trait() {
    let bad = "def broken(:\n    pass\n";
    let err = PySourceSyntaxCheck::check("broken.py", bad).unwrap_err();
    assert_eq!(err.exception_kind(), "SyntaxError");
    assert!(err.format_traceback().contains("broken.py"));

    let rt = PyModRuntime::new();
    assert_eq!(rt.name(), "python");
    let ok_src = b"from cs16 import mod\n@mod.chat_command('/hi')\ndef hi(p): p.chat('hi')\n";
    let mut boxed = rt
        .instantiate(
            "hi_mod",
            ok_src,
            default_python_permissions(),
            DEFAULT_MEMORY_BYTES,
        )
        .unwrap();
    assert!(boxed.memory_bytes() > 0);
    let snap = make_sample_snapshot();
    let storage = BTreeMap::new();
    let ev = ActiveEvent {
        i32_args: [0, 0, 0, 0, 0, 0],
        str_args: ["/hi".to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let out = boxed
        .call_hook(HookId::PlayerSay, HookPhase::Pre, &exec_ctx(&snap, &ev, &storage))
        .unwrap();
    assert!(out
        .ops
        .iter()
        .any(|op| matches!(op, EngineOp::SayText { slot: Some(0), text } if text == "hi")));
    let _unused: Permissions = default_python_permissions();
    let _arc: Arc<PyModRuntime> = Arc::new(rt);
}
