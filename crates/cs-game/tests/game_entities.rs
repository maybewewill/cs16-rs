//! The entity registry, the world and the dispatch of engine calls on a `Server<Rig>`: a
//! `CsGame` that also runs closures the test queued, at the start of the next frame. The tests
//! that need the installed game return without assertions when it is absent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use cs_assets::bsp::{BspMap, Entity, SpawnTeam};
use cs_cvar::CVarRegistry;
use cs_engine::consts::{
    FL_CLIENT, FL_WORLDBRUSH, MOVETYPE_FLY, MOVETYPE_NONE, MOVETYPE_PUSH, SOLID_BBOX, SOLID_BSP,
    SOLID_NOT, SOLID_TRIGGER,
};
use cs_engine::{EdictId, EngineApi, GameDll, MoveFilter, Server, ServerConfig, ServerMap};
use cs_game::entity::DispatchEvent;
use cs_game::world::{CT_SPAWN_CLASS, T_SPAWN_CLASS, select_spawn_point};
use cs_game::{CsGame, GameEntity};
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::messages::SvcMessage;
use cs_transport::PeerAddr;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_sim::trace::Hull;
use cs_transport::LoopbackHub;
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

type Action = Box<dyn FnOnce(&mut CsGame, &mut EngineApi<'_>)>;

/// `CsGame` plus the closures the test queued, which run at the start of the next frame.
struct Rig {
    game: CsGame,
    actions: Vec<Action>,
}

impl GameDll for Rig {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.game.usermsgs()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_activate(eng);
    }
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_deactivate(eng);
    }
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.game.spawn_entity(eng, id, classname, keys)
    }
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.game.client_connect(eng, slot)
    }
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_put_in_server(eng, slot);
    }
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_disconnect(eng, slot);
    }
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.game.client_command(eng, slot, argv);
    }
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_userinfo_changed(eng, slot);
    }
    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        self.game.run_usercmd(eng, slot, cmd, random_seed);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        for action in std::mem::take(&mut self.actions) {
            action(&mut self.game, eng);
        }
        self.game.start_frame(eng, frametime);
    }
    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.game.think(eng, id);
    }
    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        self.game.touch(eng, toucher, other);
    }
    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        self.game.blocked(eng, pusher, other);
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.game.update_client_data(eng, slot)
    }
}

/// A server on `map` running a [`Rig`], and its clock.
struct Bench {
    server: Server<Rig>,
    hub: LoopbackHub,
    now: f64,
}

impl Bench {
    fn on(map: ServerMap) -> Self {
        let cfg = ServerConfig {
            max_clients: 2,
            max_edicts: 256,
            ..ServerConfig::default()
        };
        let rig = Rig {
            game: CsGame::new(),
            actions: Vec::new(),
        };
        Self {
            server: Server::new(
                cfg,
                map,
                rig,
                DeltaRegistry::cs16_default(),
                CVarRegistry::new_with_defaults(),
            ),
            hub: LoopbackHub::new(),
            now: 0.0,
        }
    }

    fn stub(entities: Vec<Entity>) -> Self {
        Self::on(ServerMap::stub_with_entities("test", entities))
    }

    fn step(&mut self, frames: usize) {
        for _ in 0..frames {
            self.now = ((self.now / 0.01).round() + 1.0) * 0.01;
            self.server.frame(&mut self.hub, self.now);
        }
    }

    /// Runs `f` as the game at the start of the next frame and returns what it returned.
    fn run<T: 'static>(
        &mut self,
        f: impl FnOnce(&mut CsGame, &mut EngineApi<'_>) -> T + 'static,
    ) -> T {
        let out = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&out);
        self.server
            .game_mut()
            .actions
            .push(Box::new(move |game, eng| *sink.borrow_mut() = Some(f(game, eng))));
        self.step(1);
        let result = out.borrow_mut().take();
        result.expect("the game ran the action")
    }

    fn log(&self) -> Vec<DispatchEvent> {
        self.server.game().game.state().dispatch_log.clone()
    }
}

