//! The player: connecting, spawning, damage and death, the `kill` command, the fall damage and
//! the client data, on the shared bench of `common`. The tests that need the installed game
//! return without assertions when it is absent.

mod common;

use common::{Bench, discover, dust2, entity, spawn_at, stub_spawns};
use cs_engine::consts::{
    FL_CLIENT, FL_DORMANT, FL_ONGROUND, FL_SPECTATOR, MOVETYPE_NONE, MOVETYPE_TOSS, MOVETYPE_WALK,
    SOLID_NOT, SOLID_SLIDEBOX,
};
use cs_engine::{EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::player::{ArmorType, DEAD_DEAD, DEAD_DYING, DEAD_NO, Team, dmg, fall_damage};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::trace::Hull;
use glam::Vec3;

fn cmd(msec: u8) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        ..Default::default()
    }
}

#[test]
fn connected_player_waits_unassigned() {
    let mut b = Bench::stub(stub_spawns());
    let (player, v, moved) = b.run(|game, eng| {
        let id = eng.client_edict(0);
        let before = eng.ent(id).unwrap().origin;
        // A command of an unassigned player moves nothing; neither does a stray `kill`.
        let walk = GoldSrcUserCmd {
            msec: 10,
            forwardmove: 250.0,
            ..Default::default()
        };
        game.run_usercmd(eng, 0, &walk, 0);
        game.client_command(eng, 0, &["kill".to_string()]);
        game.client_command(eng, 0, &[]);
        let v = eng.ent(id).unwrap().clone();
        (
            game.state().players[0].clone(),
            v.clone(),
            v.origin != before,
        )
    });
    let p = player.expect("the player entered the game");
    assert_eq!(p.team, Team::Unassigned);
    assert!(!p.alive && !p.takes_damage);
    assert_eq!((p.frags, p.deaths), (0, 0));
    assert_eq!(p.money, 800);
    assert_eq!(v.movetype, MOVETYPE_NONE);
    assert_eq!(v.solid, SOLID_NOT);
    assert_eq!(v.modelindex, 0);
    assert_ne!(v.effects & 128, 0, "EF_NODRAW");
    assert_eq!(v.deadflag, DEAD_DEAD);
    assert_ne!(v.flags & FL_CLIENT, 0);
    assert_ne!(v.flags & FL_SPECTATOR, 0);
    assert!(!moved);
    // It stands at a counter-terrorist spawn point (the first, which the join used), looking
    // where the point looks, with the standing view offset.
    assert_eq!(v.origin, Vec3::new(100.0, 0.0, 1.0));
    assert_eq!((v.angles.y, v.v_angle), (90.0, Vec3::ZERO));
    assert_eq!(v.view_ofs.z, 17.0);
    // Nobody else is in the game.
    assert!(b.server.game().game.state().players[1].is_none());
}

