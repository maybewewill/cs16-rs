//! Ground and air movement kernels of `pm_shared`: `PM_AddToTouched`, `PM_CheckVelocity`,
//! `PM_ClipVelocity`, gravity, `PM_FlyMove`, `PM_Accelerate`, `PM_AirAccelerate`, `PM_AirMove`,
//! `PM_Friction`, `PM_WalkMove`. Ported from ReGameDLL_CS `regamedll/pm_shared/pm_shared.cpp`
//! (vanilla branch, no `REGAMEDLL_*`), read 2026-11-22.
//!
//! Float rules: `real_t` is `float`; an expression with a `double` literal is computed in `f64`
//! and cast where C stores it (named where it is not obvious).

#![allow(clippy::needless_range_loop)] // index loops mirror the C loops over the three axes

use glam::Vec3;

use super::consts::{MAX_CLIP_PLANES, MOVETYPE_WALK, STOP_EPSILON};
use super::math::{cross, dot, is_nan, length, vector_normalize, vector_scale};
use super::types::{PmCtx, PmTrace};

/// `PM_AddToTouched` (498-522): adds the trace to the touch list unless its entity is already in
/// it. `deltavelocity` is copied AFTER the dedupe check (so a duplicate keeps the first impact
/// velocity). The C code warns at `MAX_PHYSENTS` entries and then overruns the array; here the
/// list is a `Vec` bounded by the number of distinct entities (dedupe).
pub(crate) fn pm_add_to_touched(c: &mut PmCtx<'_>, mut tr: PmTrace, impactvelocity: Vec3) -> bool {
    if c.pm.touchindex.iter().any(|t| t.ent == tr.ent) {
        // Already in list.
        return false;
    }

    tr.deltavelocity = impactvelocity;
    c.pm.touchindex.push(tr);
    true
}

/// `PM_CheckVelocity` (524-556): resets NaN (and, by the exponent-mask test, infinite) velocity and
/// origin components to 0 and bounds the velocity by `movevars->maxvelocity`. The console messages
/// of the source are not reproduced.
pub(crate) fn pm_check_velocity(c: &mut PmCtx<'_>) {
    let maxvelocity = c.pm.movevars.maxvelocity;
    for i in 0..3 {
        // See if it's bogus.
        if is_nan(c.pm.velocity[i]) {
            c.pm.velocity[i] = 0.0;
        }

        if is_nan(c.pm.origin[i]) {
            c.pm.origin[i] = 0.0;
        }

        // Bound it.
        if c.pm.velocity[i] > maxvelocity {
            c.pm.velocity[i] = maxvelocity;
        } else if c.pm.velocity[i] < -maxvelocity {
            c.pm.velocity[i] = -maxvelocity;
        }
    }
}

/// `PM_ClipVelocity` (562-604): slides `v` off the plane `normal`. Returns the new vector and the
/// blocked flags (1 floor, 2 step/wall). `STOP_EPSILON` is a `double` literal, so the zeroing
/// comparison happens in `f64`.
pub(crate) fn pm_clip_velocity(v: Vec3, normal: Vec3, overbounce: f32) -> (Vec3, i32) {
    let angle = normal[2];

    // Assume unblocked.
    let mut blocked = 0x00;

    // If the plane that is blocking us has a positive z component, then assume it's a floor.
    if angle > 0.0 {
        blocked |= 0x01;
    }

    // If the plane has no Z, it is vertical (wall/step)
    if angle == 0.0 {
        blocked |= 0x02;
    }

    // Determine how far along plane to slide based on incoming direction.
    // Scale by overbounce factor.
    let backoff = dot(v, normal) * overbounce;

    let mut out = Vec3::ZERO;
    for i in 0..3 {
        let change = v[i] - normal[i] * backoff;
        out[i] = change;

        // If out velocity is too small, zero it out.
        if f64::from(out[i]) > -STOP_EPSILON && f64::from(out[i]) < STOP_EPSILON {
            out[i] = 0.0;
        }
    }

    // Return blocking flags.
    (out, blocked)
}

