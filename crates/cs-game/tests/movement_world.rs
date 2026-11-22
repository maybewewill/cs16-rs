//! Movement on the world: the hard-locked movement cvars, drowning, the observer on `pm`, the
//! footsteps other players hear, and ladders and water on stock maps. The real-map tests return
//! without assertions when the game is not installed.

mod common;

use common::{Bench, command, discover, find_map_with_entity, map_named, spawns};
use cs_engine::consts::FL_ONGROUND;
use cs_engine::{EngineApi, EntVars};
use cs_game::observer::OBS_ROAMING;
use cs_game::player::Team;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_sim::trace::Hull;
use glam::Vec3;

const IN_JUMP: u16 = 1 << 1;
const IN_FORWARD: u16 = 1 << 3;
const MOVETYPE_FLY: u8 = 5;
const CONTENTS_WATER: i32 = -3;

fn cmd(msec: u8, buttons: u16, forwardmove: f32, yaw: f32) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        buttons,
        forwardmove,
        viewangles: Vec3::new(0.0, yaw, 0.0),
        ..Default::default()
    }
}

fn vars(eng: &EngineApi<'_>, slot: u8) -> EntVars {
    eng.ent(eng.client_edict(slot)).unwrap().clone()
}

fn cvar(eng: &EngineApi<'_>, name: &str) -> f32 {
    eng.cvar_f32(name)
}

/// A stub-map bench whose probe is spawned as a counter-terrorist and does not fall.
fn stub_bench() -> Bench {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
    });
    b
}

#[test]
fn movement_cvars_are_hard_locked() {
    // `Think` sets them back once a second, and only when they differ.
    let mut b = Bench::stub(spawns(2, 2));
    b.run(|_, eng| {
        eng.set_cvar("sv_accelerate", "10");
        eng.set_cvar("sv_friction", "2");
        eng.set_cvar("sv_stopspeed", "100");
    });
    b.step(150);
    b.run(|_, eng| {
        assert_eq!(cvar(eng, "sv_accelerate"), 5.0);
        assert_eq!(cvar(eng, "sv_friction"), 4.0);
        assert_eq!(cvar(eng, "sv_stopspeed"), 75.0);
        // The movevars the movement reads are the locked ones.
        let mv = eng.move_vars();
        assert_eq!((mv.accelerate, mv.friction, mv.stopspeed), (5.0, 4.0, 75.0));
    });

    // `RestartRound` sets them whatever they are.
    b.run(|game, eng| {
        eng.set_cvar("sv_accelerate", "10");
        eng.set_cvar("sv_friction", "2");
        eng.set_cvar("sv_stopspeed", "100");
        game.restart_round_now(eng);
        assert_eq!(cvar(eng, "sv_accelerate"), 5.0);
        assert_eq!(cvar(eng, "sv_friction"), 4.0);
        assert_eq!(cvar(eng, "sv_stopspeed"), 75.0);
    });
}

#[test]
fn drowning_as_regamedll() {
    // `WaterMove` reads the waterlevel the previous command's movement left: the stub map has no
    // water, so the test keeps the edict's waterlevel at 3, as the movement would.
    let mut b = stub_bench();
    let air_finished = b.run(|_, eng| vars(eng, 0).air_finished);
    assert!(air_finished > 11.0, "AIRTIME is 12 s: {air_finished}");

    let mut health_log: Vec<(f32, f32)> = Vec::new();
    let mut last = 100.0;
    for _ in 0..1800 {
        let (time, health) = b.run(|game, eng| {
            let id = eng.client_edict(0);
            let v = eng.ent_mut(id).unwrap();
            v.waterlevel = 3;
            v.watertype = CONTENTS_WATER;
            common::run_cmd(game, eng, 0, &cmd(10, 0, 0.0, 0.0), 0);
            (eng.time() as f32, vars(eng, 0).health)
        });
        if health != last {
            health_log.push((time, health));
            last = health;
        }
        if health <= 83.0 {
            break;
        }
    }
    // No damage before the air is gone; then `dmg` starts at 2, grows by 1 a pain up to 5, and
    // a pain comes once a second (`pain_finished = time + 1`).
    let healths: Vec<f32> = health_log.iter().map(|&(_, h)| h).collect();
    assert_eq!(healths, [97.0, 93.0, 88.0, 83.0], "{health_log:?}");
    let first = health_log[0].0;
    assert!(
        first > air_finished && first < air_finished + 0.1,
        "first pain at {first}, the air ran out at {air_finished}"
    );
    for pair in health_log.windows(2) {
        let gap = pair[1].0 - pair[0].0;
        assert!((0.99..=1.03).contains(&gap), "pains {gap} s apart: {health_log:?}");
    }

    // Out of the water: the drowning damage (3 + 4 + 5 + 5 = 17) is given back, up to 10 points
    // every two seconds (`CheckTimeBasedDamage`, `DMG_DROWNRECOVER`).
    let mut seen = Vec::new();
    for _ in 0..800 {
        let health = b.run(|game, eng| {
            let id = eng.client_edict(0);
            let v = eng.ent_mut(id).unwrap();
            v.waterlevel = 0;
            v.watertype = -1;
            common::run_cmd(game, eng, 0, &cmd(10, 0, 0.0, 0.0), 0);
            vars(eng, 0).health
        });
        if seen.last() != Some(&health) {
            seen.push(health);
        }
    }
    assert_eq!(seen, [83.0, 93.0, 100.0], "recovery steps");
}