#[test]
fn spawned_player_has_regamedll_defaults() {
    let mut b = Bench::stub(stub_spawns());
    let (ok, p, v, model) = b.run(|game, eng| {
        let ok = game.spawn_player(eng, 0, Team::Ct, 4);
        let id = eng.client_edict(0);
        (
            ok,
            game.state().players[0].clone().unwrap(),
            eng.ent(id).unwrap().clone(),
            eng.client_key(0, "model").map(str::to_string),
        )
    });
    assert!(ok);
    assert_eq!((p.team, p.class), (Team::Ct, 4));
    assert_eq!((p.health, p.armor, p.kevlar), (100.0, 0.0, ArmorType::None));
    assert!(p.alive && p.takes_damage);
    assert_eq!(model.as_deref(), Some("gign"));
    assert_eq!(v.health, 100.0);
    assert_eq!(v.deadflag, DEAD_NO);
    assert_eq!((v.solid, v.movetype), (SOLID_SLIDEBOX, MOVETYPE_WALK));
    // `ResetMaxSpeed`: nobody moves during the freeze period a map starts in (240 with no
    // active item once the round runs: `tests/rounds.rs`).
    assert_eq!(v.max_speed, 1.0);
    assert_eq!(v.fov, 90.0);
    assert_eq!(v.effects, 0);
    assert_ne!(v.modelindex, 0);
    assert_eq!(v.flags & FL_SPECTATOR, 0);
    assert_ne!(v.flags & FL_CLIENT, 0);
    assert_eq!(
        (v.mins, v.maxs),
        (Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0))
    );
    assert_eq!(v.view_ofs, Vec3::new(0.0, 0.0, 17.0));
    // `GetPlayerSpawnSpot`: one unit above the point, at its angles, view angles cleared. The
    // join took the first counter-terrorist point, so this is the next one.
    assert_eq!(v.origin, Vec3::new(300.0, 0.0, 1.0));
    assert_eq!(v.angles.y, 90.0);
    assert_eq!(v.v_angle, Vec3::ZERO);
    assert_eq!(v.fixangle, 1);
    assert_eq!(v.friction, 1.0);
    assert_eq!(v.gravity, 1.0);

    // A terrorist on the other team's points, with the model of its class; spawning again
    // keeps what the player earned.
    let (p, model, origin) = b.run(|game, eng| {
        game.state_mut().players[0].as_mut().unwrap().frags = 3;
        game.spawn_player(eng, 0, Team::Terrorist, 2);
        let id = eng.client_edict(0);
        (
            game.state().players[0].clone().unwrap(),
            eng.client_key(0, "model").map(str::to_string),
            eng.ent(id).unwrap().origin,
        )
    });
    assert_eq!(p.team, Team::Terrorist);
    assert_eq!(model.as_deref(), Some("leet"));
    assert_eq!(origin, Vec3::new(-100.0, 0.0, 1.0));
    assert_eq!(p.frags, 3);

    // A spectator, an unassigned player and a free slot do not spawn.
    let refused = b.run(|game, eng| {
        [
            game.spawn_player(eng, 0, Team::Spectator, 0),
            game.spawn_player(eng, 0, Team::Unassigned, 0),
            game.spawn_player(eng, 1, Team::Ct, 0),
        ]
    });
    assert_eq!(refused, [false; 3]);
}

#[test]
fn fall_damage_as_regamedll() {
    // The formula: nothing up to the safe speed, 100 per 600 units above it, times 1.25.
    assert_eq!(fall_damage(500.0), 0.0);
    assert!((fall_damage(800.0) - 62.5).abs() < 1e-4);
    assert!((fall_damage(1100.0) - 125.0).abs() < 1e-3);

    // On a real map: a drop past the safe speed costs `FlPlayerFallDamage` of the speed the
    // last command ended with, a short one nothing.
    let Some(vfs) = discover() else {
        return;
    };
    let mut b = Bench::on(dust2(&vfs));
    // The rooms of the map are low, so the fall speed comes from the gravity as much as from
    // the height.
    let drop = |height: f32, gravity: f32| {
        move |game: &mut CsGame, eng: &mut EngineApi<'_>| {
            let id = eng.client_edict(0);
            // The spawn points in turn, until one has `height` of free air above the floor.
            let mut top = None;
            let mut settled = (false, 0.0);
            for attempt in 0..40 {
                let team = if attempt % 2 == 0 {
                    Team::Ct
                } else {
                    Team::Terrorist
                };
                game.spawn_player(eng, 0, team, 0);
                // Settle on the floor first: the spawn point is one unit above it.
                for _ in 0..50 {
                    game.run_usercmd(eng, 0, &cmd(10), 0);
                }
                settled = (
                    eng.ent(id).unwrap().flags & FL_ONGROUND != 0,
                    game.state().players[0].as_ref().unwrap().health,
                );
                let floor = eng.ent(id).unwrap().origin;
                let reach =
                    eng.trace_hull(floor, floor + Vec3::new(0.0, 0.0, height), Hull::Standing);
                if reach.fraction >= 1.0 {
                    top = Some(reach.end_pos);
                    break;
                }
            }
            let top = top.expect("a spawn point with free air above it");
            game.place_player(eng, 0, top);
            let saved = game.state().move_params.gravity;
            game.state_mut().move_params.gravity = gravity;
            let mut last_vz = 0.0;
            let mut landed = false;
            for _ in 0..1000 {
                game.run_usercmd(eng, 0, &cmd(10), 0);
                let v = eng.ent(id).unwrap();
                if v.flags & FL_ONGROUND != 0 {
                    landed = true;
                    break;
                }
                last_vz = v.velocity.z;
            }
            game.state_mut().move_params.gravity = saved;
            (
                settled,
                landed,
                last_vz,
                game.state().players[0].as_ref().unwrap().health,
            )
        }
    };
    let (settled, landed, last_vz, health) = b.run(drop(140.0, 2400.0));
    assert_eq!(settled, (true, 100.0), "a spawn costs no health");
    assert!(landed);
    let fall = -last_vz;
    assert!(fall > 520.0 && fall < 900.0, "fell at {fall}");
    let damage = fall_damage(fall);
    assert!(damage > 1.0 && damage < 90.0, "damage {damage}");
    assert_eq!(health, 100.0 - damage.trunc());

    let (_, landed, last_vz, health) = b.run(drop(100.0, 800.0));
    assert!(landed);
    assert!(-last_vz < 500.0);
    assert_eq!(health, 100.0, "a short fall costs nothing");
}

