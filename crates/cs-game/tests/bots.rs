//! Engine-hosted bot integration tests (`bot_add*`, `bot_kick`, movement, combat, buying,
//! and round completion via ReHLDS fake clients).

mod common;

use std::sync::Arc;

use common::{Bench, entity};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_engine::{GameDll, ServerMap};
use cs_game::player::{DEAD_NO, Team};
use glam::Vec3;

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;

fn minimal_bsp_raw() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

/// Real floor BSP map (`z < 0` solid across all 4 hulls) so bots snap, pathfind, walk, and see
/// each other over clean BSP traces without external Half-Life game files.
fn floor_bsp(entities: Vec<Entity>) -> ServerMap {
    let leaf = |contents: i32| Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    };
    let planes = vec![
        Plane {
            normal: Vec3::Z,
            dist: 0.0,
            plane_type: 2,
        },
        Plane {
            normal: Vec3::Z,
            dist: 36.0,
            plane_type: 2,
        },
        Plane {
            normal: Vec3::Z,
            dist: 32.0,
            plane_type: 2,
        },
        Plane {
            normal: Vec3::Z,
            dist: 18.0,
            plane_type: 2,
        },
    ];
    let nodes = vec![Node {
        plane: 0,
        children: [-2, -1],
        mins: [0; 3],
        maxs: [0; 3],
        first_face: 0,
        num_faces: 0,
    }];
    let clipnodes = vec![
        ClipNode {
            plane: 1,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        },
        ClipNode {
            plane: 2,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        },
        ClipNode {
            plane: 3,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        },
    ];
    let leafs = vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)];
    let models = vec![BspModel {
        mins: Vec3::new(-4096.0, -4096.0, -64.0),
        maxs: Vec3::new(4096.0, 4096.0, 4096.0),
        origin: Vec3::ZERO,
        head_nodes: [0, 0, 1, 2],
        vis_leafs: 1,
        first_face: 0,
        num_faces: 0,
    }];

    let bsp = BspMap {
        name: "bots_test".into(),
        skyname: String::new(),
        entities,
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes,
        vertices_raw: Vec::new(),
        visibility: Vec::new(),
        nodes,
        texinfos: Vec::new(),
        faces: Vec::new(),
        lighting: Vec::new(),
        clipnodes,
        leafs,
        mark_surfaces: Vec::new(),
        edges: Vec::new(),
        surf_edges: Vec::new(),
        models,
        textures: Vec::new(),
        lightmap_atlas: LightmapAtlas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            packed_faces: 0,
        },
        render_vertices: Vec::new(),
        render_indices: Vec::new(),
        batches: Vec::new(),
    };
    ServerMap::from_bsp("bots_test", &minimal_bsp_raw(), Arc::new(bsp)).expect("valid BSP")
}

fn arena_entities(ct_pos: &str, t_pos: &str) -> Vec<Entity> {
    vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", ct_pos)]),
        entity(&[("classname", "info_player_start"), ("origin", "200 120 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", t_pos)]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "-200 120 36")]),
    ]
}

fn bot_bench(ct_pos: &str, t_pos: &str, max_clients: u8) -> Bench {
    let mut b = Bench::with_slots(floor_bsp(arena_entities(ct_pos, t_pos)), max_clients);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_autoteambalance 0");
    let _ = b.server.console_command("mp_limitteams 0");
    b.idle_cmds = true;
    b
}

#[test]
fn bot_add_joins_a_team_and_spawns() {
    let mut b = bot_bench("200 0 36", "-200 0 36", 8);
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    let _ = b.server.console_command("bot_add");
    b.step(40);

    let bots: Vec<(u8, Team, bool, i32, bool, cs_game::teams::JoinState)> = b.run(|game, eng| {
        (0..eng.max_clients())
            .filter(|&s| eng.is_fake_client(s))
            .map(|s| {
                let p = game.state().players[usize::from(s)].as_ref().unwrap();
                let v = eng.ent(eng.client_edict(s)).unwrap();
                (s, p.team, p.alive, v.deadflag, v.viewmodel != 0, p.joining)
            })
            .collect()
    });
    assert_eq!(bots.len(), 3);
    assert!(bots.iter().any(|(_, t, ..)| *t == Team::Terrorist));
    assert!(bots.iter().any(|(_, t, ..)| *t == Team::Ct));
    for (s, t, alive, deadflag, has_weapon, joining) in bots {
        assert!(alive, "bot slot {s} ({t:?}, joining={joining:?}, deadflag={deadflag}) must spawn alive");
        assert_eq!(deadflag, DEAD_NO);
        assert!(has_weapon, "bot must hold a starting weapon");
    }
}

#[test]
fn bots_move_and_shoot_an_enemy() {
    let mut b = bot_bench("220 0 36", "-220 0 36", 8);
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    // Wait for Game_Commencing round restart (triggered on 1 CT + 1 T) to finish and bots to spawn.
    b.step(380);

    let initial: Vec<(u8, Vec3, f32)> = b.run(|game, eng| {
        (0..eng.max_clients())
            .filter(|&s| eng.is_fake_client(s))
            .map(|s| {
                let p = game.state().players[usize::from(s)].as_ref().unwrap();
                (s, eng.ent(eng.client_edict(s)).unwrap().origin, p.health)
            })
            .collect()
    });
    assert_eq!(initial.len(), 2);

    b.step(250);

    let (moved, hurt_or_dead) = b.run(move |game, eng| {
        let mut any_moved = false;
        let mut any_hurt = false;
        for &(slot, start_pos, _) in &initial {
            let cur_pos = eng.ent(eng.client_edict(slot)).unwrap().origin;
            if cur_pos.distance(start_pos) > 10.0 {
                any_moved = true;
            }
            let p = game.state().players[usize::from(slot)].as_ref().unwrap();
            if !p.alive || p.health < 100.0 {
                any_hurt = true;
            }
        }
        (any_moved, any_hurt)
    });
    assert!(moved, "bots must move from their spawn points");
    assert!(hurt_or_dead, "opposing bots in line of sight must shoot and deal damage");
}

