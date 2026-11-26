//! Integration tests for live `cs-mod-py` hot-reload, hook execution, ResMon accounting,
//! and syntax-error resilience inside `LocalGame` (`cs-local`).

use cs_assets::demo_pack::{DEMO_MAP_NAME, build_instant_demo_pack};
use cs_clientcore::ConnState;
use cs_local::{LocalGame, LocalOptions};
use cs_proto48::delta::GoldSrcUserCmd;

const VAMPIRE_PY: &str = r#"
from cs16 import mod, vfx, Color, HookAction, Armor

@mod.info(
    id="vampire_shockwave",
    name="Vampire Shockwave",
    version="1.0.0",
    permissions=["entities", "messages", "vfx", "storage", "commands"],
)
def _meta():
    pass

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    p.set_max_health(150.0)
    p.set_armor(100, Armor.VESTHELM)

@mod.hook("player_killed", phase="post")
def on_kill(ev):
    k = ev.killer
    v = ev.victim
    if k is None or ev.team_kill or k.slot == v.slot:
        return
    heal = 35.0 if ev.headshot else 15.0
    k.set_max_health(150.0)
    k.set_health(min(k.health + heal, 150.0))
    cnt = int(mod.storage.get("kills", "0")) + 1
    mod.storage.set("kills", str(cnt))
    vfx.shockwave_ring(v.origin, radius=220.0, rgba=(220, 30, 50, 240), life=10, width=16)
    k.dhud(f"VAMPIRE +{int(heal)} HP", x=-1.0, y=0.28, rgb=(220, 30, 50), hold=1.5)
    mod.log(f"vampire_kill count={cnt}")

@mod.chat_command("/vamp")
def cmd_vamp(player, args):
    cnt = int(mod.storage.get("kills", "0"))
    player.chat(f"{Color.GREEN}[Vampire]{Color.DEFAULT} kills={cnt}")
    vfx.shockwave_ring(player.origin, radius=180.0, rgba=(220, 30, 50, 240), life=8, width=12)
    mod.log(f"vamp_cmd slot={player.slot} kills={cnt}")
    return HookAction.SUPERCEDE
"#;

const VIP_MENU_PY: &str = r#"
from cs16 import mod, vfx, Color, HookAction, Armor, Menu

@mod.info(
    id="vip_menu",
    name="Interactive VIP Menu",
    version="1.0.0",
    permissions=["entities", "messages", "menus", "vfx", "weapons", "commands"],
)
def _meta():
    pass

def on_vip_pick(player, info):
    if info == "hp_armor":
        player.set_max_health(150.0)
        player.set_health(150.0)
        player.set_armor(100, Armor.VESTHELM)
        vfx.shockwave_ring(player.origin, radius=160.0, rgba=(80, 240, 120, 240), life=8, width=12)
        mod.log("vip_picked hp_armor")
    elif info == "money_max":
        player.set_money(16000, flash=True)
        mod.log("vip_picked money_max")

@mod.chat_command("/vip", "!vip")
def cmd_vip(player, args):
    m = Menu("\\yVIP Playground Menu\\w", callback=on_vip_pick)
    m.add_item("\\w+150 HP & 100 VestHelm", "hp_armor")
    m.add_item("\\ySet Money to $16,000", "money_max")
    m.show(player)
    return HookAction.SUPERCEDE
"#;

const PARACHUTE_PY: &str = r#"
from cs16 import mod, Button, HookAction

@mod.info(
    id="moon_gravity_parachute",
    name="Moon Gravity & Parachute",
    version="1.0.0",
    permissions=["entities", "messages", "commands"],
)
def _meta():
    pass

@mod.chat_command("/drop")
def cmd_drop(player, args):
    x, y, z = player.origin
    player.set_origin((x, y, z + 180.0))
    player.set_velocity((0.0, 0.0, -320.0))
    return HookAction.SUPERCEDE

@mod.hook("player_pre_think", phase="pre")
def on_pre_think(ev):
    p = ev.player
    if not p.alive or p.on_ground:
        return
    if (ev.buttons & Button.USE) != 0:
        vx, vy, vz = p.velocity
        if vz < -95.0:
            p.set_velocity((vx, vy, -95.0))
            p.set_gravity(0.12)
            p.hud("PARACHUTE ACTIVE", x=-1.0, y=0.78, channel=3, rgba=(130, 235, 255, 255), hold=0.4)
"#;

const DAMAGE_BOOST_PY: &str = r#"
from cs16 import mod, vfx, HookAction

@mod.info(
    id="golden_ak_damager",
    name="Golden AK-47 & Damager",
    version="1.0.0",
    permissions=["entities", "messages", "vfx"],
)
def _meta():
    pass

