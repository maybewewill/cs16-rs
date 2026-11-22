//! Movement on the world: the hard-locked movement cvars, drowning, the observer on `pm`, the
//! footsteps other players hear, and ladders and water on stock maps. The real-map tests return
//! without assertions when the game is not installed.

mod common;

use common::{Bench, command, discover, find_map_with_entity, map_named, spawns};
use cs_engine::consts::FL_ONGROUND;
use cs_engine::{EngineApi, EntVars, GameDll};
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

/// `cmd` looking `pitch` degrees down (positive) along its yaw.
fn look(pitch: f32, c: GoldSrcUserCmd) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        viewangles: Vec3::new(pitch, c.viewangles.y, 0.0),
        ..c
    }
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
                climbed = Some((origin, yaw));
                break 'ladders;
            }
        }
    }
    let (origin, yaw) = climbed.expect("a ladder on the map is climbed");

    // `PM_LadderMove`: the climb speed is `min(MAX_CLIMB_SPEED 200, maxspeed 250)`. Facing the
    // ladder level with IN_FORWARD, `velocity = forward * 200` is all into the face; the
    // decomposition turns it into 200 along the ladder's vertical (`normal x perp` = +z), with
    // nothing lateral. Allow 0.5 for `cos`/`sin` of the yaw in single precision.
    let up = drive(&mut b, origin, 10, cmd(10, IN_FORWARD, 200.0, yaw));
    let first = up.iter().find(|r| r.0 == MOVETYPE_FLY).expect("on the ladder");
    assert!((first.2.z - 200.0).abs() < 0.5, "climb speed {:?}", first.2);
    assert!(first.2.truncate().length() < 0.5, "lateral {:?}", first.2);
    assert!(up.last().unwrap().1.z > origin.z + 10.0, "climbs");

    // Looking `pitch` down, the same command is `forward = 200 * (cos, -sin)` of the view:
    // into the face 200 cos(pitch), vertical -200 sin(pitch); the conversion adds
    // +200 cos(pitch) up, so z = 200 (cos - sin): at 60 degrees -73.2, a descent.
    let down = drive(&mut b, origin, 10, look(60.0, cmd(10, IN_FORWARD, 200.0, yaw)));
    let first = down.iter().find(|r| r.0 == MOVETYPE_FLY).expect("on the ladder");
    let expected = 200.0 * (60.0f32.to_radians().cos() - 60.0f32.to_radians().sin());
    assert!((first.2.z - expected).abs() < 1.0, "{:?} expected z {expected}", first.2);
    assert!(first.2.z < -60.0);
    assert!(down.last().unwrap().1.z < origin.z, "descends");
}