fn entity(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

fn spawn_at(class: &str, origin: &str) -> Entity {
    entity(&[("classname", class), ("origin", origin)])
}

fn vars_of(b: &Bench, id: EdictId) -> cs_engine::EntVars {
    b.server.edicts().get(id).expect("live").vars.clone()
}

fn discover() -> Option<Vfs> {
    Vfs::discover(None, VfsConfig::default()).ok()
}

fn dust2(vfs: &Vfs) -> (ServerMap, BspMap) {
    let bsp = BspMap::load(vfs, "de_dust2").expect("load de_dust2");
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("read de_dust2");
    let map = ServerMap::from_bsp("de_dust2", &raw, Arc::new(bsp.clone())).expect("BSP header");
    (map, bsp)
}

#[test]
fn world_precaches_player_models_and_sounds() {
    let mut b = Bench::stub(vec![entity(&[
        ("classname", "worldspawn"),
        ("skyname", "des"),
        ("WaveHeight", "16"),
        ("MaxRange", "8192"),
    ])]);
    let (models, sounds, world_speed, scale, cvars) = b.run(|_, eng| {
        let models = [
            "models/player.mdl",
            "models/player/gign/gign.mdl",
            "models/player/leet/leet.mdl",
            "models/player/vip/vip.mdl",
            "models/hgibs.mdl",
        ]
        .map(|m| eng.model_index(m).is_some());
        let sounds = ["player/pl_step1.wav", "player/die1.wav", "common/null.wav"]
            .map(|s| eng.sound_index(s).is_some());
        let world = eng.ent(eng.world()).unwrap();
        let cvars = ["sv_skyname", "sv_zmax", "sv_wateramp", "sv_gravity", "sv_maxspeed"]
            .map(|name| eng.cvar_str(name));
        (models, sounds, world.speed, world.scale, cvars)
    });
    assert_eq!(models, [true; 5]);
    assert_eq!(sounds, [true; 3]);
    assert_eq!(world_speed, 8192.0);
    assert_eq!(scale, 2.0);
    // The world keys reach the cvars the engine sends in the movevars.
    let cvars = cvars.map(|v| v.expect("registered"));
    assert_eq!(cvars, ["des", "8192", "2", "800", "900"]);
}

#[test]
fn the_world_keys_reach_the_probe_in_the_movevars() {
    let mut b = Bench::stub(vec![entity(&[
        ("classname", "worldspawn"),
        ("skyname", "des"),
        ("WaveHeight", "16"),
        ("MaxRange", "8192"),
    ])]);
    let endpoint = b.hub.connect();
    let cfg = ProbeConfig {
        name: "p".to_string(),
        steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
        cdkey_md5_hex: md5_hex(b"probe"),
    };
    let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
    probe.connect();
    for _ in 0..300 {
        probe.pump(b.now);
        b.step(1);
        if probe.state() == &ProbeState::InGame {
            break;
        }
    }
    assert_eq!(probe.state(), &ProbeState::InGame);
    let mv = probe
        .messages()
        .iter()
        .find_map(|m| match m {
            SvcMessage::NewMoveVars(mv) => Some(mv.clone()),
            _ => None,
        })
        .expect("movevars received");
    assert_eq!(mv.sky_name, "des");
    assert_eq!((mv.zmax, mv.wave_height), (8192.0, 2.0));
}

#[test]
fn spawn_points_come_from_the_lump_by_team() {
    // The ordering and the team rule on a map of known spawn points.
    let mut b = Bench::stub(vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at(CT_SPAWN_CLASS, "100 0 0"),
        spawn_at(CT_SPAWN_CLASS, "200 0 0"),
        spawn_at(T_SPAWN_CLASS, "-100 0 0"),
        // A point at the origin is never chosen.
        spawn_at(CT_SPAWN_CLASS, "0 0 0"),
    ]);
    let picks = b.run(|game, eng| {
        let player = eng.client_edict(0);
        let mut pick = |team| {
            let spot = select_spawn_point(game.state_mut(), eng, player, team).expect("a spawn point");
            eng.ent(spot).unwrap().origin.x
        };
        // Each team cycles through its own points.
        [
            pick(SpawnTeam::CounterTerrorist),
            pick(SpawnTeam::CounterTerrorist),
            pick(SpawnTeam::CounterTerrorist),
            pick(SpawnTeam::Terrorist),
            pick(SpawnTeam::Terrorist),
        ]
    });
    assert_eq!(picks, [100.0, 200.0, 100.0, -100.0, -100.0]);

    // A point with another player within the use radius is skipped, and a dead player blocks it
    // as well (the vanilla rule).
    let picks = b.run(|game, eng| {
        let me = eng.client_edict(0);
        let other = eng.client_edict(1);
        let v = eng.ent_mut(other).unwrap();
        v.flags = FL_CLIENT;
        v.deadflag = 2;
        eng.set_origin(other, Vec3::new(100.0, 0.0, 0.0));
        let mut pick = |eng: &mut EngineApi<'_>| {
            let spot = select_spawn_point(game.state_mut(), eng, me, SpawnTeam::CounterTerrorist)
                .expect("a spawn point");
            eng.ent(spot).unwrap().origin.x
        };
        let a = pick(eng);
        let b = pick(eng);
        (a, b)
    });
    assert_eq!(picks, (200.0, 200.0));
}