#[test]
fn observer_roams_and_chases_through_pm() {
    let mut b = Bench::stub_slots(spawns(3, 3), 2);
    b.run(|_, eng| eng.set_cvar("sv_spectatormaxspeed", "100"));
    command(&mut b, 0, "jointeam 6");
    let v = b.run(|_, eng| vars(eng, 0));
    assert_eq!(v.iuser1, OBS_ROAMING);
    let before = v.origin;

    // The roaming speed cap is the movevar now (it was a constant 500).
    for _ in 0..200 {
        b.run(|game, eng| common::run_cmd(game, eng, 0, &cmd(10, 0, 400.0, 0.0), 0));
    }
    let flying = b.run(|_, eng| vars(eng, 0));
    assert!(flying.origin.x > before.x + 30.0, "{:?}", flying.origin);
    assert!(flying.velocity.length() <= 100.0 + 0.01, "{:?}", flying.velocity);
    assert!(flying.velocity.length() > 80.0, "{:?}", flying.velocity);

    // Chase: a player to follow, far from the observer (past the 256 units the movement's
    // physents reach), so `SetupVisibility` puts the observer where the target is.
    common::enter(&mut b, 1);
    common::join(&mut b, 1, 1, 1);
    b.run(|game, eng| {
        assert!(game.place_player(eng, 1, Vec3::new(-900.0, 600.0, 0.0)));
    });
    for _ in 0..40 {
        b.run(|game, eng| common::run_cmd(game, eng, 0, &cmd(10, IN_JUMP, 0.0, 0.0), 0));
        b.run(|game, eng| common::run_cmd(game, eng, 0, &cmd(10, 0, 0.0, 0.0), 0));
        let v = b.run(|_, eng| vars(eng, 0));
        if v.iuser1 != OBS_ROAMING {
            break;
        }
    }
    let (me, target) = b.run(|_, eng| (vars(eng, 0), vars(eng, 1)));
    assert_ne!(me.iuser1, OBS_ROAMING, "jumping leaves roaming");
    assert_ne!(me.iuser2, 0, "has a target");
    b.step(3);
    let (me, target2) = b.run(|_, eng| (vars(eng, 0), vars(eng, 1)));
    assert_eq!(target.origin, target2.origin);
    assert_eq!(me.origin, target2.origin);
    assert_eq!(me.velocity, Vec3::ZERO, "a chasing observer has no velocity");
}

/// The footstep sample indices, by name, as the server precached them.
fn step_sample_indices(eng: &EngineApi<'_>) -> Vec<u16> {
    let mut v = Vec::new();
    for (family, n) in [("step", 4), ("metal", 4), ("dirt", 4), ("duct", 4), ("grate", 4), ("tile", 5), ("snow", 4)] {
        for i in 1..=n {
            if let Some(index) = eng.sound_index(&format!("player/pl_{family}{i}.wav")) {
                v.push(index);
            }
        }
    }
    v
}

