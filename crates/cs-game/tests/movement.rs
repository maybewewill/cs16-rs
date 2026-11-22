//! The player's movement through `cs_sim::pm` and the game's part of `SV_RunCmd`
//! ([`cs_game::pmove`] is private; everything is seen through the edict and the client data).
//! Stub-map tests run anywhere; the de_dust2 tests return without assertions when the game is
//! not installed.

mod common;

use common::{Bench, discover, dust2, spawns};
use cs_engine::consts::{FL_DUCKING, FL_ONGROUND};
use cs_engine::{EdictId, EngineApi, EntVars, GameDll};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_sim::trace::Hull;
use glam::Vec3;

const IN_JUMP: u16 = 1 << 1;
const IN_DUCK: u16 = 1 << 2;
const IN_FORWARD: u16 = 1 << 3;

fn cmd(msec: u8, buttons: u16, forwardmove: f32, yaw: f32) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        buttons,
        forwardmove,
        viewangles: Vec3::new(0.0, yaw, 0.0),
        ..Default::default()
    }
}

fn idle(msec: u8) -> GoldSrcUserCmd {
    cmd(msec, 0, 0.0, 0.0)
}

fn vars(eng: &EngineApi<'_>, slot: u8) -> EntVars {
    eng.ent(eng.client_edict(slot)).unwrap().clone()
}

/// A stub-map bench whose probe is spawned as a counter-terrorist and does not fall
/// (`sv_gravity 0`); there is no floor, so the player is always in the air.
fn stub_bench() -> Bench {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
    });
    b
}

/// A dust2 bench with the probe spawned as a counter-terrorist and settled on the floor.
fn dust2_bench() -> Option<Bench> {
    let vfs = discover()?;
    let mut b = Bench::on(dust2(&vfs));
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        for _ in 0..50 {
            game.run_usercmd(eng, 0, &idle(10), 0);
        }
        assert_ne!(vars(eng, 0).flags & FL_ONGROUND, 0, "settled on the floor");
        // The weapons set the speed; the movement takes the entity's `maxspeed`.
        eng.ent_mut(eng.client_edict(0)).unwrap().max_speed = 250.0;
    });
    Some(b)
}

fn horizontal(v: Vec3) -> f32 {
    v.truncate().length()
}

#[test]
fn run_cmd_follows_sv_runcmd() {
    let mut b = stub_bench();
    b.run(|game, eng| {
        let id = eng.client_edict(0);
        // `fixangle` is cleared once the engine has sent the spawn angles.
        let v = eng.ent_mut(id).unwrap();
        v.fixangle = 0;
        v.v_angle = Vec3::new(5.0, 10.0, 0.0);
        v.velocity = Vec3::new(100.0, 0.0, 0.0);
        v.flags |= FL_ONGROUND;
        assert!(game.set_velocity_modifier(0, 0.5));
        let c = GoldSrcUserCmd {
            viewangles: Vec3::new(20.0, 90.0, 0.0),
            ..cmd(10, IN_FORWARD, 0.0, 90.0)
        };
        game.run_usercmd(eng, 0, &c, 7);
        let v = vars(eng, 0);
        // `PlayerPreThink` runs before the movement: on the ground the modifier takes
        // `velocity *= 0.5 + 0.01` first (the world has no floor, so the movement then finds
        // the player in the air and keeps the speed).
        assert_eq!(game.velocity_modifier(0), Some(0.51));
        assert!((v.velocity.x - 51.0).abs() < 1e-3, "{:?}", v.velocity);
        // `SV_RunCmd` takes the command's angles (`fixangle` 0) and `SV_FinishPMove` writes the
        // model angles: a third of the pitch (negated), the whole yaw.
        assert_eq!(v.v_angle, Vec3::new(20.0, 90.0, 0.0));
        assert_eq!(v.angles, Vec3::new(-20.0 / 3.0, 90.0, 0.0));
        // `SV_FinishPMove`: the ground flag follows the movement's categorisation, the buttons
        // are kept for the next command, the hull is the standing one.
        assert_eq!(v.flags & FL_ONGROUND, 0);
        assert_eq!(v.oldbuttons, i32::from(IN_FORWARD));
        assert_eq!(
            (v.mins, v.maxs),
            (Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0))
        );
        assert_eq!(v.view_ofs, Vec3::new(0.0, 0.0, 17.0));
    });
    // The angles the game kept: with `fixangle` set the command's angles are not taken.
    b.run(|game, eng| {
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.fixangle = 1;
        v.v_angle = Vec3::new(1.0, 2.0, 0.0);
        game.run_usercmd(eng, 0, &cmd(10, 0, 0.0, 170.0), 0);
        let v = vars(eng, 0);
        assert_eq!(v.v_angle.y, 2.0, "fixangle keeps the game's view angles");
    });
}

