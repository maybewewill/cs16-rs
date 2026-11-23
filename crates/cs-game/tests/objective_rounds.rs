//! Objective round rules, time expiry, economy rewards, and C4 rotation (Task 7 of phase 2E),
//! verified against vanilla `CHalfLifeMultiplay` (`multiplay_gamerules.cpp`).

mod common;

use common::{Bench, entity, spawns};
use cs_assets::bsp::Entity;
use cs_engine::GameDll;
use cs_engine::consts::{FL_ONGROUND, SOLID_TRIGGER};
use cs_game::bullets::{self, MultiDamage};
use cs_game::money::{
    REWARD_ALL_HOSTAGES_RESCUED, REWARD_BOMB_DEFUSED, REWARD_BOMB_EXPLODED, REWARD_BOMB_PLANTED,
    REWARD_HOSTAGE_NOT_RESCUED, REWARD_LOSER_BONUS_DEFAULT, REWARD_LOSER_BONUS_MIN,
    REWARD_RESCUED_HOSTAGE, REWARD_TARGET_BOMB, REWARD_TARGET_BOMB_SAVED, REWARD_TOOK_HOSTAGE,
};
use cs_game::player::dmg;
use cs_game::rules::WinStatus;
use cs_game::weapons::WeaponId;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pm::consts::{IN_ATTACK, IN_USE};
use glam::Vec3;

const ZONE_AT: Vec3 = Vec3::new(1000.0, 0.0, 0.0);

fn frames(secs: f64) -> usize {
    (secs * 100.0).round() as usize
}

fn idle_cmd() -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec: 10,
        ..Default::default()
    }
}

fn setup_match(entities: Vec<Entity>, ts: &[u8], cts: &[u8]) -> Bench {
    let max_clients = ts.iter().chain(cts.iter()).copied().max().unwrap_or(1) + 1;
    let ts_vec = ts.to_vec();
    let cts_vec = cts.to_vec();
    let mut b = Bench::with_config(
        cs_engine::ServerMap::stub_with_entities("test", entities),
        cs_engine::ServerConfig {
            max_clients: max_clients.max(4),
            max_edicts: 256,
            ..Default::default()
        },
        &[("cl_lw", "0")],
        false,
    );
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_freezetime", "2");
        eng.set_cvar("mp_roundtime", "2");
        eng.set_cvar("mp_autoteambalance", "0");
        eng.set_cvar("mp_limitteams", "0");
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
                eng.set_origin(id, origin);
            }
        }
        let cmd_args = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
        for &slot in ts_vec.iter().chain(cts_vec.iter()) {
            if slot != 0 {
                game.client_put_in_server(eng, slot);
                eng.test_set_client_active(slot, true);
            }
            for _ in 0..2 {
                game.run_usercmd(eng, slot, &idle_cmd(), 0);
                game.post_usercmd(eng, slot, &idle_cmd());
            }
        }
        for &slot in &ts_vec {
            game.client_command(eng, slot, &cmd_args("jointeam", "1"));
            game.client_command(eng, slot, &cmd_args("joinclass", "1"));
        }
        for &slot in &cts_vec {
            game.client_command(eng, slot, &cmd_args("jointeam", "2"));
            game.client_command(eng, slot, &cmd_args("joinclass", "1"));
        }
        for &slot in ts_vec.iter().chain(cts_vec.iter()) {
            game.run_usercmd(eng, slot, &idle_cmd(), 0);
            game.post_usercmd(eng, slot, &idle_cmd());
        }
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
    });
    b.step(12);
    b.probe.clear_messages();
    b
}

