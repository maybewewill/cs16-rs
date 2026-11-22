//! Behaviour of `PM_Duck`, `PM_UnDuck` and `PM_Jump` (Task 5) through `pm_move`, on `BoxWorld`.
//! Expected numbers are worked out by hand from the formulas of `pm_shared.cpp` (ReGameDLL_CS, read
//! 2026-11-22); the formula is in each test's comment. Floats are `f32` throughout, as in the source.
//!
//! The kernels themselves (exact water-jump velocities, the swim-sound timer, the bunny-hop crop)
//! are also unit-tested in `src/pm/jump.rs`.

mod pm_support;

use cs_sim::pm::consts::*;
use cs_sim::pm::{PlayerMove, PmShared};
use glam::Vec3;
use pm_support::{BoxWorld, run_cmd, standing_player};

/// A walker standing on the floor of `BoxWorld::with_floor()` (floor top z = 0, hull half height
/// 36, so the origin is at z = 36), `maxspeed` 250 as the game sets it. In `BoxWorld` a player
/// resting exactly on a plane is not moved by `PM_CategorizePosition` (the trace fraction clamps
/// to 0), so the origin stays exactly at 36.
fn on_floor() -> (PlayerMove, PmShared, BoxWorld) {
    let w = BoxWorld::with_floor();
    let (mut pm, sh) = standing_player(Vec3::new(0.0, 0.0, 36.0));
    pm.maxspeed = 250.0;
    (pm, sh, w)
}

/// A walker already ducked on the floor (duck hull, origin z = 18, view 12).
fn ducked_on_floor(w: BoxWorld) -> (PlayerMove, PmShared, BoxWorld) {
    let (mut pm, sh) = standing_player(Vec3::new(0.0, 0.0, 18.0));
    pm.maxspeed = 250.0;
    pm.usehull = 1;
    pm.flags |= FL_DUCKING;
    pm.view_ofs[2] = PM_VEC_DUCK_VIEW;
    (pm, sh, w)
}

fn assert_close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tol {tol})");
}

fn press(pm: &mut PlayerMove, buttons: i32) {
    pm.cmd.buttons = buttons as u16;
}

/// Runs 10 ms commands with `buttons` until the player is on the ground; returns the count.
fn fall_until_landed(pm: &mut PlayerMove, sh: &mut PmShared, w: &mut BoxWorld, buttons: i32) -> usize {
    for n in 1..=300 {
        press(pm, buttons);
        run_cmd(pm, sh, w, 10);
        if pm.onground != -1 {
            return n;
        }
    }
    panic!("never landed");
}

#[test]
fn duck_takes_four_tenths_and_follows_the_spline() {
    let (mut pm, mut sh, mut w) = on_floor();
    press(&mut pm, IN_DUCK);

    // Command 1 (the press): flDuckTime = 1000, bInDuck. view_ofs from time = 1 - 1000/1000 = 0:
    // fraction 0, view = (12 - 18) * 0 + 17 * 1 = 17.
    run_cmd(&mut pm, &mut sh, &mut w, 100);
    assert!(pm.b_in_duck);
    assert_eq!(pm.fl_duck_time, 1000.0);
    assert_close(pm.view_ofs[2], 17.0, 1e-5, "view at the press");
    assert_eq!(pm.flags & FL_DUCKING, 0);

    // Commands 2..4: flDuckTime 900 / 800 / 700, time = 0.1 / 0.2 / 0.3, scale 1/0.4 = 2.5,
    // v = 0.25 / 0.5 / 0.75, spline 3v^2 - 2v^3 = 0.15625 / 0.5 / 0.84375,
    // view = (12 - 18) * s + 17 * (1 - s) = 13.40625 / 5.5 / -2.40625.
    for (ms_left, view) in [(900.0, 13.406_25), (800.0, 5.5), (700.0, -2.406_25)] {
        run_cmd(&mut pm, &mut sh, &mut w, 100);
        assert_eq!(pm.fl_duck_time, ms_left);
        assert!(pm.b_in_duck);
        assert_close(pm.view_ofs[2], view, 1e-4, "view on the spline");
        assert_eq!(pm.usehull, 0);
        assert_close(pm.origin[2], 36.0, 1e-4, "still standing");
    }

    // Command 5: flDuckTime 600, 600/1000.0 <= 1.0 - 0.4 (both exactly the double 0.6): done.
    run_cmd(&mut pm, &mut sh, &mut w, 100);
    assert_eq!(pm.fl_duck_time, 600.0);
    assert!(!pm.b_in_duck);
    assert_ne!(pm.flags & FL_DUCKING, 0);
    assert_eq!(pm.usehull, 1);
    assert_eq!(pm.view_ofs[2], PM_VEC_DUCK_VIEW);
    // Grounded: the origin drops by 18 (36 -> 18, the duck hull's resting height).
    assert_close(pm.origin[2], 18.0, 1e-4, "ducked origin");
    assert_ne!(pm.onground, -1);
}