#[test]
fn zero_msec_command_runs_think() {
    let mut b = stub_bench();
    b.run(|game, eng| {
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.fixangle = 0;
        v.velocity = Vec3::new(100.0, 0.0, 0.0);
        v.flags |= FL_ONGROUND;
        assert!(game.set_velocity_modifier(0, 0.5));
        let origin = vars(eng, 0).origin;
        // 0 ms: `PreThink` (the modifier), `PM_Move` with a frame time of 0 (the buttons are
        // taken, nothing moves) and `PostThink` all run.
        game.run_usercmd(eng, 0, &cmd(0, IN_FORWARD, 0.0, 33.0), 0);
        let v = vars(eng, 0);
        assert_eq!(game.velocity_modifier(0), Some(0.51));
        assert!((v.velocity.x - 51.0).abs() < 1e-3);
        assert_eq!(v.oldbuttons, i32::from(IN_FORWARD));
        assert_eq!(v.v_angle.y, 33.0);
        assert_eq!(v.origin, origin, "no time, no movement");
        assert!(v.origin.is_finite() && v.velocity.is_finite());
    });
}

#[test]
fn zero_msec_jump_still_acts_on_dust2() {
    let Some(mut b) = dust2_bench() else {
        return;
    };
    b.run(|game, eng| {
        let before = vars(eng, 0);
        game.run_usercmd(eng, 0, &cmd(0, IN_JUMP, 0.0, 0.0), 0);
        let v = vars(eng, 0);
        // `PM_Jump` sets the jump speed whatever the frame time: sqrt(2 * 800 * 45).
        assert!((v.velocity.z - 268.3282).abs() < 0.01, "{}", v.velocity.z);
        assert_eq!(v.flags & FL_ONGROUND, 0);
        assert_eq!(v.origin, before.origin);
    });
}

#[test]
fn walking_reaches_250_on_dust2() {
    let Some(mut b) = dust2_bench() else {
        return;
    };
    let (speed, peak, moved) = b.run(|game, eng| {
        let id = eng.client_edict(0);
        let start = vars(eng, 0).origin;
        // A direction with room to run.
        let yaw = (0..16)
            .map(|i| i as f32 * 22.5)
            .find(|&yaw| {
                let dir = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
                eng.trace_hull(start, start + dir * 200.0, Hull::Standing)
                    .fraction
                    >= 1.0
            })
            .expect("a free direction from the spawn point");
        let mut peak = 0.0f32;
        for _ in 0..60 {
            game.run_usercmd(eng, 0, &cmd(10, IN_FORWARD, 400.0, yaw), 0);
            peak = peak.max(horizontal(eng.ent(id).unwrap().velocity));
        }
        let v = vars(eng, 0);
        assert_ne!(v.flags & FL_ONGROUND, 0);
        (
            horizontal(v.velocity),
            peak,
            (v.origin - start).truncate().length(),
        )
    });
    // Friction and acceleration settle at the speed `maxspeed` allows, and never above it.
    assert!((speed - 250.0).abs() < 0.5, "walking speed {speed}");
    assert!(peak <= 250.01, "peak {peak}");
    assert!(moved > 60.0, "moved {moved}");
}

