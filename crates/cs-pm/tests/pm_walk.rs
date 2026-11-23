//! Behaviour of the `cs_pm::pm` core (Task 4) through `pm_move`, on `BoxWorld`. Expected numbers
//! are worked out by hand from the formulas of `pm_shared.cpp` (ReGameDLL_CS, read 2026-11-22);
//! the formula is in each test's comment. Floats are `f32` throughout, as in the source.

mod pm_support;

use cs_pm::pm::consts::*;
use cs_pm::pm::{PlayerMove, PmShared, pm_find_texture_type, pm_init, pm_move};
use glam::Vec3;
use pm_support::{BoxWorld, run_cmd, standing_player};

/// A walker standing on the floor of `BoxWorld::with_floor()` (floor top z = 0, hull half height
/// 36, so the origin is at z = 36), `maxspeed` 250 and `accelerate` 5 as the game sets them.
fn on_floor() -> (PlayerMove, PmShared, BoxWorld) {
    let w = BoxWorld::with_floor();
    let (mut pm, sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.movevars.accelerate = 5.0;
    (pm, sh, w)
}

fn assert_close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tol {tol})");
}

/// The world calls of the commands so far, without the ladder queries (`PM_Ladder`, Task 6, asks
/// the world for moveents and its answers change the log; here the scene has no ladders).
fn core_calls(w: &BoxWorld) -> Vec<&'static str> {
    w.call_kinds().into_iter().filter(|k| !matches!(*k, "NME" | "LC" | "MB" | "MO" | "TM")).collect()
}

#[test]
fn player_move_runs_in_source_order() {
    // At rest on the floor (CheckStuck TPP; CategorizePosition = CheckWater PC + trace PT;
    // WalkMove has nothing to do at speed 0; the final CategorizePosition = PC + PT).
    let (mut pm, mut sh, mut w) = on_floor();
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(core_calls(&w), ["TPP", "PC", "PT", "PC", "PT"]);

    // Walking forward from rest: PM_WalkMove adds its destination trace between PM_Ladder and
    // the final CategorizePosition (friction has no speed to act on).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.cmd.forwardmove = 250.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(core_calls(&w), ["TPP", "PC", "PT", "PT", "PC", "PT"]);

    // At speed: PM_Friction's edge probe comes before PM_WalkMove's trace.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(100.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(core_calls(&w), ["TPP", "PC", "PT", "PT", "PT", "PC", "PT"]);

    // Observers skip the stuck check, the ladder and everything else: spectator move, then
    // CategorizePosition.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.iuser1 = OBS_ROAMING;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(core_calls(&w), ["PC", "PT"]);

    // NOCLIP skips the stuck check but still categorizes once, and does not trace again.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.movetype = MOVETYPE_NOCLIP;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(core_calls(&w), ["PC", "PT"]);
}

#[test]
fn ground_acceleration_matches_pm_accelerate() {
    // PM_Accelerate: accelspeed = accel * frametime * wishspeed * friction
    //   = 5 * 0.01 * 250 * 1 = 12.5 (frametime = 10 * 0.001 stored as f32).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.cmd.forwardmove = 250.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.x, 12.5, 1e-5, "vx after the first command");
    assert_eq!(pm.velocity.y, 0.0);
    assert_eq!(pm.velocity.z, 0.0);
    // The player moved by velocity * frametime = 0.125 along +x (yaw 0).
    assert_close(pm.origin.x, 0.125, 1e-5, "x after the first command");
    assert_eq!(pm.onground, 0);
    assert_ne!(pm.flags & FL_ONGROUND, 0);
}

#[test]
fn running_converges_to_maxspeed() {
    // Each frame friction takes 4 * 100 * 0.01 = 4 (below stopspeed...) or 4 * speed * 0.01 and
    // PM_Accelerate adds at most 12.5, capped at addspeed = wishspeed - currentspeed: the speed
    // settles at the wish speed 250 and never exceeds it.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.cmd.forwardmove = 250.0;
    let mut top = 0.0f32;
    for _ in 0..100 {
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        top = top.max(pm.velocity.length());
    }
    assert!(top <= 250.01, "speed exceeded maxspeed: {top}");
    assert_close(pm.velocity.length(), 250.0, 0.01, "final speed");
}

