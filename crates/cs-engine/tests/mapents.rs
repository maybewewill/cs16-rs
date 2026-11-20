//! Spawning the entities of a map's entity lump through `GameDll::spawn_entity`.

mod common;

use std::collections::HashMap;

use common::{Harness, TestGame};
use cs_assets::bsp::Entity;
use cs_engine::{
    EdictId, MAX_ENTITY_KEYS, MAX_KEY_BYTES, MAX_VALUE_BYTES, ServerConfig, ServerMap,
};
use cs_probe::{ProbeClient, ProbeState};
use cs_transport::LoopbackEndpoint;
use glam::Vec3;

fn ent(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

fn world() -> Entity {
    ent(&[("classname", "worldspawn")])
}

fn harness_on(max_clients: u8, entities: Vec<Entity>, game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients,
        ..ServerConfig::default()
    };
    Harness::with_map(cfg, ServerMap::stub_with_entities("lump", entities), game)
}

/// Live edicts with `classname`, in index order.
fn live_with_class(h: &Harness, classname: &str) -> Vec<(EdictId, cs_engine::EntVars)> {
    h.server
        .edicts()
        .iter_live()
        .filter(|(_, e)| e.vars.classname == classname)
        .map(|(id, e)| (id, e.vars.clone()))
        .collect()
}