#[test]
fn jump_apex_is_45_units() {
    let Some(mut b) = dust2_bench() else {
        return;
    };
    let (rise, landed) = b.run(|game, eng| {
        let id = eng.client_edict(0);
        let floor = vars(eng, 0).origin.z;
        let mut apex = floor;
        game.run_usercmd(eng, 0, &cmd(10, IN_JUMP, 0.0, 0.0), 0);
        let mut airborne = false;
        for _ in 0..200 {
            game.run_usercmd(eng, 0, &idle(10), 0);
            let v = eng.ent(id).unwrap();
            apex = apex.max(v.origin.z);
            airborne |= v.flags & FL_ONGROUND == 0;
            if airborne && v.flags & FL_ONGROUND != 0 {
                break;
            }
        }
        let v = vars(eng, 0);
        (
            apex - floor,
            (v.flags & FL_ONGROUND != 0, (v.origin.z - floor).abs()),
        )
    });
    // `PM_JumpHeight`: v = sqrt(2 * 800 * 45), so the apex is 45 units up (10 ms steps with
    // `PM_AddCorrectGravity`/`PM_FixupGravityVelocity` land within a unit of it).
    assert!((rise - 45.0).abs() < 1.0, "apex {rise}");
    assert!(landed.0 && landed.1 < 0.1, "landed {landed:?}");
}

#[test]
fn duck_view_and_hull_over_four_tenths() {
    let Some(mut b) = dust2_bench() else {
        return;
    };
    b.run(|game, eng| {
        // Holding duck: the view eases from 17 to 12 over 0.4 s, the hull stays standing until
        // then and `FL_DUCKING` comes with the end of the transition.
        for _ in 0..20 {
            game.run_usercmd(eng, 0, &cmd(10, IN_DUCK, 0.0, 0.0), 0);
        }
        let v = vars(eng, 0);
        assert!(v.b_in_duck, "in the middle of the duck");
        assert_eq!(v.flags & FL_DUCKING, 0);
        // QUIRK of `PM_Duck`: during the transition the view offset runs from 17 towards
        // `12 - 18 = -6` (the hull's own drop is added to the ducked view), and `12` is set
        // only when the transition ends.
        assert!(
            v.view_ofs.z < 17.0 && v.view_ofs.z > -6.0,
            "view {}",
            v.view_ofs.z
        );
        assert!(
            v.fl_duck_time > 0 && v.fl_duck_time < 1000,
            "flDuckTime {}",
            v.fl_duck_time
        );
        // 5 more tenths of a second: ducked.
        for _ in 0..30 {
            game.run_usercmd(eng, 0, &cmd(10, IN_DUCK, 0.0, 0.0), 0);
        }
        let v = vars(eng, 0);
        assert!(!v.b_in_duck);
        assert_ne!(v.flags & FL_DUCKING, 0);
        assert_ne!(v.flags & FL_ONGROUND, 0);
        assert_eq!(v.view_ofs.z, 12.0);
        // `CmdEnd`: the entity gets `VEC_DUCK_HULL` (the top at 32, not the movement's 18).
        assert_eq!(
            (v.mins, v.maxs),
            (Vec3::new(-16.0, -16.0, -18.0), Vec3::new(16.0, 16.0, 32.0))
        );
        // Letting go stands the player up again.
        game.run_usercmd(eng, 0, &idle(10), 0);
        let v = vars(eng, 0);
        assert_eq!(v.flags & FL_DUCKING, 0);
        assert_eq!(v.view_ofs.z, 17.0);
        assert_eq!(
            (v.mins, v.maxs),
            (Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0))
        );
    });
}

/// A spot on the map where a step of 7 to 18 units rises in front of the floor: the origin of a
/// standing player resting on the floor and the yaw to walk at the step. Looks around each of
/// `around` (the spawn points) on a grid; the map's geometry is axis aligned, so only the four
/// axis directions are tried.
fn find_step(eng: &EngineApi<'_>, around: &[Vec3]) -> Option<(Vec3, f32)> {
    let hull = Hull::Standing;
    for &centre in around {
        for gx in -100..=100 {
            for gy in -100..=100 {
                let probe = centre + Vec3::new(gx as f32 * 32.0, gy as f32 * 32.0, 48.0);
                let down = eng.trace_hull(probe, probe - Vec3::new(0.0, 0.0, 200.0), hull);
                if down.start_solid
                    || down.all_solid
                    || down.fraction >= 1.0
                    || down.plane_normal.z < 0.99
                {
                    continue;
                }
                let floor = down.end_pos;
                for i in 0..4 {
                    let yaw = i as f32 * 90.0;
                    let dir = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
                    // Blocked at the floor, free 18 units higher: a step (or a low obstacle).
                    let low = eng.trace_hull(floor, floor + dir * 40.0, hull);
                    let lifted = floor + Vec3::new(0.0, 0.0, 18.5);
                    let high = eng.trace_hull(lifted, lifted + dir * 40.0, hull);
                    if low.fraction < 1.0
                        && !low.start_solid
                        && low.plane_normal.z.abs() < 0.1
                        && high.fraction >= 1.0
                    {
                        // The floor beyond is above this one and within the step height.
                        let beyond = lifted + dir * 40.0;
                        let onto = eng.trace_hull(beyond, beyond - Vec3::new(0.0, 0.0, 60.0), hull);
                        let rise = onto.end_pos.z - floor.z;
                        if !onto.start_solid && rise > 7.0 && rise <= 18.0 {
                            return Some((floor, yaw));
                        }
                    }
                }
            }
        }
    }
    None
}