#[test]
fn friction_uses_stopspeed_control() {
    // PM_Friction below stopspeed: control = stopspeed = 100, drop = friction * (control * frametime)
    //   = 4 * (100 * 0.01) = 4 per command, so 75 -> 71 -> 67 -> ... (75 - 4n) while below 100.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(75.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.x, 71.0, 1e-4, "after one command");
    for n in 2..=5 {
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        assert_close(pm.velocity.x, 75.0 - 4.0 * n as f32, 1e-3, "after n commands");
    }

    // Above stopspeed the control is the speed itself: 200 -> 200 - 4 * (200 * 0.01) = 192.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(200.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.x, 192.0, 1e-3, "above stopspeed");
}

#[test]
fn edge_friction_applies_over_a_drop() {
    // The edge probe starts 16 units ahead at the feet and goes 34 down; when it finds nothing
    // (fraction 1) friction is multiplied by edgefriction (2): speed 100 -> 100 - 8 * (100 * 0.01) = 92.
    // Floor ends at x = -8 (its hull-expanded edge is x = 8), the probe hull at x = 16 is clear.
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-1000.0, -1000.0, -100.0), Vec3::new(-8.0, 1000.0, 0.0), "FLOOR");
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.movevars.accelerate = 5.0;
    pm.velocity = Vec3::new(100.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.x, 92.0, 1e-3, "edge friction");

    // The same move over a solid floor: 100 - 4 * (100 * 0.01) = 96.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(100.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.x, 96.0, 1e-3, "plain friction");
}

/// A walker running into a step of height `h` (a box on the floor from x = 20, whose hull-expanded
/// face is at x = 4).
fn run_into_step(h: f32) -> PlayerMove {
    let (mut pm, mut sh, mut w) = on_floor();
    w.solid(Vec3::new(20.0, -100.0, 0.0), Vec3::new(200.0, 100.0, h), "STEP");
    pm.velocity = Vec3::new(250.0, 0.0, 0.0);
    pm.cmd.forwardmove = 250.0;
    run_cmd(&mut pm, &mut sh, &mut w, 50);
    pm
}

#[test]
fn steps_up_to_stepsize_and_not_higher() {
    // stepsize 18: PM_WalkMove goes up 18, slides, presses down: an 18 high step is climbed
    // (origin z = 36 + 18) and the player moves on over it.
    let pm = run_into_step(18.0);
    assert_close(pm.origin.z, 54.0, 1e-3, "z on the 18 step");
    assert!(pm.origin.x > 4.0, "the player got over the step: x = {}", pm.origin.x);
    assert_eq!(pm.onground, 1);

    // 19 is too high: the up-and-over attempt is blocked, the slide result stands and the
    // player stays on the floor in front of the step.
    let pm = run_into_step(19.0);
    // (The press-down trace stops DIST_EPSILON = 0.03125 above the floor, as the engine does.)
    assert_close(pm.origin.z, 36.0, 0.0313, "z at the 19 step");
    assert!(pm.origin.x < 4.0, "blocked by the step: x = {}", pm.origin.x);
    assert_eq!(pm.onground, 0);
}

#[test]
fn wall_slide_clips_velocity() {
    // In the air, hit a wall at x = 100 (hull-expanded face x = 84): PM_FlyMove's bounce branch
    // (WALK, onground -1) clips with overbounce 1 + bounce * (1 - friction) = 1 along the normal
    // (-1, 0, 0): vx = 200 - (-1) * (-200) = 0, vy stays 200.
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(100.0, -1000.0, -100.0), Vec3::new(110.0, 1000.0, 500.0), "WALL");
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 200.0));
    pm.velocity = Vec3::new(200.0, 200.0, 0.0);
    let mut hit = false;
    for _ in 0..30 {
        run_cmd(&mut pm, &mut sh, &mut w, 50);
        if pm.velocity.x == 0.0 {
            hit = true;
            break;
        }
    }
    assert!(hit, "never reached the wall");
    assert_eq!(pm.velocity.y, 200.0);
    assert_close(pm.origin.x, 84.0 - 0.03125, 1e-3, "stopped a DIST_EPSILON before the face");
}