#[test]
fn footsteps_reach_others_not_self() {
    let Some(vfs) = discover() else {
        return;
    };
    let mut b = Bench::on(common::dust2(&vfs));
    let mut other = b.add_probe("other");
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Ct, 0));
    });
    // Settle both on the floor, the other one next to the runner.
    let indices = b.run(|game, eng| {
        for _ in 0..60 {
            common::run_cmd(game, eng, 0, &cmd(10, 0, 0.0, 0.0), 0);
            common::run_cmd(game, eng, 1, &cmd(10, 0, 0.0, 0.0), 0);
        }
        let start = vars(eng, 0).origin;
        eng.ent_mut(eng.client_edict(0)).unwrap().max_speed = 250.0;
        // The other player stands 40 units off the runner, out of its way.
        let spot = (0..16)
            .map(|i| i as f32 * 22.5f32)
            .map(|yaw| Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0))
            .find(|dir| eng.trace_hull(start, start + *dir * 60.0, Hull::Standing).fraction >= 1.0)
            .map_or(start, |dir| start + dir * 40.0);
        game.place_player(eng, 1, spot);
        step_sample_indices(eng)
    });
    assert!(!indices.is_empty(), "the step samples are precached");
    b.probe.clear_messages();
    other.clear_messages();

    // The runner runs along a direction with room, two seconds.
    b.run(move |game, eng| {
        let start = vars(eng, 0).origin;
        let yaw = (0..16)
            .map(|i| i as f32 * 22.5)
            .find(|&yaw| {
                let dir = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
                eng.trace_hull(start, start + dir * 300.0, Hull::Standing).fraction >= 1.0
            })
            .expect("room to run");
        for _ in 0..120 {
            common::run_cmd(game, eng, 0, &cmd(10, IN_FORWARD, 400.0, yaw), 0);
        }
        assert!(vars(eng, 0).velocity.truncate().length() > 150.0);
        assert_ne!(vars(eng, 0).flags & FL_ONGROUND, 0);
    });
    b.step_both(&mut other, 10);

    let steps_heard = |msgs: &std::collections::VecDeque<SvcMessage>| {
        msgs.iter()
            .filter(|m| matches!(m, SvcMessage::Sound(s) if s.entity == 1 && indices.contains(&s.sound_index)))
            .count()
    };
    assert!(steps_heard(other.messages()) > 0, "the other player hears the runner's steps");
    assert_eq!(steps_heard(b.probe.messages()), 0, "the runner does not hear its own steps");
}

/// Teleports the player of slot 0 to `origin` and runs `n` commands of `c`, returning what each
/// left (movetype, origin, velocity, waterlevel, watertype).
fn drive(
    b: &mut Bench,
    origin: Vec3,
    n: usize,
    c: GoldSrcUserCmd,
) -> Vec<(u8, Vec3, Vec3, i32, i32)> {
    b.run(move |game, eng| {
        game.place_player(eng, 0, origin);
        // The weapons set the speed; the movement takes the entity's `maxspeed`.
        eng.ent_mut(eng.client_edict(0)).unwrap().max_speed = 250.0;
        let mut out = Vec::new();
        for _ in 0..n {
            common::run_cmd(game, eng, 0, &c, 0);
            let v = vars(eng, 0);
            out.push((v.movetype, v.origin, v.velocity, v.waterlevel, v.watertype));
        }
        out
    })
}

#[test]
fn a_ladder_is_climbed_on_a_real_map() {
    let Some(vfs) = discover() else {
        return;
    };
    let Some(name) = find_map_with_entity(&vfs, "func_ladder") else {
        return;
    };
    let mut b = Bench::on(map_named(&vfs, &name));
    b.run(|game, eng| assert!(game.spawn_player(eng, 0, Team::Ct, 0)));
    let ladders: Vec<(Vec3, Vec3)> = b.run(|_, eng| {
        let mut out = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "func_ladder") {
            after = Some(id);
            let v = eng.ent(id).unwrap();
            out.push((v.absmin, v.absmax));
        }
        out
    });
    assert!(!ladders.is_empty(), "{name} has ladders");

    // Stand at each side of each ladder, facing it, and walk into it: the movement turns the
    // player into a climber (`MOVETYPE_FLY`) that rises.
    let mut climbed = None;
    'ladders: for (min, max) in ladders {
        let centre = (min + max) * 0.5;
        let half = (max - min) * 0.5;
        for (dir, yaw) in [
            (Vec3::X, 180.0),
            (-Vec3::X, 0.0),
            (Vec3::Y, 270.0),
            (-Vec3::Y, 90.0),
        ] {
            let reach = if dir.x != 0.0 { half.x } else { half.y };
            let origin = Vec3::new(centre.x, centre.y, min.z + 40.0) + dir * (reach + 12.0);
            let solid = b.run(move |_, eng| {
                let t = eng.trace_hull(origin, origin, Hull::Standing);
                t.start_solid || t.all_solid
            });
            if solid {
                continue;
            }
            let run = drive(&mut b, origin, 60, cmd(10, IN_FORWARD, 200.0, yaw));
            let on_ladder = run.iter().any(|r| r.0 == MOVETYPE_FLY);
            let rise = run.last().unwrap().1.z - origin.z;
            if on_ladder && rise > 30.0 {
                climbed = Some((name.clone(), rise));
                break 'ladders;
            }
        }
    }
    let (map, rise) = climbed.expect("a ladder on the map is climbed");
    assert!(rise > 30.0, "{map}: rose {rise}");
}

