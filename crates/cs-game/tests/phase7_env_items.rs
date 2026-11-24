//! Step 3 Phase 7 integration tests: environment effects (`env_*`, `beam`, `gibshooter`,
//! `cycler*`, `bodyque`), lights (`light`, `light_spot`, `light_environment`), sounds
//! (`ambient_generic`, `env_sound`, `speaker`, `soundent`), `spark_shower` dedicated behavior,
//! all 10 `ammo_*` pickups, `item_kevlar`, `item_assaultsuit`, `item_generic`, and
//! `game_player_equip`.

mod common;

use common::{Bench, entity, stub_spawns};
use cs_engine::consts::{EF_NODRAW, SOLID_NOT};
use cs_engine::GameDll;
use cs_game::entity::UseType;
use cs_game::grenade::create_spark_shower;
use cs_game::player::{ArmorType, Team};
use cs_game::targets::fire_targets;
use cs_game::weapons::{WeaponId, ammo_index, new_weapon};
use cs_proto48::messages::SvcMessage;
use cs_proto48::tempent::te;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

fn setup_bench(extra_entities: Vec<cs_assets::bsp::Entity>, slots: u8, clear_probe: bool) -> Bench {
    let mut map = stub_spawns();
    map.extend(extra_entities);
    let mut b = Bench::with_userinfo(
        cs_engine::ServerMap::stub_with_entities("test", map),
        slots,
        &[],
    );
    b.idle_cmds = true;
    b.step(4);
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_freezetime", "0");
        for s in 1..slots {
            let _ = game.client_connect(eng, s);
            game.client_put_in_server(eng, s);
            eng.test_set_client_active(s, true);
        }
        game.restart_round_now(eng);
        for s in 0..slots {
            let team = if s % 2 == 0 { Team::Ct } else { Team::Terrorist };
            assert!(game.spawn_player(eng, s, team, 0));
        }
    });
    b.step(2);
    if clear_probe {
        b.probe.clear_messages();
    }
    b
}

#[test]
fn env_fog_weather_lights_and_sound_entities() {
    let extra = vec![
        entity(&[
            ("classname", "env_fog"),
            ("targetname", "fog_ctrl"),
            ("rendercolor", "100 120 140"),
            ("density", "0.0025"),
        ]),
        entity(&[("classname", "env_rain")]),
        entity(&[("classname", "func_snow")]),
        // Unnamed static light despawns cleanly; named switchable light toggles lightstyle.
        entity(&[("classname", "light"), ("origin", "0 0 64")]),
        entity(&[
            ("classname", "light_spot"),
            ("targetname", "sw_light"),
            ("style", "32"),
            ("pattern", "mmmaaa"),
            ("spawnflags", "1"), // START_OFF
        ]),
        entity(&[("classname", "light_environment")]),
        entity(&[
            ("classname", "ambient_generic"),
            ("targetname", "amb_loop"),
            ("message", "ambience/wind1.wav"),
            ("health", "8"),
            ("spawnflags", "16"), // START_SILENT
        ]),
        entity(&[
            ("classname", "env_sound"),
            ("origin", "0 0 0"),
            ("radius", "256"),
            ("roomtype", "14"),
        ]),
        entity(&[
            ("classname", "speaker"),
            ("targetname", "pa_sys"),
            ("message", "vox/hello.wav"),
            ("spawnflags", "1"), // START_SILENT
        ]),
        entity(&[("classname", "soundent")]),
    ];

    let mut b = setup_bench(extra, 1, false);

    // Unnamed static light was freed; Fog & ReceiveW were delivered at join.
    let mut fog_seen = false;
    let mut weather_seen = None;
    for msg in b.probe.messages() {
        if let SvcMessage::UserMsg {
            name,
            decoded: CsUserMsg::Unknown { payload, .. },
            ..
        } = msg
        {
            if name == "Fog" && payload.len() == 7 {
                fog_seen = payload[0..3] == [100, 120, 140];
            } else if name == "ReceiveW" && payload.len() == 1 {
                weather_seen = Some(payload[0]);
            }
        }
    }
    assert!(fog_seen, "expected Fog usermsg with RGB 100 120 140");
    assert_eq!(weather_seen, Some(1), "env_rain takes precedence as weather=1");

    b.probe.clear_messages();
    b.run(|game, eng| {
        assert!(eng.find_by_classname(None, "light").is_none(), "unnamed light despawned");
        assert!(eng.find_by_classname(None, "light_spot").is_some());
        assert!(
            eng.find_by_classname(None, "light_environment").is_none(),
            "light_environment despawns like HLDS CEnvLight"
        );
        assert!(eng.find_by_classname(None, "soundent").is_some());

        let p0 = eng.client_edict(0);
        eng.set_origin(p0, Vec3::ZERO);

        fire_targets(game.state_mut(), eng, "sw_light", Some(p0), Some(p0), UseType::On, 0.0);
        fire_targets(game.state_mut(), eng, "amb_loop", Some(p0), Some(p0), UseType::On, 0.0);
        fire_targets(game.state_mut(), eng, "pa_sys", Some(p0), Some(p0), UseType::On, 0.0);
        fire_targets(game.state_mut(), eng, "fog_ctrl", Some(p0), Some(p0), UseType::Off, 0.0);
    });
    b.step(35);

    let got_lightstyle = b
        .probe
        .messages()
        .iter()
        .any(|m| matches!(m, SvcMessage::LightStyle { index: 32, pattern } if pattern == "mmmaaa"));
    let got_fog_off = b.probe.messages().iter().any(|m| {
        matches!(
            m,
            SvcMessage::UserMsg {
                name,
                decoded: CsUserMsg::Unknown { payload, .. },
                ..
            } if name == "Fog" && payload.len() == 7 && payload[3..7] == [0, 0, 0, 0]
        )
    });
    let got_ambient_or_pa = b
        .probe
        .messages()
        .iter()
        .any(|m| matches!(m, SvcMessage::Sound { .. }));
    assert!(got_lightstyle, "light_spot should update svc_lightstyle");
    assert!(got_fog_off, "env_fog Off should send 0 density Fog");
    assert!(got_ambient_or_pa, "ambient_generic/speaker should emit sound");

    let room = b.run(|_, eng| eng.cvar_f32("room_type"));
    assert_eq!(room, 14.0, "env_sound should set room_type near player");
}