#[test]
fn falling_uses_half_step_gravity() {
    // Per command: PM_AddCorrectGravity v -= 1 * 800 * 0.5 * 0.01 = 4, PM_FixupGravityVelocity
    // v -= 800 * 0.01 * 1 * 0.5 = 4: 8 per 10 ms command, i.e. gravity * dt.
    let w = BoxWorld::new();
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 500.0));
    let mut w = w;
    for n in 1..=10 {
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        assert_eq!(pm.velocity.z, -8.0 * n as f32, "after {n} commands");
    }
    assert_eq!(pm.onground, -1);
    // The fall speed at the start of the last command was 72.
    assert_eq!(pm.fl_fall_velocity, 72.0);
}

#[test]
fn landing_sets_punch_and_clears_fall_velocity() {
    // Falling at 300: PM_CheckFalling punches the view by flFallVelocity * 0.013 = 3.9 (z) and sets
    // flTimeStepSound 300, then clears flFallVelocity. 300 > 580 / 2 so the volume is 0.85.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.origin.z = 46.0; // 10 above the floor: not on the ground at the start of the command
    pm.velocity = Vec3::new(0.0, 0.0, -300.0);
    run_cmd(&mut pm, &mut sh, &mut w, 50);
    assert_ne!(pm.onground, -1, "landed");
    assert_eq!(pm.fl_fall_velocity, 0.0);
    assert_close(pm.punchangle.z, 3.9, 1e-5, "landing punch");
    assert_eq!(pm.fl_time_step_sound, 300);
    assert_eq!(pm.velocity.z, 0.0);
    assert_ne!(pm.flags & FL_ONGROUND, 0);

    // A soft landing (below PM_PLAYER_FALL_PUNCH_THRESHHOLD 250) leaves the view alone.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.origin.z = 46.0;
    pm.velocity = Vec3::new(0.0, 0.0, -200.0);
    run_cmd(&mut pm, &mut sh, &mut w, 100);
    assert_ne!(pm.onground, -1);
    assert_eq!(pm.punchangle.z, 0.0);
    assert_eq!(pm.fl_fall_velocity, 0.0);
}

#[test]
fn no_hit_trace_leaves_player_airborne() {
    // A trace that hits nothing leaves the plane normal at 0 (< 0.7): onground is -1 and
    // FL_ONGROUND is cleared, even if it was set.
    let mut w = BoxWorld::new();
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.flags |= FL_ONGROUND;
    pm.onground = 0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.onground, -1);
    assert_eq!(pm.flags & FL_ONGROUND, 0);

    // Also with a floor out of reach below (more than the 2 units CategorizePosition probes).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.origin.z = 80.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.onground, -1);
    assert_eq!(pm.flags & FL_ONGROUND, 0);
}