/// `PM_AddCorrectGravity` (606-626): half a gravity step before the move, plus the base velocity z.
pub(crate) fn pm_add_correct_gravity(c: &mut PmCtx<'_>) {
    if c.pm.waterjumptime != 0.0 {
        return;
    }

    let ent_gravity: f32 = if c.pm.gravity != 0.0 { c.pm.gravity } else { 1.0 };

    // Add gravity so they'll be in the correct position during movement
    // yes, this 0.5 looks wrong, but it's not.
    c.pm.velocity[2] -= ent_gravity * c.pm.movevars.gravity * 0.5f32 * c.pm.frametime;
    c.pm.velocity[2] += c.pm.basevelocity[2] * c.pm.frametime;

    c.pm.basevelocity[2] = 0.0;

    pm_check_velocity(c);
}

/// `PM_FixupGravityVelocity` (628-643): the other half step after the move. `0.5` is a `double`
/// literal here, so the subtraction is done in `f64`.
pub(crate) fn pm_fixup_gravity_velocity(c: &mut PmCtx<'_>) {
    if c.pm.waterjumptime != 0.0 {
        return;
    }

    let ent_gravity: f32 = if c.pm.gravity != 0.0 { c.pm.gravity } else { 1.0 };

    // Get the correct velocity for the end of the dt
    let step = c.pm.movevars.gravity * c.pm.frametime * ent_gravity;
    c.pm.velocity[2] = (f64::from(c.pm.velocity[2]) - f64::from(step) * 0.5) as f32;
    pm_check_velocity(c);
}

/// `PM_FlyMove` (816-996, the vanilla one; `PM_FlyMove_New` is `REGAMEDLL_ADD`): slides the player
/// along up to `MAX_CLIP_PLANES` planes in four bumps. Returns the blocked flags.
///
/// Quirks kept: the "bounce" branch for a first plane of a WALK player that is airborne or has
/// entity friction != 1; the velocity is zeroed when nothing moved (`allFraction == 0`).
pub(crate) fn pm_fly_move(c: &mut PmCtx<'_>) -> i32 {
    let numbumps = 4; // Bump up to four times
    let mut blocked = 0x00; // Assume not blocked
    let mut numplanes = 0usize; // and not sliding along any planes
    let mut planes = [Vec3::ZERO; MAX_CLIP_PLANES];
    let mut new_velocity = Vec3::ZERO;

    let mut original_velocity = c.pm.velocity; // Store original velocity
    let primal_velocity = c.pm.velocity;

    let mut all_fraction = 0.0f32;
    let mut time_left = c.pm.frametime; // Total time for this movement operation.

    for _bumpcount in 0..numbumps {
        let v = c.pm.velocity;
        if v[0] == 0.0 && v[1] == 0.0 && v[2] == 0.0 {
            break;
        }

        // Assume we can move all the way from the current origin to the end point.
        let mut end = Vec3::ZERO;
        for i in 0..3 {
            let fl_scale = time_left * c.pm.velocity[i];
            end[i] = c.pm.origin[i] + fl_scale;
        }

        // See if we can make it from origin to end point.
        let trace = c.trace(c.pm.origin, end);

        all_fraction += trace.fraction;

        // If we started in a solid object, or we were in solid space the whole way, zero out our
        // velocity and return that we are blocked by floor and wall.
        if trace.allsolid {
            // entity is trapped in another solid
            c.pm.velocity = Vec3::ZERO;
            return 4;
        }

        // If we moved some portion of the total distance, then copy the end position into the
        // pmove->origin and zero the plane counter.
        if trace.fraction > 0.0 {
            // actually covered some distance
            c.pm.origin = trace.endpos;
            original_velocity = c.pm.velocity;

            numplanes = 0;
        }

        // If we covered the entire distance, we are done and can return.
        if trace.fraction == 1.0 {
            // moved the entire distance
            break;
        }

        // Save entity that blocked us (since fraction was < 1.0) for contact
        // Add it if it's not already in the list
        let velocity = c.pm.velocity;
        pm_add_to_touched(c, trace, velocity);

        // If the plane we hit has a high z component in the normal, then it's probably a floor
        if trace.plane_normal[2] > 0.7 {
            // floor
            blocked |= 0x01;
        }

        // If the plane has a zero z component in the normal, then it's a step or wall
        if trace.plane_normal[2] == 0.0 {
            // step / wall
            blocked |= 0x02;
        }

        // Reduce amount of pmove->frametime left by total time left * fraction that we covered.
        time_left -= time_left * trace.fraction;

        // Did we run out of planes to clip against?
        if numplanes >= MAX_CLIP_PLANES {
            // this shouldn't really happen
            // Stop our movement if so.
            c.pm.velocity = Vec3::ZERO;
            break;
        }

        // Set up next clipping plane
        planes[numplanes] = trace.plane_normal;
        numplanes += 1;

        // modify original_velocity so it parallels all of the clip planes
        // relfect player velocity
        if numplanes == 1 && c.pm.movetype == MOVETYPE_WALK && (c.pm.onground == -1 || c.pm.friction != 1.0) {
            for i in 0..numplanes {
                if planes[i][2] > 0.7 {
                    // floor or slope
                    new_velocity = pm_clip_velocity(original_velocity, planes[i], 1.0).0;
                    original_velocity = new_velocity;
                } else {
                    // `1.0 + bounce * (1.0 - friction)` is a double expression, cast to the float parameter.
                    let overbounce = 1.0 + f64::from(c.pm.movevars.bounce) * (1.0 - f64::from(c.pm.friction));
                    new_velocity = pm_clip_velocity(original_velocity, planes[i], overbounce as f32).0;
                }
            }

            c.pm.velocity = new_velocity;
            original_velocity = new_velocity;
        } else {
            let mut i = 0;
            while i < numplanes {
                c.pm.velocity = pm_clip_velocity(original_velocity, planes[i], 1.0).0;

                let mut j = 0;
                while j < numplanes {
                    if j != i && dot(c.pm.velocity, planes[j]) < 0.0 {
                        break;
                    }
                    j += 1;
                }

                if j == numplanes {
                    break;
                }
                i += 1;
            }

            if i == numplanes {
                if numplanes != 2 {
                    c.pm.velocity = Vec3::ZERO;
                    break;
                }

                let dir = cross(planes[0], planes[1]);
                let d = dot(dir, c.pm.velocity);
                c.pm.velocity = vector_scale(dir, d);
            }

            if dot(c.pm.velocity, primal_velocity) <= 0.0 {
                c.pm.velocity = Vec3::ZERO;
                break;
            }
        }
    }

    if all_fraction == 0.0 {
        c.pm.velocity = Vec3::ZERO;
    }

    blocked
}