#[test]
fn env_visual_beam_shake_fade_explosion_render_message_and_spark_shower() {
    let extra = vec![
        entity(&[
            ("classname", "env_sprite"),
            ("targetname", "spr"),
            ("model", "sprites/smoke.spr"),
            ("framerate", "10"),
        ]),
        entity(&[("classname", "env_glow"), ("targetname", "glow_tgt")]),
        entity(&[("classname", "env_bombglow")]),
        entity(&[("classname", "cycler"), ("targetname", "cyc_m"), ("model", "models/player.mdl")]),
        entity(&[("classname", "cycler_sprite"), ("model", "sprites/smoke.spr")]),
        entity(&[("classname", "bodyque")]),
        entity(&[("classname", "info_target"), ("targetname", "b_start"), ("origin", "0 0 0")]),
        entity(&[("classname", "info_target"), ("targetname", "b_end"), ("origin", "100 0 0")]),
        entity(&[
            ("classname", "beam"),
            ("targetname", "beam1"),
            ("LightningStart", "b_start"),
            ("LightningEnd", "b_end"),
        ]),
        entity(&[
            ("classname", "env_beam"),
            ("targetname", "beam2"),
            ("LightningStart", "b_start"),
            ("LightningEnd", "b_end"),
            ("spawnflags", "18"), // TOGGLE + SPARKSTART
        ]),
        entity(&[("classname", "env_lightning"), ("targetname", "lt1")]),
        entity(&[("classname", "env_laser"), ("targetname", "lsr1")]),
        entity(&[("classname", "env_spark"), ("targetname", "spk1")]),
        entity(&[
            ("classname", "env_shake"),
            ("targetname", "shk1"),
            ("amplitude", "6"),
            ("duration", "1"),
            ("frequency", "3"),
            ("spawnflags", "1"), // EVERYONE
        ]),
        entity(&[
            ("classname", "env_fade"),
            ("targetname", "fad1"),
            ("duration", "1"),
            ("holdtime", "0.5"),
            ("rendercolor", "255 0 0"),
            ("renderamt", "200"),
        ]),
        entity(&[
            ("classname", "env_explosion"),
            ("targetname", "exp1"),
            ("origin", "10 0 0"),
            ("iMagnitude", "50"),
        ]),
        entity(&[
            ("classname", "env_render"),
            ("targetname", "ren1"),
            ("target", "glow_tgt"),
            ("rendermode", "5"),
            ("renderamt", "128"),
            ("rendercolor", "10 20 30"),
        ]),
        entity(&[
            ("classname", "env_message"),
            ("targetname", "msg1"),
            ("message", "#Hint_spot_clear"),
            ("spawnflags", "2"), // ALL
        ]),
        entity(&[
            ("classname", "env_shooter"),
            ("targetname", "sht1"),
            ("m_iGibs", "4"),
        ]),
        entity(&[("classname", "gibshooter"), ("targetname", "sht2")]),
        entity(&[("classname", "env_debris"), ("targetname", "sht3")]),
        entity(&[("classname", "env_blood"), ("targetname", "bld1")]),
        entity(&[("classname", "env_bubbles"), ("targetname", "bub1")]),
        entity(&[("classname", "env_funnel"), ("targetname", "fun1")]),
    ];

    let mut b = setup_bench(extra, 1, false);

    b.run(|game, eng| {
        let p0 = eng.client_edict(0);
        eng.set_origin(p0, Vec3::new(10.0, 0.0, 0.0));
        for target in [
            "spr", "cyc_m", "beam1", "beam2", "lt1", "lsr1", "spk1", "shk1", "fad1", "exp1",
            "ren1", "msg1", "sht1", "sht2", "sht3", "bld1", "bub1", "fun1",
        ] {
            fire_targets(game.state_mut(), eng, target, Some(p0), Some(p0), UseType::Toggle, 0.0);
        }
        // Dedicated spark_shower test: spawn and let it emit TE_SPARKS and auto-remove.
        create_spark_shower(game.state_mut(), eng, Vec3::new(20.0, 20.0, 20.0), Vec3::Z);
    });

    b.step(100);

    let mut saw_shake = false;
    let mut saw_fade = false;
    let mut saw_hudtext = false;
    let mut saw_sparks = false;
    let mut saw_explosion = false;
    let mut saw_beam = false;
    let mut saw_breakmodel = false;
    let mut saw_blood = false;

    for m in b.probe.messages() {
        match m {
            SvcMessage::UserMsg { name, decoded, .. } => match name.as_str() {
                "ScreenShake" => saw_shake = true,
                "ScreenFade" => saw_fade = true,
                "HudText" => {
                    if let CsUserMsg::Unknown { payload, .. } = decoded {
                        let s = String::from_utf8_lossy(payload);
                        if s.contains("#Hint_spot_clear") {
                            saw_hudtext = true;
                        }
                    }
                }
                _ => {}
            },
            SvcMessage::TempEntity { te_type, .. } => match *te_type {
                te::SPARKS => saw_sparks = true,
                te::EXPLOSION => saw_explosion = true,
                te::BEAMPOINTS => saw_beam = true,
                te::BREAKMODEL => saw_breakmodel = true,
                te::BLOODSPRITE | te::BLOODSTREAM => saw_blood = true,
                _ => {}
            },
            _ => {}
        }
    }

    assert!(saw_shake, "env_shake should send ScreenShake");
    assert!(saw_fade, "env_fade should send ScreenFade");
    assert!(saw_hudtext, "env_message should send HudText");
    assert!(saw_sparks, "env_spark/spark_shower should emit TE_SPARKS");
    assert!(saw_explosion, "env_explosion should emit TE_EXPLOSION");
    assert!(saw_beam, "beam/env_beam should emit TE_BEAMPOINTS");
    assert!(saw_breakmodel, "env_shooter/gibshooter/env_debris should emit TE_BREAKMODEL");
    assert!(saw_blood, "env_blood should emit TE_BLOODSPRITE");

    b.run(|game, eng| {
        let glow = eng.find_by_classname(None, "env_glow").unwrap();
        let gv = eng.ent(glow).unwrap();
        assert_eq!((gv.rendermode, gv.renderamt, gv.rendercolor), (5, 128, [10, 20, 30]));
        assert!(game.state().players[0].as_ref().unwrap().health < 100.0, "env_explosion dealt blast dmg");
        assert!(eng.find_by_classname(None, "spark_shower").is_none(), "spark_shower expired cleanly");
    });
}