@mod.hook("player_take_damage", phase="pre")
def on_damage_pre(ev):
    if ev.attacker is not None and not ev.team_attack:
        ev.damage = ev.damage * 1.35
        return HookAction.OVERRIDE
    return HookAction.CONTINUE

@mod.hook("player_take_damage", phase="post")
def on_damage_post(ev):
    att = ev.attacker
    if att is None or ev.team_attack:
        return
    att.hud(f"-{int(round(ev.damage))}", x=-1.0, y=0.55, channel=2, rgba=(255, 215, 60, 255), hold=0.8)
    vfx.laser_beam(att.origin, ev.victim.origin, rgba=(255, 210, 40, 235), life=6, width=8, noise=2)
    mod.log(f"hit_dmg={int(round(ev.damage))}")
"#;

fn start_modded_demo(bots: u32) -> LocalGame {
    let pack = build_instant_demo_pack();
    let vfs = pack.to_vfs();
    let opts = LocalOptions {
        max_clients: 12,
        bots,
        text_menus: true,
        enable_mods: true,
        initial_cvars: vec![("mp_freezetime".to_string(), "0".to_string())],
        ..LocalOptions::default()
    };
    let mut local =
        LocalGame::start(&vfs, DEMO_MAP_NAME, &opts).expect("start modded dm_rusthouse");
    assert!(local.is_modded());
    assert_eq!(local.connection().state(), &ConnState::InGame);
    local.connection_mut().execute_local("jointeam 2");
    local.run_frames(5);
    local.connection_mut().execute_local("joinclass 1");
    local.run_frames(10);
    local
}

#[test]
fn local_game_python_hot_reload_hooks_vfx_menus_resmon_and_syntax_safety() {
    let mut local = start_modded_demo(0);

    // 1. Hot-reload vampire_shockwave and trigger /vamp chat command -> chat + VFX + logs + ResMon.
    let rep = local
        .reload_python_plugin("vampire_shockwave.py", VAMPIRE_PY)
        .expect("load vampire_shockwave");
    assert_eq!(rep.id, "vampire_shockwave");
    assert_eq!(rep.hook_count, 2);
    assert_eq!(rep.chat_command_count, 1);

    let _ = local.connection_mut().fx_mut().tempents.take_effects();
    local.connection_mut().execute_local("say /vamp");
    local.run_frames(6);

    assert!(
        local
            .connection()
            .fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("[Vampire]")),
        "chat command should emit SayText to local HUD"
    );
    let vfx_effects = local.connection_mut().fx_mut().tempents.take_effects();
    assert!(
        !vfx_effects.is_empty(),
        "shockwave_ring VFX should emit RenderEffect onto local client"
    );

    let resmon = local.mod_resmon_json();
    assert!(
        resmon.contains("\"id\":\"vampire_shockwave\""),
        "resmon={resmon}"
    );
    assert!(resmon.contains("\"status\":\"Active\""), "resmon={resmon}");
    assert!(
        local
            .mod_resmon_snapshot()
            .iter()
            .any(|r| r.id == "vampire_shockwave" && r.hook_calls > 0 && r.fuel_used > 0),
        "ResMon should record real hook_calls and fuel_used"
    );

    let logs = local.drain_mod_logs_json();
    assert!(logs.contains("vamp_cmd slot=0"), "logs={logs}");

    // 2. Broken syntax edit returns clean error and keeps previous live plugin + match alive!
    let err = local
        .reload_python_plugin("vampire_shockwave.py", "def broken_fn(\n    return 1\n")
        .expect_err("broken python must fail cleanly");
    assert!(!err.is_empty(), "syntax error message should be non-empty");
    assert_eq!(local.connection().state(), &ConnState::InGame);
    // Advance past GoldSrc's 0.66 s `SAY_COMMAND_INTERVAL` chat anti-flood window.
    local.run_frames(75);
    local.connection_mut().execute_local("say /vamp");
    local.run_frames(6);
    let logs_after_err = local.drain_mod_logs_json();
    assert!(
        logs_after_err.contains("vamp_cmd slot=0"),
        "previous live plugin must continue running after rejected syntax error: {logs_after_err}"
    );

    // 3. Hot-reload VIP menu plugin -> say /vip opens ShowMenu -> menuselect 1 grants 150 HP + 100 armor!
    let vip_rep = local
        .reload_python_plugin("vip_menu.py", VIP_MENU_PY)
        .expect("load vip_menu");
    assert_eq!(vip_rep.id, "vip_menu");

    local.run_frames(75);
    local.connection_mut().execute_local("say /vip");
    local.run_frames(6);
    let menu_text = local
        .connection()
        .fx()
        .hud
        .menu
        .as_ref()
        .map(|m| m.text.clone())
        .unwrap_or_default();
    assert!(
        menu_text.contains("VIP Playground Menu"),
        "expected GoldSrc VIP menu, got: {menu_text:?}"
    );

    local.connection_mut().execute_local("menuselect 1");
    local.run_frames(6);
    let hp_after = local.cs_game().state().players[0]
        .as_ref()
        .expect("local player")
        .health;
    let armor_after = local.cs_game().state().players[0]
        .as_ref()
        .expect("local player")
        .armor;
    assert!(
        (hp_after - 150.0).abs() < 0.5,
        "VIP option 1 should grant 150 HP, got {hp_after}"
    );
    assert!(
        (armor_after - 100.0).abs() < 0.5,
        "VIP option 1 should grant 100 armor, got {armor_after}"
    );

    // 4. Select VIP option 2 -> $16,000 money!
    local.run_frames(75);
    local.connection_mut().execute_local("say !vip");
    local.run_frames(6);
    local.connection_mut().execute_local("menuselect 2");
    local.run_frames(6);
    let money_after = local.cs_game().state().players[0]
        .as_ref()
        .expect("local player")
        .money;
    assert_eq!(
        money_after, 16000,
        "VIP option 2 should set money to $16000"
    );

    // 5. Unload plugins cleanly.
    assert!(local.unload_python_plugin("vampire_shockwave.py"));
    assert!(local.unload_python_plugin("vip_menu"));
    assert!(local.mod_resmon_snapshot().is_empty());
}