#[test]
fn stuck_player_is_nudged_by_the_table() {
    // A plate whose hull-expanded top is z = 3; the player origin z = 2 is inside it. The stuck
    // table offsets are tried in order: z = -0.125 .. 0 are still inside, table[18] = (0, 0, 1)
    // lands exactly on the plane (empty side).
    let plate = |w: &mut BoxWorld| {
        w.solid(Vec3::new(-1000.0, -1000.0, -100.0), Vec3::new(1000.0, 1000.0, -33.0), "PLATE");
    };

    // Client side: the first free offset is taken at once (the world, ent 0, is the obstacle).
    let mut w = BoxWorld::new();
    plate(&mut w);
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 2.0));
    pm.server = false;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.origin.z, 3.0, 1e-3, "client unstuck height");
    assert_eq!(pm.onground, 0, "standing on the plate after the nudge");

    // Server side: one offset per 0.05 s, the stuck entity is recorded as touched, and offsets
    // below table index 27 do not move the origin (only the big ones do).
    let mut w = BoxWorld::new();
    plate(&mut w);
    w.time = 10.0;
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 2.0));
    pm.velocity = Vec3::new(0.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.origin, Vec3::new(0.0, 0.0, 2.0), "stuck: no movement");
    assert_eq!(pm.touchindex.len(), 1, "the stuck entity is a touch");
    assert_eq!(pm.touchindex[0].ent, 0);
    // The same instant again: too soon, so nothing happens at all (no touch recorded).
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.origin, Vec3::new(0.0, 0.0, 2.0));
    assert!(pm.touchindex.is_empty());
    // 17 more intervals walk the table up to index 18 (z + 1): free, but below index 27 the
    // origin is not changed.
    for _ in 0..18 {
        w.time += 0.1;
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        assert_eq!(pm.origin, Vec3::new(0.0, 0.0, 2.0));
    }
}

#[test]
fn noclip_and_toss_move_as_source() {
    // PM_NoClip: origin += frametime * (forward * fmove + right * smove + up * upmove); the
    // velocity is cleared. 10 ms at forwardmove 250, upmove 100: (2.5, 0, 1).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.movetype = MOVETYPE_NOCLIP;
    pm.maxspeed = 1000.0; // keep CheckParameters from scaling the command
    pm.origin.z = 100.0;
    pm.velocity = Vec3::new(5.0, 6.0, 7.0);
    pm.cmd.forwardmove = 250.0;
    pm.cmd.upmove = 100.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.origin.x, 2.5, 1e-5, "noclip x");
    assert_close(pm.origin.z, 101.0, 1e-5, "noclip z");
    assert_eq!(pm.velocity, Vec3::ZERO);
    assert_eq!(pm.movetype, MOVETYPE_NOCLIP);

    // PM_Physics_Toss (a dead player keeps its TOSS movetype): PM_AddGravity v.z -= 1 * 800 * 0.01 = 8,
    // move = velocity * frametime = (1, 0, -0.08) from (0, 0, 200).
    let mut w = BoxWorld::new();
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 200.0));
    pm.movetype = MOVETYPE_TOSS;
    pm.dead = true;
    pm.deadflag = DEAD_DEAD;
    pm.velocity = Vec3::new(100.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.origin.x, 1.0, 1e-5, "toss x");
    assert_close(pm.origin.z, 200.0 - 0.08, 1e-4, "toss z");
    assert_close(pm.velocity.z, -8.0, 1e-5, "toss vz");
    assert_eq!(pm.movetype, MOVETYPE_TOSS);

    // A toss that lands (MOVETYPE_TOSS stops on a floor: onground = the floor, velocity 0).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.movetype = MOVETYPE_TOSS;
    pm.dead = true;
    pm.deadflag = DEAD_DEAD;
    pm.origin.z = 36.5;
    pm.velocity = Vec3::new(0.0, 0.0, -100.0);
    run_cmd(&mut pm, &mut sh, &mut w, 50);
    assert_eq!(pm.onground, 0);
    assert_eq!(pm.velocity, Vec3::ZERO);
}

