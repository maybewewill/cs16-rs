//! Step 1 Phase 7 integration tests: entity I/O (`multi_manager`, `multisource`, `master`,
//! `env_global`), `game_*` rule entities, `player_weaponstrip`, all `trigger_*` + `info_*` +
//! `infodecal`, active `trigger_camera`, and `votemap`/`vote`.

mod common;

use common::{Bench, command, entity, stub_spawns};
use cs_engine::GameDll;
use cs_game::entity::UseType;
use cs_game::player::Team;
use cs_game::targets::fire_targets;
use cs_game::weapons::{WeaponId, new_weapon};
use cs_proto48::messages::SvcMessage;
use glam::Vec3;

fn setup_bench(extra_entities: Vec<cs_assets::bsp::Entity>, slots: u8) -> Bench {
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
    b.probe.clear_messages();
    b
}

#[test]
fn multi_manager_multisource_env_global_and_triggers_work_with_restart() {
    let ents = vec![
        entity(&[
            ("classname", "multi_manager"),
            ("targetname", "mm_seq"),
            ("t_first", "0.02"),
            ("t_first#1", "0.05"),
            ("t_global", "0.02"),
        ]),
        entity(&[
            ("classname", "test_target"),
            ("targetname", "t_first"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "env_global"),
            ("targetname", "t_global"),
            ("globalstate", "power_on"),
            ("triggermode", "1"),
        ]),
        entity(&[
            ("classname", "multisource"),
            ("targetname", "ms_gate"),
            ("globalstate", "power_on"),
            ("target", "t_after_gate"),
        ]),
        entity(&[
            ("classname", "test_target"),
            ("targetname", "t_after_gate"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "trigger_counter"),
            ("targetname", "tc_count"),
            ("count", "2"),
            ("target", "t_after_gate"),
            ("spawnflags", "1"),
        ]),
        entity(&[
            ("classname", "trigger_relay"),
            ("targetname", "tr_relay"),
            ("triggerstate", "1"),
            ("target", "t_first"),
            ("spawnflags", "1"),
        ]),
        entity(&[
            ("classname", "trigger_changetarget"),
            ("targetname", "tr_change"),
            ("target", "tr_relay"),
            ("m_iszNewTarget", "t_after_gate"),
        ]),
        entity(&[
            ("classname", "trigger_auto"),
            ("globalstate", "power_on"),
            ("target", "t_after_gate"),
            ("spawnflags", "1"),
        ]),
        entity(&[
            ("classname", "info_teleport_destination"),
            ("targetname", "tp_dest"),
            ("origin", "400 200 64"),
            ("angles", "0 90 0"),
        ]),
        entity(&[
            ("classname", "info_intermission"),
            ("origin", "0 0 128"),
        ]),
        entity(&[
            ("classname", "info_null"),
            ("targetname", "removed_null"),
        ]),
        entity(&[
            ("classname", "infodecal"),
            ("origin", "0 0 0"),
            ("texture", "{shot1"),
        ]),
        entity(&[
            ("classname", "trigger_teleport"),
            ("targetname", "tp_trig"),
            ("target", "tp_dest"),
            ("origin", "100 0 0"),
        ]),
        entity(&[
            ("classname", "trigger_hurt"),
            ("targetname", "hurt_trig"),
            ("dmg", "40"),
            ("origin", "200 0 0"),
        ]),
        entity(&[
            ("classname", "trigger_push"),
            ("targetname", "push_trig"),
            ("angles", "0 0 0"),
            ("speed", "300"),
            ("origin", "300 0 0"),
        ]),
        entity(&[
            ("classname", "trigger_gravity"),
            ("targetname", "grav_trig"),
            ("gravity", "0.25"),
            ("origin", "350 0 0"),
        ]),
        entity(&[
            ("classname", "trigger_once"),
            ("targetname", "once_trig"),
            ("master", "ms_gate"),
            ("target", "t_after_gate"),
            ("origin", "50 0 0"),
        ]),
        entity(&[
            ("classname", "trigger_multiple"),
            ("targetname", "mult_trig"),
            ("wait", "0.1"),
            ("target", "t_first"),
            ("origin", "75 0 0"),
        ]),
        entity(&[
            ("classname", "trigger_cdaudio"),
            ("health", "7"),
            ("origin", "90 0 0"),
        ]),
        entity(&[
            ("classname", "target_cdaudio"),
            ("targetname", "cd_target"),
            ("health", "9"),
        ]),
    ];

    let mut b = setup_bench(ents, 1);
    b.run(|game, eng| {
        assert!(
            eng.find_by_classname(None, "info_null").is_none(),
            "info_null removes itself on spawn"
        );
        for cls in [
            "trigger_teleport",
            "trigger_hurt",
            "trigger_push",
            "trigger_gravity",
            "trigger_once",
            "trigger_multiple",
            "trigger_cdaudio",
        ] {
            let id = eng
                .find_by_classname(None, cls)
                .unwrap_or_else(|| panic!("missing {cls}"));
            eng.set_size(id, Vec3::splat(-32.0), Vec3::splat(32.0));
            let org = eng.ent(id).unwrap().origin;
            eng.set_origin(id, org);
        }
        game.state_mut().dispatch_log.clear();

        let p0 = eng.client_edict(0);
        let tr_once = eng.find_by_classname(None, "trigger_once").unwrap();
        // Before env_global sets power_on, master ms_gate blocks trigger_once.
        game.touch(eng, tr_once, p0);
        assert!(
            game.state().dispatch_log.is_empty(),
            "master should block trigger_once initially"
        );

        fire_targets(
            game.state_mut(),
            eng,
            "mm_seq",
            Some(p0),
            Some(p0),
            UseType::Toggle,
            0.0,
        );
    });
    b.step(20);

    b.run(|game, eng| {
        assert!(
            game.state().dispatch_log.len() >= 2,
            "expected multi_manager + trigger_auto fires, got {:?}",
            game.state().dispatch_log
        );

        let p0 = eng.client_edict(0);
        let tr_once = eng.find_by_classname(None, "trigger_once").unwrap();
        fire_targets(
            game.state_mut(),
            eng,
            "ms_gate",
            Some(p0),
            Some(p0),
            UseType::On,
            0.0,
        );
        let before = game.state().dispatch_log.len();
        game.touch(eng, tr_once, p0);
        assert_eq!(game.state().dispatch_log.len(), before + 1);
        game.touch(eng, tr_once, p0);
        assert_eq!(
            game.state().dispatch_log.len(),
            before + 1,
            "trigger_once must stay disabled after first fire"
        );

        // trigger_counter counts 2 then fires.
        let cnt_before = game.state().dispatch_log.len();
        fire_targets(game.state_mut(), eng, "tc_count", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), cnt_before);
        fire_targets(game.state_mut(), eng, "tc_count", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), cnt_before + 1);

        // trigger_changetarget rewires trigger_relay, then fires.
        fire_targets(game.state_mut(), eng, "tr_change", Some(p0), Some(p0), UseType::Toggle, 0.0);
        fire_targets(game.state_mut(), eng, "tr_relay", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), cnt_before + 2);

        // trigger_hurt deals dmg/2 = 20 damage.
        let hurt = eng.find_by_classname(None, "trigger_hurt").unwrap();
        let hp_before = game.state().players[0].as_ref().unwrap().health;
        game.touch(eng, hurt, p0);
        assert_eq!(game.state().players[0].as_ref().unwrap().health, hp_before - 20.0);

        // trigger_push + trigger_gravity.
        let push = eng.find_by_classname(None, "trigger_push").unwrap();
        game.touch(eng, push, p0);
        assert!(eng.ent(p0).unwrap().basevelocity.x >= 299.0);
        let grav = eng.find_by_classname(None, "trigger_gravity").unwrap();
        game.touch(eng, grav, p0);
        assert_eq!(eng.ent(p0).unwrap().gravity, 0.25);

        // trigger_teleport moves player to tp_dest.
        let tp = eng.find_by_classname(None, "trigger_teleport").unwrap();
        game.touch(eng, tp, p0);
        assert!((eng.ent(p0).unwrap().origin - Vec3::new(400.0, 200.0, 65.0)).length() < 0.01);

        // trigger_cdaudio + target_cdaudio send cd play.
        let cd_trig = eng.find_by_classname(None, "trigger_cdaudio").unwrap();
        game.touch(eng, cd_trig, p0);
        fire_targets(game.state_mut(), eng, "cd_target", Some(p0), Some(p0), UseType::Toggle, 0.0);

        // Restart resets multisource and restores trigger_once.
        game.restart_round_now(eng);
        let p0 = eng.client_edict(0);
        let before_restart_touch = game.state().dispatch_log.len();
        game.touch(eng, tr_once, p0);
        assert_eq!(game.state().dispatch_log.len(), before_restart_touch, "master should be re-locked after restart");
        fire_targets(game.state_mut(), eng, "ms_gate", Some(p0), Some(p0), UseType::On, 0.0);
        let after_gate = game.state().dispatch_log.len();
        assert_eq!(after_gate, before_restart_touch + 1, "multisource fires target once unlocked");
        game.touch(eng, tr_once, p0);
        assert_eq!(game.state().dispatch_log.len(), after_gate + 1, "restored trigger_once fires once master unlocks");
    });
}

