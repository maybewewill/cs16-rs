//! Step 2 Phase 7 integration tests: doors (`func_door`, `func_water`, `func_door_rotating`,
//! `momentary_door`), buttons (`func_button`, `func_rot_button`, `momentary_rot_button`,
//! `button_target`), breakables & utility brushes (`func_breakable`, `func_wall_toggle`,
//! `func_friction`, `func_rotating`, `func_pendulum`, `func_grencatch`, `func_weaponcheck`,
//! `func_guntarget`), and movers (`func_plat`, `func_platrot`, `func_train`, `path_corner`,
//! `path_track`, `func_tracktrain`, `func_traincontrols`, `func_vehicle`, `func_vehiclecontrols`,
//! `func_pushable`, `func_conveyor`).

mod common;

use common::{Bench, entity, stub_spawns};
use cs_engine::consts::{EF_NODRAW, FL_CONVEYOR, SOLID_NOT};
use cs_engine::{EdictId, EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_game::weapons::{WeaponId, new_weapon};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::tempent::te;
use glam::Vec3;

fn setup_bench(extra_entities: Vec<cs_assets::bsp::Entity>, slots: u8) -> Bench {
    let mut map = stub_spawns();
    map.extend(extra_entities);
    let mut b = Bench::with_userinfo(
        cs_engine::ServerMap::stub_with_entities("test", map),
        slots,
        &[],
    );
    b.idle_cmds = true;
    b.step(4);
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_freezetime", "0");
        for s in 1..slots {
            let _ = game.client_connect(eng, s);
            game.client_put_in_server(eng, s);
            eng.test_set_client_active(s, true);
        }
        game.restart_round_now(eng);
        for s in 0..slots {
            let team = if s % 2 == 0 { Team::Ct } else { Team::Terrorist };
            assert!(game.spawn_player(eng, s, team, 0));
        }
    });
    b.step(2);
    b.probe.clear_messages();
    b
}

fn spawn_box(
    game: &mut CsGame,
    eng: &mut EngineApi<'_>,
    classname: &str,
    origin: Vec3,
    angles: Vec3,
    size: Vec3,
    spawnflags: u32,
    keys: &[(&str, &str)],
) -> EdictId {
    let id = eng.create_entity(classname).expect("free edict");
    eng.set_origin(id, origin);
    eng.set_size(id, Vec3::ZERO, size);
    if let Some(v) = eng.ent_mut(id) {
        v.angles = angles;
        v.spawnflags = spawnflags;
        for (k, val) in keys {
            match *k {
                "targetname" => v.targetname = (*val).to_string(),
                "target" => v.target = (*val).to_string(),
                _ => {}
            }
        }
    }
    let kv: Vec<(String, String)> = keys
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    assert!(game.spawn_entity(eng, id, classname, &kv), "spawn {classname}");
    id
}

