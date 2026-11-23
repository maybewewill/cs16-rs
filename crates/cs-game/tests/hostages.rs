//! Hostage behavior, local navigation, rescue zones, penalties, and `cs_office` parity.

mod common;

use std::sync::Arc;

use common::{Bench, command, discover, enter, entity, join, map_named};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_engine::ServerMap;
use cs_engine::collision::Hull;
use cs_engine::consts::{
    EF_NODRAW, FL_ONGROUND, MOVETYPE_TOSS, SOLID_NOT, SOLID_SLIDEBOX,
};
use cs_game::bullets;
use cs_game::player::{DEAD_DEAD, DEAD_NO, DEAD_RESPAWNABLE};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pm::consts::IN_USE;
use glam::Vec3;

const HITGROUP_CHEST: i32 = 2;

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;

fn minimal_bsp_raw() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

/// Builds a real `BspMap` whose world has a solid floor at `z = 0` (`z < 0` solid across all 4
/// hulls using GoldSrc hull offsets) plus optional axis-aligned brush submodels (`*1`, `*2`, ...).
fn floor_bsp(entities: Vec<Entity>, boxes: &[Vec3]) -> ServerMap {
    let leaf = |contents: i32| Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    };
    let mut planes = vec![
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
    // World Point hull: plane 0 (`z = 0`), empty in front (leaf 1), solid in back (leaf 0).
    let mut nodes = vec![Node {
        plane: 0,
        children: [-2, -1],
        mins: [0; 3],
        maxs: [0; 3],
        first_face: 0,
        num_faces: 0,
    }];
    // World ClipNodes for hulls 1, 2, 3 (indices 0, 1, 2).
    let mut clipnodes = vec![
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
    let mut leafs = vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)];
    let mut models = vec![BspModel {
        mins: Vec3::new(-4096.0, -4096.0, -64.0),
        maxs: Vec3::new(4096.0, 4096.0, 4096.0),
        origin: Vec3::ZERO,
        head_nodes: [0, 0, 1, 2],
        vis_leafs: 1,
        first_face: 0,
        num_faces: 0,
    }];

    for (k, &half) in boxes.iter().enumerate() {
        leafs.push(leaf(CONTENTS_SOLID));
        let mut head_nodes = [0i32; 4];
        for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
            let ext = match hull {
                Hull::Point => Vec3::ZERO,
                Hull::Standing => Vec3::new(16.0, 16.0, 36.0),
                Hull::Large => Vec3::new(32.0, 32.0, 32.0),
                Hull::Crouched => Vec3::new(16.0, 16.0, 18.0),
            };
            let grown = half + ext;
            let plane_base = planes.len() as i32;
            for i in 0..6 {
                let axis = i >> 1;
                let mut normal = Vec3::ZERO;
                normal[axis] = 1.0;
                let dist = if i & 1 == 0 {
                    grown[axis]
                } else {
                    -grown[axis]
                };
                planes.push(Plane {
                    normal,
                    dist,
                    plane_type: axis as i32,
                });
            }
            let first = if hull == Hull::Point {
                nodes.len()
            } else {
                clipnodes.len()
            } as i16;
            let outside = if hull == Hull::Point {
                -2
            } else {
                CONTENTS_EMPTY as i16
            };
            let inside_child = if hull == Hull::Point {
                -(2 + k as i16) - 1
            } else {
                CONTENTS_SOLID as i16
            };
            for i in 0..6i16 {
                let side = (i & 1) as usize;
                let mut children = [0i16; 2];
                children[side] = outside;
                children[side ^ 1] = if i != 5 { first + i + 1 } else { inside_child };
                if hull == Hull::Point {
                    nodes.push(Node {
                        plane: (plane_base + i32::from(i)) as u32,
                        children,
                        mins: [0; 3],
                        maxs: [0; 3],
                        first_face: 0,
                        num_faces: 0,
                    });
                } else {
                    clipnodes.push(ClipNode {
                        plane: plane_base + i32::from(i),
                        children,
                    });
                }
            }
            head_nodes[hull as usize] = i32::from(first);
        }
        models.push(BspModel {
            mins: -half,
            maxs: half,
            origin: Vec3::ZERO,
            head_nodes,
            vis_leafs: 0,
            first_face: 0,
            num_faces: 0,
        });
    }

    let bsp = BspMap {
        name: "hostage_test".into(),
        skyname: String::new(),
        entities: entities.clone(),
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
    ServerMap::from_bsp("hostage_test", &minimal_bsp_raw(), Arc::new(bsp)).expect("valid BSP")
}