#[test]
fn kill_command_kills_and_costs_a_frag() {
    let mut b = Bench::stub(stub_spawns());
    b.run(|game, eng| game.spawn_player(eng, 0, Team::Ct, 0));
    let kill = |game: &mut CsGame, eng: &mut EngineApi<'_>| {
        game.client_command(eng, 0, &["kill".to_string()]);
        let id = eng.client_edict(0);
        (
            game.state().players[0].clone().unwrap(),
            eng.ent(id).unwrap().clone(),
        )
    };
    let (p, v) = b.run(kill);
    assert!(!p.alive && !p.takes_damage);
    assert_eq!((p.frags, p.deaths), (-1, 1));
    assert!(p.health <= 0.0);
    assert_eq!(
        (v.deadflag, v.movetype, v.solid),
        (DEAD_DYING, MOVETYPE_TOSS, SOLID_NOT)
    );
    assert_eq!(v.health, p.health);

    // Dead players do not suicide again, whatever the case of the command.
    b.run(|game, eng| game.client_command(eng, 0, &["KILL".to_string()]));
    let (p, _) = b.run(kill);
    assert_eq!((p.frags, p.deaths), (-1, 1));

    // The scoreboard and the death notice reached the client.
    b.step(5);
    let messages = b.messages();
    assert!(messages.iter().any(|m| matches!(
        m,
        CsUserMsg::DeathMsg { killer_id: 1, victim_id: 1, weapon, .. } if weapon == "world"
    )));
    assert!(messages.iter().any(|m| matches!(
        m,
        CsUserMsg::ScoreInfo {
            player_id: 1,
            frags: -1,
            deaths: 1,
            team_id: 2,
            ..
        }
    )));

    // Alive again, a `kill` within a second of the last suicide is refused; a second later
    // it works.
    b.run(|game, eng| game.respawn_player(eng, 0));
    let (p, _) = b.run(kill);
    assert!(p.alive);
    assert_eq!((p.frags, p.deaths), (-1, 1));
    b.step(110);
    let (p, _) = b.run(kill);
    assert!(!p.alive);
    assert_eq!((p.frags, p.deaths), (-2, 2));
}

