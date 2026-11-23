//! Ladders, water and observers of `cs_pm::pm` (Task 6) through `pm_move`, on `BoxWorld`.
//! Expected numbers are worked out from the formulas of `pm_shared.cpp` (ReGameDLL_CS, read
//! 2026-11-22); the formula is in each test's comment. Floats are `f32` throughout, as in the source.

mod pm_support;

use cs_pm::pm::consts::*;
use cs_pm::pm::{PlayerMove, PmShared};
use glam::Vec3;
use pm_support::{BoxWorld, run_cmd, standing_player};

fn assert_close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tol {tol})");
}

fn wade_sounds(w: &BoxWorld) -> Vec<&str> {
    w.sounds.iter().map(|s| s.sample.as_str()).filter(|s| s.starts_with("player/pl_wade")).collect()
}

/// The floor of `with_floor` (top z = 0) with a water volume above it, `top` units deep, and a
/// walker standing on the floor (origin z = 36: feet probe z = 1, waist probe z = 36, eye z = 53).
fn pool(top: f32) -> (PlayerMove, PmShared, BoxWorld) {
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, top), CONTENTS_WATER, CONTENTS_WATER);
    let (mut pm, sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    (pm, sh, w)
}

/// A walker at (10, 0, 100) in the air in front of a ladder whose face is at x = 20 (normal -x),
/// spanning z 0..200. The ladder box expanded by the hull contains the origin.
fn at_ladder() -> (PlayerMove, PmShared, BoxWorld) {
    let mut w = BoxWorld::with_floor();
    w.ladder(Vec3::new(20.0, -16.0, 0.0), Vec3::new(24.0, 16.0, 200.0), Vec3::new(-1.0, 0.0, 0.0));
    let (mut pm, sh) = standing_player(Vec3::new(10.0, 0.0, 100.0));
    pm.maxspeed = 250.0;
    (pm, sh, w)
}

#[test]
fn climbs_a_ladder_at_200() {
    // Facing the ladder (yaw 0, vpn (1,0,0)), forward pressed: flSpeed = MAX_CLIMB_SPEED 200,
    // velocity (200,0,0); perp = (0,0,1) x (-1,0,0) = (0,-1,0); normal.v = -200;
    // result = lateral + 200 * (n x perp = (0,0,1)) = (0,0,200). PM_FlyMove then moves
    // 200 * 0.01 = 2 up.
    let (mut pm, mut sh, mut w) = at_ladder();
    pm.cmd.buttons = IN_FORWARD as u16;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.movetype, MOVETYPE_FLY);
    assert_eq!(pm.gravity, 0.0);
    assert_close(pm.velocity.z, 200.0, 1e-3, "climb speed");
    assert_close(pm.velocity.x, 0.0, 1e-3, "no push into the ladder");
    assert_close(pm.origin.z, 102.0, 1e-3, "z after one command");

    // Backwards climbs down at the same speed.
    let (mut pm, mut sh, mut w) = at_ladder();
    pm.cmd.buttons = IN_BACK as u16;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.z, -200.0, 1e-3, "climb down speed");

    // No direction keys: the velocity is cleared.
    let (mut pm, mut sh, mut w) = at_ladder();
    pm.velocity = Vec3::new(5.0, 6.0, 7.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.movetype, MOVETYPE_FLY);
    assert_eq!(pm.velocity, Vec3::ZERO);
}

#[test]
fn jumps_off_a_ladder_along_its_normal() {
    // IN_JUMP on a ladder: movetype WALK, velocity = plane normal * 270 = (-270, 0, 0). The player
    // is airborne (z = 100), so the walk code only adds air movement; with no wish the horizontal
    // velocity stays (no air friction in PM_AirMove).
    let (mut pm, mut sh, mut w) = at_ladder();
    pm.cmd.buttons = IN_JUMP as u16;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.movetype, MOVETYPE_WALK);
    assert_close(pm.velocity.x, -270.0, 1e-3, "jump velocity along the normal");
    assert_close(pm.velocity.y, 0.0, 1e-3, "no sideways component");
    // PM_AirMove moves by velocity * frametime in the same frame: -270 * 0.01 = -2.7.
    assert_close(pm.origin.x, 10.0 - 2.7, 1e-3, "displacement away from the ladder");
}

