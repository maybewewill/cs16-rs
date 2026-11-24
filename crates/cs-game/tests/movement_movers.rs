mod common;

use common::{Bench, entity, stub_spawns};
use cs_engine::ServerMap;
use cs_game::player::Team;
use cs_proto48::delta::GoldSrcUserCmd;
use glam::Vec3;

#[test]
fn player_pushes_and_rides_func_pushable() {
    let mut entities = stub_spawns();
    entities.push(entity(&[
        ("classname", "func_plat"),
        ("targetname", "static_floor"),
        ("origin", "0 0 0"),
    ]));
    entities.push(entity(&[
        ("classname", "func_pushable"),
        ("targetname", "crate1"),
        ("origin", "46 0 36"),
        ("friction", "100"),
    ]));
    let mut b = Bench::with_userinfo(ServerMap::stub_with_entities("test", entities), 2, &[]);
    b.idle_cmds = true;
    b.step(4);
    let crate_id = b.run(|game, eng| {
        let floor = eng.find_by_classname(None, "func_plat").unwrap();
        eng.set_size(floor, Vec3::new(-500.0, -500.0, -16.0), Vec3::new(500.0, 500.0, 0.0));
        eng.set_origin(floor, Vec3::ZERO);

        let id = eng.find_by_classname(None, "func_pushable").unwrap();
        eng.set_size(id, Vec3::new(-24.0, -24.0, -36.0), Vec3::new(24.0, 24.0, 36.0));
        eng.set_origin(id, Vec3::new(46.0, 0.0, 36.0));
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 36.0));
        id
    });
    b.idle_cmds = false;

    let start_crate_x = b.run(move |_, eng| eng.ent(crate_id).unwrap().origin.x);
    let walk_cmd = GoldSrcUserCmd {
        msec: 10,
        forwardmove: 250.0,
        viewangles: Vec3::ZERO,
        ..GoldSrcUserCmd::default()
    };
    for _ in 0..45 {
        b.probe.queue_cmd(walk_cmd.clone());
        b.step(1);
    }

    let pushed_x = b.run(move |_, eng| eng.ent(crate_id).unwrap().origin.x);
    assert!(
        pushed_x > start_crate_x + 2.0,
        "pushable crate should slide forward when pushed: start={start_crate_x}, now={pushed_x}"
    );

    // Standing on top of the crate carries the player on it without pushing it sideways.
    b.run(move |game, eng| {
        if let Some(v) = eng.ent_mut(crate_id) {
            v.velocity = Vec3::ZERO;
        }
        eng.set_origin(crate_id, Vec3::new(200.0, 0.0, 36.0));
        game.place_player(eng, 0, Vec3::new(200.0, 0.0, 108.0));
    });
    let before_top = b.run(move |_, eng| eng.ent(crate_id).unwrap().origin.x);
    for _ in 0..10 {
        b.probe.queue_cmd(GoldSrcUserCmd {
            msec: 10,
            ..GoldSrcUserCmd::default()
        });
        b.step(1);
    }
    let after_top = b.run(move |_, eng| eng.ent(crate_id).unwrap().origin.x);
    assert!(
        (after_top - before_top).abs() < 0.01,
        "standing on top should not push crate sideways: {before_top} vs {after_top}"
    );
}

#[test]
fn player_rides_func_plat_and_func_train() {
    // 1. `func_plat` lifts a standing player smoothly up to pos1.
    let mut entities = stub_spawns();
    entities.push(entity(&[
        ("classname", "func_plat"),
        ("origin", "0 0 200"),
        ("speed", "200"),
        ("height", "200"),
        ("wait", "2"),
    ]));
    let mut b = Bench::with_userinfo(ServerMap::stub_with_entities("test", entities), 2, &[]);
    b.idle_cmds = true;
    b.step(4);
    let plat_id = b.run(|game, eng| {
        let id = eng.find_by_classname(None, "func_plat").unwrap();
        eng.set_size(id, Vec3::new(-64.0, -64.0, -16.0), Vec3::new(64.0, 64.0, 0.0));
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 36.0));
        id
    });
    // Step with idle commands so CategorizePosition lands player onto the plat and triggers it.
    b.step(60);
    let (plat_z, player_z) = b.run(move |_, eng| {
        let pz = eng.ent(plat_id).unwrap().origin.z;
        let cz = eng.ent(eng.client_edict(0)).unwrap().origin.z;
        (pz, cz)
    });
    assert!(plat_z > 50.0, "plat should rise from z=0 toward z=200 (got {plat_z})");
    assert!(player_z > 86.0, "player riding plat should rise with it (got {player_z})");

    // 2. `func_train` drives between `path_corner` waypoints and carries a riding player.
    let mut train_ents = stub_spawns();
    train_ents.push(entity(&[
        ("classname", "path_corner"),
        ("targetname", "p1"),
        ("target", "p2"),
        ("origin", "0 0 0"),
    ]));
    train_ents.push(entity(&[
        ("classname", "path_corner"),
        ("targetname", "p2"),
        ("target", "p1"),
        ("origin", "300 0 0"),
    ]));
    train_ents.push(entity(&[
        ("classname", "func_train"),
        ("target", "p1"),
        ("speed", "200"),
        ("origin", "0 0 0"),
    ]));
    let mut tb = Bench::with_userinfo(ServerMap::stub_with_entities("test", train_ents), 2, &[]);
    tb.idle_cmds = true;
    tb.step(4);
    let train_id = tb.run(|game, eng| {
        let id = eng.find_by_classname(None, "func_train").unwrap();
        eng.set_size(id, Vec3::new(-64.0, -64.0, -16.0), Vec3::new(64.0, 64.0, 0.0));
        eng.set_origin(id, Vec3::ZERO);
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 36.0));
        id
    });
    tb.step(50);
    let (train_x, rider_x) = tb.run(move |_, eng| {
        (
            eng.ent(train_id).unwrap().origin.x,
            eng.ent(eng.client_edict(0)).unwrap().origin.x,
        )
    });
    assert!(train_x > 40.0, "func_train should travel along path_corner toward x=300 (got {train_x})");
    assert!(rider_x > 40.0, "player standing on train should be carried (got {rider_x})");
}

#[test]
fn player_is_carried_by_func_conveyor() {
    let mut entities = stub_spawns();
    entities.push(entity(&[
        ("classname", "func_conveyor"),
        ("angles", "0 0 0"),
        ("speed", "150"),
        ("origin", "0 0 0"),
    ]));
    let mut b = Bench::with_userinfo(ServerMap::stub_with_entities("test", entities), 2, &[]);
    b.idle_cmds = true;
    b.step(4);
    b.run(|game, eng| {
        let id = eng.find_by_classname(None, "func_conveyor").unwrap();
        eng.set_size(id, Vec3::new(-256.0, -64.0, -16.0), Vec3::new(256.0, 64.0, 0.0));
        eng.set_origin(id, Vec3::ZERO);
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 36.0));
    });
    b.step(40);
    let x = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().origin.x);
    assert!(x > 20.0, "conveyor should carry standing player along +x (got {x})");
}
