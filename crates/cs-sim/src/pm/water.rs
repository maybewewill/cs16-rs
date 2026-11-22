//! Swimming: `PM_CheckWaterJump`, `PM_WaterJump`, `PM_WaterMove`, `PM_PlayWaterSounds`
//! (ReGameDLL_CS `pm_shared.cpp`, read 2026-11-22).

use glam::Vec3;

use super::consts::{ATTN_NORM, CHAN_BODY, FL_WATERJUMP, IN_JUMP, PITCH_NORM, VOL_NORM, WJ_HEIGHT};
use super::math::{dot, vector_ma, vector_normalize, vector_scale};
use super::types::PmCtx;
use super::walk::pm_fly_move;

/// `PM_CheckWaterJump` (pm_shared.cpp 2832-2905, read 2026-11-22): starts a water jump when the
/// swimmer presses against a ledge it can climb onto.
///
/// Quirks kept: the probe traces use hull 2 (the point hull) through `usehull`, restored at the
/// end; `movedir` is written (`-50 * normal`) before the second trace even when that trace is
/// blocked; the early returns leave `usehull` untouched (it was not changed yet).
pub(crate) fn pm_check_water_jump(c: &mut PmCtx<'_>) {
    // Already water jumping.
    if c.pm.waterjumptime != 0.0 {
        return;
    }

    // Don't hop out if we just jumped in
    if c.pm.velocity[2] < -180.0 {
        // only hop out if we are moving up
        return;
    }

    // See if we are backing up
    let mut flatvelocity = Vec3::new(c.pm.velocity[0], c.pm.velocity[1], 0.0);

    // Must be moving
    let curspeed = vector_normalize(&mut flatvelocity);

    // see if near an edge
    let mut flatforward = Vec3::new(c.pm.forward[0], c.pm.forward[1], 0.0);
    vector_normalize(&mut flatforward);

    // Are we backing into water from steps or something?  If so, don't pop forward
    if curspeed != 0.0 && dot(flatvelocity, flatforward) < 0.0 {
        return;
    }

    let mut vec_start = c.pm.origin;
    vec_start[2] += WJ_HEIGHT;

    let mut vec_end = vector_ma(vec_start, 24.0, flatforward);

    // Trace, this trace should use the point sized collision hull
    let savehull = c.pm.usehull;
    c.pm.usehull = 2;

    let mut tr = c.trace(vec_start, vec_end);

    // Facing a near vertical wall?
    if tr.fraction < 1.0 && tr.plane_normal[2].abs() < 0.1 {
        let save_index = savehull.clamp(0, 3) as usize;
        vec_start[2] += c.pm.player_maxs[save_index][2] - WJ_HEIGHT;

        vec_end = vector_ma(vec_start, 24.0, flatforward);
        c.pm.movedir = vector_ma(Vec3::ZERO, -50.0, tr.plane_normal);

        tr = c.trace(vec_start, vec_end);

        if tr.fraction == 1.0 {
            c.pm.waterjumptime = 2000.0;
            c.pm.velocity[2] = 225.0;

            c.pm.oldbuttons |= IN_JUMP;
            c.pm.flags |= FL_WATERJUMP;
        }
    }

    // Reset the collision hull
    c.pm.usehull = savehull;
}

/// `PM_WaterJump` (pm_shared.cpp 2379-2401, read 2026-11-22): counts the water-jump timer down by
/// `cmd.msec` and keeps the horizontal velocity at `movedir`.
///
/// Quirk kept: the timer is clamped to 10000 first; the velocity is overwritten with `movedir`
/// even on the frame the jump ends.
pub(crate) fn pm_water_jump(c: &mut PmCtx<'_>) {
    if c.pm.waterjumptime > 10000.0 {
        c.pm.waterjumptime = 10000.0;
    }

    if c.pm.waterjumptime == 0.0 {
        return;
    }

    c.pm.waterjumptime -= f32::from(c.pm.cmd.msec);

    if c.pm.waterjumptime < 0.0 || c.pm.waterlevel == 0 {
        c.pm.waterjumptime = 0.0;
        c.pm.flags &= !FL_WATERJUMP;
    }

    c.pm.velocity[0] = c.pm.movedir[0];
    c.pm.velocity[1] = c.pm.movedir[1];
}

