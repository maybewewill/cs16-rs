//! Tactical shield (`weapon_shield` + pistol/knife/grenade shield variants), `autobuy` /
//! `rebuy` (`cl_setautobuy` / `cl_setrebuy`), and `game_player_equip` integration tests.

mod common;

use common::{Bench, command, entity, run_cmd, stub_spawns};
use cs_assets::bsp::Entity;
use cs_engine::{GameDll, ServerMap};
use cs_game::buy::SHIELDGUN_PRICE;
use cs_game::player::{
    HIDEHUD_CROSSHAIR, HITGROUP_SHIELD_DISABLED, HITGROUP_SHIELD_ENABLED, Team, dmg,
};
use cs_game::weapons::{WPNSTATE_SHIELD_DRAWN, WeaponId};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use glam::Vec3;

const IN_ATTACK2: u16 = 1 << 11;

fn zone_map() -> Vec<Entity> {
    let mut map = stub_spawns();
    map.push(entity(&[
        ("classname", "func_buyzone"),
        ("origin", "200 0 0"),
        ("team", "2"),
    ]));
    map.push(entity(&[
        ("classname", "func_buyzone"),
        ("origin", "-200 0 0"),
        ("team", "1"),
    ]));
    map
}

fn setup_bench(entities: Vec<Entity>, slots: u8) -> Bench {
    let mut b = Bench::with_userinfo(ServerMap::stub_with_entities("test", entities), slots, &[]);
    b.idle_cmds = true;
    b.step(6);
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_buytime", "99");
        eng.set_cvar("mp_freezetime", "0");
        game.restart_round_now(eng);
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "func_buyzone") {
            after = Some(id);
            let origin = eng.ent(id).unwrap().origin;
            eng.set_size(id, Vec3::splat(-200.0), Vec3::splat(200.0));
            eng.set_origin(id, origin);
        }
    });
    for s in 1..slots {
        b.run(move |game, eng| {
            let _ = game.client_connect(eng, s);
            game.client_put_in_server(eng, s);
            eng.test_set_client_active(s, true);
        });
    }
    b.probe.clear_messages();
    b
}

fn settle_zone(b: &mut Bench) {
    b.step(60);
    b.probe.clear_messages();
}

fn temp_entities(b: &Bench) -> Vec<u8> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::TempEntity { te_type, .. } => Some(*te_type),
            _ => None,
        })
        .collect()
}

fn send_attack2(b: &mut Bench, slot: u8) {
    let cmd = GoldSrcUserCmd {
        buttons: IN_ATTACK2,
        msec: 10,
        ..Default::default()
    };
    b.run(move |game, eng| {
        eng.ent_mut(eng.client_edict(slot)).unwrap().button = i32::from(IN_ATTACK2);
        run_cmd(game, eng, slot, &cmd, 1);
    });
    let release = GoldSrcUserCmd {
        msec: 10,
        ..Default::default()
    };
    b.run(move |game, eng| {
        eng.ent_mut(eng.client_edict(slot)).unwrap().button = 0;
        run_cmd(game, eng, slot, &release, 2);
    });
}

fn command_argv(b: &mut Bench, slot: u8, args: &[&str]) {
    let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    b.run(move |game, eng| {
        game.client_command(eng, slot, &argv);
    });
}