#[test]
fn ammo_boxes_world_items_and_player_equip() {
    let extra = vec![
        entity(&[
            ("classname", "game_player_equip"),
            ("targetname", "equip_awp"),
            ("weapon_awp", "1"),
            ("spawnflags", "1"), // USEONLY
        ]),
        entity(&[("classname", "ammo_338magnum")]),
        entity(&[("classname", "ammo_357sig")]),
        entity(&[("classname", "ammo_45acp")]),
        entity(&[("classname", "ammo_50ae")]),
        entity(&[("classname", "ammo_556nato")]),
        entity(&[("classname", "ammo_556natobox")]),
        entity(&[("classname", "ammo_57mm")]),
        entity(&[("classname", "ammo_762nato")]),
        entity(&[("classname", "ammo_9mm")]),
        entity(&[("classname", "ammo_buckshot")]),
        entity(&[("classname", "item_kevlar")]),
        entity(&[("classname", "item_assaultsuit")]),
        entity(&[("classname", "item_generic")]),
    ];

    let mut b = setup_bench(extra, 1, false);

    b.run(|game, eng| {
        let p0 = eng.client_edict(0);
        // Grant weapons for each ammo class or touch all 10 ammo_* entities directly.
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Ak47).unwrap(), 30, 0));
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Awp).unwrap(), 10, 0));

        let ammo_classes = [
            ("ammo_338magnum", "338Magnum", 10),
            ("ammo_357sig", "357SIG", 13),
            ("ammo_45acp", "45acp", 12),
            ("ammo_50ae", "50AE", 7),
            ("ammo_556nato", "556Nato", 30),
            ("ammo_556natobox", "556NatoBox", 30),
            ("ammo_57mm", "57mm", 50),
            ("ammo_762nato", "762Nato", 30),
            ("ammo_9mm", "9mm", 30),
            ("ammo_buckshot", "buckshot", 8),
        ];

        for (class, ammo_name, box_amt) in ammo_classes {
            let idx = ammo_index(ammo_name) as usize;
            let before = game.player_ammo(0, idx).unwrap();
            let box_id = eng.find_by_classname(None, class).unwrap();
            game.touch(eng, box_id, p0);
            let after = game.player_ammo(0, idx).unwrap();
            assert_eq!(after - before, box_amt, "{class} should grant {box_amt} rounds");
            assert_eq!(eng.ent(box_id).unwrap().solid, SOLID_NOT);
            assert_ne!(eng.ent(box_id).unwrap().effects & EF_NODRAW, 0);
        }

        // Armor items: kevlar then assaultsuit upgrade to VestHelm.
        let kevlar_id = eng.find_by_classname(None, "item_kevlar").unwrap();
        game.touch(eng, kevlar_id, p0);
        {
            let p = game.state().players[0].as_ref().unwrap();
            assert_eq!(p.armor, 100.0);
            assert_eq!(p.kevlar, ArmorType::Vest);
        }
        let suit_id = eng.find_by_classname(None, "item_assaultsuit").unwrap();
        game.touch(eng, suit_id, p0);
        {
            let p = game.state().players[0].as_ref().unwrap();
            assert_eq!(p.armor, 100.0);
            assert_eq!(p.kevlar, ArmorType::VestHelm);
        }

        assert!(eng.find_by_classname(None, "item_generic").is_some());

        // game_player_equip works on Use.
        fire_targets(game.state_mut(), eng, "equip_awp", Some(p0), Some(p0), UseType::Toggle, 0.0);

        // Round restart restores ammo & armor pickups.
        game.restart_round_now(eng);
        assert_eq!(eng.ent(kevlar_id).unwrap().effects & EF_NODRAW, 0);
        assert_eq!(
            eng.ent(eng.find_by_classname(None, "ammo_9mm").unwrap())
                .unwrap()
                .effects
                & EF_NODRAW,
            0
        );
    });

    b.step(4);

    let ammo_pickups = b
        .probe
        .messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::UserMsg { name, .. } if name == "AmmoPickup"))
        .count();
    assert!(ammo_pickups >= 10, "expected >= 10 AmmoPickup usermsgs, got {ammo_pickups}");
}