#[test]
fn check_parameters_scales_and_rolls() {
    // spd = |(side, forward, up)| = 500 > maxspeed 250: every move is scaled by 250 / 500 = 0.5.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.cmd.forwardmove = 400.0;
    pm.cmd.sidemove = 300.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.cmd.forwardmove, 200.0);
    assert_eq!(pm.cmd.sidemove, 150.0);

    // clientmaxspeed below maxspeed wins: maxspeed = min(100, 250).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.clientmaxspeed = 100.0;
    pm.cmd.forwardmove = 200.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.maxspeed, 100.0);
    assert_eq!(pm.cmd.forwardmove, 100.0);

    // Frozen or dead players have no move.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.flags |= FL_FROZEN;
    pm.cmd.forwardmove = 200.0;
    pm.cmd.sidemove = 50.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!((pm.cmd.forwardmove, pm.cmd.sidemove, pm.cmd.upmove), (0.0, 0.0, 0.0));

    // Angles = view angles + punch; the yaw is wrapped to <= 180; roll from the sideways speed:
    // velocity (0, 100, 0) at yaw 0 has right = (0, -1, 0), side = -100 (< rollspeed 200):
    // roll = (100 * 2 / 200) * -1 * 4 = -4.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.movevars.rollangle = 2.0;
    pm.movevars.rollspeed = 200.0;
    pm.velocity = Vec3::new(0.0, 100.0, 0.0);
    pm.cmd.viewangles = Vec3::new(10.0, 270.0, 0.0);
    // At yaw 270 right = (-1, 0, 0) ... (cos 270 = 0 and sin 270 = -1: right.y = -cos = ~0), so the
    // roll is about 0; check the yaw wrap and pitch instead, and the roll with yaw 0 below.
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.angles.x, 10.0, 1e-5, "pitch");
    assert_close(pm.angles.y, -90.0, 1e-5, "yaw wrapped from 270");

    let (mut pm, mut sh, mut w) = on_floor();
    pm.movevars.rollangle = 2.0;
    pm.movevars.rollspeed = 200.0;
    pm.velocity = Vec3::new(0.0, 100.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.angles.z, -4.0, 1e-4, "roll");

    // Dead: the angles are the old angles and the view height is PM_DEAD_VIEWHEIGHT.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.dead = true;
    pm.deadflag = DEAD_DEAD;
    pm.movetype = MOVETYPE_TOSS;
    pm.oldangles = Vec3::new(1.0, 2.0, 3.0);
    pm.cmd.viewangles = Vec3::new(40.0, 50.0, 60.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.angles, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(pm.view_ofs.z, PM_DEAD_VIEWHEIGHT);
}

#[test]
fn punch_angle_decays() {
    // PM_DropPunchAngle: len -= (10 + len * 0.5) * frametime, in double. QUIRK: frametime is the one
    // the engine prefilled (0.02 here), not the command's msec (10 -> 0.01), because
    // PM_CheckParameters runs before frametime is recomputed: 10 - (10 + 5) * 0.02 = 9.7.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.punchangle = Vec3::new(0.0, 0.0, 10.0);
    pm.cmd.msec = 10;
    pm.frametime = 0.02;
    pm_move(&mut pm, &mut sh, &mut w, true);
    assert_close(pm.punchangle.z, 9.7, 1e-5, "punch after one command");

    // It never goes negative: a small punch is gone in one step.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.punchangle = Vec3::new(0.5, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 100);
    // run_cmd prefills frametime 0.1: 0.5 - (10 + 0.25) * 0.1 < 0.
    assert_eq!(pm.punchangle, Vec3::ZERO);
}

#[test]
fn timers_reduce_by_msec() {
    // PM_ReduceTimers: each timer counts down by cmd.msec and stops at 0.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.fl_time_step_sound = 300;
    pm.fl_duck_time = 100.0;
    pm.fl_swim_time = 500.0;
    pm.fuser2 = 1000.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.fl_time_step_sound, 290);
    assert_eq!(pm.fl_duck_time, 90.0);
    assert_eq!(pm.fl_swim_time, 490.0);
    assert_eq!(pm.fuser2, 990.0);

    let (mut pm, mut sh, mut w) = on_floor();
    pm.fl_duck_time = 5.0;
    pm.fl_swim_time = 5.0;
    pm.fuser2 = 5.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.fl_duck_time, 0.0);
    assert_eq!(pm.fl_swim_time, 0.0);
    assert_eq!(pm.fuser2, 0.0);
}

