//! End-to-end bot objective integration tests (`de_dust2` plant/detonation, C4 defusal,
//! and `cs_office` hostage rescue).

mod common;

use std::sync::Arc;

use common::{Bench, command, commands_of, discover, entity, join, map_named};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_engine::consts::{FL_ONGROUND, SOLID_TRIGGER};
use cs_engine::{EngineApi, GameDll, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::rules::WinStatus;
use cs_game::weapons::WeaponId;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_sim::pm::consts::IN_ATTACK;
use glam::Vec3;

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;

fn minimal_bsp_raw() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

/// Real floor BSP map (`z < 0` solid across all 4 hulls) for deterministic C4 defusal tests.
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
        name: "full_round_test".into(),
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
    ServerMap::from_bsp("full_round_test", &minimal_bsp_raw(), Arc::new(bsp)).expect("valid BSP")
}

fn step_cmd(game: &mut CsGame, eng: &mut EngineApi<'_>, slot: u8, buttons: u16) {
    let id = eng.client_edict(slot);
    let viewangles = eng.ent(id).map_or(Vec3::ZERO, |v| v.v_angle);
    if let Some(v) = eng.ent_mut(id) {
        v.button = i32::from(buttons);
    }
    let cmd = GoldSrcUserCmd {
        msec: 10,
        viewangles,
        buttons,
        ..Default::default()
    };
    game.run_usercmd(eng, slot, &cmd, 0);
    if let Some(v) = eng.ent_mut(id) {
        v.flags |= FL_ONGROUND;
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

fn has_planted_c4(eng: &EngineApi<'_>) -> bool {
    let Some(c4_mdl) = eng.model_index("models/w_c4.mdl") else {
        return false;
    };
    if c4_mdl == 0 {
        return false;
    }
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "grenade") {
        if eng.ent(id).is_some_and(|v| v.modelindex == c4_mdl) {
            return true;
        }
        cursor = Some(id);
    }
    false
}

#[test]
fn a_bot_plants_and_the_bomb_explodes_on_de_dust2() {
    let Some(vfs) = discover() else {
        eprintln!("SKIPPED: Half-Life gamedir not found");
        return;
    };
    if !vfs.exists("maps/de_dust2.bsp") {
        eprintln!("SKIPPED: maps/de_dust2.bsp not found");
        return;
    }

    let mut b = Bench::with_config(
        map_named(&vfs, "de_dust2"),
        ServerConfig {
            max_clients: 8,
            max_edicts: 1024,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        true,
    );
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_c4timer 10");
    let _ = b.server.console_command("mp_autoteambalance 0");
    let _ = b.server.console_command("mp_limitteams 0");
    let _ = b.server.console_command("bot_add_t");
    b.step(10);
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
    });

    let mut planted_seen = false;
    let mut exploded_seen = false;
    for _ in 0..200 {
        b.step(40);
        let (is_planted, target_bombed, win_status) = b.run(|game, eng| {
            (
                has_planted_c4(eng),
                game.state().rules.target_bombed,
                game.state().rules.win_status(),
            )
        });
        if is_planted {
            planted_seen = true;
        }
        if target_bombed && win_status == WinStatus::Terrorists {
            exploded_seen = true;
            break;
        }
    }

    assert!(
        planted_seen && exploded_seen,
        "Terrorist bot should plant C4 on de_dust2 and trigger detonation win \
         (planted_seen={planted_seen}, exploded_seen={exploded_seen})"
    );
}

#[test]
fn a_bot_defuses_a_planted_bomb() {
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", "-220 0 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "0 0 36")]),
        entity(&[("classname", "func_bomb_target"), ("origin", "0 0 0")]),
    ];
    let mut b = Bench::with_config(
        floor_bsp(entities),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        false,
    );
    b.run(|_, eng| {
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-140.0), Vec3::splat(140.0));
                eng.set_origin(id, origin);
            }
        }
    });

    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_c4timer 45");
    let _ = b.server.console_command("mp_startmoney 5000");
    let _ = b.server.console_command("mp_autoteambalance 0");
    let _ = b.server.console_command("mp_limitteams 0");

    // Slot 0: dummy T human (probe slot 0) to plant the C4 cleanly inside the zone.
    b.run(|game, eng| commands_of(game, eng, 0, 3));
    join(&mut b, 0, 1, 1);
    // Slot 1: CT bot that buys a defuse kit and will defuse the planted C4.
    let _ = b.server.console_command("bot_add_ct");
    b.step(380); // Let #Game_Commencing complete its initial restart cleanly.

    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        assert!(game.test_give_c4(eng, 0));
        assert!(game.select_weapon(eng, 0, WeaponId::C4));
        game.set_next_attack(0, 0.0);
        if let Some(w) = game.weapon_mut(0, WeaponId::C4) {
            w.next_primary_attack = 0.0;
        }
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 36.0));
        // Hold the CT bot beyond combat LOS while T plants C4, then bring CT back to spawn.
        game.place_player(eng, 1, Vec3::new(-3200.0, 0.0, 36.0));
    });
    for _ in 0..60 {
        b.run(|game, eng| {
            game.place_player(eng, 1, Vec3::new(-3200.0, 0.0, 36.0));
            step_cmd(game, eng, 0, 0);
        });
    }

    for _ in 0..420 {
        b.run(|game, eng| {
            game.place_player(eng, 1, Vec3::new(-3200.0, 0.0, 36.0));
            step_cmd(game, eng, 0, IN_ATTACK as u16);
        });
        if b.run(|_, eng| has_planted_c4(eng)) {
            break;
        }
    }
    assert!(
        b.run(|_, eng| has_planted_c4(eng)),
        "setup T slot must finish planting C4"
    );

    // Eliminate the T planter so CT bot focuses purely on defusal without combat interruption.
    command(&mut b, 0, "kill");
    b.run(|game, eng| {
        game.place_player(eng, 1, Vec3::new(-220.0, 0.0, 36.0));
    });

    let mut defused_seen = false;
    let mut ct_wins = 0;
    for _ in 0..120 {
        b.step(20);
        let (wins, bomb_defused) = b.run(|game, _| {
            (game.state().num_ct_wins, game.state().rules.bomb_defused)
        });
        if bomb_defused {
            defused_seen = true;
        }
        ct_wins = wins;
        if defused_seen && ct_wins >= 1 {
            break;
        }
    }

    assert!(
        defused_seen && ct_wins >= 1,
        "CT bot must walk to planted C4 and defuse it \
         (ct_wins={ct_wins}, defused_seen={defused_seen})"
    );
}

#[test]
fn a_bot_rescues_a_hostage_on_cs_office() {
    let Some(vfs) = discover() else {
        eprintln!("SKIPPED: Half-Life gamedir not found");
        return;
    };
    if !vfs.exists("maps/cs_office.bsp") {
        eprintln!("SKIPPED: maps/cs_office.bsp not found");
        return;
    }

    let mut b = Bench::with_config(
        map_named(&vfs, "cs_office"),
        ServerConfig {
            max_clients: 8,
            max_edicts: 1024,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        true,
    );
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_autoteambalance 0");
    let _ = b.server.console_command("mp_limitteams 0");
    let _ = b.server.console_command("bot_add_ct");
    let _ = b.server.console_command("bot_add_ct");
    b.step(120);

    let mut rescued = 0;
    for _ in 0..240 {
        b.step(50);
        rescued = b.run(|game, _| game.state().rules.hostages_rescued);
        if rescued >= 1 {
            break;
        }
    }

    assert!(
        rescued >= 1,
        "CT bot(s) should follow+escort at least one hostage to rescue on cs_office (rescued={rescued})"
    );
}