fn kill_hostage(
    game: &mut cs_game::CsGame,
    eng: &mut cs_engine::EngineApi<'_>,
    attacker: cs_engine::EdictId,
    hostage: cs_engine::EdictId,
) {
    let mut mm = MultiDamage::new(None);
    mm.clear();
    let tr = cs_engine::EntTrace {
        trace: cs_engine::collision::TraceResult {
            fraction: 0.5,
            end_pos: Vec3::ZERO,
            hitgroup: 2,
            ..cs_engine::collision::TraceResult::clear(Vec3::ZERO)
        },
        ent: Some(hostage),
    };
    let state = game.state_mut();
    bullets::trace_attack(
        state,
        eng,
        hostage,
        Some(attacker),
        &mut mm,
        500.0,
        Vec3::X,
        &tr,
        dmg::BULLET,
    );
    bullets::apply_multi_damage(state, eng, &mm, Some(attacker), Some(attacker));
}

fn cmd_slot(
    game: &mut cs_game::CsGame,
    eng: &mut cs_engine::EngineApi<'_>,
    slot: u8,
    buttons: u16,
    on_ground: bool,
) {
    let id = eng.client_edict(slot);
    let viewangles = eng.ent(id).unwrap().v_angle;
    eng.ent_mut(id).unwrap().button = i32::from(buttons);
    let cmd = GoldSrcUserCmd {
        msec: 10,
        buttons,
        viewangles,
        ..Default::default()
    };
    game.run_usercmd(eng, slot, &cmd, 0);
    if let Some(v) = eng.ent_mut(id) {
        if on_ground {
            v.flags |= FL_ONGROUND;
        } else {
            v.flags &= !FL_ONGROUND;
        }
    }
    if let Some((pmin, pmax)) = eng.ent(id).map(|v| (v.absmin, v.absmax)) {
        for other in eng.entity_ids() {
            if other == id {
                continue;
            }
            if let Some(o) = eng.ent(other)
                && o.solid == SOLID_TRIGGER
                && o.absmin.x <= pmax.x
                && o.absmax.x >= pmin.x
                && o.absmin.y <= pmax.y
                && o.absmax.y >= pmin.y
                && o.absmin.z <= pmax.z
                && o.absmax.z >= pmin.z
            {
                game.touch(eng, other, id);
            }
        }
    }
    game.post_usercmd(eng, slot, &cmd);
}

fn bomb_map_entities() -> Vec<Entity> {
    let mut e = spawns(8, 8);
    e.push(entity(&[
        ("classname", "func_bomb_target"),
        ("origin", "1000 0 0"),
    ]));
    e
}

fn plant_bomb_by_slot(b: &mut Bench, slot: u8) -> cs_engine::EdictId {
    b.run(move |game, eng| {
        assert!(game.select_weapon(eng, slot, WeaponId::C4));
        let eid = eng.client_edict(slot);
        eng.set_origin(eid, ZONE_AT);
        eng.ent_mut(eid).unwrap().flags |= FL_ONGROUND;
        game.set_next_attack(slot, 0.0);
        if let Some(w) = game.weapon_mut(slot, WeaponId::C4) {
            w.next_primary_attack = 0.0;
        }
    });
    // Advance > 0.5 s so `HandleSignals` latches `SIGNAL_BOMB`.
    for _ in 0..60 {
        b.run(move |game, eng| cmd_slot(game, eng, slot, 0, true));
    }
    // Hold IN_ATTACK for 3.2 s across 10 ms usercmds.
    for _ in 0..320 {
        b.run(move |game, eng| cmd_slot(game, eng, slot, IN_ATTACK as u16, true));
    }
    b.run(|_, eng| {
        let mut cursor = None;
        while let Some(id) = eng.find_by_classname(cursor, "grenade") {
            cursor = Some(id);
            if eng
                .private::<Box<dyn cs_game::entity::GameEntity>>(id)
                .is_some_and(|e| e.is_planted_c4())
            {
                return id;
            }
        }
        panic!("expected a planted C4 entity");
    })
}

fn has_center_text(b: &Bench, text: &str) -> bool {
    b.messages().iter().any(
        |m| matches!(m, CsUserMsg::TextMsg { dest: 4, message, .. } if message == text),
    )
}

fn has_audio(b: &Bench, code: &str) -> bool {
    b.messages()
        .iter()
        .any(|m| matches!(m, CsUserMsg::SendAudio { audio_code, .. } if audio_code == code))
}

