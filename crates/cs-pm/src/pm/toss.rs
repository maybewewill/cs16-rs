//! Non-walking movement of `pm_shared`: `PM_NoClip`, `PM_AddGravity`, `PM_PushEntity`,
//! `PM_Physics_Toss`. Ported from ReGameDLL_CS `regamedll/pm_shared/pm_shared.cpp` (vanilla
//! branch), read 2026-11-22.

#![allow(clippy::needless_range_loop)] // index loops mirror the C loops over the three axes

use glam::Vec3;

use super::consts::{MOVETYPE_BOUNCE, MOVETYPE_BOUNCEMISSILE, MOVETYPE_FLY, MOVETYPE_FLYMISSILE};
use super::math::{dot, vector_compare, vector_ma, vector_normalize, vector_scale};
use super::position::pm_check_water;
use super::types::{PmCtx, PmTrace};
use super::walk::{pm_add_to_touched, pm_check_velocity, pm_clip_velocity};

/// `PM_AddGravity` (2403-2417): a full gravity step (the toss physics; walking uses the half-step
/// pair in `walk.rs`).
pub(crate) fn pm_add_gravity(c: &mut PmCtx<'_>) {
    let ent_gravity: f32 = if c.pm.gravity != 0.0 { c.pm.gravity } else { 1.0 };

    c.pm.velocity[2] -= ent_gravity * c.pm.movevars.gravity * c.pm.frametime;
    c.pm.velocity[2] += c.pm.basevelocity[2] * c.pm.frametime;

    c.pm.basevelocity[2] = 0.0;
    pm_check_velocity(c);
}

/// `PM_PushEntity` (2420-2438): moves the origin by `push`; does not change the velocity. A blocked
/// push (not all solid) is added to the touch list.
pub(crate) fn pm_push_entity(c: &mut PmCtx<'_>, push: Vec3) -> PmTrace {
    let end = c.pm.origin + push;

    let trace = c.trace(c.pm.origin, end);

    c.pm.origin = trace.endpos;

    // So we can run impact function afterwards.
    if trace.fraction < 1.0 && !trace.allsolid {
        let velocity = c.pm.velocity;
        pm_add_to_touched(c, trace, velocity);
    }

    trace
}

/// `PM_Physics_Toss` (2440-2543): gravity, bounce and rest for the non-player movetypes (TOSS,
/// BOUNCE, ...). `PM_PlayerMove` reaches it for TOSS/BOUNCE players (dead players).
pub(crate) fn pm_physics_toss(c: &mut PmCtx<'_>) {
    pm_check_water(c);

    if c.pm.velocity[2] > 0.0 {
        c.pm.onground = -1;
    }

    // If on ground and not moving, return.
    if c.pm.onground != -1 && vector_compare(c.pm.basevelocity, Vec3::ZERO) && vector_compare(c.pm.velocity, Vec3::ZERO)
    {
        return;
    }

    pm_check_velocity(c);

    // add gravity
    if c.pm.movetype != MOVETYPE_FLY && c.pm.movetype != MOVETYPE_BOUNCEMISSILE && c.pm.movetype != MOVETYPE_FLYMISSILE {
        pm_add_gravity(c);
    }

    // move origin
    // Base velocity is not properly accounted for since this entity will move again after the bounce without
    // taking it into account
    c.pm.velocity += c.pm.basevelocity;

    pm_check_velocity(c);
    let move_ = vector_scale(c.pm.velocity, c.pm.frametime);
    c.pm.velocity -= c.pm.basevelocity;

    // Should this clear basevelocity
    let trace = pm_push_entity(c, move_);

    pm_check_velocity(c);

    if trace.allsolid {
        // entity is trapped in another solid
        c.pm.onground = trace.ent;
        c.pm.velocity = Vec3::ZERO;
        return;
    }

    if trace.fraction == 1.0 {
        pm_check_water(c);
        return;
    }

    let backoff = if c.pm.movetype == MOVETYPE_BOUNCE {
        2.0 - c.pm.friction
    } else if c.pm.movetype == MOVETYPE_BOUNCEMISSILE {
        2.0
    } else {
        1.0
    };

    c.pm.velocity = pm_clip_velocity(c.pm.velocity, trace.plane_normal, backoff).0;

    // stop if on ground
    if trace.plane_normal[2] > 0.7 {
        let base = Vec3::ZERO;
        if c.pm.velocity[2] < c.pm.movevars.gravity * c.pm.frametime {
            // we're rolling on the ground, add static friction.
            c.pm.onground = trace.ent;
            c.pm.velocity[2] = 0.0;
        }

        let vel = dot(c.pm.velocity, c.pm.velocity);

        if vel < (30 * 30) as f32 || (c.pm.movetype != MOVETYPE_BOUNCE && c.pm.movetype != MOVETYPE_BOUNCEMISSILE) {
            c.pm.onground = trace.ent;
            c.pm.velocity = Vec3::ZERO;
        } else {
            let move_ = vector_scale(c.pm.velocity, (1.0 - trace.fraction) * c.pm.frametime * 0.9);
            // The C code stores this second push's trace into `trace` and never reads it.
            pm_push_entity(c, move_);
        }

        c.pm.velocity -= base;
    }

    // check for in water
    pm_check_water(c);
}

/// `PM_NoClip` (2545-2581): free flight along the view vectors; the velocity is zeroed.
pub(crate) fn pm_noclip(c: &mut PmCtx<'_>) {
    // Copy movement amounts
    let fmove = c.pm.cmd.forwardmove;
    let smove = c.pm.cmd.sidemove;

    vector_normalize(&mut c.pm.forward);
    vector_normalize(&mut c.pm.right);

    // Determine x and y parts of velocity
    let mut wishvel = Vec3::ZERO;
    for i in 0..3 {
        wishvel[i] = c.pm.forward[i] * fmove + c.pm.right[i] * smove;
    }

    wishvel[2] += c.pm.cmd.upmove;

    c.pm.origin = vector_ma(c.pm.origin, c.pm.frametime, wishvel);

    // Zero out the velocity so that we don't accumulate a huge downward velocity from
    // gravity, etc.
    c.pm.velocity = Vec3::ZERO;
}