fn find_hostage_edicts(b: &mut Bench) -> Vec<cs_engine::EdictId> {
    b.run(|_, eng| {
        let mut out = Vec::new();
        let mut cur = None;
        while let Some(id) = eng.find_by_classname(cur, "hostage_entity") {
            out.push(id);
            cur = Some(id);
        }
        out
    })
}

fn send_use(b: &mut Bench, slot: u8, buttons: u16) {
    b.run(move |game, eng| {
        let cmd = GoldSrcUserCmd {
            msec: 10,
            buttons,
            ..Default::default()
        };
        common::run_cmd(game, eng, slot, &cmd, 0);
    });
}

fn walk_towards(b: &mut Bench, slot: u8, yaw: f32, forwardmove: f32, steps: usize) {
    for _ in 0..steps {
        b.run(move |game, eng| {
            let cmd = GoldSrcUserCmd {
                msec: 10,
                viewangles: Vec3::new(0.0, yaw, 0.0),
                forwardmove,
                ..Default::default()
            };
            common::run_cmd(game, eng, slot, &cmd, 0);
        });
    }
}

fn spawn_and_ready(b: &mut Bench, slot: u8, team_choice: u8, class: u8) {
    b.run(|_, eng| eng.set_cvar("mp_freezetime", "0"));
    b.idle_cmds = true;
    join(b, slot, team_choice, class);
    // Let weapon deploy delay (`m_flNextAttack` ~ 0.75s) finish so `+use` is accepted.
    b.step(90);
}

#[test]
fn use_toggles_follow_with_the_one_second_gate() {
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", "100 0 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "-200 0 36")]),
        entity(&[("classname", "info_hostage_rescue"), ("origin", "2000 0 36")]),
        entity(&[("classname", "hostage_entity"), ("origin", "130 0 10")]),
    ];
    let mut b = Bench::on(floor_bsp(entities, &[]));
    spawn_and_ready(&mut b, 0, 2, 1);

    let hostage = find_hostage_edicts(&mut b)[0];
    let initial_money = b.run(|game, _| game.state().players[0].as_ref().unwrap().money);

    // Put CT close enough for +use and face +X.
    b.run(move |_, eng| {
        let p = eng.client_edict(0);
        eng.set_origin(p, Vec3::new(100.0, 0.0, 36.0));
        if let Some(v) = eng.ent_mut(p) {
            v.v_angle = Vec3::new(0.0, 0.0, 0.0);
        }
    });

    // First +use: starts following and pays +150 to CT and +100 to CT team pool.
    send_use(&mut b, 0, IN_USE as u16);
    send_use(&mut b, 0, 0);
    let (money_after_use, touched, acc_ct) = b.run(|game, _| {
        (
            game.state().players[0].as_ref().unwrap().money,
            game.state().rules.hostages_touched,
            game.state().rules.economy().account_ct(),
        )
    });
    assert_eq!(money_after_use, initial_money + 150);
    assert_eq!(touched, 1);
    assert_eq!(acc_ct, 100);

    // Move CT far enough (>80 u) to observe follow movement, and press +use within < 1.0 s (gated).
    walk_towards(&mut b, 0, 180.0, 250.0, 10);
    send_use(&mut b, 0, IN_USE as u16);
    send_use(&mut b, 0, 0);

    // Move CT out to x = -120 and step 0.4 s: hostage should still follow (was gated!).
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), Vec3::new(-120.0, 0.0, 36.0));
    });
    b.step(40);
    let mid_x = b.run(move |_, eng| eng.ent(hostage).unwrap().origin.x);
    assert!(
        mid_x < 128.0,
        "immediate second +use should be gated (<1s) so hostage keeps following, got x={mid_x}"
    );

    // Wait > 1.0 s from first use, put CT within +use range and toggle back to Stand.
    b.step(70);
    b.run(move |_, eng| {
        let pos = eng.ent(hostage).unwrap().origin;
        eng.set_origin(eng.client_edict(0), Vec3::new(pos.x - 30.0, pos.y, 36.0));
    });
    send_use(&mut b, 0, IN_USE as u16);
    send_use(&mut b, 0, 0);

    // CT moves far away; hostage stands still and no duplicate touch bonus is awarded.
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), Vec3::new(400.0, 0.0, 36.0));
    });
    let stopped_pos = b.run(move |_, eng| eng.ent(hostage).unwrap().origin);
    b.step(120);
    let (end_pos, final_money, final_touched) = b.run(move |game, eng| {
        (
            eng.ent(hostage).unwrap().origin,
            game.state().players[0].as_ref().unwrap().money,
            game.state().rules.hostages_touched,
        )
    });
    assert!(
        (end_pos - stopped_pos).truncate().length() < 5.0,
        "hostage should remain stopped after 1s-gated toggle off: {stopped_pos:?} -> {end_pos:?}"
    );
    assert_eq!(final_money, initial_money + 150);
    assert_eq!(final_touched, 1);

    // Terrorist +use should be refused with #Only_CT_Can_Move_Hostages.
    enter(&mut b, 1);
    join(&mut b, 1, 1, 1);
    // Wait for Game_Commencing restart & weapon deploy, then position T in front of hostage.
    b.step(350);
    for _ in 0..90 {
        b.run(|game, eng| common::commands_of(game, eng, 1, 1));
    }
    b.run(move |_, eng| {
        let pos = eng.ent(hostage).unwrap().origin;
        let t = eng.client_edict(1);
        eng.set_origin(t, Vec3::new(pos.x - 30.0, pos.y, 36.0));
        if let Some(v) = eng.ent_mut(t) {
            v.v_angle = Vec3::ZERO;
        }
    });
    send_use(&mut b, 1, IN_USE as u16);
    b.step(15);
    let t_used = b.run(move |game, _| game.display_history(1).unwrap_or(0) & (1 << 21) != 0);
    assert!(t_used, "terrorist +use should record DHF_HOSTAGE_CTMOVE");
}