#[test]
fn bomb_exploded_wins_for_terrorists() {
    let mut b = setup_match(bomb_map_entities(), &[0], &[1]);
    b.run(|_, eng| eng.set_cvar("mp_c4timer", "10"));
    b.run(|game, eng| game.restart_round_now(eng));
    b.run(|game, eng| game.end_freeze_period(eng));
    b.step(10);
    b.probe.clear_messages();

    let _bomb = plant_bomb_by_slot(&mut b, 0);
    // Eliminating the last T while a live planted C4 ticks must NOT hand CT an elimination win.
    b.run(|game, eng| {
        game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET);
    });
    b.step(2);
    assert_eq!(b.server.game().game.state().rules.win_status(), WinStatus::None);

    // Wait out the 10-second C4 timer until detonation.
    b.probe.clear_messages();
    b.step(frames(11.0));
    let state = b.server.game().game.state();
    assert_eq!(state.rules.win_status(), WinStatus::Terrorists);
    assert_eq!(state.num_terrorist_wins, 1);
    assert_eq!(state.rules.economy().account_terrorist(), REWARD_TARGET_BOMB);
    assert!(has_center_text(&b, "#Target_Bombed"));
    assert!(has_audio(&b, "%!MRAD_terwin"));

    let t_before = state.players[0].as_ref().unwrap().money;
    let ct_before = state.players[1].as_ref().unwrap().money;
    b.step(frames(5.5));
    let state = b.server.game().game.state();
    assert_eq!(state.players[0].as_ref().unwrap().money, t_before + REWARD_TARGET_BOMB);
    assert_eq!(
        state.players[1].as_ref().unwrap().money,
        ct_before + REWARD_LOSER_BONUS_DEFAULT
    );
}

#[test]
fn bomb_defused_wins_for_cts() {
    let mut b = setup_match(bomb_map_entities(), &[0], &[1]);
    let bomb = plant_bomb_by_slot(&mut b, 0);
    // Give CT a defuser and move CT right next to the bomb facing it (+X).
    b.run(move |game, eng| {
        game.test_set_defuser(1, true);
        let bomb_pos = eng.ent(bomb).unwrap().origin;
        let ct_id = eng.client_edict(1);
        eng.set_origin(ct_id, bomb_pos - Vec3::new(32.0, 0.0, 0.0));
        if let Some(v) = eng.ent_mut(ct_id) {
            v.flags |= FL_ONGROUND;
            v.v_angle = Vec3::ZERO;
        }
        game.set_next_attack(1, 0.0);
    });
    b.probe.clear_messages();
    for _ in 0..600 {
        let done = b.run(|game, eng| {
            cmd_slot(game, eng, 1, IN_USE as u16, true);
            game.state().rules.bomb_defused
        });
        if done {
            break;
        }
    }
    b.step(10);
    let state = b.server.game().game.state();
    assert_eq!(state.rules.win_status(), WinStatus::Cts);
    assert_eq!(state.num_ct_wins, 1);
    assert_eq!(state.rules.economy().account_ct(), REWARD_BOMB_DEFUSED);
    assert_eq!(state.rules.economy().account_terrorist(), REWARD_BOMB_PLANTED);
    assert!(has_center_text(&b, "#Bomb_Defused"));
    assert!(has_audio(&b, "%!MRAD_ctwin"));

    let t_before = state.players[0].as_ref().unwrap().money;
    let ct_before = state.players[1].as_ref().unwrap().money;
    b.step(frames(5.5));
    let state = b.server.game().game.state();
    assert_eq!(state.players[1].as_ref().unwrap().money, ct_before + REWARD_BOMB_DEFUSED);
    assert_eq!(
        state.players[0].as_ref().unwrap().money,
        t_before + REWARD_BOMB_PLANTED + REWARD_LOSER_BONUS_DEFAULT
    );
}