#[test]
fn local_game_python_parachute_pre_think_and_combat_damage_hooks() {
    let mut local = start_modded_demo(0);
    local
        .reload_python_plugin("moon_gravity_parachute.py", PARACHUTE_PY)
        .expect("load parachute");

    // Lift local player into the air with fast downward velocity via `/drop` and hold IN_USE (1 << 5).
    local.connection_mut().execute_local("say /drop");
    local.run_frames(3);
    let cmd = GoldSrcUserCmd {
        buttons: 1 << 5, // IN_USE
        msec: 10,
        ..GoldSrcUserCmd::default()
    };
    for _ in 0..8 {
        local.connection_mut().queue_cmd(cmd.clone());
        local.step();
    }

    let (vz, gravity) = {
        let sv = local.modded_server().expect("modded server");
        let eid = sv.edicts().client(0);
        let ent = sv.edicts().get(eid).expect("player edict");
        (ent.vars.velocity.z, ent.vars.gravity)
    };
    assert!(
        vz >= -140.0,
        "parachute +use pre_think should arrest downward fall velocity, got vz={vz}"
    );
    assert!(
        (gravity - 0.12).abs() < 0.05,
        "parachute +use should set low gravity (0.12), got {gravity}"
    );
    let effects = local.connection_mut().fx_mut().tempents.take_effects();
    assert!(
        effects.iter().any(|e| matches!(
            e,
            cs_clientcore::tempents::RenderEffect::TextMessage(m) if m.text.contains("PARACHUTE")
        )),
        "parachute hook should emit TE_TEXTMESSAGE HUD effect, got: {effects:?}"
    );

    // Now test live combat hooks (`player_take_damage` + `player_killed`) with bots on dm_rusthouse!
    let mut combat = start_modded_demo(9);
    combat
        .reload_python_plugin("golden_ak_damager.py", DAMAGE_BOOST_PY)
        .expect("load damager");
    combat
        .reload_python_plugin("vampire_shockwave.py", VAMPIRE_PY)
        .expect("load vampire");

    for _ in 0..700 {
        combat.step();
        if combat.modded_server().unwrap().fake_client_count() >= 9 {
            break;
        }
    }
    let _ = combat.console("sv_restart 1");
    combat.run_frames(130);

    // Note: `load_package` records 1 call for `call_init`, so wait until live ZBot combat fires
    // BOTH `player_take_damage` Pre and Post hooks (`hook_calls >= 3`).
    let mut saw_damage_hook = false;
    for _ in 0..900 {
        combat.step();
        let snap = combat.mod_resmon_snapshot();
        if snap
            .iter()
            .any(|r| r.id == "golden_ak_damager" && r.hook_calls >= 3)
        {
            saw_damage_hook = true;
            break;
        }
    }
    assert!(
        saw_damage_hook,
        "ZBot combat on dm_rusthouse should fire Python player_take_damage hooks and update ResMon"
    );
    let logs = combat.drain_mod_logs_json();
    assert!(
        logs.contains("hit_dmg="),
        "expected golden_ak_damager mod.log output from live combat, got: {logs}"
    );
}