/// `PM_Accelerate` (998-1036). `accelspeed = accel * frametime * wishspeed * friction` is evaluated
/// left to right in `float`.
pub(crate) fn pm_accelerate(c: &mut PmCtx<'_>, wishdir: Vec3, wishspeed: f32, accel: f32) {
    // Dead player's don't accelerate
    if c.pm.dead {
        return;
    }

    // If waterjumping, don't accelerate
    if c.pm.waterjumptime != 0.0 {
        return;
    }

    // See if we are changing direction a bit
    let currentspeed = dot(c.pm.velocity, wishdir);

    // Reduce wishspeed by the amount of veer.
    let addspeed = wishspeed - currentspeed;

    // If not going to add any speed, done.
    if addspeed <= 0.0 {
        return;
    }

    // Determine amount of accleration.
    let mut accelspeed = accel * c.pm.frametime * wishspeed * c.pm.friction;

    // Cap at addspeed
    if accelspeed > addspeed {
        accelspeed = addspeed;
    }

    // Adjust velocity.
    for i in 0..3 {
        c.pm.velocity[i] += accelspeed * wishdir[i];
    }
}

/// `PM_WalkMove` (1039-1248): moves along the ground, with step up/down.
///
/// Quirks kept: `pmove->up` is used as a scratch variable for the stepped origin; the jump-penalty
/// ratio from `fuser2` scales the horizontal velocity first; stairs are not tried when airborne
/// (`oldonground == -1 && waterlevel == 0`).
pub(crate) fn pm_walk_move(c: &mut PmCtx<'_>) {
    // jump penalty
    if c.pm.fuser2 > 0.0 {
        // `(100 - fuser2 * 0.001 * 19) * 0.01` is a double expression stored into a real_t.
        let fl_ratio = ((100.0 - f64::from(c.pm.fuser2) * 0.001 * 19.0) * 0.01) as f32;

        c.pm.velocity[0] *= fl_ratio;
        c.pm.velocity[1] *= fl_ratio;
    }

    // Copy movement amounts
    let fmove = c.pm.cmd.forwardmove;
    let smove = c.pm.cmd.sidemove;
    let maxspeed = c.pm.maxspeed;

    // Zero out z components of movement vectors
    c.pm.forward[2] = 0.0;
    c.pm.right[2] = 0.0;

    // Normalize remainder of vectors.
    vector_normalize(&mut c.pm.forward);
    vector_normalize(&mut c.pm.right);

    // Determine x and y parts of velocity
    let mut wishvel = Vec3::ZERO;
    for i in 0..2 {
        wishvel[i] = c.pm.forward[i] * fmove + c.pm.right[i] * smove;
    }

    // Zero out z part of velocity
    wishvel[2] = 0.0;

    // Determine maginitude of speed of move
    let mut wishdir = wishvel;
    let mut wishspeed = vector_normalize(&mut wishdir);

    // Clamp to server defined max speed. (The C code also rescales `wishvel`, which is never read
    // again: dead store, omitted.)
    if wishspeed > maxspeed {
        wishspeed = maxspeed;
    }

    // Set pmove velocity
    c.pm.velocity[2] = 0.0;
    let accelerate = c.pm.movevars.accelerate;
    pm_accelerate(c, wishdir, wishspeed, accelerate);
    c.pm.velocity[2] = 0.0;

    // Add in any base velocity to the current velocity.
    c.pm.velocity += c.pm.basevelocity;

    let spd = length(c.pm.velocity);

    if spd < 1.0 {
        c.pm.velocity = Vec3::ZERO;
        return;
    }

    let oldonground = c.pm.onground;

    // first try just moving to the destination
    let mut dest = Vec3::new(
        c.pm.origin[0] + c.pm.velocity[0] * c.pm.frametime,
        c.pm.origin[1] + c.pm.velocity[1] * c.pm.frametime,
        c.pm.origin[2],
    );

    // first try moving directly to the next spot
    let trace = c.trace(c.pm.origin, dest);

    // If we made it all the way, then copy trace end as new player position.
    if trace.fraction == 1.0 {
        c.pm.origin = trace.endpos;
        return;
    }

    // Don't walk up stairs if not on ground.
    if oldonground == -1 && c.pm.waterlevel == 0 {
        return;
    }

    // If we are jumping out of water, don't do anything more.
    if c.pm.waterjumptime != 0.0 {
        return;
    }

    // Try sliding forward both on ground and up 16 pixels
    // take the move that goes farthest

    // Save out original pos &
    let original = c.pm.origin;

    // velocity.
    let originalvel = c.pm.velocity;

    // Slide move
    pm_fly_move(c);

    // Copy the results out
    let down = c.pm.origin;
    let downvel = c.pm.velocity;

    // Reset original values.
    c.pm.origin = original;
    c.pm.velocity = originalvel;

    // Start out up one stair height
    dest = c.pm.origin;

    dest[2] += c.pm.movevars.stepsize;

    let trace = c.trace(c.pm.origin, dest);

    // If we started okay and made it part of the way at least, copy the results to the movement
    // start position and then run another move try.
    if !trace.startsolid && !trace.allsolid {
        c.pm.origin = trace.endpos;
    }

    // slide move the rest of the way.
    pm_fly_move(c);

    // Now try going back down from the end point press down the stepheight
    dest = c.pm.origin;
    dest[2] -= c.pm.movevars.stepsize;

    let trace = c.trace(c.pm.origin, dest);

    // If we are not on the ground any more then use the original movement attempt
    let mut use_down = trace.plane_normal[2] < 0.7;
    if !use_down {
        // If the trace ended up in empty space, copy the end over to the origin.
        if !trace.startsolid && !trace.allsolid {
            c.pm.origin = trace.endpos;
        }

        // Copy this origion to up.
        c.pm.up = c.pm.origin;

        // decide which one went farther
        let downdist = (down[0] - original[0]) * (down[0] - original[0]) + (down[1] - original[1]) * (down[1] - original[1]);
        let updist = (c.pm.up[0] - original[0]) * (c.pm.up[0] - original[0])
            + (c.pm.up[1] - original[1]) * (c.pm.up[1] - original[1]);

        if downdist > updist {
            use_down = true;
        } else {
            // copy z value from slide move
            c.pm.velocity[2] = downvel[2];
        }
    }

    if use_down {
        c.pm.origin = down;
        c.pm.velocity = downvel;
    }
}