#[test]
fn doors_and_buttons_open_close_rotate_and_restart() {
    let extra = vec![entity(&[
        ("classname", "test_target"),
        ("targetname", "door_done"),
        ("caps", "8"),
    ])];
    let mut b = setup_bench(extra, 1);

    let (door_id, rot_door_id, mom_door_id, btn_id, rot_btn_id, mom_btn_id, btarg_id, water_id) =
        b.run(|game, eng| {
            let door = spawn_box(
                game,
                eng,
                "func_door",
                Vec3::new(0.0, 300.0, 0.0),
                Vec3::new(0.0, 90.0, 0.0),
                Vec3::new(64.0, 64.0, 64.0),
                0,
                &[
                    ("targetname", "slide_door"),
                    ("target", "door_done"),
                    ("speed", "200"),
                    ("wait", "0.2"),
                ],
            );
            let rot_door = spawn_box(
                game,
                eng,
                "func_door_rotating",
                Vec3::new(200.0, 0.0, 0.0),
                Vec3::ZERO,
                Vec3::new(32.0, 32.0, 64.0),
                0,
                &[
                    ("targetname", "rot_door"),
                    ("distance", "90"),
                    ("speed", "300"),
                    ("wait", "-1"),
                ],
            );
            let mom_door = spawn_box(
                game,
                eng,
                "momentary_door",
                Vec3::new(400.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 64.0),
                0,
                &[("targetname", "mom_door"), ("speed", "400")],
            );
            let btn = spawn_box(
                game,
                eng,
                "func_button",
                Vec3::new(50.0, 0.0, 36.0),
                Vec3::ZERO,
                Vec3::new(16.0, 16.0, 16.0),
                0,
                &[
                    ("targetname", "press_btn"),
                    ("target", "slide_door"),
                    ("wait", "0.2"),
                ],
            );
            let rot_btn = spawn_box(
                game,
                eng,
                "func_rot_button",
                Vec3::new(200.0, 200.0, 36.0),
                Vec3::ZERO,
                Vec3::new(16.0, 16.0, 16.0),
                32, // SF_BUTTON_TOGGLE
                &[
                    ("targetname", "rot_btn"),
                    ("target", "rot_door"),
                    ("distance", "45"),
                    ("speed", "450"),
                ],
            );
            let mom_btn = spawn_box(
                game,
                eng,
                "momentary_rot_button",
                Vec3::new(300.0, 200.0, 36.0),
                Vec3::ZERO,
                Vec3::new(16.0, 16.0, 16.0),
                0,
                &[
                    ("targetname", "wheel_btn"),
                    ("target", "mom_door"),
                    ("distance", "100"),
                    ("speed", "500"),
                ],
            );
            let btarg = spawn_box(
                game,
                eng,
                "button_target",
                Vec3::new(400.0, 200.0, 36.0),
                Vec3::ZERO,
                Vec3::new(16.0, 16.0, 16.0),
                1, // SF_BTARGET_USE
                &[("targetname", "shoot_btn"), ("target", "door_done")],
            );
            let water = spawn_box(
                game,
                eng,
                "func_water",
                Vec3::new(-200.0, 0.0, 0.0),
                Vec3::ZERO,
                Vec3::new(64.0, 64.0, 32.0),
                0,
                &[("skin", "-3")],
            );
            assert_eq!(eng.ent(water).unwrap().solid, SOLID_NOT);
            assert_eq!(eng.ent(water).unwrap().skin, -3);

            // Place CT slot 0 aiming right at `press_btn` ([50..66, 0..16, 36..52]) and +use.
            let p0 = eng.client_edict(0);
            eng.set_origin(p0, Vec3::new(35.0, 8.0, 28.0));
            eng.ent_mut(p0).unwrap().v_angle = Vec3::ZERO;
            eng.ent_mut(p0).unwrap().button = 1 << 5;
            common::run_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    buttons: 1 << 5, // IN_USE
                    ..Default::default()
                },
                0,
            );

            game.test_use_entity(eng, rot_btn);
            game.test_use_entity(eng, mom_btn);
            game.test_damage_entity(eng, btarg, Some(p0), 10.0, cs_game::player::dmg::BULLET);
            (door, rot_door, mom_door, btn, rot_btn, mom_btn, btarg, water)
        });
    let _ = water_id;

    b.step(8);

    let (door_y, rot_yaw, mom_x, btarg_frame) = b.run(move |_game, eng| {
        (
            eng.ent(door_id).unwrap().origin.y,
            eng.ent(rot_door_id).unwrap().angles.y,
            eng.ent(mom_door_id).unwrap().origin.x,
            eng.ent(btarg_id).unwrap().frame,
        )
    });
    assert!(door_y > 300.0, "func_button should open func_door along +Y, got {door_y}");
    assert!(rot_yaw > 15.0, "func_rot_button should rotate func_door_rotating, got {rot_yaw}");
    assert!(mom_x > 400.0, "momentary_rot_button should drive momentary_door past 400, got {mom_x}");
    assert_eq!(btarg_frame, 1.0, "button_target toggles frame on damage");

    // Step past door/button wait so auto-return completes, and restart resets everything.
    b.step(40);
    b.run(move |game, eng| {
        game.restart_round_now(eng);
        assert_eq!(eng.ent(door_id).unwrap().origin, Vec3::new(0.0, 300.0, 0.0));
        assert_eq!(eng.ent(rot_door_id).unwrap().angles, Vec3::ZERO);
        assert_eq!(eng.ent(mom_door_id).unwrap().origin, Vec3::new(400.0, 0.0, 0.0));
        assert_eq!(eng.ent(btn_id).unwrap().origin, Vec3::new(50.0, 0.0, 36.0));
        assert_eq!(eng.ent(rot_btn_id).unwrap().angles, Vec3::ZERO);
        assert_eq!(eng.ent(mom_btn_id).unwrap().angles, Vec3::ZERO);
        assert_eq!(eng.ent(btarg_id).unwrap().frame, 0.0);
    });
}