#[test]
fn time_up_on_a_bomb_map_is_target_saved() {
    // Part A: a planted bomb holds the round open past round-time expiry.
    let mut b = setup_match(bomb_map_entities(), &[0, 2], &[1]);
    b.run(|_, eng| {
        eng.set_cvar("mp_roundtime", "1");
        eng.set_cvar("mp_freezetime", "0");
        eng.set_cvar("mp_c4timer", "30");
    });
    b.run(|game, eng| game.restart_round_now(eng));
    b.step(5);
    let bomber = b
        .server
        .game()
        .game
        .state()
        .players
        .iter()
        .position(|p| p.as_ref().is_some_and(|p| p.has_c4))
        .expect("a terrorist carrier") as u8;
    // Advance 50 seconds, then plant the bomb before 60 seconds run out.
    b.step(frames(50.0));
    let _bomb = plant_bomb_by_slot(&mut b, bomber);
    b.step(frames(12.0));
    assert_eq!(b.server.game().game.state().rules.win_status(), WinStatus::None);

    // Part B: fresh round without a planted bomb; kill slot 2 (T) before expiry, keep slot 0 alive.
    let mut b = setup_match(bomb_map_entities(), &[0, 2], &[1]);
    b.run(|_, eng| {
        eng.set_cvar("mp_roundtime", "1");
        eng.set_cvar("mp_freezetime", "0");
    });
    b.run(|game, eng| game.restart_round_now(eng));
    b.run(|game, eng| {
        game.damage_player(eng, 2, Some(1), 500.0, dmg::BULLET);
    });
    b.step(5);
    let t_alive_before = b.server.game().game.state().players[0].as_ref().unwrap().money;
    let t_dead_before = b.server.game().game.state().players[2].as_ref().unwrap().money;
    let ct_before = b.server.game().game.state().players[1].as_ref().unwrap().money;

    b.probe.clear_messages();
    b.step(frames(61.0));
    let state = b.server.game().game.state();
    assert_eq!(state.rules.win_status(), WinStatus::Cts);
    assert_eq!(state.num_ct_wins, 1);
    assert_eq!(state.rules.economy().account_ct(), REWARD_TARGET_BOMB_SAVED);
    assert!(has_center_text(&b, "#Target_Saved"));
    assert!(has_audio(&b, "%!MRAD_ctwin"));
    assert!(state.players[0].as_ref().unwrap().receives_no_money_next_round);
    assert!(!state.players[2].as_ref().unwrap().receives_no_money_next_round);

    b.step(frames(5.5));
    let state = b.server.game().game.state();
    assert_eq!(
        state.players[1].as_ref().unwrap().money,
        ct_before + REWARD_TARGET_BOMB_SAVED
    );
    // Living T receives nothing; dead T still receives the loser bonus.
    assert_eq!(state.players[0].as_ref().unwrap().money, t_alive_before);
    assert_eq!(
        state.players[2].as_ref().unwrap().money,
        t_dead_before + REWARD_LOSER_BONUS_DEFAULT
    );
}