#[test]
fn shield_buy_draw_speed_hitbox_deflection_drop_and_pickup() {
    let mut b = setup_bench(zone_map(), 3);
    // Slot 0 = CT buyer, Slot 1 = T shooter, Slot 2 = CT teammate for pickup.
    b.run(|game, eng| {
        for s in 0..3 {
            game.test_set_not_killed(s, false);
        }
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        assert!(game.spawn_player(eng, 2, Team::Ct, 1));
        game.place_player(eng, 0, Vec3::new(200.0, 0.0, 0.0));
        game.place_player(eng, 1, Vec3::new(100.0, 0.0, 0.0));
        game.place_player(eng, 2, Vec3::new(200.0, 250.0, 0.0));
        game.state_mut().players[0].as_mut().unwrap().money = 10000;
    });
    settle_zone(&mut b);

    // Give primary first so buying the shield drops it and grants the tactical shield for $2200.
    command(&mut b, 0, "m4a1");
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(cs_game::inventory::has_player_item(p, WeaponId::M4a1));
    });
    command(&mut b, 0, "shield");
    b.run(|game, eng| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.owns_shield);
        assert!(!cs_game::inventory::has_player_item(p, WeaponId::M4a1));
        assert_eq!(
            p.money,
            10000 - 3100 - SHIELDGUN_PRICE,
            "money mismatch after m4a1 + shield"
        );
        let id = eng.client_edict(0);
        assert_eq!(eng.ent(id).unwrap().gamestate, HITGROUP_SHIELD_ENABLED);
    });
    // Giving/picking up a primary or Elite while holding a shield is blocked (`CanHavePlayerItem`).
    b.run(|game, eng| {
        assert!(!game.test_give_named_item(eng, 0, "weapon_famas"));
        assert!(!game.test_give_named_item(eng, 0, "weapon_elite"));
        let p = game.state().players[0].as_ref().unwrap();
        assert!(!cs_game::inventory::has_player_item(p, WeaponId::Famas));
        assert!(!cs_game::inventory::has_player_item(p, WeaponId::Elite));
        assert!(p.owns_shield);
    });

    // Advance past deploy delay (0.8s) so IN_ATTACK2 toggles shield drawn.
    b.step(85);
    let max_speed_open = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().max_speed);
    assert!(
        (max_speed_open - 250.0).abs() < 1.0,
        "expected pistol speed 250 before drawing shield, got {max_speed_open}"
    );
    send_attack2(&mut b, 0);
    b.step(5);
    b.run(|game, eng| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.shield_drawn, "right-click should raise tactical shield");
        assert_ne!(
            p.test_active_weapon_state().unwrap_or(0) & WPNSTATE_SHIELD_DRAWN,
            0
        );
        let v = eng.ent(eng.client_edict(0)).unwrap();
        assert!(
            (v.max_speed - 180.0).abs() < 1.0,
            "drawn shield speed should be 180, got {}",
            v.max_speed
        );
        assert_ne!(p.test_hide_hud() & HIDEHUD_CROSSHAIR, 0);
    });

    // Frontal shot against shield-holding CT is deflected (0 damage + TE_SPARKS).
    b.probe.clear_messages();
    b.run(|game, eng| {
        let v0 = eng.client_edict(0);
        let ent0 = eng.ent_mut(v0).unwrap();
        ent0.angles = Vec3::new(0.0, 180.0, 0.0);
        ent0.v_angle = Vec3::new(0.0, 180.0, 0.0);
        game.test_trace_attack(eng, 0, 1, Some(WeaponId::Ak47), 36.0, 2);
    });
    b.step(6);
    assert!(temp_entities(&b).contains(&cs_proto48::tempent::te::SPARKS));
    let hp_after_front = b.run(|game, _| game.state().players[0].as_ref().unwrap().health);
    assert_eq!(hp_after_front, 100.0);

    // Shot from behind (facing +X, shot travels +X into back) deals normal damage.
    b.run(|game, eng| {
        let v0 = eng.client_edict(0);
        let ent0 = eng.ent_mut(v0).unwrap();
        ent0.angles = Vec3::new(0.0, 0.0, 0.0);
        ent0.v_angle = Vec3::new(0.0, 0.0, 0.0);
        let p0 = game.state_mut().players[0].as_mut().unwrap();
        p0.armor = 0.0;
        game.test_trace_attack(eng, 0, 1, Some(WeaponId::Ak47), 36.0, 2);
    });
    let hp_after_back = b.run(|game, _| game.state().players[0].as_ref().unwrap().health);
    assert!(hp_after_back < 100.0, "back shot should deal damage");

    // Undraw shield, switch to knife, and drop -> spawns weapon_shield world entity; teammate CT picks it up.
    b.step(50);
    send_attack2(&mut b, 0);
    b.step(50);
    command(&mut b, 0, "weapon_knife");
    command(&mut b, 0, "drop");
    b.step(5);
    let shield_ent = b.run(|game, eng| {
        let p0 = game.state().players[0].as_ref().unwrap();
        assert!(!p0.owns_shield);
        assert_eq!(
            eng.ent(eng.client_edict(0)).unwrap().gamestate,
            HITGROUP_SHIELD_DISABLED
        );
        eng.find_by_classname(None, "weapon_shield")
            .expect("drop should spawn world weapon_shield")
    });
    // Teammate (slot 2) touches dropped shield -> equips it and entity disappears.
    b.run(move |game, eng| {
        game.touch(eng, shield_ent, eng.client_edict(2));
    });
    b.step(1);
    b.run(|game, eng| {
        assert!(game.state().players[2].as_ref().unwrap().owns_shield);
        assert!(
            eng.find_by_classname(None, "weapon_shield").is_none(),
            "picked up world shield should be removed"
        );
    });
    let _ = dmg::BULLET;
}