#[test]
fn bots_buy_at_round_start() {
    let mut b = bot_bench("300 0 36", "-300 0 36", 8);
    let _ = b.server.console_command("mp_startmoney 5000");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    // Step past initial spawn + buy ticks while bots stand at their spawn points (within OLD buy radius).
    b.step(80);

    let bought: Vec<(Team, i32, f32, bool)> = b.run(|game, eng| {
        let vm_m4 = eng.model_index("models/v_m4a1.mdl");
        let vm_ak = eng.model_index("models/v_ak47.mdl");
        (0..eng.max_clients())
            .filter(|&s| eng.is_fake_client(s))
            .map(|s| {
                let p = game.state().players[usize::from(s)].as_ref().unwrap();
                let v = eng.ent(eng.client_edict(s)).unwrap();
                let expected = match p.team {
                    Team::Ct => vm_m4,
                    _ => vm_ak,
                };
                (p.team, p.money, p.armor, Some(v.viewmodel) == expected)
            })
            .collect()
    });
    assert_eq!(bought.len(), 2);
    for (team, money, armor, has_bought_rifle) in bought {
        assert!(has_bought_rifle, "{team:?} bot should buy & equip its primary rifle");
        assert!(armor > 0.0, "{team:?} bot should buy armor");
        assert!(money < 5000, "{team:?} bot should spend money buying gear, remaining={money}");
    }
}

#[test]
fn bot_kick_frees_the_slot() {
    let mut b = bot_bench("300 0 36", "-300 0 36", 8);
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    b.step(30);

    let fake_slots = |b: &mut Bench| -> Vec<(u8, Team)> {
        b.run(|game, eng| {
            (0..eng.max_clients())
                .filter(|&s| eng.is_fake_client(s))
                .map(|s| (s, game.state().players[usize::from(s)].as_ref().unwrap().team))
                .collect()
        })
    };
    let initial = fake_slots(&mut b);
    assert_eq!(initial.len(), 3);
    let ct_slot = initial
        .iter()
        .find(|(_, t)| *t == Team::Ct)
        .map(|&(s, _)| s)
        .unwrap();

    let _ = b.server.console_command("bot_kick ct");
    b.step(5);
    let after_ct_kick = fake_slots(&mut b);
    assert_eq!(after_ct_kick.len(), 2);
    let ct_freed = b.run(move |game, eng| {
        !eng.is_fake_client(ct_slot) && game.state().players[usize::from(ct_slot)].is_none()
    });
    assert!(ct_freed, "bot_kick ct must free the CT bot slot and clear Player state");

    let _ = b.server.console_command("bot_kick");
    b.step(5);
    assert!(fake_slots(&mut b).is_empty());
    let all_cleared = b.run(|game, eng| {
        (1..eng.max_clients()).all(|s| game.state().players[usize::from(s)].is_none())
    });
    assert!(all_cleared, "bot_kick must free every bot Player entry");
}

#[test]
fn a_round_with_bots_ends() {
    let mut b = bot_bench("140 0 36", "-140 0 36", 8);
    let _ = b.server.console_command("mp_startmoney 5000");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    // Let #Game_Commencing complete its initial restart first.
    b.step(380);

    let mut wins = (0, 0);
    for _ in 0..30 {
        b.step(50);
        wins = b.run(|game, _| (game.state().num_terrorist_wins, game.state().num_ct_wins));
        if wins.0 + wins.1 >= 1 {
            break;
        }
    }
    assert!(
        wins.0 + wins.1 >= 1,
        "a round fought by bots must finish with a winner, got scores={wins:?}"
    );
}

/// `CBotManager::StartFrame` costs a server without bots nothing: the navigation graph is built
/// on the first frame that has a bot, not at the map load.
#[test]
fn a_map_without_bots_builds_no_nav() {
    let mut b = bot_bench("200 0 36", "-200 0 36", 8);
    b.step(40);
    assert!(!b.run(|game, _| game.bot_nav_built()), "no bot, no navigation graph");

    let _ = b.server.console_command("bot_add_ct");
    b.step(10);
    assert!(b.run(|game, _| game.bot_nav_built()), "the first bot builds it");
}

/// `CCSBotManager::AddBot` (`cs_bot_manager.cpp` 1577-1613) refuses a full team before it
/// creates the bot: no fake client is made (and so none is kicked again every half second).
#[test]
fn a_full_team_costs_no_fake_client() {
    // The arena has two CT spawn points: the third CT bot has no room.
    let mut b = bot_bench("200 0 36", "-200 0 36", 8);
    let _ = b.server.console_command("bot_add_ct");
    let _ = b.server.console_command("bot_add_ct");
    b.step(10);
    let fakes = |b: &mut Bench| -> usize {
        b.run(|_, eng| (0..eng.max_clients()).filter(|&s| eng.is_fake_client(s)).count())
    };
    assert_eq!(fakes(&mut b), 2);
    // Inside one closure: a fake client that is made and kicked would still be there.
    let after = b.run(|game, eng| {
        let reply = game.server_command(eng, &["bot_add_ct".to_string()]);
        let count = (0..eng.max_clients()).filter(|&s| eng.is_fake_client(s)).count();
        (reply, count)
    });
    assert_eq!(after.1, 2, "a full team must not create a fake client");
    assert_eq!(after.0.as_deref(), Some("Failed to add CT bot\n"));
}