#[test]
fn all_hostages_rescued_wins_for_cts() {
    let mut entities = spawns(4, 4);
    entities.push(entity(&[
        ("classname", "info_hostage_rescue"),
        ("origin", "1500 0 0"),
    ]));
    entities.push(entity(&[
        ("classname", "hostage_entity"),
        ("origin", "1500 0 0"),
    ]));
    entities.push(entity(&[
        ("classname", "hostage_entity"),
        ("origin", "0 200 0"),
    ]));
    let mut b = setup_match(entities, &[0], &[1]);

    // Find the two hostages; rescue #1 with CT via +use while #2 stays alive.
    let (h1, h2) = b.run(|_, eng| {
        let a = eng.find_by_classname(None, "hostage_entity").unwrap();
        let b = eng.find_by_classname(Some(a), "hostage_entity").unwrap();
        (a, b)
    });
    b.run(move |game, eng| {
        let ct = eng.client_edict(1);
        eng.set_origin(ct, Vec3::new(1460.0, 0.0, 0.0));
        if let Some(v) = eng.ent_mut(ct) {
            v.flags |= FL_ONGROUND;
            v.v_angle = Vec3::ZERO;
        }
        eng.ent_mut(h1).unwrap().flags |= FL_ONGROUND;
        game.set_next_attack(1, 0.0);
        let cmd = GoldSrcUserCmd {
            msec: 10,
            buttons: IN_USE as u16,
            ..Default::default()
        };
        game.run_usercmd(eng, 1, &cmd, 0);
        game.post_usercmd(eng, 1, &cmd);
        let rel = GoldSrcUserCmd {
            msec: 10,
            ..Default::default()
        };
        game.run_usercmd(eng, 1, &rel, 0);
        game.post_usercmd(eng, 1, &rel);
    });
    b.step(frames(0.3));
    let state = b.server.game().game.state();
    assert_eq!(state.rules.hostages_rescued, 1);
    // Hostage #2 is still alive, so round must still be in progress.
    assert_eq!(state.rules.win_status(), WinStatus::None);

    // T kills hostage #2 -> 0 hostages alive and 1/2 rescued (>= 50%), so CTs win.
    b.probe.clear_messages();
    b.run(move |game, eng| {
        let t = eng.client_edict(0);
        kill_hostage(game, eng, t, h2);
    });
    b.step(10);
    let state = b.server.game().game.state();
    assert_eq!(state.rules.win_status(), WinStatus::Cts);
    assert_eq!(state.rules.economy().account_ct(), REWARD_ALL_HOSTAGES_RESCUED + 100);
    assert!(has_center_text(&b, "#All_Hostages_Rescued"));
    assert!(has_audio(&b, "%!MRAD_ctwin"));

    let ct_before = state.players[1].as_ref().unwrap().money;
    b.step(frames(5.5));
    let state = b.server.game().game.state();
    // RestartRound settles rescued bonus (750) + all-rescued (2500) + touch-acc (100).
    assert_eq!(
        state.players[1].as_ref().unwrap().money,
        ct_before + REWARD_ALL_HOSTAGES_RESCUED + 100 + REWARD_RESCUED_HOSTAGE
    );

    // Next round: if T kills BOTH hostages (0/2 rescued), rescue win check must not fire.
    let (h1, h2) = b.run(|_, eng| {
        let a = eng.find_by_classname(None, "hostage_entity").unwrap();
        let b = eng.find_by_classname(Some(a), "hostage_entity").unwrap();
        (a, b)
    });
    b.run(move |game, eng| {
        let t = eng.client_edict(0);
        kill_hostage(game, eng, t, h1);
        kill_hostage(game, eng, t, h2);
    });
    assert_eq!(b.server.game().game.state().rules.win_status(), WinStatus::None);
}

#[test]
fn time_up_on_a_hostage_map_wins_for_terrorists() {
    let mut entities = spawns(4, 4);
    entities.push(entity(&[
        ("classname", "hostage_entity"),
        ("origin", "0 200 0"),
    ]));
    entities.push(entity(&[
        ("classname", "hostage_entity"),
        ("origin", "0 300 0"),
    ]));
    let mut b = setup_match(entities, &[0], &[1, 2]);
    b.run(|_, eng| {
        eng.set_cvar("mp_roundtime", "1");
        eng.set_cvar("mp_freezetime", "0");
    });
    b.run(|game, eng| game.restart_round_now(eng));
    b.run(|game, eng| {
        game.damage_player(eng, 2, Some(0), 500.0, dmg::BULLET);
    });
    b.step(5);
    let t_before = b.server.game().game.state().players[0].as_ref().unwrap().money;
    let ct_alive_before = b.server.game().game.state().players[1].as_ref().unwrap().money;
    let ct_dead_before = b.server.game().game.state().players[2].as_ref().unwrap().money;

    b.probe.clear_messages();
    b.step(frames(61.0));
    let state = b.server.game().game.state();
    assert_eq!(state.rules.win_status(), WinStatus::Terrorists);
    assert_eq!(state.num_terrorist_wins, 1);
    assert_eq!(state.rules.economy().account_terrorist(), REWARD_HOSTAGE_NOT_RESCUED);
    assert!(has_center_text(&b, "#Hostages_Not_Rescued"));
    assert!(has_audio(&b, "%!MRAD_terwin"));
    assert!(state.players[1].as_ref().unwrap().receives_no_money_next_round);
    assert!(!state.players[2].as_ref().unwrap().receives_no_money_next_round);

    b.step(frames(5.5));
    let state = b.server.game().game.state();
    // 2 living hostages grant 2 * 150 = 300 surviving bonus to the winning team (Terrorists).
    assert_eq!(
        state.players[0].as_ref().unwrap().money,
        t_before + REWARD_HOSTAGE_NOT_RESCUED + 2 * REWARD_TOOK_HOSTAGE
    );
    assert_eq!(state.players[1].as_ref().unwrap().money, ct_alive_before);
    assert_eq!(
        state.players[2].as_ref().unwrap().money,
        ct_dead_before + REWARD_LOSER_BONUS_DEFAULT
    );
}