#[test]
fn climbs_sideways_along_the_ladder() {
    // Facing the ladder (yaw 0): v_right = (0,-1,0). IN_MOVERIGHT: right = +200, velocity
    // (0,-200,0); normal.v = 0, so lateral = velocity and the result is (0,-200,0). IN_MOVELEFT
    // mirrors it.
    let (mut pm, mut sh, mut w) = at_ladder();
    pm.cmd.buttons = IN_MOVERIGHT as u16;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.y, -200.0, 1e-3, "right");
    assert_close(pm.velocity.z, 0.0, 1e-3, "right z");

    let (mut pm, mut sh, mut w) = at_ladder();
    pm.cmd.buttons = IN_MOVELEFT as u16;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.y, 200.0, 1e-3, "left");
}

#[test]
fn the_first_of_two_overlapping_ladders_wins() {
    // Both ladders contain the player; PM_Ladder returns the first (moveent 0), so only its
    // bounds are asked for and its normal (-x) gives the jump velocity.
    let (mut pm, mut sh, mut w) = at_ladder();
    w.ladder(Vec3::new(20.0, -16.0, 0.0), Vec3::new(24.0, 16.0, 200.0), Vec3::new(0.0, -1.0, 0.0));
    pm.cmd.buttons = IN_JUMP as u16;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    let calls = w.calls();
    assert!(calls.contains(&pm_support::Call::MoveentBounds(0)));
    assert!(!calls.contains(&pm_support::Call::MoveentBounds(1)));
    assert!(!calls.contains(&pm_support::Call::LadderContains(1)));
    assert_close(pm.velocity.x, -270.0, 1e-3, "normal of ladder 0");
}

#[test]
fn swimming_up_with_upmove() {
    // upmove 100, no horizontal input: wishvel.z = +100 (no sinking drift); wishspeed =
    // 100 * 0.8 = 80; velocity 0 so accelspeed = 10 * 0.01 * 80 = 8 -> velocity.z = +8.
    let (mut pm, mut sh, mut w) = pool(100.0);
    pm.origin.z = 60.0;
    pm.cmd.upmove = 100.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterlevel, 3);
    assert_close(pm.velocity.z, 8.0, 1e-4, "swim up velocity");
    assert_close(pm.origin.z, 60.08, 1e-3, "origin");
}

#[test]
fn water_levels_one_two_three() {
    // PM_CheckWater probes: feet z = origin - 36 + 1 = 1, waist z = origin + (mins+maxs)/2 = 36,
    // eyes z = origin + view_ofs = 53. Water to z = 10 wets the feet only; to 40 the waist; to 100
    // the eyes.
    for (top, level) in [(0.5f32, 0), (10.0, 1), (40.0, 2), (100.0, 3)] {
        let (mut pm, mut sh, mut w) = pool(top);
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        assert_eq!(pm.waterlevel, level, "water top {top}");
        let expected_type = if level == 0 { CONTENTS_EMPTY } else { CONTENTS_WATER };
        assert_eq!(pm.watertype, expected_type, "water top {top}");
    }
}

#[test]
fn swimming_sinks_and_is_slower() {
    // Sinking (PM_WaterMove): no input -> wishvel.z = -60; wishspeed = 60 * 0.8 = 48;
    // velocity 0 so newspeed = 0; addspeed = 48; accelspeed = accelerate(10) * friction(1) *
    // 0.01 * 48 = 4.8 -> velocity.z = -4.8.
    let (mut pm, mut sh, mut w) = pool(100.0);
    pm.origin.z = 60.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterlevel, 3);
    assert_close(pm.velocity.z, -4.8, 1e-4, "sink velocity");
    assert_close(pm.origin.z, 60.0 - 0.048, 1e-3, "origin after one frame");

    // Swimming forward at wish 400 (capped to maxspeed 250, then * 0.8 = 200): each frame
    // newspeed = 0.96 * speed (friction 4 * 0.01), accelspeed = 10 * 0.01 * 200 = 20 capped by
    // addspeed = 200 - newspeed. The speed converges to exactly 200, below the 250 of the ground.
    let (mut pm, mut sh, mut w) = pool(200.0);
    pm.origin = Vec3::new(-500.0, 0.0, 100.0);
    pm.cmd.forwardmove = 400.0;
    let mut first = true;
    for _ in 0..200 {
        pm.basevelocity = Vec3::ZERO;
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        if first {
            // frame 1: accelspeed 20 along +x.
            assert_close(pm.velocity.x, 20.0, 1e-4, "first swim frame");
            first = false;
        }
    }
    assert_eq!(pm.waterlevel, 3);
    assert_close(pm.velocity.x, 200.0, 1e-3, "terminal swim speed");
    assert!(pm.velocity.x < pm.maxspeed, "swimming is slower than maxspeed");
    assert_close(pm.velocity.z, 0.0, 1e-3, "no drift while moving");
}