#[test]
fn a_hostage_follows_its_leader_around_a_corner() {
    // Wall *1 centered at (220, 0, 64) with half-extents (16, 100, 64):
    // blocks direct X line at y=0, leaving corridor around y > 100.
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", "120 0 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "-200 0 36")]),
        entity(&[("classname", "info_hostage_rescue"), ("origin", "2000 0 36")]),
        entity(&[
            ("classname", "func_wall"),
            ("model", "*1"),
            ("origin", "220 0 64"),
        ]),
        entity(&[("classname", "hostage_entity"), ("origin", "150 0 10")]),
    ];
    let mut b = Bench::on(floor_bsp(entities, &[Vec3::new(16.0, 100.0, 64.0)]));
    spawn_and_ready(&mut b, 0, 2, 1);

    let hostage = find_hostage_edicts(&mut b)[0];
    b.run(|_, eng| {
        let p = eng.client_edict(0);
        eng.set_origin(p, Vec3::new(120.0, 0.0, 36.0));
        if let Some(v) = eng.ent_mut(p) {
            v.v_angle = Vec3::ZERO;
        }
    });
    send_use(&mut b, 0, IN_USE as u16);
    send_use(&mut b, 0, 0);

    // CT moves smoothly around the +Y end of the wall so LocalNav breadcrumbs lead around y > 100.
    let waypoints = [
        Vec3::new(120.0, 140.0, 36.0),
        Vec3::new(220.0, 145.0, 36.0),
        Vec3::new(300.0, 145.0, 36.0),
        Vec3::new(300.0, 0.0, 36.0),
    ];
    for wp in waypoints {
        b.run(move |_, eng| {
            eng.set_origin(eng.client_edict(0), wp);
        });
        b.step(100);
    }

    let pos = b.run(move |_, eng| eng.ent(hostage).unwrap().origin);
    assert!(
        pos.x > 210.0,
        "hostage should have routed around the corner wall to x > 210, got {pos:?}"
    );
}