#[test]
fn stairs_are_climbed() {
    let Some(mut b) = dust2_bench() else {
        return;
    };
    b.run(|game, eng| {
        let mut around = Vec::new();
        for class in ["info_player_start", "info_player_deathmatch"] {
            let mut after = None;
            while let Some(id) = eng.find_by_classname(after, class) {
                after = Some(id);
                around.push(eng.ent(id).unwrap().origin);
            }
        }
        let (floor, yaw) = find_step(eng, &around).expect("a step on de_dust2");
        let id = eng.client_edict(0);
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.place_player(eng, 0, floor + Vec3::new(0.0, 0.0, 0.5));
        eng.ent_mut(id).unwrap().max_speed = 250.0;
        eng.ent_mut(id).unwrap().fixangle = 0;
        for _ in 0..10 {
            game.run_usercmd(eng, 0, &idle(10), 0);
        }
        let start = vars(eng, 0).origin;
        let mut best = start.z;
        for _ in 0..80 {
            game.run_usercmd(eng, 0, &cmd(10, IN_FORWARD, 400.0, yaw), 0);
            best = best.max(eng.ent(id).unwrap().origin.z);
        }
        let v = vars(eng, 0);
        assert!(
            best > start.z + 5.0,
            "stepped up from {} to {}",
            start.z,
            best
        );
        assert_ne!(v.flags & FL_ONGROUND, 0, "walks on, on the ground");
    });
}

#[test]
fn landing_punch_reaches_client_data() {
    let Some(mut b) = dust2_bench() else {
        return;
    };
    b.run(|game, eng| {
        let id = eng.client_edict(0);
        let floor = vars(eng, 0).origin;
        // A drop of a hundred units: 400 units a second at the end, past `PM_CheckFalling`'s
        // 350, short of the fall damage's 500.
        let top = eng.trace_hull(floor, floor + Vec3::new(0.0, 0.0, 100.0), Hull::Standing);
        assert!(top.fraction >= 1.0, "room above the spawn point");
        game.place_player(eng, 0, floor + Vec3::new(0.0, 0.0, 100.0));
        let mut last_vz = 0.0;
        let mut landed = false;
        for _ in 0..200 {
            game.run_usercmd(eng, 0, &idle(10), 0);
            let v = eng.ent(id).unwrap();
            if v.flags & FL_ONGROUND != 0 {
                landed = true;
                break;
            }
            last_vz = v.velocity.z;
        }
        assert!(landed);
        // The landing punch is the movement's: `punchangle[2] = flFallVelocity * 0.013` (a
        // double constant), straight into the client data of the command that landed.
        let fall = -last_vz;
        assert!(fall > 350.0 && fall < 500.0, "fell at {fall}");
        let data = game.update_client_data(eng, 0);
        let v = vars(eng, 0);
        let expected = (f64::from(fall) * 0.013) as f32;
        assert!(
            (v.punchangle.z - expected).abs() < 1e-3,
            "{} vs {expected}",
            v.punchangle.z
        );
        assert_eq!(data.punchangle, v.punchangle);
        assert_eq!(v.health, 100.0, "no damage below the safe speed");
        // The next command decays it.
        game.run_usercmd(eng, 0, &idle(10), 0);
        assert!(vars(eng, 0).punchangle.z < expected);
    });
}