#[test]
fn water_jump_lifts_out_at_a_ledge() {
    // Waist-deep water (surface z = 45: feet and waist wet, eyes at 53 dry -> level 2) in front
    // of a ledge whose wall is at x = 20 and whose top is z = 60. PM_CheckWaterJump: probe from
    // z = 36 + 8 = 44 hits the wall (normal -x, |nz| < 0.1); the second probe from
    // 44 + (36 - 8) = 72 clears the top (60) -> water jump:
    // waterjumptime 2000, velocity.z 225, movedir = -50 * normal = (50, 0, 0), FL_WATERJUMP.
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 45.0), CONTENTS_WATER, CONTENTS_WATER);
    w.solid(Vec3::new(20.0, -200.0, -100.0), Vec3::new(200.0, 200.0, 60.0), "LEDGE");
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.cmd.forwardmove = 250.0;

    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterlevel, 2);
    assert_eq!(pm.waterjumptime, 2000.0);
    assert_ne!(pm.flags & FL_WATERJUMP, 0);
    assert_eq!(pm.movedir, Vec3::new(50.0, 0.0, 0.0));
    // PM_WaterMove ran in the same frame: water friction 1 - 4 * 0.01 scales (0,0,225) to 216;
    // wishvel has no z, so the lift stays 216. (oldbuttons has IN_JUMP cleared again by the
    // "jump not pressed" branch, a quirk of the source.)
    assert_close(pm.velocity.z, 216.0, 1e-3, "lift velocity");
    assert_eq!(pm.oldbuttons & IN_JUMP, 0);
    assert_eq!(pm.usehull, 0, "the probe hull is restored");

    // Next frame: PM_WaterJump counts down by cmd.msec and forces the horizontal velocity to
    // movedir; PM_FlyMove then moves.
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterjumptime, 1990.0);
    assert_close(pm.velocity.x, 50.0, 1e-3, "forced horizontal velocity");
    assert_close(pm.velocity.y, 0.0, 1e-3, "no sideways velocity");
    assert_ne!(pm.flags & FL_WATERJUMP, 0);

    // When the timer runs out the flag goes away.
    pm.waterjumptime = 5.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterjumptime, 0.0);
    assert_eq!(pm.flags & FL_WATERJUMP, 0);
}

#[test]
fn no_water_jump_when_the_wall_is_too_high_or_the_player_backs_in() {
    // Wall top z = 100 is above the second probe (72): blocked, no jump, but movedir was written.
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 45.0), CONTENTS_WATER, CONTENTS_WATER);
    w.solid(Vec3::new(20.0, -200.0, -100.0), Vec3::new(200.0, 200.0, 100.0), "WALL");
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.cmd.forwardmove = 250.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterlevel, 2);
    assert_eq!(pm.waterjumptime, 0.0);
    assert_eq!(pm.flags & FL_WATERJUMP, 0);
    assert_eq!(pm.movedir, Vec3::new(50.0, 0.0, 0.0), "movedir is written before the second probe");

    // Backing into the ledge (moving against the facing direction) returns early.
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 45.0), CONTENTS_WATER, CONTENTS_WATER);
    w.solid(Vec3::new(20.0, -200.0, -100.0), Vec3::new(200.0, 200.0, 60.0), "LEDGE");
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.velocity = Vec3::new(-50.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterjumptime, 0.0);

    // Falling fast (just jumped in): velocity.z < -180 returns early.
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 45.0), CONTENTS_WATER, CONTENTS_WATER);
    w.solid(Vec3::new(20.0, -200.0, -100.0), Vec3::new(200.0, 200.0, 60.0), "LEDGE");
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.cmd.forwardmove = 250.0;
    pm.velocity = Vec3::new(0.0, 0.0, -200.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterjumptime, 0.0);
}