/// `PM_Friction` (1251-1332): ground friction (and the edge-friction probe). Water friction is
/// commented out in the source and not ported.
pub(crate) fn pm_friction(c: &mut PmCtx<'_>) {
    // If we are in water jump cycle, don't apply friction
    if c.pm.waterjumptime != 0.0 {
        return;
    }

    // Get velocity
    let vel = c.pm.velocity;

    // Calculate speed
    let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();

    // If too slow, return
    if speed < 0.1 {
        return;
    }

    let mut drop = 0.0f32;

    // apply ground friction
    // On an entity that is the ground
    if c.pm.onground != -1 {
        let hull = c.pm.hull_index();
        let x = c.pm.origin[0] + vel[0] / speed * 16.0;
        let y = c.pm.origin[1] + vel[1] / speed * 16.0;
        let start = Vec3::new(x, y, c.pm.origin[2] + c.pm.player_mins[hull][2]);
        let stop = Vec3::new(x, y, start[2] - 34.0);

        let trace = c.trace(start, stop);

        let mut friction = if trace.fraction == 1.0 {
            c.pm.movevars.friction * c.pm.movevars.edgefriction
        } else {
            c.pm.movevars.friction
        };

        // player friction?
        friction *= c.pm.friction;

        // Bleed off some speed, but if we have less than the bleed
        // threshhold, bleed the theshold amount.
        let control = if speed < c.pm.movevars.stopspeed { c.pm.movevars.stopspeed } else { speed };

        // Add the amount to t'he drop amount.
        drop += friction * (control * c.pm.frametime);
    }

    // scale the velocity
    let mut newspeed = speed - drop;

    if newspeed < 0.0 {
        newspeed = 0.0;
    }

    // Determine proportion of old speed we are using.
    newspeed /= speed;

    // Adjust velocity according to proportion.
    c.pm.velocity = Vec3::new(vel[0] * newspeed, vel[1] * newspeed, vel[2] * newspeed);
}

