//! Per-frame preparation and position queries of `pm_shared`: `PM_CheckParameters`, `PM_CalcRoll`,
//! `PM_DropPunchAngle`, `PM_ReduceTimers`, `PM_InWater`, `PM_CheckWater`, `PM_CategorizePosition`,
//! `PM_CheckFalling`. Ported from ReGameDLL_CS `regamedll/pm_shared/pm_shared.cpp` (vanilla
//! branch), read 2026-11-22.

use glam::Vec3;

use super::consts::{
    CONTENTS_CURRENT_0, CONTENTS_CURRENT_DOWN, CONTENTS_EMPTY, CONTENTS_TRANSLUCENT, CONTENTS_WATER, FL_FROZEN,
    FL_ONTRAIN, PM_DEAD_VIEWHEIGHT, PM_PLAYER_FALL_PUNCH_THRESHHOLD, PM_PLAYER_MAX_SAFE_FALL_SPEED,
    PM_PLAYER_MIN_BOUNCE_SPEED,
};
use super::math::{PITCH, ROLL, YAW, angle_vectors, dot, vector_normalize, vector_scale};
use super::sound::{pm_catagorize_texture_type, pm_map_texture_type_step_type, pm_play_step_sound};
use super::types::PmCtx;
use super::walk::pm_add_to_touched;

/// `PM_CalcRoll` (2978-3003): view roll from the sideways velocity.
pub(crate) fn pm_calc_roll(angles: Vec3, velocity: Vec3, rollangle: f32, rollspeed: f32) -> f32 {
    let (_forward, right, _up) = angle_vectors(angles);

    let mut side = dot(velocity, right);
    let sign: f32 = if side < 0.0 { -1.0 } else { 1.0 };
    side = side.abs();

    let value = rollangle;

    if side < rollspeed {
        side = side * value / rollspeed;
    } else {
        side = value;
    }

    side * sign
}

/// `PM_DropPunchAngle` (3005-3017): shrinks the punch angle by `10 + len/2` units per second.
/// QUIRK: uses `pmove->frametime` as the engine prefilled it; `PM_PlayerMove` recomputes it from
/// `cmd.msec` only after `PM_CheckParameters`. `len -= (10.0 + len * 0.5) * frametime` is a double
/// expression (`10.0`, `0.5` are `double` literals) stored back into a `float`.
pub(crate) fn pm_drop_punch_angle(c: &mut PmCtx<'_>) {
    let mut punch = c.pm.punchangle;
    let mut len = vector_normalize(&mut punch);
    len = (f64::from(len) - (10.0 + f64::from(len) * 0.5) * f64::from(c.pm.frametime)) as f32;
    len = if len > 0.0 { len } else { 0.0 };
    c.pm.punchangle = vector_scale(punch, len);
}

/// `PM_CheckParameters` (3019-3087): clamps the command to `maxspeed`, builds `angles` (with the
/// roll) from the view angles plus the punch, and sets the dead view height.
pub(crate) fn pm_check_parameters(c: &mut PmCtx<'_>) {
    let cmd = c.pm.cmd;
    let spd = (cmd.sidemove * cmd.sidemove + cmd.forwardmove * cmd.forwardmove + cmd.upmove * cmd.upmove).sqrt();

    let maxspeed = c.pm.clientmaxspeed;

    // Q_min(a, b) is `a < b ? a : b`.
    if maxspeed != 0.0 {
        c.pm.maxspeed = if maxspeed < c.pm.maxspeed { maxspeed } else { c.pm.maxspeed };
    }

    if spd != 0.0 && spd > c.pm.maxspeed {
        let f_ratio = c.pm.maxspeed / spd;

        c.pm.cmd.forwardmove *= f_ratio;
        c.pm.cmd.sidemove *= f_ratio;
        c.pm.cmd.upmove *= f_ratio;
    }

    if (c.pm.flags & (FL_FROZEN | FL_ONTRAIN)) != 0 || c.pm.dead {
        c.pm.cmd.forwardmove = 0.0;
        c.pm.cmd.sidemove = 0.0;
        c.pm.cmd.upmove = 0.0;
    }

    pm_drop_punch_angle(c);

    // Take angles from command.
    if !c.pm.dead {
        let v_angle = c.pm.cmd.viewangles + c.pm.punchangle;

        // Set up view angles.
        c.pm.angles[ROLL] = pm_calc_roll(v_angle, c.pm.velocity, c.pm.movevars.rollangle, c.pm.movevars.rollspeed) * 4.0;
        c.pm.angles[PITCH] = v_angle[PITCH];
        c.pm.angles[YAW] = v_angle[YAW];
    } else {
        c.pm.angles = c.pm.oldangles;
    }

    // Set dead player view_offset
    if c.pm.dead {
        c.pm.view_ofs[2] = PM_DEAD_VIEWHEIGHT;
    }

    // Adjust client view angles to match values used on server.
    if c.pm.angles[YAW] > 180.0 {
        c.pm.angles[YAW] -= 360.0;
    }
}