#[test]
fn shield_variants_work_for_pistols_knife_and_grenades() {
    let mut b = setup_bench(zone_map(), 2);
    b.run(|game, eng| {
        game.test_set_not_killed(0, false);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.place_player(eng, 0, Vec3::new(200.0, 0.0, 0.0));
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    settle_zone(&mut b);

    command(&mut b, 0, "shield");
    command(&mut b, 0, "hegren");
    command(&mut b, 0, "flash");
    command(&mut b, 0, "sgren");

    for (buy_cmd, select_cmd, expected_vm) in [
        ("usp", "weapon_usp", "models/shield/v_shield_usp.mdl"),
        ("deagle", "weapon_deagle", "models/shield/v_shield_deagle.mdl"),
        ("glock", "weapon_glock18", "models/shield/v_shield_glock18.mdl"),
        ("p228", "weapon_p228", "models/shield/v_shield_p228.mdl"),
        ("fiveseven", "weapon_fiveseven", "models/shield/v_shield_fiveseven.mdl"),
    ] {
        command(&mut b, 0, buy_cmd);
        command(&mut b, 0, select_cmd);
        b.step(85);
        b.run(move |game, eng| {
            let p = game.state().players[0].as_ref().unwrap();
            assert!(p.owns_shield);
            let vm = eng.ent(eng.client_edict(0)).unwrap().viewmodel;
            assert_eq!(
                Some(vm),
                eng.model_index(expected_vm),
                "expected shield viewmodel {expected_vm} for {select_cmd}"
            );
        });
        send_attack2(&mut b, 0);
        b.step(5);
        b.run(move |game, _| {
            let p = game.state().players[0].as_ref().unwrap();
            assert_ne!(
                p.test_active_weapon_state().unwrap_or(0) & WPNSTATE_SHIELD_DRAWN,
                0,
                "{select_cmd} should draw shield"
            );
        });
        b.step(50);
        send_attack2(&mut b, 0);
        b.step(50);
    }

    for (select_cmd, expected_vm) in [
        ("weapon_knife", "models/shield/v_shield_knife.mdl"),
        ("weapon_hegrenade", "models/shield/v_shield_hegrenade.mdl"),
        ("weapon_flashbang", "models/shield/v_shield_flashbang.mdl"),
        ("weapon_smokegrenade", "models/shield/v_shield_smokegrenade.mdl"),
    ] {
        command(&mut b, 0, select_cmd);
        b.step(85);
        b.run(move |_game, eng| {
            let vm = eng.ent(eng.client_edict(0)).unwrap().viewmodel;
            assert_eq!(
                Some(vm),
                eng.model_index(expected_vm),
                "expected shield viewmodel {expected_vm} for {select_cmd}"
            );
        });
        send_attack2(&mut b, 0);
        b.step(5);
        b.run(move |game, _| {
            let p = game.state().players[0].as_ref().unwrap();
            assert!(p.shield_drawn, "{select_cmd} should allow raising shield");
        });
        b.step(50);
        send_attack2(&mut b, 0);
        b.step(50);
    }
}

#[test]
fn autobuy_and_rebuy_equip_team_loadouts_and_respect_custom_lists() {
    let mut b = setup_bench(zone_map(), 2);
    // 1. Default CT autobuy equips M4A1 + vesthelm + ammo.
    b.run(|game, eng| {
        game.test_set_not_killed(0, false);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.place_player(eng, 0, Vec3::new(200.0, 0.0, 0.0));
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    settle_zone(&mut b);
    command(&mut b, 0, "autobuy");
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(cs_game::inventory::has_player_item(p, WeaponId::M4a1));
        assert_eq!(p.armor, 100.0);
        assert!(p.money < 16000);
    });

    // 2. Hostile oversize cl_setautobuy and custom CT list (`famas deagle hegren`).
    let mut spam_args = vec!["cl_setautobuy".to_string()];
    spam_args.extend((0..64).map(|i| format!("badtoken_{i}_{}", "x".repeat(40))));
    b.run(move |game, eng| {
        game.client_command(eng, 0, &spam_args);
    });
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.test_autobuy_list().len() <= 32);
    });

    command_argv(&mut b, 0, &["cl_setautobuy", "famas deagle hegren vest"]);
    b.run(|game, eng| {
        game.test_set_not_killed(0, false);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.place_player(eng, 0, Vec3::new(200.0, 0.0, 0.0));
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    settle_zone(&mut b);
    command(&mut b, 0, "autobuy");
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(cs_game::inventory::has_player_item(p, WeaponId::Famas));
        assert!(cs_game::inventory::has_player_item(p, WeaponId::Deagle));
        assert!(cs_game::inventory::has_player_item(p, WeaponId::HeGrenade));
    });

    // 3. Rebuy after respawning restores previous round loadout via custom cl_setrebuy.
    command_argv(
        &mut b,
        0,
        &[
            "cl_setrebuy",
            "SecondaryWeapon",
            "PrimaryWeapon",
            "Armor",
            "HEGrenade",
        ],
    );
    b.run(|game, eng| {
        game.test_set_not_killed(0, false);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.place_player(eng, 0, Vec3::new(200.0, 0.0, 0.0));
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    settle_zone(&mut b);
    command(&mut b, 0, "rebuy");
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(cs_game::inventory::has_player_item(p, WeaponId::Famas));
        assert!(cs_game::inventory::has_player_item(p, WeaponId::Deagle));
        assert!(cs_game::inventory::has_player_item(p, WeaponId::HeGrenade));
        assert_eq!(p.armor, 100.0);
    });

    // 4. T autobuy skips CT-only M4A1 and buys AK47 cleanly.
    command_argv(&mut b, 0, &["cl_setautobuy", "m4a1", "ak47", "vesthelm"]);
    b.run(|game, eng| {
        game.test_set_not_killed(0, false);
        assert!(game.spawn_player(eng, 0, Team::Terrorist, 0));
        game.place_player(eng, 0, Vec3::new(-200.0, 0.0, 0.0));
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    settle_zone(&mut b);
    command(&mut b, 0, "autobuy");
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(cs_game::inventory::has_player_item(p, WeaponId::Ak47));
        assert_eq!(p.armor, 100.0);
    });
}