/// `PM_AirAccelerate` (1336-1374): like `PM_Accelerate` with the wish speed capped at 30 for the
/// veer test and `accel * wishspeed * frametime * friction` for the step.
pub(crate) fn pm_air_accelerate(c: &mut PmCtx<'_>, wishdir: Vec3, wishspeed: f32, accel: f32) {
    let mut wishspd = wishspeed;

    if c.pm.dead || c.pm.waterjumptime != 0.0 {
        return;
    }

    // Cap speed
    if wishspd > 30.0 {
        wishspd = 30.0;
    }

    // Determine veer amount
    let currentspeed = dot(c.pm.velocity, wishdir);

    // See how much to add
    let addspeed = wishspd - currentspeed;

    // If not adding any, done.
    if addspeed <= 0.0 {
        return;
    }

    // Determine acceleration speed after acceleration
    let mut accelspeed = accel * wishspeed * c.pm.frametime * c.pm.friction;

    // Cap it
    if accelspeed > addspeed {
        accelspeed = addspeed;
    }

    // Adjust pmove vel.
    for i in 0..3 {
        c.pm.velocity[i] += accelspeed * wishdir[i];
    }
}

/// `PM_AirMove` / `PM_AirMove_internal` (1485-1536; the wrapper's `playerIndex` is ignored).
pub(crate) fn pm_air_move(c: &mut PmCtx<'_>) {
    // Copy movement amounts
    let fmove = c.pm.cmd.forwardmove;
    let smove = c.pm.cmd.sidemove;

    // Zero out z components of movement vectors
    c.pm.forward[2] = 0.0;
    c.pm.right[2] = 0.0;

    // Renormalize
    vector_normalize(&mut c.pm.forward);
    vector_normalize(&mut c.pm.right);

    // Determine x and y parts of velocity
    let mut wishvel = Vec3::ZERO;
    for i in 0..2 {
        wishvel[i] = c.pm.forward[i] * fmove + c.pm.right[i] * smove;
    }

    // Zero out z part of velocity
    wishvel[2] = 0.0;

    // Determine maginitude of speed of move
    let mut wishdir = wishvel;
    let mut wishspeed = vector_normalize(&mut wishdir);

    // Clamp to server defined max speed. (The rescale of `wishvel` in C is a dead store.)
    if wishspeed > c.pm.maxspeed {
        wishspeed = c.pm.maxspeed;
    }

    let airaccelerate = c.pm.movevars.airaccelerate;
    pm_air_accelerate(c, wishdir, wishspeed, airaccelerate);

    // Add in any base velocity to the current velocity.
    c.pm.velocity += c.pm.basevelocity;

    pm_fly_move(c);
}