#[test]
fn with_every_point_taken_the_pick_is_the_first_of_the_search() {
    // Two CT points 30 apart with a player between them: both are within the use radius.
    let mut b = Bench::stub(vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at(CT_SPAWN_CLASS, "100 0 0"),
        spawn_at(CT_SPAWN_CLASS, "130 0 0"),
    ]);
    let (picks, last) = b.run(|game, eng| {
        let me = eng.client_edict(0);
        let other = eng.client_edict(1);
        eng.ent_mut(other).unwrap().flags = FL_CLIENT;
        eng.set_origin(other, Vec3::new(115.0, 0.0, 0.0));
        let mut pick = || {
            let spot = select_spawn_point(game.state_mut(), eng, me, SpawnTeam::CounterTerrorist)
                .expect("a spawn point");
            (spot, eng.ent(spot).unwrap().origin.x)
        };
        let (_, x1) = pick();
        let (s2, x2) = pick();
        ([x1, x2], (s2, game.state().last_ct_spawn))
    });
    // The search starts after the last point used, so the first of it moves on each time.
    assert_eq!(picks, [100.0, 130.0]);
    assert_eq!(last.1, Some(last.0));
}

#[test]
fn the_fallback_spawn_becomes_the_teams_last_one() {
    let mut b = Bench::stub(vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at(T_SPAWN_CLASS, "-100 0 0"),
    ]);
    let (spot, last) = b.run(|game, eng| {
        let me = eng.client_edict(0);
        let spot = select_spawn_point(game.state_mut(), eng, me, SpawnTeam::CounterTerrorist);
        (spot, game.state().last_ct_spawn)
    });
    assert!(spot.is_some());
    assert_eq!(last, spot);
}

#[test]
fn spawn_points_of_de_dust2_are_the_lumps_by_team() {
    let Some(vfs) = discover() else {
        return;
    };
    let (map, bsp) = dust2(&vfs);
    let mut b = Bench::on(map);
    let (ct, t) = b.run(|_, eng| {
        let all = |class: &str| {
            let mut found = Vec::new();
            let mut after = None;
            while let Some(id) = eng.find_by_classname(after, class) {
                after = Some(id);
                let o = eng.ent(id).unwrap().origin;
                found.push([o.x, o.y, o.z].map(f32::to_bits));
            }
            found.sort_unstable();
            found
        };
        (all(CT_SPAWN_CLASS), all(T_SPAWN_CLASS))
    });
    let from_bsp = |team: SpawnTeam| {
        let mut points: Vec<_> = bsp
            .spawns
            .iter()
            .filter(|s| s.team == team)
            .map(|s| [s.position.x, s.position.y, s.position.z].map(f32::to_bits))
            .collect();
        points.sort_unstable();
        points
    };
    assert!(!ct.is_empty() && !t.is_empty());
    assert_eq!(ct, from_bsp(SpawnTeam::CounterTerrorist));
    assert_eq!(t, from_bsp(SpawnTeam::Terrorist));

    // And the choice picks from the right class.
    let (ct_pick, t_pick) = b.run(|game, eng| {
        let player = eng.client_edict(0);
        let ct = select_spawn_point(game.state_mut(), eng, player, SpawnTeam::CounterTerrorist);
        let t = select_spawn_point(game.state_mut(), eng, player, SpawnTeam::Terrorist);
        (
            ct.map(|id| eng.ent(id).unwrap().classname.clone()),
            t.map(|id| eng.ent(id).unwrap().classname.clone()),
        )
    });
    assert_eq!(ct_pick.as_deref(), Some(CT_SPAWN_CLASS));
    assert_eq!(t_pick.as_deref(), Some(T_SPAWN_CLASS));
}