#[test]
fn currents_push_through_basevelocity() {
    // A CONTENTS_CURRENT_0 volume (+x current): PM_CheckWater adds 50 * waterlevel(3) = 150 * (1,0,0)
    // to basevelocity. PM_WaterMove adds it to the velocity: (150,0,0); friction
    // newspeed = 150 - 4 * 0.01 * 150 = 144; the sinking wish (48) cannot add speed; the player moves
    // 144 * 0.01 = 1.44 along +x; pm_move then subtracts basevelocity: 144 - 150 = -6.
    let mut w = BoxWorld::with_floor();
    w.water_volume(
        Vec3::new(-1000.0, -1000.0, 0.0),
        Vec3::new(1000.0, 1000.0, 200.0),
        CONTENTS_WATER,
        CONTENTS_CURRENT_0,
    );
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 100.0));
    pm.maxspeed = 250.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.waterlevel, 3);
    assert_close(pm.origin.x, 1.44, 1e-3, "carried by the current");
    assert_close(pm.velocity.x, -6.0, 1e-3, "velocity after the base velocity is taken out");
}

#[test]
fn entering_and_leaving_water_plays_wade() {
    // Entering: the player starts dry on the floor and the water begins at x = 0.05; the first
    // command moves it 0.125 along +x (accelspeed 12.5 * 0.01), so at the end of the frame the feet
    // and waist are wet (level 2, surface 50) while oldwaterlevel (taken after the first
    // categorize) is 0. RandomLong(0, 3) = 2 -> pl_wade3 on CHAN_BODY at VOL_NORM/ATTN_NORM.
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(0.05, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 50.0), CONTENTS_WATER, CONTENTS_WATER);
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.movevars.accelerate = 5.0;
    pm.cmd.forwardmove = 250.0;
    w.random_values.push_back(2);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.oldwaterlevel, 0);
    assert_eq!(pm.waterlevel, 2);
    assert_eq!(wade_sounds(&w), ["player/pl_wade3.wav"]);
    let s = w.sounds.iter().find(|s| s.sample.starts_with("player/pl_wade")).unwrap();
    assert_eq!((s.channel, s.volume, s.attenuation, s.flags, s.pitch), (CHAN_BODY, VOL_NORM, ATTN_NORM, 0, PITCH_NORM));

    // Leaving: the water ends at x = 0.05 (surface 10: feet wet only, level 1); the player starts
    // inside and moves 0.125 out. old = 1, new = 0 -> pl_wade1 (RandomLong = 0).
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(0.05, 1000.0, 10.0), CONTENTS_WATER, CONTENTS_WATER);
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    pm.movevars.accelerate = 5.0;
    pm.cmd.forwardmove = 250.0;
    w.random_values.push_back(0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.oldwaterlevel, 1);
    assert_eq!(pm.waterlevel, 0);
    assert_eq!(wade_sounds(&w), ["player/pl_wade1.wav"]);

    // Staying in the water (level 1 before and after) or on dry land is silent and draws no
    // random number.
    let mut w = BoxWorld::with_floor();
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 10.0), CONTENTS_WATER, CONTENTS_WATER);
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    w.random_values.push_back(3);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert!(wade_sounds(&w).is_empty());
    assert_eq!(w.random_values.len(), 1, "no RandomLong drawn");
}