#[test]
fn damage_follows_the_rules() {
    let mut b = Bench::stub(stub_spawns());
    b.enter_second_client();
    b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.spawn_player(eng, 1, Team::Terrorist, 0);
    });
    let hurt = |slot: u8, attacker: Option<u8>, damage: f32, bits: u32| {
        move |game: &mut CsGame, eng: &mut EngineApi<'_>| {
            let taken = game.damage_player(eng, slot, attacker, damage, bits);
            (
                taken,
                game.state().players[usize::from(slot)].clone().unwrap(),
            )
        }
    };
    // An enemy's bullet.
    let (taken, p) = b.run(hurt(0, Some(1), 25.0, dmg::BULLET));
    assert!(taken);
    assert_eq!(p.health, 75.0);
    // The damage is cut to an integer.
    let (_, p) = b.run(hurt(0, Some(1), 4.9, dmg::BULLET));
    assert_eq!(p.health, 71.0);

    // Armor takes half of the damage off the health at a cost of half of that.
    b.run(|game, _| {
        let p = game.state_mut().players[0].as_mut().unwrap();
        p.armor = 100.0;
        p.kevlar = ArmorType::Vest;
        p.health = 100.0;
    });
    let (_, p) = b.run(hurt(0, Some(1), 20.0, dmg::BULLET));
    assert_eq!((p.health, p.armor), (90.0, 95.0));
    // Not against a fall.
    let (_, p) = b.run(hurt(0, None, 10.0, dmg::FALL));
    assert_eq!((p.health, p.armor), (80.0, 95.0));
    // Armor that runs out takes the rest of the damage in health.
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().armor = 2.0);
    let (_, p) = b.run(hurt(0, Some(1), 40.0, dmg::BULLET));
    assert_eq!(p.armor, 0.0);
    assert_eq!(p.kevlar, ArmorType::None);
    assert_eq!(p.health, 80.0 - (40.0 - 2.0 * 2.0));

    // Your own bullets hurt less (a teammate's rule), your fall does not.
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().health = 100.0);
    let (_, p) = b.run(hurt(0, Some(0), 100.0, dmg::BULLET));
    assert_eq!(p.health, 65.0);
    let (_, p) = b.run(hurt(0, Some(0), 10.0, dmg::FALL));
    assert_eq!(p.health, 55.0);

    // A teammate's damage needs friendly fire.
    b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.spawn_player(eng, 1, Team::Ct, 0);
    });
    let (taken, p) = b.run(hurt(0, Some(1), 50.0, dmg::BULLET));
    assert!(!taken);
    assert_eq!(p.health, 100.0);
    b.run(|_, eng| eng.set_cvar("mp_friendlyfire", "1"));
    let (taken, p) = b.run(hurt(0, Some(1), 50.0, dmg::BULLET));
    assert!(taken);
    assert_eq!(p.health, 100.0 - 17.0);
}

#[test]
fn kills_score_by_who_killed() {
    let mut b = Bench::stub(stub_spawns());
    b.enter_second_client();
    let frags = |b: &mut Bench| {
        b.run(|game, _| {
            let s = game.state();
            let at = |i: usize| {
                let p = s.players[i].as_ref().unwrap();
                (p.frags, p.deaths, p.alive)
            };
            (at(0), at(1))
        })
    };
    b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.spawn_player(eng, 1, Team::Terrorist, 0);
    });
    // Killing an enemy is a frag for the killer and a death for the victim.
    b.run(|game, eng| game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET));
    assert_eq!(frags(&mut b), ((0, 1, false), (1, 0, true)));
    // A dead player takes no more damage.
    let again = b.run(|game, eng| game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET));
    assert!(!again);
    assert_eq!(frags(&mut b), ((0, 1, false), (1, 0, true)));
    // The world kills without a frag lost or gained, and the death notice names it.
    b.run(|game, eng| game.damage_player(eng, 1, None, 500.0, dmg::CRUSH));
    assert_eq!(frags(&mut b), ((0, 1, false), (1, 1, false)));
    b.step(5);
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::DeathMsg { killer_id: 0, victim_id: 2, weapon, .. } if weapon == "worldspawn"
    )));
    // A teammate's kill costs the killer a frag.
    b.run(|game, eng| {
        eng.set_cvar("mp_friendlyfire", "1");
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.spawn_player(eng, 1, Team::Ct, 0);
        game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET)
    });
    assert_eq!(frags(&mut b), ((0, 2, false), (0, 1, true)));
}