#[test]
fn client_data_carries_duck_step_swim_fields() {
    let Some(mut b) = dust2_bench() else {
        return;
    };
    b.run(|game, eng| {
        let id = eng.client_edict(0);
        // Walking sets the step-sound timer (`PM_UpdateStepSound`), ducking the duck timers;
        // the client data is what the edict has.
        for _ in 0..40 {
            game.run_usercmd(eng, 0, &cmd(10, IN_FORWARD, 400.0, 0.0), 0);
        }
        let v = vars(eng, 0);
        let data = game.update_client_data(eng, 0);
        assert!(v.fl_time_step_sound > 0, "a step timer is running");
        assert_eq!(data.fl_time_step_sound, v.fl_time_step_sound);
        game.run_usercmd(eng, 0, &cmd(10, IN_DUCK, 0.0, 0.0), 0);
        let v = vars(eng, 0);
        let data = game.update_client_data(eng, 0);
        assert!(v.b_in_duck && v.fl_duck_time > 0);
        assert_eq!(data.b_in_duck, 1);
        assert_eq!(data.fl_duck_time, v.fl_duck_time);

        // The swim, water jump and push fields, and the stamina floats, as the entity has them
        // (`UpdateClientData`: `waterjumptime = int(teleport_time)`; `fuser4` is not sent).
        let e = eng.ent_mut(id).unwrap();
        e.fl_swim_time = 777;
        e.teleport_time = 123.9;
        e.pushmsec = 5;
        e.fuser1 = 1.5;
        e.fuser2 = 2.5;
        e.fuser3 = 3.5;
        e.fuser4 = 4.5;
        let data = game.update_client_data(eng, 0);
        assert_eq!(data.fl_swim_time, 777);
        assert_eq!(data.waterjumptime, 123);
        assert_eq!(data.pushmsec, 5);
        assert_eq!((data.fuser1, data.fuser2, data.fuser3), (1.5, 2.5, 3.5));
        // `iuser3` carries FREEZE_TIME_OVER (bit 1) while the freeze period lasts, and nothing
        // else for now.
        let freeze = game.state().rules.is_freeze_period();
        assert_eq!(data.iuser3, if freeze { 2 } else { 0 });
    });
}

#[test]
fn hostile_usercmds_keep_the_player_finite() {
    let hostile = |msec: u8, buttons: u16, f: f32, s: f32, u: f32, angle: f32| GoldSrcUserCmd {
        msec,
        buttons,
        forwardmove: f,
        sidemove: s,
        upmove: u,
        viewangles: Vec3::new(angle, -angle, angle),
        ..Default::default()
    };
    let cmds = [
        hostile(255, 0xffff, 1e30, -1e30, 1e30, 1e30),
        hostile(0, 0xffff, f32::MAX, f32::MAX, f32::MIN, f32::MAX),
        hostile(
            255,
            IN_JUMP | IN_DUCK,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ),
        hostile(1, IN_DUCK, 400.0, 400.0, 400.0, f32::INFINITY),
        hostile(50, IN_JUMP, -1e20, 1e20, 0.0, -1e20),
        hostile(255, 0, 0.0, 0.0, 0.0, 0.0),
    ];
    fn check(game: &mut CsGame, eng: &mut EngineApi<'_>, cmds: &[GoldSrcUserCmd]) {
        for round in 0..6 {
            for c in cmds {
                game.run_usercmd(eng, 0, c, 0xdead_beef);
                let v = vars(eng, 0);
                for (name, x) in [
                    ("origin", v.origin),
                    ("velocity", v.velocity),
                    ("v_angle", v.v_angle),
                    ("angles", v.angles),
                    ("view_ofs", v.view_ofs),
                    ("punchangle", v.punchangle),
                    ("basevelocity", v.basevelocity),
                ] {
                    assert!(x.is_finite(), "{name} {x:?} after round {round} of {c:?}");
                }
                assert!(v.origin.abs().max_element() < 1e6, "origin {:?}", v.origin);
                let data = game.update_client_data(eng, 0);
                assert!(data.origin.is_finite() && data.velocity.is_finite());
            }
        }
    }
    // In the air of a stub map, and on the floor of dust2.
    let mut b = stub_bench();
    let c = cmds.to_vec();
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "800");
        check(game, eng, &c);
    });
    if let Some(mut b) = dust2_bench() {
        b.run(move |game, eng| check(game, eng, &cmds));
    }
}