#[test]
fn hostage_touch_pushes() {
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", "0 200 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "0 -200 36")]),
        entity(&[("classname", "hostage_entity"), ("origin", "150 0 10")]),
    ];
    let mut b = Bench::on(floor_bsp(entities, &[]));
    spawn_and_ready(&mut b, 0, 2, 1);

    let hostage = find_hostage_edicts(&mut b)[0];
    let ct = b.run(|_, eng| eng.client_edict(0));
    let before = b.run(move |_, eng| eng.ent(hostage).unwrap().velocity);
    assert_eq!(before, Vec3::ZERO);

    b.run(move |game, eng| {
        eng.set_origin(ct, Vec3::new(125.0, 0.0, 36.0));
        cs_engine::GameDll::touch(game, eng, hostage, ct);
    });
    let after = b.run(move |_, eng| eng.ent(hostage).unwrap().velocity);
    assert!(
        (after.x - 50.0).abs() < 1e-3 && after.y.abs() < 1e-3,
        "CT touch should apply 50 u/s push away from CT, got {after:?}"
    );

    // Terrorist touch must NOT push the hostage.
    enter(&mut b, 1);
    join(&mut b, 1, 1, 1);
    b.step(350);
    let t = b.run(|_, eng| eng.client_edict(1));
    b.run(move |game, eng| {
        if let Some(v) = eng.ent_mut(hostage) {
            v.velocity = Vec3::ZERO;
        }
        eng.set_origin(t, Vec3::new(175.0, 0.0, 36.0));
        cs_engine::GameDll::touch(game, eng, hostage, t);
    });
    let after_t = b.run(move |_, eng| eng.ent(hostage).unwrap().velocity);
    assert_eq!(after_t, Vec3::ZERO);
}

#[test]
fn killing_a_hostage_costs_money_and_sends_hostage_k() {
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", "0 200 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "0 0 36")]),
        entity(&[("classname", "hostage_entity"), ("origin", "120 0 10")]),
    ];
    let mut b = Bench::on(floor_bsp(entities, &[]));
    spawn_and_ready(&mut b, 0, 2, 1); // Living CT probe in slot 0.
    enter(&mut b, 1);
    join(&mut b, 1, 1, 1); // T attacker in slot 1.
    // Let #Game_Commencing restart finish so `rules.is_restarting()` is false when killing.
    b.step(350);

    let hostage = find_hostage_edicts(&mut b)[0];
    b.run(move |game, _eng| {
        if let Some(p) = game.state_mut().players[1].as_mut() {
            p.money = 10000;
        }
    });
    b.step(10);
    b.probe.clear_messages();

    // Non-lethal injury: costs 20 * actual_damage (chest multiplier 1.5x -> 15 dmg -> -$300).
    let t_id = b.run(|_, eng| eng.client_edict(1));
    let hurt_alive = b.run(move |game, eng| {
        let mut mm = bullets::MultiDamage::new(None);
        mm.clear();
        let tr = cs_engine::EntTrace {
            trace: cs_engine::collision::TraceResult {
                fraction: 0.5,
                end_pos: Vec3::new(110.0, 0.0, 30.0),
                hitgroup: HITGROUP_CHEST,
                ..cs_engine::collision::TraceResult::clear(Vec3::new(110.0, 0.0, 30.0))
            },
            ent: Some(hostage),
        };
        let state = game.state_mut();
        bullets::trace_attack(
            state,
            eng,
            hostage,
            Some(t_id),
            &mut mm,
            10.0,
            Vec3::X,
            &tr,
            cs_game::player::dmg::BULLET,
        );
        bullets::apply_multi_damage(state, eng, &mm, Some(t_id), Some(t_id));
        eng.ent(hostage).unwrap().health
    });
    assert!(hurt_alive < 100.0 && hurt_alive > 0.0);
    let money_after_injure = b.run(|game, _| game.state().players[1].as_ref().unwrap().money);
    assert!(money_after_injure < 10000);

    // Lethal shot: kills hostage, deducts money, sends HostageK(1) to living CT.
    let (deadflag, solid, movetype, money_after_kill) = b.run(move |game, eng| {
        let mut mm = bullets::MultiDamage::new(None);
        mm.clear();
        let tr = cs_engine::EntTrace {
            trace: cs_engine::collision::TraceResult {
                fraction: 0.5,
                end_pos: Vec3::new(110.0, 0.0, 50.0),
                hitgroup: HITGROUP_CHEST,
                ..cs_engine::collision::TraceResult::clear(Vec3::new(110.0, 0.0, 50.0))
            },
            ent: Some(hostage),
        };
        let state = game.state_mut();
        bullets::trace_attack(
            state,
            eng,
            hostage,
            Some(t_id),
            &mut mm,
            200.0,
            Vec3::X,
            &tr,
            cs_game::player::dmg::BULLET,
        );
        bullets::apply_multi_damage(state, eng, &mm, Some(t_id), Some(t_id));
        let hv = eng.ent(hostage).unwrap();
        (
            hv.deadflag,
            hv.solid,
            hv.movetype,
            state.players[1].as_ref().unwrap().money,
        )
    });
    assert_eq!(deadflag, DEAD_DEAD);
    assert_eq!(solid, SOLID_NOT);
    assert_eq!(movetype, MOVETYPE_TOSS);
    assert!(money_after_kill < money_after_injure - 500);

    b.step(5);
    assert!(
        b.messages().contains(&CsUserMsg::HostageK(1)),
        "living CT must receive HostageK(1), got {:?}",
        b.messages()
    );

    // After 3 seconds Remove() stops thinking and zeroes size.
    b.step(320);
    let nextthink_after_remove = b.run(move |_, eng| eng.ent(hostage).unwrap().nextthink);
    assert_eq!(nextthink_after_remove, 0.0);
}