#[test]
fn duck_in_the_air_is_immediate() {
    let (mut pm, mut sh, mut w) = on_floor();
    pm.origin[2] = 200.0;
    press(&mut pm, IN_DUCK);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    // `onground == -1`: the duck finishes in the press command; no 18 unit shift (only gravity moved us).
    assert_eq!(pm.onground, -1);
    assert!(!pm.b_in_duck);
    assert_ne!(pm.flags & FL_DUCKING, 0);
    assert_eq!(pm.usehull, 1);
    assert_eq!(pm.view_ofs[2], PM_VEC_DUCK_VIEW);
    assert!(pm.origin[2] > 199.9, "origin z {}", pm.origin[2]);
}

#[test]
fn unduck_is_blocked_under_a_low_ceiling() {
    // Ceiling bottom at 60: the standing hull needs origin 36 + 36 = 72 of headroom.
    let mut w = BoxWorld::with_floor();
    w.solid(Vec3::new(-1000.0, -1000.0, 60.0), Vec3::new(1000.0, 1000.0, 100.0), "CEIL");
    let (mut pm, mut sh, mut w) = ducked_on_floor(w);
    pm.fl_time_step_sound = 250;
    press(&mut pm, 0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    // UnDuck: newOrigin z = 18 + 18 = 36; the duck hull fits (top 54 < 60), the standing hull does
    // not (top 72 > 60): `usehull` goes back to 1 and nothing else changes.
    assert_eq!(pm.usehull, 1);
    assert_ne!(pm.flags & FL_DUCKING, 0);
    assert_eq!(pm.view_ofs[2], PM_VEC_DUCK_VIEW);
    assert_close(pm.origin[2], 18.0, 1e-4, "stays ducked on the floor");
    assert_eq!(pm.fl_time_step_sound, 240); // only the 10 ms of PM_ReduceTimers

    // Ceiling bottom at 80: the player stands up, 100 ms of step-sound timer are given back.
    let mut w = BoxWorld::with_floor();
    w.solid(Vec3::new(-1000.0, -1000.0, 80.0), Vec3::new(1000.0, 1000.0, 120.0), "CEIL");
    let (mut pm, mut sh, mut w) = ducked_on_floor(w);
    pm.fl_time_step_sound = 250;
    pm.fl_duck_time = 500.0;
    pm.b_in_duck = true;
    press(&mut pm, 0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.usehull, 0);
    assert_eq!(pm.flags & FL_DUCKING, 0);
    assert!(!pm.b_in_duck);
    assert_eq!(pm.view_ofs[2], PM_VEC_VIEW);
    assert_eq!(pm.fl_duck_time, 0.0);
    assert_close(pm.origin[2], 36.0, 1e-4, "standing origin (+18)");
    // 250 - 10 (ReduceTimers) - 100 (UnDuck) = 140.
    assert_eq!(pm.fl_time_step_sound, 140);
    assert_ne!(pm.onground, -1);
}

#[test]
fn jump_speed_is_268_and_stamina_reduces_the_next() {
    let (mut pm, mut sh, mut w) = on_floor();
    press(&mut pm, IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    // vz = sqrt(2 * 800 * 45) = 268.3282, then two gravity half steps of 800 * 0.01 * 0.5 = 4
    // (PM_FixupGravityVelocity in PM_Jump and the one after PM_AirMove) = 260.3282.
    assert_close(pm.velocity[2], 260.328_2, 2e-3, "vz after the first jump");
    assert_eq!(pm.onground, -1);
    assert_eq!(pm.fuser2, 1_315.789_4_f32);
    assert_ne!(pm.oldbuttons & IN_JUMP, 0);
    let first = pm.velocity[2];

    // Fall back (the key is released on the way, which re-arms the jump).
    fall_until_landed(&mut pm, &mut sh, &mut w, 0);
    assert!(pm.fuser2 > 0.0, "stamina left: {}", pm.fuser2);
    assert_eq!(pm.oldbuttons & IN_JUMP, 0);

    // Second jump: ReduceTimers first takes 10 ms off fuser2 (f), then
    // ratio = (100 - f * 0.001 * 19) * 0.01 and vz = 268.3282 * ratio - 8.
    let f = f64::from(pm.fuser2 - 10.0);
    let ratio = (100.0 - f * 0.001 * 19.0) * 0.01;
    assert!(ratio < 1.0);
    press(&mut pm, IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    let expect = 268.328_15_f64 * ratio - 8.0;
    assert!((f64::from(pm.velocity[2]) - expect).abs() < 1e-2, "{} vs {expect}", pm.velocity[2]);
    assert!(pm.velocity[2] < first);
    assert_eq!(pm.fuser2, 1_315.789_4_f32);
}

#[test]
fn holding_jump_does_not_rejump() {
    let (mut pm, mut sh, mut w) = on_floor();
    press(&mut pm, IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.onground, -1);

    // Keep the key down until the player lands, then a few commands more: no new jump.
    fall_until_landed(&mut pm, &mut sh, &mut w, IN_JUMP);
    for _ in 0..5 {
        press(&mut pm, IN_JUMP);
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        assert_ne!(pm.onground, -1);
        assert_eq!(pm.velocity[2], 0.0);
    }

    // Release for one command, press again: it jumps.
    press(&mut pm, 0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.oldbuttons & IN_JUMP, 0);
    press(&mut pm, IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.onground, -1);
    assert!(pm.velocity[2] > 200.0, "vz {}", pm.velocity[2]);
}

#[test]
fn mega_bunny_jumping_is_capped() {
    // maxspeed 250: limit 1.2 * 250 = 300. PM_AddCorrectGravity runs before the jump and leaves
    // vz = -800 * 0.01 * 0.5 = -4 (the player is still counted as grounded), so the speed the jump
    // sees is sqrt(400^2 + 4^2) = 400.02 and vx becomes 400 * (300 / 400.02) * 0.8 = 239.988
    // (no friction afterwards: the player is airborne, and without wish input PM_AirMove adds nothing).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(400.0, 0.0, 0.0);
    press(&mut pm, IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.onground, -1);
    let spd = (400.0f64 * 400.0 + 16.0).sqrt();
    assert_close(pm.velocity[0], (400.0 * (300.0 / spd) * 0.8) as f32, 1e-3, "vx after the cropped jump");
    assert!(pm.velocity[0] < 240.0);

    // Under the limit (sqrt(299^2 + 4^2) = 299.03 <= 300) nothing is cropped.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.velocity = Vec3::new(299.0, 0.0, 0.0);
    press(&mut pm, IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_close(pm.velocity[0], 299.0, 1e-3, "vx under the limit");
}

#[test]
fn duck_jump_rules_as_source() {
    // Walk commands are scaled by 0.333 in every command with the duck key down (float product).
    let (mut pm, mut sh, mut w) = on_floor();
    pm.cmd.forwardmove = 150.0;
    pm.cmd.sidemove = 100.0;
    pm.cmd.upmove = 50.0;
    press(&mut pm, IN_DUCK);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.cmd.forwardmove, 150.0f32 * 0.333f32);
    assert_eq!(pm.cmd.sidemove, 100.0f32 * 0.333f32);
    assert_eq!(pm.cmd.upmove, 50.0f32 * 0.333f32);

    // ... and untouched without it.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.cmd.forwardmove = 150.0;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.cmd.forwardmove, 150.0);

    // While the duck transition is running (bInDuck with FL_DUCKING) jumping does nothing, and the
    // jump key is not even remembered in oldbuttons.
    let (mut pm, mut sh, mut w) = ducked_on_floor(BoxWorld::with_floor());
    pm.b_in_duck = true;
    pm.fl_duck_time = 800.0;
    pm.oldbuttons = IN_DUCK;
    press(&mut pm, IN_DUCK | IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_ne!(pm.onground, -1);
    assert_eq!(pm.velocity[2], 0.0);
    assert_eq!(pm.oldbuttons & IN_JUMP, 0);

    // A finished duck jumps normally (vanilla has no long jump and no duck penalty).
    let (mut pm, mut sh, mut w) = ducked_on_floor(BoxWorld::with_floor());
    pm.oldbuttons = IN_DUCK;
    press(&mut pm, IN_DUCK | IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.onground, -1);
    assert_close(pm.velocity[2], 260.328_2, 2e-3, "ducked jump vz");
    assert_ne!(pm.flags & FL_DUCKING, 0);
    assert_eq!(pm.usehull, 1);

    // A dead player does not jump, but the key is remembered.
    let (mut pm, mut sh, mut w) = on_floor();
    pm.dead = true;
    press(&mut pm, IN_JUMP);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_ne!(pm.onground, -1);
    assert_eq!(pm.velocity[2], 0.0);
    assert_ne!(pm.oldbuttons & IN_JUMP, 0);
}

/// A player at the origin of a huge liquid volume of type `contents` (waterlevel 3).
fn in_liquid(contents: i32) -> (PlayerMove, PmShared, BoxWorld) {
    let mut w = BoxWorld::new();
    w.water_volume(Vec3::splat(-5000.0), Vec3::splat(5000.0), contents, contents);
    let (pm, sh) = standing_player(Vec3::ZERO);
    (pm, sh, w)
}

#[test]
fn jumping_in_deep_water_swims_up() {
    // The jump sets vz to 100 (water) / 80 (slime) / 50 (lava) (exact values: unit test in
    // src/pm/jump.rs); the swim move that follows may change them, but their order and sign stay.
    let mut vz = Vec::new();
    for contents in [CONTENTS_WATER, CONTENTS_SLIME, CONTENTS_LAVA] {
        let (mut pm, mut sh, mut w) = in_liquid(contents);
        press(&mut pm, IN_JUMP);
        run_cmd(&mut pm, &mut sh, &mut w, 10);
        assert_eq!(pm.waterlevel, 3);
        assert_eq!(pm.watertype, contents);
        assert_eq!(pm.onground, -1);
        vz.push(pm.velocity[2]);
    }
    assert!(vz[0] > vz[1] && vz[1] > vz[2] && vz[2] > 0.0, "{vz:?}");

    // The wade sound: RandomLong(0, 3) picks `pl_wade<n+1>`; once per second of flSwimTime
    // (1000, counted down by PM_ReduceTimers, 50 ms per command: the 21st command plays again).
    let wades = |w: &BoxWorld| -> Vec<String> {
        w.sounds.iter().filter(|s| s.sample.starts_with("player/pl_wade")).map(|s| s.sample.clone()).collect()
    };
    let (mut pm, mut sh, mut w) = in_liquid(CONTENTS_WATER);
    w.random_values.extend([0, 1]);
    press(&mut pm, IN_JUMP); // held the whole time: the water jump has no oldbuttons rule
    run_cmd(&mut pm, &mut sh, &mut w, 50);
    assert_eq!(pm.fl_swim_time, 1000.0);
    assert_eq!(wades(&w), ["player/pl_wade1.wav"]);
    for _ in 2..=20 {
        run_cmd(&mut pm, &mut sh, &mut w, 50);
    }
    assert_eq!(pm.fl_swim_time, 50.0);
    assert_eq!(wades(&w), ["player/pl_wade1.wav"]);
    run_cmd(&mut pm, &mut sh, &mut w, 50);
    assert_eq!(wades(&w), ["player/pl_wade1.wav", "player/pl_wade2.wav"]);
    let s = &w.sounds[w.sounds.len() - 1];
    assert_eq!((s.channel, s.volume, s.attenuation, s.flags, s.pitch), (CHAN_BODY, 1.0, 0.8, 0, 100));
}