#[test]
fn unknown_classes_are_removed() {
    let mut b = Bench::stub(vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at(CT_SPAWN_CLASS, "100 0 0"),
        spawn_at("light", "1 2 3"),
        spawn_at("ambient_generic", "4 5 6"),
        entity(&[("classname", "func_buyzone")]),
        // A brush entity without a brush model has nothing to collide with.
        entity(&[("classname", "func_wall")]),
    ]);
    let classes = b.run(|_, eng| {
        eng.entity_ids()
            .into_iter()
            .map(|id| eng.ent(id).unwrap().classname.clone())
            .collect::<Vec<_>>()
    });
    // The world and the one spawn point: nothing else survived.
    let mut named: Vec<String> = classes.into_iter().filter(|c| !c.is_empty()).collect();
    named.sort();
    assert_eq!(named, [CT_SPAWN_CLASS, "worldspawn"]);
}

/// Classnames of the brush entities that stand as solid walls.
const SOLID_CLASSES: [&str; 8] = [
    "func_wall",
    "func_door",
    "func_door_rotating",
    "func_breakable",
    "func_button",
    "func_plat",
    "func_train",
    "func_pushable",
];

#[test]
fn brush_entities_stay_solid() {
    let Some(vfs) = discover() else {
        return;
    };
    let (map, bsp) = dust2(&vfs);
    let bsp_ladders = bsp
        .entities
        .iter()
        .filter(|e| e.classname() == "func_ladder")
        .count();
    let mut b = Bench::on(map);
    let (solid, unsolid_ok, hit, ladders, illusionary_ok) = b.run(|game, eng| {
        let mut solid = 0;
        let mut unsolid_ok = true;
        let mut hit = 0;
        let mut illusionary_ok = true;
        for id in eng.entity_ids() {
            let v = eng.ent(id).unwrap().clone();
            if SOLID_CLASSES.contains(&v.classname.as_str()) {
                solid += 1;
                unsolid_ok &= v.solid == SOLID_BSP && v.movetype == MOVETYPE_PUSH;
                if v.classname == "func_wall" {
                    unsolid_ok &= v.flags & FL_WORLDBRUSH != 0;
                }
                // A trace from outside to the middle of the brush entity finds an entity (the
                // world may stand in the way of some walls, so count them).
                let centre = (v.absmin + v.absmax) * 0.5;
                let start = Vec3::new(v.absmax.x + 40.0, centre.y, centre.z);
                let t = eng.trace_hull_ents(start, centre, Hull::Point, MoveFilter::Normal, None);
                if t.ent == Some(id) {
                    hit += 1;
                    // The same trace against that entity alone stops short of the middle.
                    let alone = eng.trace_entity(id, start, centre, Hull::Point);
                    unsolid_ok &= alone.trace.fraction < 1.0;
                }
                unsolid_ok &= eng.entities_in_box(v.absmin, v.absmax).contains(&id);
            }
            if v.classname == "func_illusionary" {
                illusionary_ok &= v.solid == SOLID_NOT
                    && v.movetype == MOVETYPE_NONE
                    && v.modelindex != 0
                    && v.effects & 128 == 0;
            }
        }
        (solid, unsolid_ok, hit, game.state().ladders.len(), illusionary_ok)
    });
    assert!(solid > 0, "de_dust2 has brush entities");
    assert!(unsolid_ok);
    assert!(hit > 0, "a trace into one of {solid} brush entities stops at it");
    assert!(illusionary_ok);
    // Every `func_ladder` of the lump with a model gave its bounds.
    assert_eq!(ladders, bsp_ladders);
}