/// `PM_ReduceTimers` (3089-3142): the step-sound, duck, swim and jump-stamina timers count down by
/// `cmd.msec` and stop at 0. `flTimeStepSound` is an `int`; the others are `float`.
pub(crate) fn pm_reduce_timers(c: &mut PmCtx<'_>) {
    let msec = c.pm.cmd.msec;

    if c.pm.fl_time_step_sound > 0 {
        c.pm.fl_time_step_sound -= i32::from(msec);

        if c.pm.fl_time_step_sound < 0 {
            c.pm.fl_time_step_sound = 0;
        }
    }

    if c.pm.fl_duck_time > 0.0 {
        c.pm.fl_duck_time -= f32::from(msec);

        if c.pm.fl_duck_time < 0.0 {
            c.pm.fl_duck_time = 0.0;
        }
    }

    if c.pm.fl_swim_time > 0.0 {
        c.pm.fl_swim_time -= f32::from(msec);

        if c.pm.fl_swim_time < 0.0 {
            c.pm.fl_swim_time = 0.0;
        }
    }

    if c.pm.fuser2 > 0.0 {
        c.pm.fuser2 -= f32::from(msec);

        if c.pm.fuser2 < 0.0 {
            c.pm.fuser2 = 0.0;
        }
    }
}

/// `PM_InWater` (1538): swimming (waterlevel above 1).
pub(crate) fn pm_in_water(c: &PmCtx<'_>) -> bool {
    c.pm.waterlevel > 1
}

/// `cont <= CONTENTS_WATER && cont > CONTENTS_TRANSLUCENT`: a water-like (non solid, non empty) volume.
fn is_water_contents(cont: i32) -> bool {
    cont <= CONTENTS_WATER && cont > CONTENTS_TRANSLUCENT
}