#[test]
fn game_io_entities_camera_and_strip_function_properly() {
    let ents = vec![
        entity(&[
            ("classname", "game_counter"),
            ("targetname", "gc_one"),
            ("frags", "3"),
            ("target", "t_gc_done"),
        ]),
        entity(&[
            ("classname", "game_counter_set"),
            ("targetname", "gc_set"),
            ("frags", "2"),
            ("target", "gc_one"),
        ]),
        entity(&[
            ("classname", "test_target"),
            ("targetname", "t_gc_done"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "game_player_hurt"),
            ("targetname", "gp_hurt"),
            ("dmg", "25"),
        ]),
        entity(&[
            ("classname", "game_score"),
            ("targetname", "gp_score"),
            ("points", "5"),
        ]),
        entity(&[
            ("classname", "game_team_master"),
            ("targetname", "tm_ct"),
            ("teamindex", "2"),
            ("triggerstate", "1"),
            ("target", "t_gc_done"),
        ]),
        entity(&[
            ("classname", "game_team_set"),
            ("targetname", "tm_set"),
            ("target", "tm_ct"),
        ]),
        entity(&[
            ("classname", "game_player_team"),
            ("targetname", "gp_team"),
            ("target", "TERRORIST"),
        ]),
        entity(&[
            ("classname", "game_text"),
            ("targetname", "gt_msg"),
            ("message", "Welcome to sector 7"),
            ("spawnflags", "1"),
        ]),
        entity(&[
            ("classname", "game_zone_player"),
            ("targetname", "gzp_zone"),
            ("origin", "0 0 0"),
            ("intarget", "t_gc_done"),
        ]),
        entity(&[
            ("classname", "player_weaponstrip"),
            ("targetname", "p_strip"),
        ]),
        entity(&[
            ("classname", "info_target"),
            ("targetname", "cam_target"),
            ("origin", "200 0 0"),
        ]),
        entity(&[
            ("classname", "path_corner"),
            ("targetname", "cam_p1"),
            ("origin", "100 0 50"),
        ]),
        entity(&[
            ("classname", "trigger_camera"),
            ("targetname", "cam_main"),
            ("origin", "0 0 50"),
            ("target", "cam_target"),
            ("moveto", "cam_p1"),
            ("wait", "0.2"),
        ]),
        entity(&[
            ("classname", "game_end"),
            ("targetname", "ge_finish"),
        ]),
    ];

    let mut b = setup_bench(ents, 2);
    b.run(|game, eng| {
        let p0 = eng.client_edict(0);
        let p1 = eng.client_edict(1);

        // Size game_zone_player around p0.
        let z = eng.find_by_classname(None, "game_zone_player").unwrap();
        let p0_org = eng.ent(p0).unwrap().origin;
        eng.set_size(z, Vec3::splat(-64.0), Vec3::splat(64.0));
        eng.set_origin(z, p0_org);
        eng.set_origin(p1, p0_org + Vec3::new(1000.0, 0.0, 0.0));

        // game_counter_set sets gc_one to 2; one more toggle hits frags=3 -> fires t_gc_done.
        fire_targets(game.state_mut(), eng, "gc_set", Some(p0), Some(p0), UseType::Toggle, 0.0);
        let before = game.state().dispatch_log.len();
        fire_targets(game.state_mut(), eng, "gc_one", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), before + 1);

        // game_player_hurt damages player.
        let hp = game.state().players[0].as_ref().unwrap().health;
        fire_targets(game.state_mut(), eng, "gp_hurt", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().players[0].as_ref().unwrap().health, hp - 25.0);

        // game_score adds frags.
        let frags_before = game.state().players[0].as_ref().unwrap().frags;
        fire_targets(game.state_mut(), eng, "gp_score", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().players[0].as_ref().unwrap().frags, frags_before + 5);

        // game_team_master blocks Terrorist p1, allows CT p0; game_team_set rebinds to T.
        let tm_before = game.state().dispatch_log.len();
        fire_targets(game.state_mut(), eng, "tm_ct", Some(p1), Some(p1), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), tm_before);
        fire_targets(game.state_mut(), eng, "tm_ct", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), tm_before + 1);
        fire_targets(game.state_mut(), eng, "tm_set", Some(p1), Some(p1), UseType::Toggle, 0.0);
        fire_targets(game.state_mut(), eng, "tm_ct", Some(p1), Some(p1), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), tm_before + 2);

        // game_zone_player fires intarget for p0 inside zone.
        fire_targets(game.state_mut(), eng, "gzp_zone", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().dispatch_log.len(), tm_before + 3);

        // game_player_team swaps player 0 to Terrorist.
        fire_targets(game.state_mut(), eng, "gp_team", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert_eq!(game.state().players[0].as_ref().unwrap().team, Team::Terrorist);

        // player_weaponstrip removes AK47 + Knife.
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Ak47).unwrap(), 30, 90));
        assert!(!game.carried_weapons(0).is_empty());
        fire_targets(game.state_mut(), eng, "p_strip", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert!(game.carried_weapons(0).is_empty());

        // game_text sends TE_TEXTMESSAGE (29) + trigger_camera runs + game_end triggers intermission.
        fire_targets(game.state_mut(), eng, "gt_msg", Some(p0), Some(p0), UseType::Toggle, 0.0);
        fire_targets(game.state_mut(), eng, "cam_main", Some(p0), Some(p0), UseType::On, 0.0);
    });
    b.step(25);
    assert!(
        b.probe
            .messages()
            .iter()
            .any(|m| matches!(m, SvcMessage::TempEntity { te_type: 29, .. })),
        "expected TE_TEXTMESSAGE from game_text"
    );
    b.run(|game, eng| {
        let p0 = eng.client_edict(0);
        fire_targets(game.state_mut(), eng, "ge_finish", Some(p0), Some(p0), UseType::Toggle, 0.0);
        assert!(game.state().rules.game_over());
    });
}