#[test]
fn a_full_spawn_kills_its_occupants() {
    // Two points 30 apart: a player at the first leaves neither free for the next spawn.
    let mut b = Bench::stub(vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "100 0 0"),
        spawn_at("info_player_start", "130 0 0"),
    ]);
    b.enter_second_client();
    b.run(|game, eng| game.spawn_player(eng, 1, Team::Ct, 0));
    let (alive, victim, origin) = b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        let id = eng.client_edict(0);
        let p = game.state().players[1].clone().unwrap();
        (
            game.state().players[0].as_ref().unwrap().alive,
            p,
            eng.ent(id).unwrap().origin,
        )
    });
    assert!(alive);
    assert!(!victim.alive && victim.health <= 0.0);
    // The world killed it: no frag changes hands, the death counts.
    assert_eq!((victim.frags, victim.deaths), (0, 1));
    assert_eq!(origin.x, 130.0);
}

#[test]
fn a_dying_player_ends_up_dead_and_can_respawn() {
    let mut b = Bench::stub(stub_spawns());
    b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Terrorist, 0);
        game.client_command(eng, 0, &["kill".to_string()]);
    });
    let mut frames = 0;
    let deadflag = loop {
        let flag = b.run(|game, eng| {
            game.run_usercmd(eng, 0, &cmd(10), 0);
            eng.ent(eng.client_edict(0)).unwrap().deadflag
        });
        frames += 1;
        if flag == DEAD_DEAD || frames >= 600 {
            break flag;
        }
    };
    // In the empty stub world the body never lands: the time limit ends the dying.
    assert_eq!(deadflag, DEAD_DEAD);

    let (p, v) = b.run(|game, eng| {
        let ok = game.respawn_player(eng, 0);
        let id = eng.client_edict(0);
        assert!(ok);
        (
            game.state().players[0].clone().unwrap(),
            eng.ent(id).unwrap().clone(),
        )
    });
    assert!(p.alive && p.takes_damage);
    assert_eq!((p.health, p.deaths, p.frags), (100.0, 1, -1));
    assert_eq!(
        (v.deadflag, v.solid, v.movetype),
        (DEAD_NO, SOLID_SLIDEBOX, MOVETYPE_WALK)
    );
    assert_eq!(v.health, 100.0);
}

#[test]
fn client_data_reflects_the_player() {
    let mut b = Bench::stub(stub_spawns());
    b.run(|game, eng| game.spawn_player(eng, 0, Team::Ct, 0));
    let data = b.run(|game, eng| game.update_client_data(eng, 0));
    assert_eq!(data.health, 100.0);
    // The map starts in its freeze period.
    assert_eq!(data.maxspeed, 1.0);
    assert_eq!(data.fov, 90.0);
    assert_eq!(data.deadflag, DEAD_NO);
    assert_eq!(data.view_ofs.z, 17.0);
    assert_eq!(data.origin, Vec3::new(300.0, 0.0, 1.0));
    assert_eq!(data.waterlevel, 0);
    // The suit bit `PlayerSpawn` sets, nothing else yet.
    assert_eq!(data.weapons, 1 << 31);
    assert_eq!(
        (data.b_in_duck, data.fl_duck_time, data.weaponanim),
        (0, 0, 0)
    );
    assert_ne!(data.flags & FL_CLIENT, 0);

    // Hurt: the health follows, and the client is told.
    let data = b.run(|game, eng| {
        game.damage_player(eng, 0, Some(1), 30.0, dmg::BULLET);
        game.update_client_data(eng, 0)
    });
    assert_eq!(data.health, 70.0);
    b.step(5);
    let health: Vec<u8> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::Health(h) => Some(h),
            _ => None,
        })
        .collect();
    assert_eq!(health.last(), Some(&70));

    // Dead: the dead flag, and the client was told zero.
    let data = b.run(|game, eng| {
        game.client_command(eng, 0, &["kill".to_string()]);
        game.update_client_data(eng, 0)
    });
    assert_eq!(data.deadflag, DEAD_DYING);
    assert!(data.health <= 0.0);
    // `SendFOV(0)` on death.
    assert_eq!(data.fov, 0.0);
    b.step(5);
    let health: Vec<u8> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::Health(h) => Some(h),
            _ => None,
        })
        .collect();
    assert_eq!(health.last(), Some(&0));

    // A slot nobody plays in gives empty data.
    let empty = b.run(|game, eng| game.update_client_data(eng, 1));
    assert_eq!(empty.health, 0.0);
}