/// The first stock map (`preferred` first, then in name order) whose world has a pool deep
/// enough to swim in, with the points the scan found: water at the point and 36 units above and
/// below it, and 150 units on along +x, so a short swim stays in the water.
fn find_pool(vfs: &cs_vfs::Vfs, preferred: &str) -> Option<(String, Bench, Vec<Vec3>)> {
    let mut names = vec![preferred.to_string()];
    names.extend(vfs.list_maps());
    for name in names {
        if !vfs.exists(&format!("maps/{name}.bsp")) {
            continue;
        }
        let bsp = cs_assets::bsp::BspMap::load(vfs, &name).expect("load");
        if !bsp.textures.iter().any(|t| t.name.starts_with('!')) {
            continue;
        }
        let m = bsp.models[0];
        let bounds = (m.mins, m.maxs);
        let mut b = Bench::on(map_named(vfs, &name));
        b.run(|game, eng| assert!(game.spawn_player(eng, 0, Team::Ct, 0)));
        let spots: Vec<Vec3> = b.run(move |_, eng| {
            let mut out = Vec::new();
            let step = 64.0;
            let wet =
                |eng: &EngineApi<'_>, q: Vec3| eng.point_contents(q, Hull::Point) == CONTENTS_WATER;
            let mut x = bounds.0.x + 8.0;
            while x < bounds.1.x {
                let mut y = bounds.0.y + 8.0;
                while y < bounds.1.y {
                    let mut z = bounds.0.z + 8.0;
                    while z < bounds.1.z {
                        let p = Vec3::new(x, y, z);
                        if wet(eng, p)
                            && wet(eng, p + Vec3::Z * 36.0)
                            && wet(eng, p - Vec3::Z * 36.0)
                            && wet(eng, p + Vec3::X * 150.0)
                            && !eng.trace_hull(p, p, Hull::Standing).start_solid
                            && eng
                                .trace_hull(p, p + Vec3::X * 150.0, Hull::Standing)
                                .fraction
                                == 1.0
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
            return Some((name, b, spots));
        }
    }
    None
}

/// The stock map the water test tries first (the first with a pool the scan finds in this
/// installation; any other is found by the scan).
const PREFERRED_WATER_MAP: &str = "c0a0d";

#[test]
fn water_is_swum_on_a_real_map() {
    let Some(vfs) = discover() else {
        return;
    };
    let Some((name, mut b, spots)) = find_pool(&vfs, PREFERRED_WATER_MAP) else {
        panic!("no stock map has a deep pool the scan can find");
    };

    let mut swum = false;
    for spot in spots.into_iter().take(40) {
        // Idle: `PM_WaterMove` drifts towards the bottom (`wishvel.z -= 60`, `wishspeed 60 * 0.8`
        // = 48); friction takes 4% a command and the acceleration is 5 * 0.01 * 48 = 2.4, so the
        // sink speed settles at exactly 48.
        let idle = drive(&mut b, spot, 150, cmd(10, 0, 0.0, 0.0));
        let last = idle.last().unwrap();
        if last.3 < 2 {
            continue;
        }
        assert_eq!(last.4, CONTENTS_WATER, "{name}");
        assert!((last.2.z + 48.0).abs() < 0.1, "sink speed {:?}", last.2);

        // Swimming forward at forwardmove 250 (maxspeed 250): `wishspeed = 250 * 0.8 = 200`; each
        // command friction takes 4% and the acceleration adds `min(5 * 0.01 * 200 = 10,
        // 200 - speed)`, so the speed settles at 200 (192 + 8).
        let swim = drive(&mut b, spot, 40, cmd(10, IN_FORWARD, 250.0, 0.0));
        let wet = swim.iter().filter(|r| r.3 >= 2).count();
        assert!(wet * 5 >= swim.len() * 4, "in the water for {wet} of {} commands", swim.len());
        let travelled = (swim.last().unwrap().1 - spot).truncate().length();
        assert!(travelled > 20.0, "swam {travelled}");
        let speed = swim.last().unwrap().2.truncate().length();
        assert!((199.0..=200.01).contains(&speed), "swim speed {speed}");
        assert!(swim[0].2.length() < 20.0, "starts from rest: {:?}", swim[0].2);

        // Jumping in water: `PM_Jump` sets `velocity.z = 100`; `PM_WaterMove` friction takes 4%
        // of it: 96 on the first command, which then rises by 0.96 a command.
        let up = drive(&mut b, spot, 20, cmd(10, IN_JUMP, 0.0, 0.0));
        assert!((up[0].2.z - 96.0).abs() < 0.01, "jump speed {:?}", up[0].2);
        let rise = up.last().unwrap().1.z - spot.z;
        assert!(rise > 15.0, "rose {rise}");
        swum = true;
        break;
    }
    assert!(swum, "{name}: the player was in water (waterlevel 2 or more) somewhere");
}

/// `func_water` spawns as `CBaseDoor::Spawn` does for a door with special contents: its `skin`
/// is the contents and it is not solid (a stub map: no collision data, so only the spawn is
/// checked; the stock-map test below checks the movement's view of it).
#[test]
fn func_water_spawns_as_a_non_solid_special_contents_door() {
    let mut b = Bench::stub(spawns(2, 2));
    // (class, model, keys): the stub map has no brush models, so the entities are made by hand
    // with the model name the engine would have set.
    type Case<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);
    let cases: [Case<'_>; 5] = [
        ("func_water", "*1", vec![("skin", "-3")]),
        ("func_door", "*2", vec![]),
        ("func_door", "*3", vec![("spawnflags", "8")]),
        ("func_door", "*4", vec![("skin", "-4")]),
        ("func_door_rotating", "*5", vec![("skin", "-3")]),
    ];
    let seen = b.run(move |game, eng| {
        let mut out = Vec::new();
        for (class, model, keys) in cases {
            let id = eng.create_entity(class).expect("free edict");
            eng.ent_mut(id).unwrap().model = model.to_string();
            if let Some((_, flags)) = keys.iter().find(|(k, _)| *k == "spawnflags") {
                eng.ent_mut(id).unwrap().spawnflags = flags.parse().unwrap();
            }
            let keys: Vec<(String, String)> = keys
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            assert!(game.spawn_entity(eng, id, class, &keys), "{class} {model}");
            let v = eng.ent(id).unwrap();
            out.push((model, v.skin, v.solid, v.movetype));
        }
        out
    });
    let state_of = |model: &str| seen.iter().find(|e| e.0 == model).map(|e| (e.1, e.2, e.3));
    // Water: skin -3, SOLID_NOT (0), MOVETYPE_PUSH (7).
    assert_eq!(state_of("*1"), Some((-3, 0, 7)), "{seen:?}");
    // A plain door is solid (SOLID_BSP 4); SF_DOOR_PASSABLE makes it not solid; a door with a
    // special-contents skin is not solid either.
    assert_eq!(state_of("*2"), Some((0, 4, 7)), "{seen:?}");
    assert_eq!(state_of("*3"), Some((0, 0, 7)), "{seen:?}");
    assert_eq!(state_of("*4"), Some((-4, 0, 7)), "{seen:?}");
    // A rotating door has no skin rule (`CRotDoor::Spawn`): it stays solid.
    assert_eq!(state_of("*5").map(|e| e.1), Some(4), "{seen:?}");
}

#[test]
fn func_water_gives_the_player_water_on_a_stock_map() {
    let Some(vfs) = discover() else {
        return;
    };
    let Some(name) = find_map_with_entity(&vfs, "func_water") else {
        eprintln!("no stock map has a func_water: skipped");
        return;
    };
    let mut b = Bench::on(map_named(&vfs, &name));
    b.run(|game, eng| assert!(game.spawn_player(eng, 0, Team::Ct, 0)));
    let pools: Vec<(Vec3, i16)> = b.run(|_, eng| {
        let mut out = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "func_water") {
            after = Some(id);
            let v = eng.ent(id).unwrap();
            assert_eq!(v.solid, 0, "func_water is not solid");
            out.push(((v.absmin + v.absmax) * 0.5, v.skin));
        }
        out
    });
    assert!(!pools.is_empty(), "{name} spawned its func_water");
    // At the centre of one of them the movement's point contents is water, and a player there
    // has a waterlevel.
    let mut found = false;
    for (centre, skin) in pools {
        if i32::from(skin) != CONTENTS_WATER {
            continue;
        }
        let (contents, level) = b.run(move |game, eng| {
            game.place_player(eng, 0, centre);
            let me = eng.client_edict(0);
            let pe = eng.pm_physents(me);
            let contents = eng.pm_point_contents(&pe, centre).0;
            eng.ent_mut(me).unwrap().max_speed = 250.0;
            common::run_cmd(game, eng, 0, &cmd(10, 0, 0.0, 0.0), 0);
            (contents, vars(eng, 0).waterlevel)
        });
        if contents == CONTENTS_WATER && level > 0 {
            found = true;
            break;
        }
    }
    assert!(found, "{name}: a player in a func_water has water");
}