#[test]
fn msec_zero_changes_no_position() {
    // frametime 0: every dt term vanishes. On the ground with speed, in the air with speed.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(100.0, 50.0, 0.0);
    pm.cmd.forwardmove = 250.0;
    let before = pm.origin;
    run_cmd(&mut pm, &mut sh, &mut w, 0);
    assert_eq!(pm.origin, before);
    assert_eq!(pm.frametime, 0.0);

    let mut w = BoxWorld::new();
    let (mut pm, mut sh) = standing_player(Vec3::new(1.0, 2.0, 300.0));
    pm.velocity = Vec3::new(100.0, 50.0, -30.0);
    let before = pm.origin;
    run_cmd(&mut pm, &mut sh, &mut w, 0);
    assert_eq!(pm.origin, before);
}

#[test]
fn nan_velocity_is_reset() {
    // PM_CheckVelocity: a NaN (or infinite) velocity or origin component becomes 0.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(f32::NAN, f32::INFINITY, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert!(pm.velocity.is_finite(), "velocity {:?}", pm.velocity);
    assert_eq!(pm.velocity.x, 0.0);

    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(0.0, 0.0, 0.0);
    pm.origin = Vec3::new(f32::NAN, 0.0, 36.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert!(pm.origin.is_finite(), "origin {:?}", pm.origin);

    // maxvelocity bounds the velocity: 5000 -> 2000 after the first check.
    let w = BoxWorld::new();
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 300.0));
    let mut w = w;
    pm.velocity = Vec3::new(5000.0, -5000.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert!(pm.velocity.x <= 2000.0 && pm.velocity.y >= -2000.0);
}

#[test]
fn touches_are_recorded_once_per_entity() {
    // Sliding into the corner of two walls (ents 1 and 2) in one command hits entity 1 first and
    // then entity 2; the touch list holds each once, with the velocity at the impact.
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-1000.0, -1000.0, -1000.0), Vec3::new(-999.0, -999.0, -999.0), "WORLD");
    w.solid(Vec3::new(100.0, -1000.0, -100.0), Vec3::new(110.0, 1000.0, 500.0), "WALL A");
    w.solid(Vec3::new(-1000.0, 100.0, -100.0), Vec3::new(1000.0, 110.0, 500.0), "WALL B");
    let (mut pm, mut sh) = standing_player(Vec3::new(80.0, 80.0, 200.0));
    pm.velocity = Vec3::new(200.0, 200.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 50);
    let ents: Vec<i32> = pm.touchindex.iter().map(|t| t.ent).collect();
    assert_eq!(ents, [1, 2]);
    assert_eq!(pm.touchindex[0].deltavelocity.x, 200.0, "velocity at the first impact");

    // The list is rebuilt every command: numtouch = 0 at the start of PM_PlayerMove.
    run_cmd(&mut pm, &mut sh, &mut w, 50);
    let mut seen = std::collections::HashSet::new();
    for t in &pm.touchindex {
        assert!(seen.insert(t.ent), "entity {} listed twice", t.ent);
    }

    // Standing on an entity other than the world touches it (CategorizePosition: tr.ent > 0).
    let (mut pm, mut sh, mut w) = on_floor();
    let crate_ent = w.solid(Vec3::new(-50.0, -50.0, 0.0), Vec3::new(50.0, 50.0, 10.0), "CRATE");
    pm.origin.z = 46.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.onground, crate_ent);
    assert_eq!(pm.touchindex.iter().filter(|t| t.ent == crate_ent).count(), 1);
}