#[test]
fn a_disconnected_player_leaves_the_game() {
    let mut b = Bench::stub(stub_spawns());
    let (gone, flags) = b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.client_disconnect(eng, 0);
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (game.state().players[0].is_none(), (v.flags, v.solid))
    });
    assert!(gone);
    assert_eq!(flags, (FL_DORMANT, SOLID_NOT));
    // Commands of the empty slot are ignored.
    b.run(|game, eng| {
        game.run_usercmd(eng, 0, &cmd(10), 0);
        game.client_command(eng, 0, &["kill".to_string()]);
    });
}

#[test]
fn a_spawner_does_not_telefrag_itself_or_the_dead() {
    // One point only, so it is always the one the spawner picks.
    let mut b = Bench::stub(vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "100 0 0"),
    ]);
    b.enter_second_client();
    // The occupant is the spawner itself: respawning where it stands kills nobody.
    let (p, deaths) = b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.respawn_player(eng, 0);
        (
            game.state().players[0].clone().unwrap(),
            game.state().players[0].as_ref().unwrap().deaths,
        )
    });
    assert!(p.alive);
    assert_eq!((p.health, deaths), (100.0, 0));

    // The occupant is already dead: the point still counts as taken, the dead player is not
    // killed again (no second death, no frag change) and the spawner lives.
    let (spawner, dead) = b.run(|game, eng| {
        game.spawn_player(eng, 1, Team::Ct, 0);
        game.damage_player(eng, 1, None, 500.0, dmg::CRUSH);
        game.spawn_player(eng, 0, Team::Ct, 0);
        (
            game.state().players[0].clone().unwrap(),
            game.state().players[1].clone().unwrap(),
        )
    });
    assert!(spawner.alive);
    assert!(!dead.alive);
    assert_eq!((dead.deaths, dead.frags), (1, 0));
}

#[test]
fn a_dead_player_leaves_a_corpse() {
    let mut b = Bench::stub(stub_spawns());
    // A teammate stays alive, or the death would end the round and the next round would bring
    // the dead player back.
    b.enter_second_client();
    let spawn_x = b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 4);
        game.spawn_player(eng, 1, Team::Ct, 0);
        game.client_command(eng, 0, &["kill".to_string()]);
        eng.ent(eng.client_edict(0)).unwrap().origin.x
    });
    let corpses = |b: &Bench| {
        b.messages()
            .into_iter()
            .filter(|m| matches!(m, CsUserMsg::ClCorpse { .. }))
            .collect::<Vec<_>>()
    };
    // No corpse while the player is dying or has just died.
    for _ in 0..50 {
        b.run(|game, eng| game.run_usercmd(eng, 0, &cmd(10), 0));
    }
    assert!(corpses(&b).is_empty());
    // It is sent once, three seconds after the death was over.
    for _ in 0..800 {
        b.run(|game, eng| game.run_usercmd(eng, 0, &cmd(10), 0));
    }
    b.step(5);
    let corpses = corpses(&b);
    assert_eq!(corpses.len(), 1);
    let CsUserMsg::ClCorpse {
        model,
        team,
        player_id,
        origin,
        ..
    } = &corpses[0]
    else {
        unreachable!()
    };
    assert_eq!((model.as_str(), *team, *player_id), ("gign", 2, 1));
    // The body is where the player died: `origin` is the position times 128.
    assert_eq!(origin[0], (spawn_x * 128.0) as i32);
}
