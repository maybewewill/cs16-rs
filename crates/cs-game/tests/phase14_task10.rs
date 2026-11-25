//! Phase 14 Task 10 tests: Built-in KZ/bhop mode (`sv_kz_mode`, checkpoints,
//! `func_button` map timers, non-lethal fall damage, `sv_kz_keep_teleport_vel`) and
//! Kreedz JumpStats (`sv_ljstats`, `+32.0 u` `Hull::Standing` width, strafes/sync, drop rejection).

mod common;

use std::sync::Arc;

use common::{Bench, run_cmd, spawns};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_cvar::CVarRegistry;
use cs_engine::collision::{CONTENTS_EMPTY, CONTENTS_SOLID};
use cs_engine::consts::{EF_NOINTERP, FL_ONGROUND};
use cs_engine::{EdictId, EngineApi, GameDll, ServerMap};
use cs_game::kz::MAX_KZ_CHECKPOINTS;
use cs_game::player::Team;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pm::consts::IN_JUMP;
use glam::Vec3;

fn ent(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    }
}

fn find_button(eng: &EngineApi<'_>, targetname: &str) -> EdictId {
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "func_button") {
        cursor = Some(id);
        if eng.ent(id).is_some_and(|v| v.targetname == targetname) {
            return id;
        }
    }
    panic!("func_button `{targetname}` not found");
}

fn minimal_bsp_raw() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

fn leaf(contents: i32) -> Leaf {
    Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    }
}

/// Real flat floor BSP (`z < 0` solid across all 4 hulls) for deterministic KZ movement & LJStats.
fn flat_floor_bsp(entities: Vec<Entity>) -> ServerMap {
    let planes = vec![
        Plane { normal: Vec3::Z, dist: 0.0, plane_type: 2 },
        Plane { normal: Vec3::Z, dist: 36.0, plane_type: 2 },
        Plane { normal: Vec3::Z, dist: 32.0, plane_type: 2 },
        Plane { normal: Vec3::Z, dist: 18.0, plane_type: 2 },
    ];
    let nodes = vec![
        Node {
            plane: 0,
            children: [-2, -1],
            mins: [-4096, -4096, -4096],
            maxs: [4096, 4096, 4096],
            first_face: 0,
            num_faces: 0,
        },
        Node {
            plane: 0,
            children: [-2, -1],
            mins: [-4096, -4096, -4096],
            maxs: [4096, 4096, 4096],
            first_face: 0,
            num_faces: 0,
        },
    ];
    let clipnodes = vec![
        ClipNode { plane: 1, children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16] },
        ClipNode { plane: 2, children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16] },
        ClipNode { plane: 3, children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16] },
        ClipNode { plane: 3, children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16] },
    ];
    let leafs = vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)];
    let models = vec![
        BspModel {
            mins: Vec3::new(-4096.0, -4096.0, -64.0),
            maxs: Vec3::new(4096.0, 4096.0, 4096.0),
            origin: Vec3::ZERO,
            head_nodes: [0, 0, 1, 2],
            vis_leafs: 1,
            first_face: 0,
            num_faces: 0,
        },
        BspModel {
            mins: Vec3::new(-64.0, -64.0, -64.0),
            maxs: Vec3::new(64.0, 64.0, 64.0),
            origin: Vec3::ZERO,
            head_nodes: [0, 0, 1, 2],
            vis_leafs: 1,
            first_face: 0,
            num_faces: 0,
        },
    ];
    let bsp = BspMap {
        name: "kz_test".into(),
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
    ServerMap::from_bsp("kz_test", &minimal_bsp_raw(), Arc::new(bsp)).expect("valid BSP")
}