#[test]
fn map_entities_reach_the_game_with_parsed_vars() {
    let h = harness_on(
        4,
        vec![
            world(),
            ent(&[
                ("classname", "test_keep"),
                ("origin", "1 2 3"),
                ("angles", "0 90 0"),
                ("targetname", "a"),
                ("target", "b"),
                ("spawnflags", "5"),
                ("renderamt", "128"),
                ("rendercolor", "10 20 30"),
            ]),
        ],
        TestGame::default(),
    );
    let game = h.server.game();
    // The world was offered first, then the entity.
    assert_eq!(game.spawned[0].0, "worldspawn");
    assert_eq!(game.spawned[1].0, "test_keep");
    assert_eq!(
        game.spawned[1].1[0],
        ("classname".into(), "test_keep".into())
    );
    assert!(
        game.spawned[1]
            .1
            .contains(&("origin".into(), "1 2 3".into()))
    );

    let kept = live_with_class(&h, "test_keep");
    assert_eq!(kept.len(), 1);
    let (id, vars) = &kept[0];
    assert!(id.index > 4, "a new edict above the player slots");
    assert_eq!(vars.origin, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(vars.angles, Vec3::new(0.0, 90.0, 0.0));
    assert_eq!(vars.targetname, "a");
    assert_eq!(vars.target, "b");
    assert_eq!(vars.spawnflags, 5);
    assert_eq!(vars.renderamt, 128);
    assert_eq!(vars.rendercolor, [10, 20, 30]);
    // The world edict carries the world's class.
    let edicts = h.server.edicts();
    assert_eq!(
        edicts.get(edicts.world()).unwrap().vars.classname,
        "worldspawn"
    );
}

#[test]
fn rejected_entities_are_removed() {
    let h = harness_on(
        4,
        vec![
            world(),
            ent(&[("classname", "test_drop")]),
            ent(&[("classname", "test_keep")]),
        ],
        TestGame::default(),
    );
    assert!(live_with_class(&h, "test_drop").is_empty());
    assert_eq!(live_with_class(&h, "test_keep").len(), 1);
    // Players and the world, plus the one kept entity.
    assert_eq!(h.server.edicts().iter_live().count(), 4 + 1 + 1);
}

#[test]
fn a_game_that_removes_the_entity_itself_is_safe() {
    let h = harness_on(
        4,
        vec![
            world(),
            ent(&[("classname", "test_selfremove")]),
            ent(&[("classname", "test_keep")]),
        ],
        TestGame::default(),
    );
    assert!(live_with_class(&h, "test_selfremove").is_empty());
    assert_eq!(live_with_class(&h, "test_keep").len(), 1);
    assert_eq!(h.server.edicts().iter_live().count(), 4 + 1 + 1);
}

#[test]
fn spawn_happens_before_activate_and_precache_still_works() {
    let h = harness_on(
        4,
        vec![
            world(),
            ent(&[("classname", "test_keep"), ("model", "models/mapent.mdl")]),
            ent(&[
                ("classname", "test_keep_b"),
                ("model", "models/mapent2.mdl"),
            ]),
        ],
        TestGame::default(),
    );
    let game = h.server.game();
    assert_eq!(
        game.call_order,
        [
            "spawn:worldspawn",
            "spawn:test_keep",
            "spawn:test_keep_b",
            "activate"
        ]
    );
    assert_eq!(game.spawn_precache.len(), 2);
    assert!(game.spawn_precache.iter().all(Result::is_ok));
    for (_, vars) in live_with_class(&h, "test_keep") {
        assert_eq!(vars.model, "models/mapent.mdl");
        assert_ne!(vars.modelindex, 0);
    }
    assert_eq!(game.server_activate_calls, 1);
}

#[test]
fn a_model_that_is_not_precached_is_left_to_the_game() {
    let mut map = ServerMap::stub_with_entities(
        "lump",
        vec![
            world(),
            ent(&[("classname", "test_keep"), ("model", "models/unknown.mdl")]),
            ent(&[
                ("classname", "test_keep"),
                ("model", "*1"),
                ("targetname", "brush"),
            ]),
        ],
    );
    map.submodel_count = 2;
    let cfg = ServerConfig {
        max_clients: 4,
        ..ServerConfig::default()
    };
    let h = Harness::with_map(cfg, map, TestGame::default());
    // The engine could not set the unknown model, so the game found the model empty and
    // precached it itself; the brush sub-model was set by the engine before the call.
    assert_eq!(h.server.game().spawn_precache.len(), 1);
    let kept = live_with_class(&h, "test_keep");
    let brush = kept.iter().find(|(_, v)| v.targetname == "brush").unwrap();
    assert_eq!(brush.1.model, "*1");
    assert_ne!(brush.1.modelindex, 0);
    let other = kept.iter().find(|(_, v)| v.targetname.is_empty()).unwrap();
    assert_eq!(other.1.model, "models/unknown.mdl");
}

#[test]
fn angle_key_shortcuts() {
    let h = harness_on(
        4,
        vec![
            world(),
            ent(&[
                ("classname", "test_keep"),
                ("targetname", "yaw"),
                ("angle", "90"),
            ]),
            ent(&[
                ("classname", "test_keep"),
                ("targetname", "up"),
                ("angle", "-1"),
            ]),
            ent(&[
                ("classname", "test_keep"),
                ("targetname", "down"),
                ("angle", "-2"),
            ]),
        ],
        TestGame::default(),
    );
    let by_name = |name: &str| {
        live_with_class(&h, "test_keep")
            .into_iter()
            .find(|(_, v)| v.targetname == name)
            .unwrap()
            .1
    };
    assert_eq!(by_name("yaw").angles, Vec3::new(0.0, 90.0, 0.0));
    assert_eq!(by_name("up").angles.x, -90.0);
    assert_eq!(by_name("down").angles.x, 90.0);
    // The raw key still reaches the game.
    assert!(
        h.server
            .game()
            .spawned
            .iter()
            .any(|(_, keys)| keys.contains(&("angle".into(), "-1".into())))
    );
}

#[test]
fn only_the_first_worldspawn_is_the_world() {
    let h = harness_on(
        4,
        vec![
            world(),
            ent(&[("classname", "worldspawn"), ("targetname", "second")]),
        ],
        TestGame::default(),
    );
    // Both were offered to the game; the game refuses `worldspawn` here, so the world stays
    // and the second one is removed.
    assert_eq!(h.server.game().spawned.len(), 2);
    let edicts = h.server.edicts();
    assert_eq!(
        edicts.get(edicts.world()).unwrap().vars.classname,
        "worldspawn"
    );
    assert_eq!(live_with_class(&h, "worldspawn").len(), 1);
    assert_eq!(edicts.iter_live().count(), 4 + 1);
}

#[test]
fn a_map_without_worldspawn_gives_the_world_no_spawn_call() {
    let h = harness_on(
        4,
        vec![ent(&[("classname", "test_keep")])],
        TestGame::default(),
    );
    assert_eq!(h.server.game().spawned.len(), 1);
    let edicts = h.server.edicts();
    assert_eq!(edicts.get(edicts.world()).unwrap().vars.classname, "");
}

#[test]
fn entries_without_a_classname_are_skipped() {
    let big = "c".repeat(MAX_VALUE_BYTES + 1);
    let h = harness_on(
        4,
        vec![
            world(),
            ent(&[("origin", "1 2 3")]),
            ent(&[("classname", "")]),
            ent(&[("classname", &big)]),
        ],
        TestGame::default(),
    );
    assert_eq!(h.server.game().spawned.len(), 1);
    assert_eq!(h.server.edicts().iter_live().count(), 4 + 1);
}

#[test]
fn hostile_entity_lump_is_bounded() {
    let mut lump = vec![world()];
    // One entry with far too many keys, an oversized key and an oversized value, and numbers
    // that do not parse.
    let mut monster: Vec<(String, String)> = (0..2000)
        .map(|i| (format!("key{i:04}"), "v".to_string()))
        .collect();
    monster.push(("classname".into(), "test_keep".into()));
    monster.push(("k".repeat(500), "v".into()));
    monster.push(("model_extra".into(), "x".repeat(100 * 1024)));
    monster.push(("origin".into(), "1e999 0 0".into()));
    monster.push(("angles".into(), "NaN 0 0".into()));
    monster.push(("spawnflags".into(), "abc".into()));
    lump.push(Entity {
        properties: monster.into_iter().collect(),
    });
    for (key, value) in [
        ("origin", "abc"),
        ("origin", "1e999 1e999 1e999"),
        ("origin", "NaN NaN NaN"),
        ("origin", "999999999 0 0"),
        ("angles", "inf 0 0"),
        ("angle", "NaN"),
        ("renderamt", "NaN"),
        ("rendercolor", "1e999 0 0"),
    ] {
        lump.push(ent(&[("classname", "test_keep"), (key, value)]));
    }
    // Entries without a classname, and thousands of ordinary ones.
    lump.push(ent(&[("origin", "1 2 3")]));
    for i in 0..5000 {
        lump.push(ent(&[
            ("classname", "test_keep"),
            ("origin", &format!("{i} 0 0")),
        ]));
    }
    let h = harness_on(4, lump, TestGame::default());

    let live = h.server.edicts().iter_live().count();
    assert!(live <= cs_engine::DEFAULT_MAX_EDICTS, "{live} live edicts");
    // Activation went on normally after the pool ran out.
    assert_eq!(h.server.game().server_activate_calls, 1);
    assert_eq!(h.server.game().call_order.last().unwrap(), "activate");

    for (classname, keys) in &h.server.game().spawned {
        assert!(!classname.is_empty());
        assert!(keys.len() <= MAX_ENTITY_KEYS, "{} keys", keys.len());
        for (key, value) in keys {
            assert!(key.len() <= MAX_KEY_BYTES, "key of {} bytes", key.len());
            assert!(
                value.len() <= MAX_VALUE_BYTES,
                "value of {} bytes",
                value.len()
            );
        }
    }
    // The monster entry got the cap exactly, with its class first.
    let monster = h
        .server
        .game()
        .spawned
        .iter()
        .find(|(_, keys)| keys.iter().any(|(k, _)| k == "key0000"))
        .unwrap();
    assert_eq!(monster.1.len(), MAX_ENTITY_KEYS);
    assert_eq!(monster.1[0].0, "classname");
    assert!(monster.1.iter().all(|(k, _)| k != "model_extra"));

    for (_, edict) in h.server.edicts().iter_live() {
        let v = &edict.vars;
        assert!(v.origin.is_finite(), "{:?}", v.origin);
        assert!(v.angles.is_finite(), "{:?}", v.angles);
        assert!(v.origin.abs().max_element() <= 131_072.0);
    }
}

#[test]
fn rejected_entries_reuse_their_slots_during_activation() {
    // More entries than the pool has slots; the game refuses every one (`test_drop`), so each
    // is spawned, freed and its slot reused by the next.
    let mut lump = vec![world()];
    for i in 0..2000 {
        lump.push(ent(&[("classname", "test_drop"), ("targetname", &i.to_string())]));
    }
    let h = harness_on(4, lump, TestGame::default());
    assert_eq!(h.server.game().spawned.len(), 1 + 2000);
    assert_eq!(h.server.edicts().iter_live().count(), 4 + 1);
    assert_eq!(h.server.game().server_activate_calls, 1);
}

#[test]
fn pool_exhaustion_skips_the_rest_and_activates() {
    let mut lump = vec![world()];
    for i in 0..2000 {
        lump.push(ent(&[
            ("classname", "test_keep"),
            ("targetname", &i.to_string()),
        ]));
    }
    let h = harness_on(4, lump, TestGame::default());
    // 900 slots, 5 of them the world and the players.
    assert_eq!(live_with_class(&h, "test_keep").len(), 900 - 5);
    assert_eq!(h.server.game().spawned.len(), 1 + (900 - 5));
    assert_eq!(h.server.game().server_activate_calls, 1);
}

type Probe = ProbeClient<LoopbackEndpoint>;

fn join(h: &mut Harness, name: &str) -> Probe {
    let mut probe = h.add_probe(name);
    for _ in 0..500 {
        h.step(&mut [&mut probe], 1);
        if probe.state() == &ProbeState::InGame {
            return probe;
        }
    }
    panic!("{name} did not get in the game: {:?}", probe.state());
}

#[test]
fn changelevel_respawns_entities_from_the_new_map() {
    let mut h = harness_on(
        4,
        vec![
            world(),
            ent(&[("classname", "test_keep"), ("targetname", "old")]),
        ],
        TestGame::default(),
    );
    let mut probe = join(&mut h, "A");
    h.step(&mut [&mut probe], 20);
    let old = live_with_class(&h, "test_keep")[0].0;

    let second = vec![
        world(),
        ent(&[("classname", "test_keep"), ("targetname", "new1")]),
        ent(&[("classname", "test_keep"), ("targetname", "new2")]),
    ];
    let spawned_before = h.server.game().spawned.len();
    h.server
        .change_map(ServerMap::stub_with_entities("second", second));

    let names: Vec<String> = live_with_class(&h, "test_keep")
        .into_iter()
        .map(|(_, v)| v.targetname)
        .collect();
    assert_eq!(names, ["new1", "new2"]);
    assert!(
        h.server.edicts().get(old).is_none(),
        "ids of the old map are stale"
    );
    let game = h.server.game();
    assert_eq!(game.spawned.len() - spawned_before, 3);
    // Spawned at the server time of the change.
    let last = *game.spawn_times.last().unwrap();
    assert!((last - h.now).abs() < 1e-9, "{last} vs {}", h.now);
    assert_eq!(game.call_order.last().unwrap(), "activate");
}

#[test]
fn map_entity_is_visible_to_a_probe() {
    let mut h = harness_on(
        4,
        vec![
            world(),
            ent(&[
                ("classname", "test_keep"),
                ("model", "models/mapent.mdl"),
                ("origin", "0 0 32"),
            ]),
        ],
        TestGame::default(),
    );
    let (id, vars) = live_with_class(&h, "test_keep").remove(0);
    assert_ne!(vars.modelindex, 0);
    let mut probe = join(&mut h, "A");
    h.step(&mut [&mut probe], 30);
    let seen = probe
        .entities()
        .get(&id.index)
        .expect("the probe sees the map entity");
    assert_eq!(seen.origin, Vec3::new(0.0, 0.0, 32.0));
    assert_eq!(seen.modelindex, vars.modelindex);
}