#[test]
fn breakables_and_utility_brushes_work_and_reset_on_round_restart() {
    let extra = vec![
        entity(&[
            ("classname", "test_target"),
            ("targetname", "brk_fired"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "test_target"),
            ("targetname", "gren_caught"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "test_target"),
            ("targetname", "wcheck_pass"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "test_target"),
            ("targetname", "wcheck_fail"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "test_target"),
            ("targetname", "gun_done"),
            ("caps", "8"),
        ]),
        entity(&[
            ("classname", "path_corner"),
            ("targetname", "gt_c1"),
            ("origin", "300 0 0"),
            ("target", "gt_c2"),
        ]),
        entity(&[
            ("classname", "path_corner"),
            ("targetname", "gt_c2"),
            ("origin", "300 200 0"),
            ("target", "gt_c1"),
        ]),
    ];
    let mut b = setup_bench(extra, 1);

    let (brk_id, wall_id, fric_id, rot_id, pend_id, gren_id, wcheck_id, guntarg_id) =
        b.run(|game, eng| {
            let brk = spawn_box(
                game,
                eng,
                "func_breakable",
                Vec3::new(100.0, 0.0, 0.0),
                Vec3::ZERO,
                Vec3::new(32.0, 32.0, 32.0),
                0,
                &[
                    ("targetname", "glass1"),
                    ("target", "brk_fired"),
                    ("health", "25"),
                    ("material", "0"),
                ],
            );
            let wall = spawn_box(
                game,
                eng,
                "func_wall_toggle",
                Vec3::new(200.0, 0.0, 0.0),
                Vec3::ZERO,
                Vec3::new(32.0, 32.0, 32.0),
                1, // SF_WALL_START_OFF
                &[("targetname", "tog_wall")],
            );
            assert_eq!(eng.ent(wall).unwrap().solid, SOLID_NOT);
            assert_ne!(eng.ent(wall).unwrap().effects & EF_NODRAW, 0);

            let fric = spawn_box(
                game,
                eng,
                "func_friction",
                Vec3::new(0.0, 100.0, 0.0),
                Vec3::ZERO,
                Vec3::new(64.0, 64.0, 16.0),
                0,
                &[("modifier", "25")],
            );
            let rot = spawn_box(
                game,
                eng,
                "func_rotating",
                Vec3::new(0.0, 200.0, 0.0),
                Vec3::ZERO,
                Vec3::new(32.0, 32.0, 32.0),
                1, // START_ON
                &[("speed", "180")],
            );
            let pend = spawn_box(
                game,
                eng,
                "func_pendulum",
                Vec3::new(0.0, 300.0, 0.0),
                Vec3::ZERO,
                Vec3::new(16.0, 16.0, 64.0),
                1, // START_ON
                &[("distance", "45"), ("speed", "200")],
            );
            let gren = spawn_box(
                game,
                eng,
                "func_grencatch",
                Vec3::new(50.0, 50.0, 0.0),
                Vec3::ZERO,
                Vec3::new(64.0, 64.0, 64.0),
                0,
                &[("grenadetype", "smoke"), ("triggerongrenade", "gren_caught")],
            );
            let wcheck = spawn_box(
                game,
                eng,
                "func_weaponcheck",
                Vec3::new(150.0, 150.0, 0.0),
                Vec3::ZERO,
                Vec3::new(64.0, 64.0, 64.0),
                0,
                &[
                    ("item1", "weapon_ak47"),
                    ("target", "wcheck_pass"),
                    ("netname", "wcheck_fail"),
                ],
            );
            let guntarg = spawn_box(
                game,
                eng,
                "func_guntarget",
                Vec3::new(300.0, 0.0, 0.0),
                Vec3::ZERO,
                Vec3::new(32.0, 32.0, 32.0),
                1, // START_ON
                &[
                    ("target", "gt_c1"),
                    ("message", "gun_done"),
                    ("health", "30"),
                    ("speed", "120"),
                ],
            );

            let p0 = eng.client_edict(0);
            // Break the glass with 30 damage.
            game.test_damage_entity(eng, brk, Some(p0), 30.0, 2);
            assert_eq!(eng.ent(brk).unwrap().solid, SOLID_NOT);
            assert_ne!(eng.ent(brk).unwrap().effects & EF_NODRAW, 0);

            // Toggle wall on.
            game.test_use_entity(eng, wall);
            assert_ne!(eng.ent(wall).unwrap().solid, SOLID_NOT);

            // Touch func_friction.
            game.touch(eng, fric, p0);
            assert!((eng.ent(p0).unwrap().friction - 0.25).abs() < 1e-4);

            // Simulate smoke grenade touching func_grencatch.
            let fake_smoke = eng.create_entity("grenade").unwrap();
            eng.ent_mut(fake_smoke).unwrap().model = "models/w_smokegrenade.mdl".to_string();
            game.touch(eng, gren, fake_smoke);

            // WeaponCheck fails without AK, then passes after giving AK.
            game.test_use_entity_by(eng, wcheck, p0);
            assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Ak47).unwrap(), 30, 90));
            game.test_use_entity_by(eng, wcheck, p0);

            // Shoot func_guntarget to 0 health.
            game.test_damage_entity(eng, guntarg, Some(p0), 40.0, 2);

            (brk, wall, fric, rot, pend, gren, wcheck, guntarg)
        });
    let _ = fric_id;

    b.step(15);

    let got_breakmodel = b.probe.messages().iter().any(|m| {
        matches!(m, SvcMessage::TempEntity { te_type, .. } if *te_type == te::BREAKMODEL)
    });
    assert!(got_breakmodel, "func_breakable should emit TE_BREAKMODEL");

    let (rot_avel, pend_ang, dispatched) = b.run(move |game, eng| {
        let mut names = Vec::new();
        for ev in &game.state().dispatch_log {
            if let cs_game::entity::DispatchEvent::Use { id, .. } = ev {
                names.push(eng.ent(*id).unwrap().targetname.clone());
            }
        }
        (
            eng.ent(rot_id).unwrap().avelocity.y,
            eng.ent(pend_id).unwrap().angles.x,
            names,
        )
    });
    assert!(rot_avel.abs() > 100.0, "func_rotating should spin");
    assert!(pend_ang.abs() > 0.0, "func_pendulum should swing");
    for expected in ["brk_fired", "gren_caught", "wcheck_fail", "wcheck_pass", "gun_done"] {
        assert!(
            dispatched.iter().any(|n| n == expected),
            "expected `{expected}` to fire, got {dispatched:?}"
        );
    }

    // Round restart restores breakable, wall toggle, grencatch, guntarget.
    b.run(move |game, eng| {
        game.restart_round_now(eng);
        assert_ne!(eng.ent(brk_id).unwrap().solid, SOLID_NOT);
        assert_eq!(eng.ent(brk_id).unwrap().effects & EF_NODRAW, 0);
        assert_eq!(eng.ent(brk_id).unwrap().health, 25.0);
        assert_eq!(eng.ent(wall_id).unwrap().solid, SOLID_NOT);
        assert_eq!(eng.ent(guntarg_id).unwrap().health, 30.0);
        let _ = (gren_id, wcheck_id);
    });
}