#[test]
fn rescue_in_the_zone_pays_the_rescuer() {
    // Brush *1 is func_hostage_rescue at (300, 0, 32) with half-extents (64, 64, 64).
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", "100 0 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "-200 0 36")]),
        entity(&[
            ("classname", "func_hostage_rescue"),
            ("model", "*1"),
            ("origin", "300 0 32"),
        ]),
        entity(&[("classname", "hostage_entity"), ("origin", "130 0 10")]),
    ];
    let mut b = Bench::on(floor_bsp(entities, &[Vec3::new(64.0, 64.0, 64.0)]));
    spawn_and_ready(&mut b, 0, 2, 1);

    let hostage = find_hostage_edicts(&mut b)[0];
    b.run(|_, eng| {
        let p = eng.client_edict(0);
        eng.set_origin(p, Vec3::new(100.0, 0.0, 36.0));
        if let Some(v) = eng.ent_mut(p) {
            v.v_angle = Vec3::ZERO;
        }
    });
    send_use(&mut b, 0, IN_USE as u16);
    send_use(&mut b, 0, 0);
    let money_after_touch = b.run(|game, _| game.state().players[0].as_ref().unwrap().money);
    b.probe.clear_messages();

    // CT steps directly into the rescue zone; hostage follows into func_hostage_rescue.
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), Vec3::new(300.0, 0.0, 36.0));
    });
    b.step(200);

    let (money_after_rescue, rescued_count, hv_effects, hv_solid, hv_deadflag) =
        b.run(move |game, eng| {
            let hv = eng.ent(hostage).unwrap();
            (
                game.state().players[0].as_ref().unwrap().money,
                game.state().rules.hostages_rescued,
                hv.effects,
                hv.solid,
                hv.deadflag,
            )
        });
    assert_eq!(money_after_rescue, money_after_touch + 1000);
    assert_eq!(rescued_count, 1);
    assert_ne!(hv_effects & EF_NODRAW, 0);
    assert_eq!(hv_solid, SOLID_NOT);
    assert_eq!(hv_deadflag, DEAD_RESPAWNABLE);

    let msgs = b.messages();
    assert!(msgs.contains(&CsUserMsg::HostageK(1)));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::SendAudio { audio_code, .. } if audio_code == "%!MRAD_rescued"
    )));
}