#[test]
fn hostile_cmds_do_not_break_the_move() {
    // NaN, +-1e30, infinities, buttons 0xFFFF, msec 0 / 1 / 255: no panic, finite origin and
    // velocity after every command.
    let values = [f32::NAN, 1e30, -1e30, f32::INFINITY, f32::NEG_INFINITY, 0.0, 250.0];
    let msecs = [0u8, 1, 50, 255];
    let mut commands = 0;
    for &f in &values {
        for &s in &values {
            for &msec in &msecs {
                let (mut pm, mut sh, mut w) = on_floor();
                w.solid(Vec3::new(100.0, -1000.0, 0.0), Vec3::new(110.0, 1000.0, 200.0), "WALL");
                for step in 0..3 {
                    pm.cmd.forwardmove = f;
                    pm.cmd.sidemove = s;
                    pm.cmd.upmove = if step == 1 { f } else { 0.0 };
                    pm.cmd.buttons = 0xFFFF;
                    pm.cmd.viewangles = if step == 2 { Vec3::new(f, s, f) } else { Vec3::new(0.0, 45.0, 0.0) };
                    run_cmd(&mut pm, &mut sh, &mut w, msec);
                    assert!(pm.origin.is_finite(), "origin {:?} (f {f}, s {s}, msec {msec})", pm.origin);
                    assert!(pm.velocity.is_finite(), "velocity {:?} (f {f}, s {s}, msec {msec})", pm.velocity);
                    commands += 1;
                }
            }
        }
    }
    assert_eq!(commands, 7 * 7 * 4 * 3);

    // Every movetype and the observer branch survive the same garbage.
    for movetype in [MOVETYPE_NONE, MOVETYPE_WALK, MOVETYPE_FLY, MOVETYPE_TOSS, MOVETYPE_NOCLIP, MOVETYPE_BOUNCE, 99, -5] {
        let (mut pm, mut sh, mut w) = on_floor();
        pm.movetype = movetype;
        pm.cmd.forwardmove = f32::NAN;
        pm.cmd.buttons = 0xFFFF;
        pm.usehull = 77;
        run_cmd(&mut pm, &mut sh, &mut w, 255);
        assert!(pm.velocity.is_finite(), "movetype {movetype}");
        pm.iuser1 = OBS_ROAMING;
        run_cmd(&mut pm, &mut sh, &mut w, 255);
    }
}

#[test]
fn move_tail_mirrors_ground_flag_and_single_player_friction() {
    // PM_Move: FL_ONGROUND follows onground; in single player a walker's entity friction is 1.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.multiplayer = false;
    pm.friction = 0.5;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_ne!(pm.flags & FL_ONGROUND, 0);
    assert_eq!(pm.friction, 1.0);

    // In multiplayer the entity friction is left alone.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.friction = 0.5;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.friction, 0.5);
}

#[test]
fn texture_lookup_defaults_to_concrete() {
    let sh = pm_init(None);
    assert_eq!(pm_find_texture_type(&sh, "ANYTHING"), b'C');
    let sh = pm_init(Some(b"M METAL\n"));
    assert_eq!(pm_find_texture_type(&sh, "METAL"), b'M');
}

#[test]
fn walk_move_applies_the_jump_penalty_ratio() {
    // Friction first: 100 - 4 * (100 * 0.01) = 96. PM_ReduceTimers takes 10 off fuser2 (1000 -> 990),
    // then PM_WalkMove scales vx by (100 - fuser2 * 0.001 * 19) * 0.01 = (100 - 18.81) * 0.01.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(100.0, 0.0, 0.0);
    pm.fuser2 = 1000.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    let ratio = ((100.0 - 990.0f64 * 0.001 * 19.0) * 0.01) as f32;
    assert_close(pm.velocity.x, 96.0 * ratio, 1e-3, "penalised vx");
    assert!(pm.velocity.x < 80.0);
}

#[test]
fn fly_move_bounce_branch_uses_entity_friction() {
    // Airborne WALK player, first plane a wall, entity friction 0.5: overbounce
    // 1 + bounce * (1 - friction) = 1.5, so vx = 200 - (-1) * (-200 * 1.5) = -100.
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(100.0, -1000.0, -100.0), Vec3::new(110.0, 1000.0, 500.0), "WALL");
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 200.0));
    pm.friction = 0.5;
    pm.velocity = Vec3::new(200.0, 0.0, 0.0);
    let mut bounced = false;
    for _ in 0..30 {
        run_cmd(&mut pm, &mut sh, &mut w, 50);
        if pm.velocity.x < 0.0 {
            bounced = true;
            break;
        }
    }
    assert!(bounced, "never reached the wall");
    assert_close(pm.velocity.x, -100.0, 1e-3, "bounced vx");
}
