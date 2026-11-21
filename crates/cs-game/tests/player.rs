//! The player: connecting, spawning, damage and death, the `kill` command, the fall damage and
//! the client data, on a `Server<Rig>` (a `CsGame` that also runs closures the test queued, at
//! the start of the next frame). The tests that need the installed game return without
//! assertions when it is absent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use cs_assets::bsp::{BspMap, Entity};
use cs_cvar::CVarRegistry;
use cs_engine::consts::{
    FL_CLIENT, FL_DORMANT, FL_ONGROUND, FL_SPECTATOR, MOVETYPE_NONE, MOVETYPE_TOSS, MOVETYPE_WALK,
    SOLID_NOT, SOLID_SLIDEBOX,
};
use cs_engine::{EdictId, EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::{ArmorType, DEAD_DEAD, DEAD_DYING, DEAD_NO, Team, dmg, fall_damage};
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::{CsUserMsg, UserMsgRegistry};
use cs_sim::trace::Hull;
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
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

/// A server on a map running a [`Rig`], with one probe client in slot 0, and its clock.
struct Bench {
    server: Server<Rig>,
    hub: LoopbackHub,
    probe: ProbeClient<LoopbackEndpoint>,
    now: f64,
}

impl Bench {
    /// A two-slot server on `map`; the probe has entered the game when this returns.
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
        let mut server = Server::new(
            cfg,
            map,
            rig,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        let mut hub = LoopbackHub::new();
        let endpoint = hub.connect();
        let cfg = ProbeConfig {
            name: "p".to_string(),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(b"probe"),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        probe.connect();
        let mut now = 0.0;
        for _ in 0..300 {
            probe.pump(now);
            now = ((now / 0.01).round() + 1.0) * 0.01;
            server.frame(&mut hub, now);
            if probe.state() == &ProbeState::InGame {
                break;
            }
        }
        assert_eq!(probe.state(), &ProbeState::InGame);
        Self {
            server,
            hub,
            probe,
            now,
        }
    }

    fn stub(entities: Vec<Entity>) -> Self {
        Self::on(ServerMap::stub_with_entities("test", entities))
    }

    /// Runs `frames` server frames (10 ms each) with the probe pumped.
    fn step(&mut self, frames: usize) {
        for _ in 0..frames {
            self.probe.pump(self.now);
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
            .push(Box::new(move |game, eng| {
                *sink.borrow_mut() = Some(f(game, eng))
            }));
        self.step(1);
        let result = out.borrow_mut().take();
        result.expect("the game ran the action")
    }

    /// Puts a second client in the server (slot 1), as the engine does when one signs on.
    fn enter_second_client(&mut self) {
        self.run(|game, eng| game.client_put_in_server(eng, 1));
    }

    /// The game's user messages the probe has received, in order.
    fn messages(&self) -> Vec<CsUserMsg> {
        self.probe
            .messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::UserMsg { decoded, .. } => Some(decoded.clone()),
                _ => None,
            })
            .collect()
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
    entity(&[
        ("classname", class),
        ("origin", origin),
        ("angles", "0 90 0"),
    ])
}

/// A stub map with the world and a few spawn points per team.
fn stub_spawns() -> Vec<Entity> {
    vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "100 0 0"),
        spawn_at("info_player_start", "300 0 0"),
        spawn_at("info_player_deathmatch", "-100 0 0"),
        spawn_at("info_player_deathmatch", "-300 0 0"),
    ]
}

fn discover() -> Option<Vfs> {
    Vfs::discover(None, VfsConfig::default()).ok()
}

fn dust2(vfs: &Vfs) -> ServerMap {
    let bsp = BspMap::load(vfs, "de_dust2").expect("load de_dust2");
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("read de_dust2");
    ServerMap::from_bsp("de_dust2", &raw, Arc::new(bsp)).expect("BSP header")
}

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
    // `ResetMaxSpeed` with no active item.
    assert_eq!(v.max_speed, 240.0);
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
    // `GetPlayerSpawnSpot`: one unit above the point, at its angles, view angles cleared.
    assert_eq!(v.origin, Vec3::new(100.0, 0.0, 1.0));
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

    // Dead players do not suicide again.
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
    // The world kills without a frag lost or gained.
    b.run(|game, eng| game.damage_player(eng, 1, None, 500.0, dmg::CRUSH));
    assert_eq!(frags(&mut b), ((0, 1, false), (1, 1, false)));
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
    assert_eq!(data.maxspeed, 240.0);
    assert_eq!(data.fov, 90.0);
    assert_eq!(data.deadflag, DEAD_NO);
    assert_eq!(data.view_ofs.z, 17.0);
    assert_eq!(data.origin, Vec3::new(100.0, 0.0, 1.0));
    assert_eq!(data.waterlevel, 0);
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