fn collected_prints(b: &Bench) -> Vec<String> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Print(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn collected_texts(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

#[test]
fn task10_cvars_default_to_vanilla_disabled() {
    let reg = CVarRegistry::new_with_defaults();
    assert_eq!(reg.get_str("sv_kz_mode"), "0");
    assert_eq!(reg.get_str("sv_kz_keep_teleport_vel"), "0");
    assert_eq!(reg.get_str("sv_ljstats"), "0");
    assert_eq!(reg.get_str("cl_ljstats"), "0");
    assert_eq!(reg.get_str("cl_ljstats_min"), "215");
}

#[test]
fn kz_checkpoints_and_map_timer_workflow() {
    let mut ents = spawns(2, 2);
    ents.push(ent(&[
        ("classname", "func_button"),
        ("model", "*1"),
        ("targetname", "counter_start"),
        ("spawnflags", "1"),
    ]));
    ents.push(ent(&[
        ("classname", "func_button"),
        ("model", "*1"),
        ("targetname", "counter_off"),
        ("spawnflags", "1"),
    ]));
    let mut b = Bench::on(flat_floor_bsp(ents));
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        let id = eng.client_edict(0);
        eng.set_origin(id, Vec3::new(0.0, 0.0, 36.0));
        eng.ent_mut(id).unwrap().flags |= FL_ONGROUND;

        // Vanilla default (`sv_kz_mode 0`): `/cp` must not save checkpoints.
        game.client_command(eng, 0, &["say".into(), "/cp".into()]);
        assert!(game.state().kz.players[0].checkpoints.is_empty());

        // Enable `sv_kz_mode 1`.
        eng.set_cvar("sv_kz_mode", "1");

        // Trigger map start button (`counter_start`).
        let start_btn = find_button(eng, "counter_start");
        game.test_use_entity_by(eng, start_btn, id);
        assert!(game.state().kz.players[0].timer_started_at.is_some());

        // Airborne `/cp` is rejected.
        eng.ent_mut(id).unwrap().flags &= !FL_ONGROUND;
        game.client_command(eng, 0, &["say".into(), "/cp".into()]);
        assert!(game.state().kz.players[0].checkpoints.is_empty());

        // Solid ground `/cp` saves checkpoints.
        eng.ent_mut(id).unwrap().flags |= FL_ONGROUND;
        eng.set_origin(id, Vec3::new(100.0, 0.0, 36.0));
        game.client_command(eng, 0, &["say".into(), "/cp".into()]);
        eng.set_origin(id, Vec3::new(200.0, 20.0, 36.0));
        game.client_command(eng, 0, &["cp".into()]);
        assert_eq!(game.state().kz.players[0].checkpoints.len(), 2);

        // Move away with velocity and basevelocity, then `/tp` -> zeroes both + sets EF_NOINTERP.
        eng.set_origin(id, Vec3::new(400.0, 80.0, 36.0));
        {
            let v = eng.ent_mut(id).unwrap();
            v.velocity = Vec3::new(250.0, 0.0, 100.0);
            v.basevelocity = Vec3::new(50.0, 0.0, 0.0);
            v.effects = 0;
        }
        game.client_command(eng, 0, &["say".into(), "/tp".into()]);
        let v = eng.ent(id).unwrap();
        assert!((v.origin - Vec3::new(200.0, 20.0, 36.0)).length() < 0.01);
        assert_eq!(v.velocity, Vec3::ZERO);
        assert_eq!(v.basevelocity, Vec3::ZERO);
        assert_ne!(v.effects & EF_NOINTERP, 0);

        // `/stuck` steps back to checkpoint #1 (100, 0, 36).
        game.client_command(eng, 0, &["say".into(), "/stuck".into()]);
        let v = eng.ent(id).unwrap();
        assert!((v.origin - Vec3::new(100.0, 0.0, 36.0)).length() < 0.01);

        // Bounded at MAX_KZ_CHECKPOINTS = 64.
        for i in 0..70 {
            eng.set_origin(id, Vec3::new(i as f32, 0.0, 36.0));
            game.client_command(eng, 0, &["cp".into()]);
        }
        assert_eq!(game.state().kz.players[0].checkpoints.len(), MAX_KZ_CHECKPOINTS);
    });

    // Advance 120 frames (~1.2s) and press `counter_off` -> records top-10 run!
    b.step(120);
    b.run(|game, eng| {
        let id = eng.client_edict(0);
        let stop_btn = find_button(eng, "counter_off");
        game.test_use_entity_by(eng, stop_btn, id);
        assert!(game.state().kz.players[0].timer_started_at.is_none());
        assert_eq!(game.state().kz.top_runs.len(), 1);
        assert!(game.state().kz.top_runs[0].time >= 1.0);
        assert_eq!(game.state().kz.top_runs[0].teleports, 2);

        // `/start` returns to start pose and clears checkpoints.
        game.client_command(eng, 0, &["say".into(), "/start".into()]);
        assert!(game.state().kz.players[0].checkpoints.is_empty());
        assert!((eng.ent(id).unwrap().origin - Vec3::new(0.0, 0.0, 36.0)).length() < 0.01);
    });

    b.step(2);
    let texts = collected_texts(&b).join("\n");
    assert!(texts.contains("[KZ] Timer started!"), "{texts}");
    assert!(texts.contains("finished in"), "{texts}");
}

#[test]
fn kz_non_lethal_fall_damage() {
    let mut b = Bench::on(flat_floor_bsp(spawns(2, 2)));
    b.run(|game, eng| {
        eng.set_cvar("sv_kz_mode", "1");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        let id = eng.client_edict(0);
        eng.set_origin(id, Vec3::new(0.0, 0.0, 500.0));
        eng.ent_mut(id).unwrap().flags &= !FL_ONGROUND;
        eng.ent_mut(id).unwrap().velocity = Vec3::new(0.0, 0.0, -1400.0);
    });
    for _ in 0..80 {
        b.run(|game, eng| {
            run_cmd(game, eng, 0, &GoldSrcUserCmd { msec: 10, ..Default::default() }, 0);
        });
    }
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.alive, "fall damage in sv_kz_mode 1 must be non-lethal");
        assert!(p.health >= 1.0, "health={} must remain >= 1.0", p.health);
    });
}

