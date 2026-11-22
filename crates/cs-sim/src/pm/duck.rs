//! Ducking: `PM_Duck`, `PM_UnDuck`, `PM_FixPlayerCrouchStuck`, `PM_SplineFraction`.
//! Ported from ReGameDLL_CS `regamedll/pm_shared/pm_shared.cpp` (vanilla branch), read 2026-11-22.

use super::consts::{
    FL_DUCKING, HALF_HUMAN_HEIGHT, IN_DUCK, PLAYER_DUCKING_MULTIPLIER, PM_VEC_DUCK_HULL_MIN, PM_VEC_DUCK_VIEW,
    PM_VEC_HULL_MIN, PM_VEC_VIEW, STUCK_MOVEUP,
};
use super::position::pm_categorize_position;
use super::types::PmCtx;

/// `TIME_TO_DUCK` as the `double` literal it is in C (`#define TIME_TO_DUCK 0.4`); the shared
/// constant is the `f32` rounding of it, which the double comparisons below must not use.
const TIME_TO_DUCK_F64: f64 = 0.4;

/// `PM_Duck` (pm_shared.cpp 2105-2214, read 2026-11-22): two-phase ducking. A press of the duck
/// button on the ground starts a 400 ms transition (`bInDuck`, `flDuckTime` 1000 counting down);
/// the view offset follows a spline from 17 to 12; when the time is over, or at once in the air, the
/// duck hull is taken and a grounded player is moved down by 18. Releasing the button calls
/// [`pm_unduck`].
///
/// Quirks kept: the walk commands are scaled by 0.333 for every command in which the player is
/// ducking, ducked or pressing duck (and `upmove` too); `flDuckTime / 1000.0` and `1.0 - TIME_TO_DUCK`
/// are `double` expressions; when `time < 0` the spline fraction stays at its initial `PM_VEC_VIEW`
/// (17, a bug of the source); `fMore` is the literal difference of the hull constants (18), not the
/// hull table.
pub(crate) fn pm_duck(c: &mut PmCtx<'_>) {
    let buttons = i32::from(c.pm.cmd.buttons);
    let buttons_changed = c.pm.oldbuttons ^ buttons; // These buttons have changed this frame
    let n_button_pressed = buttons_changed & buttons; // The changed ones still down are "pressed"

    if buttons & IN_DUCK != 0 {
        c.pm.oldbuttons |= IN_DUCK;
    } else {
        c.pm.oldbuttons &= !IN_DUCK;
    }

    if c.pm.dead || (buttons & IN_DUCK == 0 && !c.pm.b_in_duck && c.pm.flags & FL_DUCKING == 0) {
        return;
    }

    let mult: f32 = PLAYER_DUCKING_MULTIPLIER;

    c.pm.cmd.forwardmove *= mult;
    c.pm.cmd.sidemove *= mult;
    c.pm.cmd.upmove *= mult;

    if buttons & IN_DUCK != 0 {
        if n_button_pressed & IN_DUCK != 0 && c.pm.flags & FL_DUCKING == 0 {
            // Use 1 second so super long jump will work
            c.pm.fl_duck_time = 1000.0;
            c.pm.b_in_duck = true;
        }

        if c.pm.b_in_duck {
            // Finish ducking immediately if duck time is over or not on ground
            if (f64::from(c.pm.fl_duck_time) / 1000.0) <= (1.0 - TIME_TO_DUCK_F64) || c.pm.onground == -1 {
                c.pm.usehull = 1;
                c.pm.view_ofs[2] = PM_VEC_DUCK_VIEW;
                c.pm.flags |= FL_DUCKING;
                c.pm.b_in_duck = false;

                // HACKHACK - Fudge for collision bug - no time to fix this properly
                if c.pm.onground != -1 {
                    c.pm.origin[2] -= 18.0;

                    // See if we are stuck?
                    pm_fix_player_crouch_stuck(c, STUCK_MOVEUP);

                    // Recatagorize position since ducking can change origin
                    pm_categorize_position(c);
                }
            } else {
                let mut duck_fraction: f32 = PM_VEC_VIEW;
                let time = (1.0 - f64::from(c.pm.fl_duck_time) / 1000.0) as f32;

                // Calc parametric time
                if time >= 0.0 {
                    duck_fraction = pm_spline_fraction(time, (1.0 / TIME_TO_DUCK_F64) as f32);
                }

                let f_more: f32 = PM_VEC_DUCK_HULL_MIN - PM_VEC_HULL_MIN;

                c.pm.view_ofs[2] = ((PM_VEC_DUCK_VIEW - f_more) * duck_fraction) + (PM_VEC_VIEW * (1.0 - duck_fraction));
            }
        }
    }
    // Try to unduck
    else {
        pm_unduck(c);
    }
}

