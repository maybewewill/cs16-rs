//! Observer movement: `PM_SpectatorMove` (ReGameDLL_CS `pm_shared.cpp`, read 2026-11-22).

use glam::Vec3;

use super::consts::OBS_ROAMING;
use super::math::{dot, length, vector_ma, vector_normalize, vector_scale};
use super::types::PmCtx;

/// `PM_SpectatorMove` (pm_shared.cpp 1829-1977, read 2026-11-22), vanilla branch.
///
/// `OBS_ROAMING` flies freely (extra friction `friction * 1.5`, a `double` product stored into a
/// `real_t`; acceleration capped by `spectatormaxspeed`); every other mode copies the origin and
/// angles of the physent whose `info == iuser2` and zeroes the velocity. The `CLIENT_DLL`
/// `iJumpSpectator` branch and the `REGAMEDLL_ADD`/`REGAMEDLL_FIXES` parts are not ported.
///
/// Quirks kept: `forward` and `right` are normalised in place in `pmove`; a command that cannot
/// add speed (`addspeed <= 0`) returns before the move, so the origin does not change that frame.
pub(crate) fn pm_spectator_move(c: &mut PmCtx<'_>) {
    if c.pm.iuser1 == OBS_ROAMING {
        // Move around in normal spectator method
        let speed = length(c.pm.velocity);

        if speed >= 1.0 {
            let mut drop = 0.0f32;

            // extra friction
            let friction = (f64::from(c.pm.movevars.friction) * 1.5) as f32;
            let control = if speed < c.pm.movevars.stopspeed { c.pm.movevars.stopspeed } else { speed };
            drop += friction * (control * c.pm.frametime);

            // scale the velocity
            let mut newspeed = speed - drop;

            if newspeed < 0.0 {
                newspeed = 0.0;
            }
            newspeed /= speed;

            c.pm.velocity = vector_scale(c.pm.velocity, newspeed);
        } else {
            c.pm.velocity = Vec3::ZERO;
        }

        // accelerate
        let fmove = c.pm.cmd.forwardmove;
        let smove = c.pm.cmd.sidemove;

        let spectatormaxspeed = c.pm.movevars.spectatormaxspeed;

        vector_normalize(&mut c.pm.forward);
        vector_normalize(&mut c.pm.right);

        let mut wishvel = Vec3::new(
            c.pm.forward[0] * fmove + c.pm.right[0] * smove,
            c.pm.forward[1] * fmove + c.pm.right[1] * smove,
            c.pm.forward[2] * fmove + c.pm.right[2] * smove,
        );

        wishvel[2] += c.pm.cmd.upmove;

        let mut wishdir = wishvel;
        let mut wishspeed = vector_normalize(&mut wishdir);

        // clamp to server defined max speed (the source also rescales `wishvel`, which it never
        // reads again)
        if wishspeed > spectatormaxspeed {
            wishspeed = spectatormaxspeed;
        }

        let currentspeed = dot(c.pm.velocity, wishdir);

        let addspeed = wishspeed - currentspeed;

        if addspeed <= 0.0 {
            return;
        }

        let mut accelspeed = c.pm.movevars.accelerate * c.pm.frametime * wishspeed;
        if accelspeed > addspeed {
            accelspeed = addspeed;
        }

        c.pm.velocity = vector_ma(c.pm.velocity, accelspeed, wishdir);

        // move
        c.pm.origin = vector_ma(c.pm.origin, c.pm.frametime, c.pm.velocity);
    } else {
        // all other modes just track some kind of target, so spectator PVS = target PVS

        // no valid target ?
        if c.pm.iuser2 <= 0 {
            return;
        }

        // Find the client this player's targeting
        let Some((origin, angles)) = c.w.physent_by_info(c.pm.iuser2) else {
            return;
        };

        // use targets position as own origin for PVS
        c.pm.angles = angles;
        c.pm.origin = origin;

        // no velocity
        c.pm.velocity = Vec3::ZERO;
    }
}