#[test]
fn objective_money_as_regamedll() {
    // 14 living hostages hit the >= 2000 break threshold (adds up to 2100, then breaks).
    let mut entities = spawns(4, 4);
    for i in 0..15 {
        let origin = format!("{} 300 0", i * 40);
        entities.push(entity(&[
            ("classname", "hostage_entity"),
            ("origin", origin.as_str()),
        ]));
    }
    let mut b = setup_match(entities, &[0], &[1]);
    // Simulate a streak loss + 2 rescued hostages + 15 hostages in world.
    b.run(|game, eng| {
        game.state_mut().rules.hostages_rescued = 2;
        game.damage_player(eng, 1, Some(0), 500.0, dmg::BULLET);
    });
    let t_before = b.server.game().game.state().players[0].as_ref().unwrap().money;
    let ct_before = b.server.game().game.state().players[1].as_ref().unwrap().money;
    b.run(|game, eng| game.restart_round_now(eng));
    let state = b.server.game().game.state();
    // 14 solid hostages counted before `acct_tmp >= 2000` break -> 14 * 150 = 2100 to T winner.
    assert_eq!(
        state.players[0].as_ref().unwrap().money,
        t_before + 3000 + 14 * REWARD_TOOK_HOSTAGE
    );
    // CT receives loser bonus (1400) + 2 rescued * 750 (1500).
    assert_eq!(
        state.players[1].as_ref().unwrap().money,
        ct_before + REWARD_LOSER_BONUS_DEFAULT + 2 * REWARD_RESCUED_HOSTAGE
    );

    // Verify losing streak reset after two losses then a win.
    b.run(|game, eng| {
        game.damage_player(eng, 1, Some(0), 500.0, dmg::BULLET);
        game.restart_round_now(eng);
    });
    assert_eq!(
        b.server.game().game.state().rules.economy().consecutive_ct_loses(),
        2
    );
    assert_eq!(
        b.server.game().game.state().rules.economy().loser_bonus(),
        REWARD_LOSER_BONUS_DEFAULT + 500
    );
    b.run(|game, eng| {
        game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET);
        game.restart_round_now(eng);
    });
    assert_eq!(
        b.server.game().game.state().rules.economy().loser_bonus(),
        REWARD_LOSER_BONUS_MIN
    );
}