/// The `current_table` of `PM_CheckWater`, indexed by `CONTENTS_CURRENT_0 - truecont`.
const CURRENT_TABLE: [Vec3; 6] = [
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(-1.0, 0.0, 0.0),
    Vec3::new(0.0, -1.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
    Vec3::new(0.0, 0.0, -1.0),
];

/// `PM_CheckWater` (1544-1618): sets `waterlevel` (1 feet, 2 waist, 3 eyes) and `watertype`, adds
/// the water current to `basevelocity`. Returns whether the player is swimming (level above 1).
/// (The `REGAMEDLL_FIXES` skip for dead players is not part of vanilla.)
pub(crate) fn pm_check_water(c: &mut PmCtx<'_>) -> bool {
    // Assume that we are not in water at all.
    c.pm.waterlevel = 0;
    c.pm.watertype = CONTENTS_EMPTY;

    let hull = c.pm.hull_index();
    let mins = c.pm.player_mins[hull];
    let maxs = c.pm.player_maxs[hull];
    let origin = c.pm.origin;

    // Pick a spot just above the players feet.
    let mut point = Vec3::new(
        origin[0] + (mins[0] + maxs[0]) * 0.5,
        origin[1] + (mins[1] + maxs[1]) * 0.5,
        origin[2] + mins[2] + 1.0,
    );

    // Grab point contents.
    let (cont, truecont) = c.w.point_contents(point);

    // Are we under water? (not solid and not empty?)
    if is_water_contents(cont) {
        // Set water type
        c.pm.watertype = cont;

        // We are at least at level one
        c.pm.waterlevel = 1;

        let height = mins[2] + maxs[2];
        // `height * 0.5` is a double product stored into a float (exact).
        let heightover2 = (f64::from(height) * 0.5) as f32;

        // Now check a point that is at the player hull midpoint.
        point[2] = origin[2] + heightover2;
        let (cont, _) = c.w.point_contents(point);

        // If that point is also under water...
        if is_water_contents(cont) {
            // Set a higher water level.
            c.pm.waterlevel = 2;

            // Now check the eye position.  (view_ofs is relative to the origin)
            point[2] = origin[2] + c.pm.view_ofs[2];

            let (cont, _) = c.w.point_contents(point);
            if is_water_contents(cont) {
                // In over our eyes
                c.pm.waterlevel = 3;
            }
        }

        // Adjust velocity based on water current, if any.
        if (CONTENTS_CURRENT_DOWN..=CONTENTS_CURRENT_0).contains(&truecont) {
            // The deeper we are, the stronger the current. `50.0 * waterlevel` is a double product
            // (exact) passed as a float scale.
            let scale = (50.0 * f64::from(c.pm.waterlevel)) as f32;
            let dir = CURRENT_TABLE[(CONTENTS_CURRENT_0 - truecont) as usize];
            c.pm.basevelocity = super::math::vector_ma(c.pm.basevelocity, scale, dir);
        }
    }

    c.pm.waterlevel > 1
}

/// `PM_CategorizePosition` (1620-1691): finds the entity under the player (`onground`, -1 = none)
/// and snaps the origin down onto it.
///
/// Quirks kept: a trace that hit nothing leaves the plane normal at 0, which is below 0.7, so
/// `onground` is -1; a fast upward velocity (> 180) means airborne without tracing; the origin is
/// only snapped when `waterlevel < 2` and the trace did not start solid.
pub(crate) fn pm_categorize_position(c: &mut PmCtx<'_>) {
    // if the player hull point one unit down is solid, the player is on ground

    // see if standing on something solid

    // Doing this before we move may introduce a potential latency in water detection, but
    // doing it after can get us stuck on the bottom in water if the amount we move up is less
    // than the 1 pixel 'threshold' we're about to snap to.
    pm_check_water(c);

    let point = Vec3::new(c.pm.origin[0], c.pm.origin[1], c.pm.origin[2] - 2.0);

    // Shooting up really fast.  Definitely not on ground.
    if c.pm.velocity[2] > 180.0 {
        c.pm.onground = -1;
        return;
    }

    // Try and move down.
    let tr = c.trace(c.pm.origin, point);

    // If we hit a steep plane, we are not on ground
    if tr.plane_normal[2] < 0.7 {
        // too steep
        c.pm.onground = -1;
    } else {
        // Otherwise, point to index of ent under us.
        c.pm.onground = tr.ent;
    }

    // If we are on something...
    if c.pm.onground != -1 {
        // Then we are not in water jump sequence
        c.pm.waterjumptime = 0.0;

        // If we could make the move, drop us down that 1 pixel
        if c.pm.waterlevel < 2 && !tr.startsolid && !tr.allsolid {
            c.pm.origin = tr.endpos;
        }
    }

    // Standing on an entity other than the world
    // So signal that we are touching something.
    if tr.ent > 0 {
        let velocity = c.pm.velocity;
        pm_add_to_touched(c, tr, velocity);
    }
}

/// `PM_CheckFalling` (2907-2953): the landing thud and screen punch. The sound goes through
/// `sound.rs` (Task 7); `flFallVelocity` is cleared whenever the player is on the ground.
pub(crate) fn pm_check_falling(c: &mut PmCtx<'_>) {
    if c.pm.onground != -1
        && !c.pm.dead
        && c.pm.fl_fall_velocity >= PM_PLAYER_FALL_PUNCH_THRESHHOLD as f32
    {
        let mut fvol = 0.5f32;

        if c.pm.waterlevel <= 0 {
            if c.pm.fl_fall_velocity > PM_PLAYER_MAX_SAFE_FALL_SPEED as f32 {
                fvol = 1.0;
            } else if c.pm.fl_fall_velocity > (PM_PLAYER_MAX_SAFE_FALL_SPEED / 2) as f32 {
                // `580 / 2` is an integer division: 290.
                fvol = 0.85;
            } else if c.pm.fl_fall_velocity < PM_PLAYER_MIN_BOUNCE_SPEED as f32 {
                fvol = 0.0;
            }
        }

        if fvol > 0.0 {
            pm_catagorize_texture_type(c);

            // play step sound for current texture
            let step = pm_map_texture_type_step_type(c.pm.chtexturetype);
            pm_play_step_sound(c, step, fvol);

            c.pm.fl_time_step_sound = 300;

            // Knock the screen around a little bit, temporary effect
            // punch z axis. `flFallVelocity * 0.013` is a double product stored into a float.
            c.pm.punchangle[2] = (f64::from(c.pm.fl_fall_velocity) * 0.013) as f32;

            if c.pm.punchangle[0] > 8.0 {
                c.pm.punchangle[0] = 8.0;
            }
        }
    }

    if c.pm.onground != -1 {
        c.pm.fl_fall_velocity = 0.0;
    }
}