#[test]
fn roaming_spectator_flies_at_spectatormaxspeed() {
    // OBS_ROAMING with forwardmove 1000 (maxspeed 1000 so PM_CheckParameters leaves it): wishvel = forward * 1000 (forward (1,0,0), already unit);
    // wishspeed 1000 is capped to spectatormaxspeed 500. First frame: velocity is zero (speed < 1
    // clears it); addspeed = 500; accelspeed = accelerate(10) * 0.01 * 500 = 50 -> velocity.x = 50,
    // the origin moves 50 * 0.01 = 0.5.
    let w0 = || BoxWorld::new();
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 500.0));
    let mut w = w0();
    pm.iuser1 = OBS_ROAMING;
    pm.cmd.forwardmove = 1000.0;
    pm.maxspeed = 1000.0; // PM_CheckParameters clamps the command to maxspeed first
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.x, 50.0, 1e-4, "first frame velocity");
    assert_close(pm.origin.x, 0.5, 1e-4, "first frame origin");

    // The speed settles at exactly spectatormaxspeed: friction = 4 * 1.5 = 6, control = max(100, speed),
    // drop = 6 * control * 0.01; the last step is capped by addspeed = 500 - newspeed.
    for _ in 0..100 {
        run_cmd(&mut pm, &mut sh, &mut w, 10);
    }
    assert_close(pm.velocity.x, 500.0, 1e-3, "terminal speed");
    assert_eq!(pm.velocity.y, 0.0);
    assert!(pm.origin.x > 100.0, "flew forward: {}", pm.origin.x);

    // A server with a lower spectatormaxspeed caps lower.
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 500.0));
    pm.iuser1 = OBS_ROAMING;
    pm.cmd.forwardmove = 1000.0;
    pm.maxspeed = 1000.0; // PM_CheckParameters clamps the command to maxspeed first
    pm.movevars.spectatormaxspeed = 300.0;
    for _ in 0..100 {
        run_cmd(&mut pm, &mut sh, &mut w, 10);
    }
    assert_close(pm.velocity.x, 300.0, 1e-3, "capped by the movevar");
}

#[test]
fn roaming_spectator_without_input_only_slows_down() {
    // speed 100: friction = 4 * 1.5 = 6, control = 100 (not below stopspeed 100),
    // drop = 6 * (100 * 0.01) = 6 -> speed 94. No wish: addspeed = 0 - 0 <= 0, the function
    // returns before the move, so the origin does not change that frame (a quirk of the source).
    let (mut pm, mut sh) = standing_player(Vec3::new(0.0, 0.0, 500.0));
    let mut w = BoxWorld::new();
    pm.iuser1 = OBS_ROAMING;
    pm.velocity = Vec3::new(100.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity.x, 94.0, 1e-4, "velocity after friction");
    assert_eq!(pm.origin, Vec3::new(0.0, 0.0, 500.0));

    // Below speed 1 the velocity is zeroed.
    pm.velocity = Vec3::new(0.5, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.velocity, Vec3::ZERO);
}

#[test]
fn chasing_spectator_takes_the_target_origin() {
    // Every mode but OBS_ROAMING copies the origin and angles of the physent whose info is iuser2
    // and zeroes the velocity.
    for mode in [OBS_CHASE_LOCKED, OBS_CHASE_FREE, OBS_IN_EYE] {
        let (mut pm, mut sh) = standing_player(Vec3::new(1.0, 2.0, 3.0));
        let mut w = BoxWorld::new();
        w.infos.push((7, Vec3::new(100.0, 200.0, 300.0), Vec3::new(10.0, 20.0, 30.0)));
        pm.iuser1 = mode;
        pm.iuser2 = 7;
        pm.velocity = Vec3::new(5.0, 5.0, 5.0);
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        assert_eq!(pm.origin, Vec3::new(100.0, 200.0, 300.0), "mode {mode}");
        assert_eq!(pm.angles, Vec3::new(10.0, 20.0, 30.0), "mode {mode}");
        assert_eq!(pm.velocity, Vec3::ZERO, "mode {mode}");
    }

    // No target (iuser2 <= 0): nothing changes and the world is not asked.
    let (mut pm, mut sh) = standing_player(Vec3::new(1.0, 2.0, 3.0));
    let mut w = BoxWorld::new();
    w.infos.push((7, Vec3::new(100.0, 200.0, 300.0), Vec3::ZERO));
    pm.iuser1 = OBS_CHASE_LOCKED;
    pm.iuser2 = 0;
    pm.velocity = Vec3::new(5.0, 5.0, 5.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.origin, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(pm.velocity, Vec3::new(5.0, 5.0, 5.0));
    assert!(!w.call_kinds().contains(&"PBI"));

    // A target that is not among the physents: nothing changes either.
    pm.iuser2 = 9;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.origin, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(pm.velocity, Vec3::new(5.0, 5.0, 5.0));
}