/// A `class` probe entity made by the game itself, as `spawn_entity` makes it.
fn probe_of(class: &str, game: &mut CsGame, eng: &mut EngineApi<'_>) -> EdictId {
    let id = eng.create_entity(class).expect("free edict");
    assert!(game.spawn_entity(eng, id, class, &[]));
    id
}

fn probe(game: &mut CsGame, eng: &mut EngineApi<'_>) -> EdictId {
    probe_of("test_dispatch", game, eng)
}

/// The class of the game entity of `id`, `None` for a freed edict or one with no entity.
fn game_class(b: &mut Bench, id: EdictId) -> Option<&'static str> {
    b.run(move |_, eng| eng.private::<Box<dyn GameEntity>>(id).map(|e| e.classname()))
}

fn thinks_of(b: &Bench, id: EdictId) -> usize {
    b.log()
        .iter()
        .filter(|e| **e == DispatchEvent::Think { id })
        .count()
}

/// A probe of `class` that thinks in the next frame or two.
fn thinking_probe(b: &mut Bench, class: &'static str) -> EdictId {
    b.run(move |game, eng| {
        let id = probe_of(class, game, eng);
        let due = eng.time() + 0.015;
        eng.ent_mut(id).unwrap().nextthink = due;
        id
    })
}

#[test]
fn an_entity_may_remove_itself_in_its_own_think() {
    let mut b = Bench::stub(vec![entity(&[("classname", "worldspawn")])]);
    let id = thinking_probe(&mut b, "test_dispatch_remove_on_think");
    b.step(5);
    assert!(b.server.edicts().get(id).is_none());
    assert_eq!(b.log(), [DispatchEvent::Think { id }], "one call, no panic");
}

#[test]
fn an_entity_may_remove_itself_in_its_own_touch() {
    let mut b = Bench::stub(vec![entity(&[("classname", "worldspawn")])]);
    let (trigger, ent) = b.run(|game, eng| {
        let trigger = probe_of("test_dispatch_remove_on_touch", game, eng);
        eng.ent_mut(trigger).unwrap().solid = SOLID_TRIGGER;
        eng.set_size(trigger, Vec3::splat(-64.0), Vec3::splat(64.0));
        eng.set_origin(trigger, Vec3::new(0.0, 0.0, 500.0));
        let ent = body(eng, "ent", Vec3::new(0.0, 0.0, 500.0), 8.0);
        eng.ent_mut(ent).unwrap().movetype = MOVETYPE_NONE;
        eng.link_entity(ent, true);
        (trigger, ent)
    });
    b.step(5);
    assert!(b.server.edicts().get(trigger).is_none());
    assert_eq!(
        b.log(),
        [DispatchEvent::Touch {
            id: trigger,
            other: ent
        }]
    );
}

#[test]
fn an_entity_may_replace_itself_during_its_call() {
    let mut b = Bench::stub(vec![entity(&[("classname", "worldspawn")])]);
    let id = thinking_probe(&mut b, "test_dispatch_replace_on_think");
    b.step(5);
    // The replacement stays; the old entity is dropped, not put back over it.
    assert_eq!(game_class(&mut b, id), Some("test_dispatch"));
    assert_eq!(thinks_of(&b, id), 1);
    assert_eq!(b.log().len(), 1);
}

#[test]
fn an_entity_may_create_another_during_its_call() {
    let mut b = Bench::stub(vec![entity(&[("classname", "worldspawn")])]);
    let creator = thinking_probe(&mut b, "test_dispatch_create_on_think");
    b.step(8);
    let log = b.log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!(log[0], DispatchEvent::Think { id: creator });
    let DispatchEvent::Think { id: child } = log[1] else {
        panic!("{log:?}");
    };
    assert_ne!(child, creator);
    // Both are still there, each with its entity, and each thought once.
    assert_eq!(game_class(&mut b, creator), Some("test_dispatch_create_on_think"));
    assert_eq!(game_class(&mut b, child), Some("test_dispatch"));
}