/// `PM_UnDuck` (pm_shared.cpp 2033-2101, read 2026-11-22): stands up when the standing hull fits.
/// On the ground the origin first moves up by 18; if the standing hull is stuck there the player
/// stays ducked.
///
/// Quirks kept: the first trace uses the hull in force (the duck hull), then `usehull` is set to 0
/// and the same trace is repeated with the standing hull (it was `usehull` that changed, the
/// positions are the same); a stuck result puts `usehull` back to 1 and returns without clearing
/// `FL_DUCKING`; the step-sound timer loses 100 ms (not below 0).
pub(crate) fn pm_unduck(c: &mut PmCtx<'_>) {
    let mut new_origin = c.pm.origin;

    if c.pm.onground != -1 {
        new_origin[2] += 18.0;
    }

    let trace = c.trace(new_origin, new_origin);
    if !trace.startsolid {
        c.pm.usehull = 0;

        // Oh, no, changing hulls stuck us into something, try unsticking downward first.
        let trace = c.trace(new_origin, new_origin);

        if trace.startsolid {
            // See if we are stuck?  If so, stay ducked with the duck hull until we have a clear spot
            c.pm.usehull = 1;
            return;
        }

        c.pm.flags &= !FL_DUCKING;
        c.pm.b_in_duck = false;
        c.pm.view_ofs[2] = PM_VEC_VIEW;
        c.pm.fl_duck_time = 0.0;

        c.pm.fl_time_step_sound -= 100;

        if c.pm.fl_time_step_sound < 0 {
            c.pm.fl_time_step_sound = 0;
        }

        c.pm.origin = new_origin;

        // Recatagorize position since ducking can change origin
        pm_categorize_position(c);
    }
}

/// `PM_FixPlayerCrouchStuck(direction)` (pm_shared.cpp 2001-2029, read 2026-11-22): when the hull
/// at the origin is stuck, walks the origin by `direction` units up to `HalfHumanHeight` times
/// looking for a free spot; restores the origin when none is found.
///
/// `HalfHumanHeight` is not defined in the fetched headers (it comes from the HL SDK, 32): assumption.
pub(crate) fn pm_fix_player_crouch_stuck(c: &mut PmCtx<'_>, direction: i32) {
    let (hitent, _) = c.test_position(c.pm.origin);

    if hitent == -1 {
        return;
    }

    let test = c.pm.origin;

    for _ in 0..HALF_HUMAN_HEIGHT {
        c.pm.origin[2] += direction as f32;
        let (hitent, _) = c.test_position(c.pm.origin);

        if hitent == -1 {
            return;
        }
    }

    // Failed
    c.pm.origin = test;
}

/// `PM_SplineFraction(value, scale)` (pm_shared.cpp 1981-1990, read 2026-11-22): the ease-in,
/// ease-out curve `3v^2 - 2v^3` of `v = scale * value`, all in `float` (`real_t` is `float`).
pub(crate) fn pm_spline_fraction(value: f32, scale: f32) -> f32 {
    let value = scale * value;
    let value_squared = value * value;

    // Nice little ease-in, ease-out spline-like curve
    3.0 * value_squared - 2.0 * value_squared * value
}
