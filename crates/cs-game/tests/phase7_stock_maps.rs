//! Step 4 Phase 7 integration tests: verify all 25 stock CS 1.6 `.bsp` maps recognize every
//! map entity classname, boot within edict limits, spawn CT & T players, step 50 frames, restart
//! rounds cleanly, and run targeted entity interactions across representative retail maps.

mod common;

use common::{Bench, discover, map_named};
use cs_assets::bsp::BspMap;
use cs_engine::consts::{EF_NODRAW, SOLID_NOT};
use cs_engine::{DEFAULT_MAX_EDICTS, GameDll, ServerConfig};
use cs_game::create_entity;
use cs_game::player::Team;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;

const STOCK_CS16_MAPS: [&str; 25] = [
    "as_oilrig",
    "cs_747",
    "cs_assault",
    "cs_backalley",
    "cs_estate",
    "cs_havana",
    "cs_italy",
    "cs_militia",
    "cs_office",
    "cs_siege",
    "de_airstrip",
    "de_aztec",
    "de_cbble",
    "de_chateau",
    "de_dust",
    "de_dust2",
    "de_inferno",
    "de_nuke",
    "de_piranesi",
    "de_prodigy",
    "de_storm",
    "de_survivor",
    "de_torn",
    "de_train",
    "de_vertigo",
];

fn stock_bench(map: cs_engine::ServerMap) -> Bench {
    let cfg = ServerConfig {
        max_clients: 2,
        max_edicts: DEFAULT_MAX_EDICTS,
        ..ServerConfig::default()
    };
    let mut b = Bench::with_config(map, cfg, &[], true);
    b.idle_cmds = true;
    b
}

#[test]
fn all_25_stock_maps_load_spawn_tick_and_restart_cleanly() {
    let Some(vfs) = discover() else {
        return;
    };

    for name in STOCK_CS16_MAPS {
        assert!(
            vfs.exists(&format!("maps/{name}.bsp")),
            "expected stock CS 1.6 map {name}.bsp in installed CS16_GAMEDIR"
        );
        let bsp = BspMap::load(&vfs, name).unwrap_or_else(|e| panic!("load {name}: {e:?}"));
        for ent in &bsp.entities {
            let class = ent.classname();
            assert!(
                create_entity(class).is_some(),
                "{name}: unrecognized map entity classname `{class}`"
            );
        }

        let mut b = stock_bench(map_named(&vfs, name));
        b.step(4);
        let edict_count = b.run(move |game, eng| {
            eng.set_cvar("mp_freezetime", "0");
            let _ = game.client_connect(eng, 1);
            game.client_put_in_server(eng, 1);
            eng.test_set_client_active(1, true);
            assert!(game.spawn_player(eng, 0, Team::Ct, 0), "{name}: spawn CT");
            assert!(game.spawn_player(eng, 1, Team::Terrorist, 0), "{name}: spawn T");
            eng.entity_ids().len()
        });
        assert!(
            edict_count < DEFAULT_MAX_EDICTS,
            "{name}: active edict count {edict_count} exceeded {DEFAULT_MAX_EDICTS}"
        );

        b.step(50);
        b.run(move |game, eng| {
            game.restart_round_now(eng);
            assert!(game.spawn_player(eng, 0, Team::Ct, 0), "{name}: respawn CT");
            assert!(game.spawn_player(eng, 1, Team::Terrorist, 0), "{name}: respawn T");
        });
        b.step(20);
        let after_restart = b.run(|_, eng| eng.entity_ids().len());
        assert!(
            after_restart < DEFAULT_MAX_EDICTS,
            "{name}: post-restart edict count {after_restart} exceeded {DEFAULT_MAX_EDICTS}"
        );
    }
}