/// Two players on a stub map, slot 0 at the origin and slot 1 at `other`; no gravity.
fn two_players(other: Vec3) -> Bench {
    // The probe sends no commands: the engine would run the stuck player's own and the
    // `PM_CheckStuck` retry gate (50 ms) would refuse ours.
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.enter_second_client();
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        assert!(game.place_player(eng, 0, Vec3::ZERO));
        assert!(game.place_player(eng, 1, other));
        for slot in 0..2 {
            let v = eng.ent_mut(eng.client_edict(slot)).unwrap();
            v.fixangle = 0;
            v.max_speed = 250.0;
        }
    });
    b
}

#[test]
fn players_block_each_other_through_physents() {
    // The other player is 40 units ahead: the hulls (32 wide) meet at 32, so there are 8 units
    // of room.
    let run = |with_other: bool| {
        let mut b = two_players(Vec3::new(40.0, 0.0, 0.0));
        b.run(move |game, eng| {
            if !with_other {
                // Out of the way: the same run without the obstacle.
                game.place_player(eng, 1, Vec3::new(40.0, 400.0, 0.0));
            }
            let mut front = 0.0f32;
            for _ in 0..60 {
                game.run_usercmd(eng, 0, &cmd(10, IN_FORWARD, 400.0, 0.0), 0);
                front = front.max(vars(eng, 0).origin.x);
            }
            front
        })
    };
    let blocked = run(true);
    let free = run(false);
    assert!(blocked <= 8.01, "stopped at {blocked}");
    assert!(blocked > 0.0, "it did get up to the other player");
    assert!(free > 12.0, "without the other player it walks on: {free}");
}

#[test]
fn a_player_stuck_in_another_is_nudged_out_by_the_flailing_rule() {
    // 30 units apart: inside each other (the hulls meet at 32). `PM_CheckStuck` lets a stuck
    // player that presses jump, duck or attack try positions around itself, but only when the
    // physent it is stuck in is a player: `physent_is_player` of the engine's world.
    let mut b = two_players(Vec3::new(30.0, 0.0, 0.0));
    b.run(|game, eng| {
        game.run_usercmd(eng, 0, &cmd(10, IN_JUMP, 0.0, 0.0), 0);
        let gap = vars(eng, 1).origin.x - vars(eng, 0).origin.x;
        assert!(
            gap.abs() >= 32.0 - 0.01,
            "pushed out of the other player: gap {gap}"
        );
    });
    // Not pressing anything the player stays where it is, stuck.
    let mut b = two_players(Vec3::new(30.0, 0.0, 0.0));
    b.run(|game, eng| {
        game.run_usercmd(eng, 0, &idle(10), 0);
        let gap = vars(eng, 1).origin.x - vars(eng, 0).origin.x;
        assert!(gap.abs() < 32.0, "still stuck: gap {gap}");
    });
}

#[test]
fn the_world_knows_players_and_models() {
    // Stub map: the other player is a physent with `player` set and no model; the world is the
    // one with a model.
    let mut b = two_players(Vec3::new(100.0, 0.0, 0.0));
    b.run(|game, eng| {
        let flags = game.pm_world_flags(eng, 0);
        let other = i32::from(eng.client_edict(1).index);
        assert!(flags.contains(&(0, false, true)), "{flags:?}");
        assert!(flags.contains(&(other, true, false)), "{flags:?}");
    });
    // dust2: a brush entity within reach has a model and is no player.
    let Some(vfs) = discover() else {
        return;
    };
    let mut b = Bench::on(dust2(&vfs));
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        let mut brush: Option<EdictId> = None;
        for class in [
            "func_wall",
            "func_breakable",
            "func_door",
            "func_door_rotating",
            "func_train",
        ] {
            let mut after = None;
            while let Some(id) = eng.find_by_classname(after, class) {
                after = Some(id);
                let v = eng.ent(id).unwrap();
                if v.modelindex != 0 && v.solid == cs_engine::consts::SOLID_BSP {
                    brush = Some(id);
                    break;
                }
            }
            if brush.is_some() {
                break;
            }
        }
        let brush = brush.expect("a solid brush entity on de_dust2");
        let v = eng.ent(brush).unwrap();
        // Next to its box, so it is within the movement's reach.
        let at = (v.absmin + v.absmax) * 0.5 + Vec3::new(0.0, 0.0, 0.0);
        game.place_player(eng, 0, at);
        let flags = game.pm_world_flags(eng, 0);
        let info = i32::from(brush.index);
        assert!(flags.contains(&(info, false, true)), "{info} in {flags:?}");
    });
}