#[test]
fn kz_teleport_keeps_velocity_and_invalidates_jumpstats() {
    let mut ents = spawns(2, 2);
    ents.push(ent(&[
        ("classname", "info_teleport_destination"),
        ("origin", "500 0 36"),
        ("targetname", "tp_dest"),
    ]));
    ents.push(ent(&[
        ("classname", "trigger_teleport"),
        ("model", "*1"),
        ("origin", "0 0 36"),
        ("target", "tp_dest"),
    ]));
    let mut b = Bench::on(flat_floor_bsp(ents));
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        let id = eng.client_edict(0);
        let tp = eng.find_by_classname(None, "trigger_teleport").expect("trigger_teleport");
        eng.set_size(tp, Vec3::new(-64.0, -64.0, -64.0), Vec3::new(64.0, 64.0, 64.0));

        // 1. Vanilla default (`sv_kz_keep_teleport_vel 0`) zeroes velocity.
        eng.ent_mut(id).unwrap().velocity = Vec3::new(350.0, 40.0, 120.0);
        game.touch(eng, tp, id);
        assert_eq!(eng.ent(id).unwrap().velocity, Vec3::ZERO);

        // 2. `sv_kz_keep_teleport_vel 1` preserves velocity across trigger_teleport.
        eng.set_cvar("sv_kz_keep_teleport_vel", "1");
        eng.set_origin(id, Vec3::new(0.0, 0.0, 36.0));
        let keep = Vec3::new(420.0, -35.0, 110.0);
        eng.ent_mut(id).unwrap().velocity = keep;
        game.touch(eng, tp, id);
        assert_eq!(eng.ent(id).unwrap().velocity, keep);
    });
}

#[test]
fn kz_ljstats_server_console_output_and_drop_rejection() {
    let mut b = Bench::on(flat_floor_bsp(spawns(2, 2)));
    b.run(|game, eng| {
        eng.set_cvar("sv_ljstats", "1");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        let id = eng.client_edict(0);
        eng.set_origin(id, Vec3::new(0.0, 0.0, 36.0));
        eng.ent_mut(id).unwrap().flags |= FL_ONGROUND;
        let _ = game.select_weapon(eng, 0, cs_game::weapons::WeaponId::Knife);
        eng.ent_mut(id).unwrap().max_speed = 250.0;

        // Prestrafe / accelerate along +X on ground to reach full 250 u/s speed.
        for i in 0..45 {
            run_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    forwardmove: 400.0,
                    viewangles: Vec3::ZERO,
                    ..Default::default()
                },
                i,
            );
        }

        // Jump (`IN_JUMP`) and execute 6 alternating air-strafes + crouch-land (`IN_DUCK`).
        const IN_DUCK: u16 = 1 << 2;
        for i in 0..80 {
            let mut buttons = if i == 0 { IN_JUMP as u16 } else { 0 };
            if i >= 60 {
                buttons |= IN_DUCK;
            }
            let vel = eng.ent(id).unwrap().velocity;
            let vel_deg = vel.y.atan2(vel.x).to_degrees();
            let strafe_idx = i / 11;
            let (sidemove, yaw) = if i >= 66 {
                (0.0, vel_deg)
            } else if strafe_idx % 2 == 0 {
                (-400.0, vel_deg - 6.0)
            } else {
                (400.0, vel_deg + 6.0)
            };
            run_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    forwardmove: 0.0,
                    sidemove,
                    buttons,
                    viewangles: Vec3::new(0.0, yaw, 0.0),
                    ..Default::default()
                },
                100 + i as u32,
            );
        }

        let v = eng.ent(id).unwrap();
        let dbg_info = format!(
            "origin={:?} vel={:?} flags={:#x} max_speed={} mv_max={}",
            v.origin,
            v.velocity,
            v.flags,
            v.max_speed,
            eng.move_vars().maxspeed
        );
        let stats = game.state().kz.players[0]
            .last_ljstats
            .as_ref()
            .unwrap_or_else(|| panic!("flat LJ stats recorded: {dbg_info}"));
        assert!(stats.distance >= 215.0, "dist={}", stats.distance);
        assert!(stats.strafes >= 2, "strafes={}", stats.strafes);
        assert!(stats.sync_pct > 0.0, "sync={}", stats.sync_pct);
    });
    b.step(2);
    let prints = collected_prints(&b).join("\n");
    assert!(prints.contains("[LJ] Distance:"), "{prints}");
    assert!(prints.contains("Strafes:"), "{prints}");
    assert!(prints.contains("Sync:"), "{prints}");
}