#[test]
fn votemap_and_vote_commands_tally_votes_and_trigger_actions() {
    let mut b = setup_bench(vec![], 4);
    b.run(|game, eng| {
        game.state_mut().map_cycle.test_set_maps(&["de_dust2", "cs_office", "de_inferno"]);
        eng.set_cvar("mp_mapvoteratio", "0.5");
        eng.set_cvar("mp_kickpercent", "0.5");
    });

    // Two CTs (slots 0 and 2): 1 vote on 4 players (< 50%), 2nd vote hits 2/4 = 50% -> change_level.
    command(&mut b, 0, "votemap 2");
    assert!(b.server.take_pending_change_level().is_none());
    command(&mut b, 2, "votemap cs_office");
    assert_eq!(b.server.take_pending_change_level().as_deref(), Some("cs_office"));

    // Kick vote among CTs (slots 0 and 2): slot 2 votes to kick slot 0 (`vote 1`, 1/2 = 50% -> kicked).
    command(&mut b, 2, "vote 1");
    b.step(2);
    let slot0_present = b.run(|game, _| game.state().players[0].is_some());
    assert!(!slot0_present, "slot 0 should be kicked after 50% team vote");
    assert!(matches!(b.probe.state(), cs_probe::ProbeState::Disconnected(_)));
}