#[test]
fn game_player_equip_gives_configured_weapons_on_spawn_and_use() {
    let mut map = stub_spawns();
    // Auto-equip on spawn (spawnflags 0): AK47 + assault suit.
    map.push(entity(&[
        ("classname", "game_player_equip"),
        ("spawnflags", "0"),
        ("weapon_ak47", "1"),
        ("item_assaultsuit", "1"),
    ]));
    // Use-only equip (spawnflags 1): Deagle + shield + defuser.
    map.push(entity(&[
        ("classname", "game_player_equip"),
        ("spawnflags", "1"),
        ("targetname", "vip_kit"),
        ("weapon_deagle", "1"),
        ("weapon_shield", "1"),
        ("item_thighpack", "1"),
    ]));

    let mut b = setup_bench(map, 2);
    b.run(|game, eng| {
        game.test_set_not_killed(0, false);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
    });
    b.step(2);
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(
            cs_game::inventory::has_player_item(p, WeaponId::Ak47),
            "spawn-active game_player_equip should grant weapon_ak47"
        );
        assert_eq!(p.armor, 100.0);
        assert!(!p.owns_shield);
        assert!(!p.has_defuser);
    });

    // Triggering the SF_PLAYEREQUIP_USEONLY entity grants its kit to activator.
    b.run(|game, eng| {
        let kit = eng
            .find_by_classname(None, "game_player_equip")
            .and_then(|first| eng.find_by_classname(Some(first), "game_player_equip"))
            .expect("second game_player_equip");
        let player = eng.client_edict(0);
        game.test_use_entity_by(eng, kit, player);
    });
    b.step(1);
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(cs_game::inventory::has_player_item(p, WeaponId::Deagle));
        assert!(p.owns_shield);
        assert!(p.has_defuser);
    });
}