#[test]
fn movers_tracktrain_and_vehicle_move_and_send_train_hud() {
    let extra = vec![
        entity(&[
            ("classname", "path_corner"),
            ("targetname", "pc1"),
            ("origin", "100 0 0"),
            ("target", "pc2"),
        ]),
        entity(&[
            ("classname", "path_corner"),
            ("targetname", "pc2"),
            ("origin", "300 0 0"),
            ("target", "pc1"),
        ]),
        entity(&[
            ("classname", "path_track"),
            ("targetname", "pt1"),
            ("origin", "0 200 0"),
            ("target", "pt2"),
        ]),
        entity(&[
            ("classname", "path_track"),
            ("targetname", "pt2"),
            ("origin", "200 200 0"),
            ("target", "pt1"),
        ]),
    ];
    let mut b = setup_bench(extra, 1);

    let (plat_id, platrot_id, train_id, ttrain_id, veh_id, conv_id, push_id) = b.run(|game, eng| {
        let plat = spawn_box(
            game,
            eng,
            "func_plat",
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 80.0),
            0,
            &[("height", "60"), ("speed", "200")],
        );
        let platrot = spawn_box(
            game,
            eng,
            "func_platrot",
            Vec3::new(100.0, 0.0, 100.0),
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 80.0),
            0,
            &[("height", "60"), ("rotation", "90"), ("speed", "200")],
        );
        let train = spawn_box(
            game,
            eng,
            "func_train",
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::ZERO,
            Vec3::new(32.0, 32.0, 32.0),
            0,
            &[("target", "pc1"), ("speed", "200")],
        );
        let ttrain = spawn_box(
            game,
            eng,
            "func_tracktrain",
            Vec3::new(0.0, 200.0, 0.0),
            Vec3::ZERO,
            Vec3::new(32.0, 32.0, 32.0),
            0,
            &[("targetname", "my_ttrain"), ("target", "pt1"), ("speed", "200")],
        );
        let tctrl = spawn_box(
            game,
            eng,
            "func_traincontrols",
            Vec3::new(0.0, 200.0, 16.0),
            Vec3::ZERO,
            Vec3::new(16.0, 16.0, 16.0),
            0,
            &[("target", "my_ttrain")],
        );
        let veh = spawn_box(
            game,
            eng,
            "func_vehicle",
            Vec3::new(0.0, -200.0, 0.0),
            Vec3::ZERO,
            Vec3::new(64.0, 32.0, 32.0),
            0,
            &[("targetname", "my_apc"), ("speed", "300")],
        );
        let vctrl = spawn_box(
            game,
            eng,
            "func_vehiclecontrols",
            Vec3::new(0.0, -200.0, 16.0),
            Vec3::ZERO,
            Vec3::new(16.0, 16.0, 16.0),
            0,
            &[("target", "my_apc")],
        );
        let conv = spawn_box(
            game,
            eng,
            "func_conveyor",
            Vec3::new(0.0, 400.0, 0.0),
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 16.0),
            0,
            &[("speed", "120")],
        );
        let push = spawn_box(
            game,
            eng,
            "func_pushable",
            Vec3::new(0.0, 500.0, 0.0),
            Vec3::ZERO,
            Vec3::new(32.0, 32.0, 32.0),
            0,
            &[("friction", "100")],
        );

        assert_ne!(eng.ent(conv).unwrap().flags & FL_CONVEYOR, 0);

        let p0 = eng.client_edict(0);
        game.test_use_entity(eng, plat);
        game.test_use_entity(eng, platrot);
        game.test_use_entity_by(eng, tctrl, p0);
        game.test_use_entity_by(eng, vctrl, p0);
        (plat, platrot, train, ttrain, veh, conv, push)
    });

    b.step(12);

    let train_msgs: Vec<Vec<u8>> = b
        .probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::UserMsg {
                name,
                decoded: cs_proto48::usermsg::CsUserMsg::Unknown { payload, .. },
                ..
            } if name == "Train" => Some(payload.clone()),
            _ => None,
        })
        .collect();
    assert!(
        train_msgs.iter().any(|p| p.first().copied().unwrap_or(0) > 0),
        "Train usermsg should be emitted on tracktrain/vehicle use, got {train_msgs:?}"
    );

    b.run(move |game, eng| {
        assert!(eng.ent(ttrain_id).unwrap().velocity.length() > 1.0);
        assert!(eng.ent(veh_id).unwrap().velocity.length() > 1.0);
        assert!(eng.ent(platrot_id).unwrap().angles.y.abs() > 0.0 || eng.ent(platrot_id).unwrap().origin.z > 40.0);
        game.restart_round_now(eng);
        assert_eq!(eng.ent(ttrain_id).unwrap().velocity, Vec3::ZERO);
        assert_eq!(eng.ent(veh_id).unwrap().velocity, Vec3::ZERO);
        let _ = (plat_id, train_id, conv_id, push_id);
    });
}