#[test]
fn give_c4_rotates() {
    let mut b = setup_match(bomb_map_entities(), &[0, 1, 2], &[3]);
    let holders: Vec<usize> = (0..4)
        .map(|_| {
            b.probe.clear_messages();
            b.run(|game, eng| game.restart_round_now(eng));
            b.step(10);
            assert!(b.messages().contains(&CsUserMsg::BombPickup));
            assert!(b.messages().contains(&CsUserMsg::ShowTimer));
            b.server
                .game()
                .game
                .state()
                .players
                .iter()
                .position(|p| p.as_ref().is_some_and(|p| p.has_c4))
                .expect("one terrorist carries C4")
        })
        .collect();
    assert_eq!(holders.len(), 4);
    assert_ne!(holders[0], holders[1]);
    assert_ne!(holders[1], holders[2]);
    assert_eq!(holders[0], holders[3]);

    // Mid-round T join (`get_into_game`) gives C4 if no bomber and no bomb exist on the map.
    let mut b2 = setup_match(bomb_map_entities(), &[0, 3], &[1]);
    let (before_join, after_join) = b2.run(|game, eng| {
        game.client_disconnect(eng, 0);
        let boxes: Vec<_> = eng
            .entity_ids()
            .into_iter()
            .filter(|&id| eng.ent(id).is_some_and(|v| v.classname == "weaponbox"))
            .collect();
        for id in boxes {
            eng.remove_entity(id);
        }
        let before = game.state().players.iter().flatten().any(|p| p.has_c4);
        // Put a living T in via the join flow on slot 2.
        game.client_put_in_server(eng, 2);
        eng.test_set_client_active(2, true);
        for _ in 0..2 {
            game.run_usercmd(eng, 2, &idle_cmd(), 0);
            game.post_usercmd(eng, 2, &idle_cmd());
        }
        let cmd_args = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
        game.client_command(eng, 2, &cmd_args("jointeam", "1"));
        game.client_command(eng, 2, &cmd_args("joinclass", "1"));
        game.run_usercmd(eng, 2, &idle_cmd(), 0);
        game.post_usercmd(eng, 2, &idle_cmd());
        let after = game.state().players.iter().flatten().any(|p| p.has_c4);
        (before, after)
    });
    assert!(
        !before_join && after_join,
        "mid-round joining T should trigger GiveC4 when no bomb/bomber exists"
    );
}

#[test]
fn round_rewards_on_a_bomb_map() {
    let mut b = setup_match(bomb_map_entities(), &[0], &[1]);

    // CT wins by elimination -> 3250 (`REWARD_BOMB_DEFUSED`) on bomb map.
    b.run(|game, eng| {
        game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET);
    });
    assert_eq!(b.server.game().game.state().rules.win_status(), WinStatus::Cts);
    assert_eq!(
        b.server.game().game.state().rules.economy().account_ct(),
        REWARD_BOMB_DEFUSED
    );
    let t0 = b.server.game().game.state().players[0].as_ref().unwrap().money;
    let ct0 = b.server.game().game.state().players[1].as_ref().unwrap().money;
    b.run(|game, eng| game.restart_round_now(eng));
    assert_eq!(
        b.server.game().game.state().players[1].as_ref().unwrap().money,
        ct0 + REWARD_BOMB_DEFUSED
    );
    assert_eq!(
        b.server.game().game.state().players[0].as_ref().unwrap().money,
        t0 + REWARD_LOSER_BONUS_DEFAULT
    );

    // T wins by elimination -> 3250 (`REWARD_BOMB_EXPLODED`) on bomb map.
    b.run(|game, eng| {
        game.damage_player(eng, 1, Some(0), 500.0, dmg::BULLET);
    });
    assert_eq!(
        b.server.game().game.state().rules.win_status(),
        WinStatus::Terrorists
    );
    assert_eq!(
        b.server
            .game()
            .game
            .state()
            .rules
            .economy()
            .account_terrorist(),
        REWARD_BOMB_EXPLODED
    );
    let t1 = b.server.game().game.state().players[0].as_ref().unwrap().money;
    let ct1 = b.server.game().game.state().players[1].as_ref().unwrap().money;
    b.run(|game, eng| game.restart_round_now(eng));
    assert_eq!(
        b.server.game().game.state().players[0].as_ref().unwrap().money,
        t1 + REWARD_BOMB_EXPLODED
    );
    assert_eq!(
        b.server.game().game.state().players[1].as_ref().unwrap().money,
        ct1 + REWARD_LOSER_BONUS_DEFAULT
    );
}