/// A box that stays where it is, with no game entity.
fn body(eng: &mut EngineApi<'_>, class: &str, origin: Vec3, half: f32) -> EdictId {
    let id = eng.create_entity(class).expect("free edict");
    let v = eng.ent_mut(id).unwrap();
    v.solid = SOLID_BBOX;
    v.movetype = MOVETYPE_FLY;
    eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
    eng.set_origin(id, origin);
    id
}

#[test]
fn entity_dispatch_reaches_the_owner() {
    let mut b = Bench::stub(vec![entity(&[("classname", "worldspawn")])]);

    // think: due in the next frame or two.
    let thinker = b.run(|game, eng| {
        let id = probe(game, eng);
        let due = eng.time() + 0.015;
        eng.ent_mut(id).unwrap().nextthink = due;
        id
    });
    b.step(5);
    assert_eq!(b.log(), [DispatchEvent::Think { id: thinker }]);
    assert_eq!(vars_of(&b, thinker).nextthink, 0.0, "one think, not a loop");

    // touch: the trigger is told, the body (no game entity) is not.
    let (trigger, ent) = b.run(|game, eng| {
        let trigger = probe(game, eng);
        eng.ent_mut(trigger).unwrap().solid = SOLID_TRIGGER;
        eng.set_size(trigger, Vec3::splat(-64.0), Vec3::splat(64.0));
        eng.set_origin(trigger, Vec3::new(0.0, 0.0, 500.0));
        let ent = body(eng, "ent", Vec3::new(0.0, 0.0, 500.0), 8.0);
        // A fly mover would be linked with touches again by every frame's physics.
        eng.ent_mut(ent).unwrap().movetype = MOVETYPE_NONE;
        eng.link_entity(ent, true);
        (trigger, ent)
    });
    assert_eq!(
        b.log()[1..],
        [DispatchEvent::Touch {
            id: trigger,
            other: ent
        }]
    );

    // blocked: a pusher moving into a body that cannot go anywhere.
    let (pusher, crushed) = b.run(|game, eng| {
        let pusher = probe(game, eng);
        let v = eng.ent_mut(pusher).unwrap();
        v.solid = SOLID_BBOX;
        v.movetype = MOVETYPE_PUSH;
        v.velocity = Vec3::new(100.0, 0.0, 0.0);
        v.nextthink = 1.0;
        eng.set_size(pusher, Vec3::splat(-16.0), Vec3::splat(16.0));
        eng.set_origin(pusher, Vec3::new(0.0, 0.0, 1000.0));
        let crushed = body(eng, "crushed", Vec3::new(24.0, 0.0, 1000.0), 8.0);
        body(eng, "wall", Vec3::new(40.0, 0.0, 1000.0), 8.0);
        (pusher, crushed)
    });
    let blocked = DispatchEvent::Blocked {
        id: pusher,
        other: crushed,
    };
    assert!(b.log().contains(&blocked), "{:?}", b.log());

    // The entity was put back after its calls: it thinks again when rescheduled.
    b.run(move |_, eng| {
        let due = eng.time() + 0.015;
        eng.ent_mut(thinker).unwrap().nextthink = due;
    });
    b.step(5);
    let thinks = b
        .log()
        .iter()
        .filter(|e| **e == DispatchEvent::Think { id: thinker })
        .count();
    assert_eq!(thinks, 2);

    // An entity removed before its think is due is not called.
    let doomed = b.run(|game, eng| {
        let id = probe(game, eng);
        let due = eng.time() + 0.015;
        eng.ent_mut(id).unwrap().nextthink = due;
        id
    });
    b.run(move |_, eng| eng.remove_entity(doomed));
    b.step(5);
    assert!(b.server.edicts().get(doomed).is_none());
    assert!(
        !b.log()
            .iter()
            .any(|e| matches!(e, DispatchEvent::Think { id } if *id == doomed))
    );
}