#[test]
fn hostages_restart_to_their_spawn() {
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[("classname", "info_player_start"), ("origin", "100 0 36")]),
        entity(&[("classname", "info_player_deathmatch"), ("origin", "-200 0 36")]),
        entity(&[
            ("classname", "func_hostage_rescue"),
            ("model", "*1"),
            ("origin", "300 0 32"),
        ]),
        entity(&[
            ("classname", "hostage_entity"),
            ("origin", "130 0 10"),
            ("angles", "0 45 0"),
        ]),
    ];
    let mut b = Bench::on(floor_bsp(entities, &[Vec3::new(64.0, 64.0, 64.0)]));
    spawn_and_ready(&mut b, 0, 2, 1);

    let hostage = find_hostage_edicts(&mut b)[0];
    let spawn_origin = b.run(move |_, eng| eng.ent(hostage).unwrap().origin);

    // Rescue the hostage (moves it away, sets EF_NODRAW, SOLID_NOT, DEAD_RESPAWNABLE).
    b.run(|_, eng| {
        let p = eng.client_edict(0);
        eng.set_origin(p, Vec3::new(100.0, 0.0, 36.0));
        if let Some(v) = eng.ent_mut(p) {
            v.v_angle = Vec3::ZERO;
        }
    });
    send_use(&mut b, 0, IN_USE as u16);
    send_use(&mut b, 0, 0);
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), Vec3::new(300.0, 0.0, 36.0));
    });
    b.step(200);
    assert_eq!(b.run(|game, _| game.state().rules.hostages_rescued), 1);

    // Restart the round via sv_restart 1 (polled at 1 Hz + 1 s countdown).
    b.run(|_, eng| eng.set_cvar("sv_restart", "1"));
    b.step(250);

    let (origin, angles, health, deadflag, solid, effects, flags, rescued, touched) =
        b.run(move |game, eng| {
            let hv = eng.ent(hostage).unwrap();
            (
                hv.origin,
                hv.angles,
                hv.health,
                hv.deadflag,
                hv.solid,
                hv.effects,
                hv.flags,
                game.state().rules.hostages_rescued,
                game.state().rules.hostages_touched,
            )
        });
    assert!((origin - spawn_origin).length() < 1.0, "{origin:?} != {spawn_origin:?}");
    assert!((angles.y - 45.0).abs() < 1e-3);
    assert_eq!(health, 100.0);
    assert_eq!(deadflag, DEAD_NO);
    assert_eq!(solid, SOLID_SLIDEBOX);
    assert_eq!(effects & EF_NODRAW, 0);
    assert_ne!(flags & FL_ONGROUND, 0);
    assert_eq!(rescued, 0);
    assert_eq!(touched, 0);
}

#[test]
fn cs_office_hostages_spawn_as_hlds() {
    let Some(vfs) = discover() else {
        return;
    };
    let mut b = Bench::on(map_named(&vfs, "cs_office"));
    b.idle_cmds = true;

    let (hostages, mins, maxs, model_name, has_model, has_smoke) = b.run(|_, eng| {
        let mut ids = Vec::new();
        let mut cur = None;
        while let Some(id) = eng.find_by_classname(cur, "hostage_entity") {
            ids.push(id);
            cur = Some(id);
        }
        let first = eng.ent(ids[0]).unwrap();
        let m = first.model.clone();
        let has_model = !m.is_empty() && eng.model_index(&m).is_some();
        let has_smoke = eng.model_index("sprites/smoke.spr").is_some();
        (ids, first.mins, first.maxs, m, has_model, has_smoke)
    });
    assert_eq!(hostages.len(), 4);
    assert_eq!(mins, Vec3::new(-10.0, -10.0, 0.0));
    assert_eq!(maxs, Vec3::new(10.0, 10.0, 62.0));
    assert!(has_model && has_smoke, "model={model_name:?} has_model={has_model} has_smoke={has_smoke}");

    // When a CT spawns, InitHUD / UpdateClientData sends HostagePos(flag=1) for each hostage 1..=4.
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 2");
    command(&mut b, 0, "joinclass 1");
    b.step(5);

    let pos_ids: Vec<u8> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::HostagePos {
                flag: 1,
                hostage_id,
                ..
            } => Some(hostage_id),
            _ => None,
        })
        .collect();
    assert_eq!(pos_ids, vec![1, 2, 3, 4]);
}