/// `PM_WaterMove` (pm_shared.cpp 1376-1481, read 2026-11-22).
///
/// Quirks kept: with no input the player drifts down (`wishvel.z -= 60`), else `upmove` is added
/// to `wishvel.z`; `wishspeed *= 0.8` is a `double` product stored into a `real_t`; the base
/// velocity is added to the velocity here (and subtracted by the caller); the water friction uses
/// `movevars.friction` (not `waterfriction`) times the entity friction; a wish below 0.1 returns
/// before the move, leaving the origin unchanged; the move first tries a straight trace from a
/// step height above the destination, else falls back to `PM_FlyMove`.
pub(crate) fn pm_water_move(c: &mut PmCtx<'_>) {
    // user intentions
    let mut wishvel = Vec3::new(
        c.pm.forward[0] * c.pm.cmd.forwardmove + c.pm.cmd.sidemove * c.pm.right[0],
        c.pm.forward[1] * c.pm.cmd.forwardmove + c.pm.cmd.sidemove * c.pm.right[1],
        c.pm.forward[2] * c.pm.cmd.forwardmove + c.pm.cmd.sidemove * c.pm.right[2],
    );

    // Sinking after no other movement occurs
    if c.pm.cmd.forwardmove == 0.0 && c.pm.cmd.sidemove == 0.0 && c.pm.cmd.upmove == 0.0 {
        // drift towards bottom
        wishvel[2] -= 60.0;
    } else {
        // Go straight up by upmove amount.
        wishvel[2] += c.pm.cmd.upmove;
    }

    // Copy it over and determine speed
    let mut wishdir = wishvel;
    let mut wishspeed = vector_normalize(&mut wishdir);

    // Cap speed.
    if wishspeed > c.pm.maxspeed {
        wishvel = vector_scale(wishvel, c.pm.maxspeed / wishspeed);
        wishspeed = c.pm.maxspeed;
    }

    // Slow us down a bit.
    wishspeed = (f64::from(wishspeed) * 0.8) as f32;
    c.pm.velocity += c.pm.basevelocity;

    // Water friction
    let mut temp = c.pm.velocity;
    let speed = vector_normalize(&mut temp);

    let newspeed = if speed != 0.0 {
        let mut ns = speed - c.pm.movevars.friction * c.pm.friction * c.pm.frametime * speed;

        if ns < 0.0 {
            ns = 0.0;
        }

        c.pm.velocity = vector_scale(c.pm.velocity, ns / speed);
        ns
    } else {
        0.0
    };

    // water acceleration
    if wishspeed < 0.1 {
        return;
    }

    let addspeed = wishspeed - newspeed;

    if addspeed > 0.0 {
        vector_normalize(&mut wishvel);
        let mut accelspeed = c.pm.movevars.accelerate * c.pm.friction * c.pm.frametime * wishspeed;

        if accelspeed > addspeed {
            accelspeed = addspeed;
        }

        for i in 0..3 {
            c.pm.velocity[i] += accelspeed * wishvel[i];
        }
    }

    // Now move
    // assume it is a stair or a slope, so press down from stepheight above
    let dest = vector_ma(c.pm.origin, c.pm.frametime, c.pm.velocity);
    let mut start = dest;

    start[2] += c.pm.movevars.stepsize + 1.0;
    let trace = c.trace(start, dest);

    // FIXME: check steep slope?
    if !trace.startsolid && !trace.allsolid {
        // walked up the step, so just keep result and exit
        c.pm.origin = trace.endpos;
        return;
    }

    // Try moving straight along out normal path.
    pm_fly_move(c);
}

/// `PM_PlayWaterSounds` (pm_shared.cpp 2955-2976, read 2026-11-22): a random wade sound when the
/// player entered or left the water (`oldwaterlevel` vs `waterlevel`).
///
/// Quirk kept: it plays through the world callback even when `runfuncs` is off, as the source
/// does not check it here.
pub(crate) fn pm_play_water_sounds(c: &mut PmCtx<'_>) {
    // Did we enter or leave water?
    if c.pm.oldwaterlevel != 0 {
        if c.pm.waterlevel != 0 {
            return;
        }
    } else if c.pm.waterlevel == 0 {
        return;
    }

    let sample = match c.w.random_long(0, 3) {
        0 => "player/pl_wade1.wav",
        1 => "player/pl_wade2.wav",
        2 => "player/pl_wade3.wav",
        3 => "player/pl_wade4.wav",
        _ => return,
    };
    c.w.play_sound(CHAN_BODY, sample, VOL_NORM, ATTN_NORM, 0, PITCH_NORM);
}