#[test]
fn water_is_swum_on_a_real_map() {
    let Some(vfs) = discover() else {
        return;
    };
    // Maps with water: the world's water textures (`!name`) are in their texture list; the
    // first of them that has a deep pool the scan finds is used.
    let candidates: Vec<String> = vfs
        .list_maps()
        .into_iter()
        .filter(|name| {
            cs_assets::bsp::BspMap::load(&vfs, name)
                .is_ok_and(|bsp| bsp.textures.iter().any(|t| t.name.starts_with('!')))
        })
        .collect();
    let mut found = None;
    for name in candidates {
        let bounds = {
            let bsp = cs_assets::bsp::BspMap::load(&vfs, &name).expect("load");
            let m = bsp.models[0];
            (m.mins, m.maxs)
        };
        let mut b = Bench::on(map_named(&vfs, &name));
        b.run(|game, eng| assert!(game.spawn_player(eng, 0, Team::Ct, 0)));
        // Points in the water with water over the head too (a coarse scan of the world's box).
        let spots: Vec<Vec3> = b.run(move |_, eng| {
            let mut out = Vec::new();
            let step = 32.0;
            let mut x = bounds.0.x + 8.0;
            while x < bounds.1.x {
                let mut y = bounds.0.y + 8.0;
                while y < bounds.1.y {
                    let mut z = bounds.0.z + 8.0;
                    while z < bounds.1.z {
                        let p = Vec3::new(x, y, z);
                        let wet = |q: Vec3| eng.point_contents(q, Hull::Point) == CONTENTS_WATER;
                        if wet(p)
                            && wet(p + Vec3::Z * 36.0)
                            && wet(p - Vec3::Z * 36.0)
                            && !eng.trace_hull(p, p, Hull::Standing).start_solid
                        {
                            out.push(p);
                        }
                        z += step;
                    }
                    y += step;
                }
                x += step;
            }
            out
        });
        if !spots.is_empty() {
            found = Some((name, b, spots));
            break;
        }
    }
    let Some((name, mut b, spots)) = found else {
        panic!("no stock map has a deep pool the scan can find");
    };

    let mut swum = false;
    for spot in spots.into_iter().take(40) {
        let idle = drive(&mut b, spot, 100, cmd(10, 0, 0.0, 0.0));
        let last = idle.last().unwrap();
        if last.3 < 2 {
            continue;
        }
        // In water: the type is water, the player sinks slowly instead of falling
        // (`PM_WaterMove` sinks at 60), never at gravity's pace.
        assert_eq!(last.4, CONTENTS_WATER, "{name}");
        assert!(idle.iter().all(|r| r.2.z > -120.0), "sank like a stone");
        // Swimming forward moves it; jumping pushes up at 100 (water).
        let swim = drive(&mut b, spot, 60, cmd(10, IN_FORWARD, 250.0, 0.0));
        let moved = (swim.last().unwrap().1 - spot).truncate().length();
        assert!(moved > 20.0 || swim.iter().any(|r| r.3 < 2), "swam {moved}");
        let up = drive(&mut b, spot, 20, cmd(10, IN_JUMP, 0.0, 0.0));
        assert!(
            up.iter().any(|r| r.2.z > 40.0) || up.last().unwrap().1.z > spot.z + 10.0,
            "jumping rises in water"
        );
        swum = true;
        break;
    }
    assert!(swum, "{name}: the player was in water (waterlevel 2 or more) somewhere");
}