#[test]
fn stock_map_spot_checks() {
    let Some(vfs) = discover() else {
        return;
    };

    // 1. de_aztec: env_rain delivers ReceiveW(1) to joining player.
    {
        let mut b = stock_bench(map_named(&vfs, "de_aztec"));
        b.step(4);
        b.run(|game, eng| assert!(game.spawn_player(eng, 0, Team::Ct, 0)));
        b.step(2);
        let rain = b.probe.messages().iter().any(|m| {
            matches!(
                m,
                SvcMessage::UserMsg {
                    decoded: CsUserMsg::ReceiveW(1),
                    ..
                }
            )
        });
        assert!(rain, "de_aztec should send ReceiveW(1) for env_rain");
    }

    // 2. de_survivor: func_tracktrain + path_track + func_breakable + env_explosion + func_button.
    {
        let mut b = stock_bench(map_named(&vfs, "de_survivor"));
        b.step(4);
        b.run(|game, eng| {
            assert!(eng.find_by_classname(None, "func_tracktrain").is_some());
            assert!(eng.find_by_classname(None, "path_track").is_some());
            assert!(eng.find_by_classname(None, "func_breakable").is_some());
            assert!(eng.find_by_classname(None, "env_explosion").is_some());
            let btn = eng.find_by_classname(None, "func_button").unwrap();
            game.test_use_entity(eng, btn);
        });
        b.step(20);
    }

    // 3. cs_office: func_breakable breaks on damage and restores cleanly on round restart.
    {
        let mut b = stock_bench(map_named(&vfs, "cs_office"));
        b.step(4);
        let brk = b.run(|game, eng| {
            assert!(eng.find_by_classname(None, "hostage_entity").is_some());
            let brk = eng.find_by_classname(None, "func_breakable").unwrap();
            assert!(game.spawn_player(eng, 0, Team::Ct, 0));
            let p0 = eng.client_edict(0);
            game.test_damage_entity(eng, brk, Some(p0), 1000.0, 2);
            assert_eq!(eng.ent(brk).unwrap().solid, SOLID_NOT);
            assert_ne!(eng.ent(brk).unwrap().effects & EF_NODRAW, 0);
            brk
        });
        b.run(move |game, eng| {
            game.restart_round_now(eng);
            assert_ne!(eng.ent(brk).unwrap().solid, SOLID_NOT, "cs_office breakable restored");
            assert_eq!(eng.ent(brk).unwrap().effects & EF_NODRAW, 0);
        });
    }

    // 4. cs_assault: trigger_camera, func_button, func_door, func_door_rotating, func_breakable, multisource.
    {
        let mut b = stock_bench(map_named(&vfs, "cs_assault"));
        b.step(4);
        b.run(|game, eng| {
            assert!(game.spawn_player(eng, 0, Team::Ct, 0));
            let p0 = eng.client_edict(0);
            let cam = eng.find_by_classname(None, "trigger_camera").unwrap();
            let btn = eng.find_by_classname(None, "func_button").unwrap();
            assert!(eng.find_by_classname(None, "func_door").is_some());
            assert!(eng.find_by_classname(None, "func_door_rotating").is_some());
            assert!(eng.find_by_classname(None, "multisource").is_some());
            game.test_use_entity_by(eng, cam, p0);
            game.test_use_entity_by(eng, btn, p0);
        });
        b.step(20);
        b.run(|game, eng| game.restart_round_now(eng));
    }

    // 5. cs_siege: func_door, func_door_rotating, func_button, env_sound, env_spark, hostage_entity.
    {
        let mut b = stock_bench(map_named(&vfs, "cs_siege"));
        b.step(4);
        b.run(|game, eng| {
            assert!(game.spawn_player(eng, 0, Team::Ct, 0));
            let p0 = eng.client_edict(0);
            assert!(eng.find_by_classname(None, "func_door").is_some());
            assert!(eng.find_by_classname(None, "func_door_rotating").is_some());
            assert!(eng.find_by_classname(None, "env_sound").is_some());
            assert!(eng.find_by_classname(None, "hostage_entity").is_some());
            let spk = eng.find_by_classname(None, "env_spark").unwrap();
            let btn = eng.find_by_classname(None, "func_button").unwrap();
            game.test_use_entity(eng, spk);
            game.test_use_entity_by(eng, btn, p0);
        });
        b.step(20);
    }

    // 6. as_oilrig: info_vip_start, func_vip_safetyzone, func_door, func_button, trigger_hurt, func_water.
    {
        let mut b = stock_bench(map_named(&vfs, "as_oilrig"));
        b.step(4);
        b.run(|game, eng| {
            assert!(eng.find_by_classname(None, "info_vip_start").is_some());
            assert!(eng.find_by_classname(None, "func_vip_safetyzone").is_some());
            assert!(eng.find_by_classname(None, "trigger_hurt").is_some());
            assert!(eng.find_by_classname(None, "func_water").is_some());
            let _ = game.client_connect(eng, 1);
            game.client_put_in_server(eng, 1);
            eng.test_set_client_active(1, true);
            assert!(game.spawn_player(eng, 0, Team::Ct, 0));
            assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
            game.restart_round_now(eng);
            let door = eng.find_by_classname(None, "func_door").unwrap();
            let btn = eng.find_by_classname(None, "func_button").unwrap();
            game.test_use_entity(eng, door);
            game.test_use_entity(eng, btn);
        });
        b.step(20);
    }
}
