//! `pm_shared`: the player movement code shared by the game server and the client's prediction.
//! Ported from ReGameDLL_CS `regamedll/pm_shared/` (vanilla branch: code not under
//! `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`, `REGAMEDLL_API`), read 2026-11-22. The module is
//! engine-neutral: the world is reached only through [`PmWorld`].
//!
//! Entry points: [`pm_init`], [`pm_move`], [`pm_find_texture_type`].

mod duck;
mod jump;
mod ladder;
mod materials;
mod spectator;
mod sound;
mod stuck;
mod toss;
mod water;

pub mod consts;
pub mod math;
mod position;
mod types;
mod walk;

pub use materials::TextureTable;
pub use sound::all_sample_names;
pub use types::{MoveVars, PlayerMove, PmCmd, PmShared, PmTrace, PmWorld};

pub(crate) use types::PmCtx;

use consts::{
    DEAD_DEAD, FL_ONGROUND, FL_ONTRAIN, IN_JUMP, MOVETYPE_BOUNCE, MOVETYPE_FLY, MOVETYPE_NOCLIP, MOVETYPE_NONE,
    MOVETYPE_TOSS, MOVETYPE_WALK,
};
use duck::pm_duck;
use jump::pm_jump;
use ladder::{pm_ladder, pm_ladder_move};
use math::{angle_vectors, vector_subtract};
use position::{
    pm_categorize_position, pm_check_falling, pm_check_parameters, pm_check_water, pm_in_water, pm_reduce_timers,
};
use sound::pm_update_step_sound;
use spectator::pm_spectator_move;
use stuck::{pm_check_stuck, pm_create_stuck_table};
use toss::{pm_noclip, pm_physics_toss};
use walk::{
    pm_add_correct_gravity, pm_air_move, pm_check_velocity, pm_fixup_gravity_velocity, pm_fly_move, pm_friction,
    pm_walk_move,
};
use water::{pm_check_water_jump, pm_play_water_sounds, pm_water_jump, pm_water_move};

/// `PM_Init` (pm_shared.cpp 3593-3603, read 2026-11-22): builds the stuck table and the texture
/// table from the bytes of `sound/materials.txt` (`None` = the load failed: an empty table, every
/// lookup answers concrete). The engine fills the hulls, callbacks and movevars of `playermove_t`
/// separately; here they are `PlayerMove` fields the caller sets.
///
/// `bTextureTypeInit` (a process-wide guard in C) has no field: a `PmShared` is built once.
pub fn pm_init(materials_txt: Option<&[u8]>) -> PmShared {
    PmShared {
        initialized: true,
        stuck_table: pm_create_stuck_table(),
        stuck_last: [[0; 2]; consts::MAX_CLIENTS],
        stuck_check_time: [[0.0; 2]; consts::MAX_CLIENTS],
        textures: materials_txt.map(TextureTable::parse).unwrap_or_default(),
        i_skip_step: 0,
        g_onladder: false,
    }
}

/// `PM_Move(ppmove, server)` (pm_shared.cpp 3543-3569, read 2026-11-22): sets `pmove->server` (in C the engine
/// fills it before the call; here the argument is the one place it comes from),
/// runs `PM_PlayerMove(server)`, then mirrors `onground` into `FL_ONGROUND` and, in single player, resets the
/// entity friction of walkers to 1.
pub fn pm_move(pm: &mut PlayerMove, sh: &mut PmShared, world: &mut dyn PmWorld, server: bool) {
    pm.server = server;
    let mut c = PmCtx { pm, sh, w: world };

    pm_player_move(&mut c);

    if c.pm.onground != -1 {
        c.pm.flags |= FL_ONGROUND;
    } else {
        c.pm.flags &= !FL_ONGROUND;
    }

    if !c.pm.multiplayer && c.pm.movetype == MOVETYPE_WALK {
        c.pm.friction = 1.0;
    }
}

/// `PM_FindTextureType` (exported to the engine for bullet materials): the `CHAR_TEX_*` letter of
/// `name`, `b'C'` when it is not in the table.
pub fn pm_find_texture_type(sh: &PmShared, name: &str) -> u8 {
    debug_assert!(sh.initialized, "PM_FindTextureType before PM_Init (DbgAssert in the source)");
    sh.textures.find(name)
}

/// `PM_ShouldDoSpectMode` (3144).
fn pm_should_do_spect_mode(c: &PmCtx<'_>) -> bool {
    c.pm.iuser3 <= 0 || c.pm.deadflag == DEAD_DEAD
}

/// `PM_PlayerMove` (pm_shared.cpp 3152-3404, read 2026-11-22): one movement command, in the
/// source's order. Origin, angles and velocity are modified in place; `touchindex` lists the
/// entities touched.
///
/// Quirks kept: `PM_CheckParameters` runs before `frametime` is recomputed from `cmd.msec` (the
/// punch decay uses the engine's prefilled `frametime`); `frametime = cmd.msec * 0.001` is a
/// `double` product stored into a `float`; a player that is alive and not on a train, whose
/// movetype is anything but WALK and NOCLIP (NONE, STEP, FLY, TOSS, BOUNCE...), is reset to WALK
/// unless a ladder holds it; the water-jump and ladder-jump
/// paths return without `PM_CategorizePosition`/`PM_CheckFalling`.
fn pm_player_move(c: &mut PmCtx<'_>) {
    let mut p_ladder: Option<usize> = None;

    // Adjust speeds etc.
    pm_check_parameters(c);

    // Assume we don't touch anything
    c.pm.touchindex.clear();

    // # of msec to apply movement
    c.pm.frametime = (f64::from(c.pm.cmd.msec) * 0.001) as f32;

    pm_reduce_timers(c);

    // Convert view angles to vectors
    let (forward, right, up) = angle_vectors(c.pm.angles);
    c.pm.forward = forward;
    c.pm.right = right;
    c.pm.up = up;

    // Special handling for spectator and observers. (iuser1 is set if the player's in observer mode)
    if (c.pm.spectator != 0 || c.pm.iuser1 > 0) && pm_should_do_spect_mode(c) {
        pm_spectator_move(c);
        pm_categorize_position(c);
        return;
    }

    // Always try and unstick us unless we are in NOCLIP mode
    if c.pm.movetype != MOVETYPE_NOCLIP && c.pm.movetype != MOVETYPE_NONE && pm_check_stuck(c) {
        // Can't move, we're stuck
        return;
    }

    // Now that we are "unstuck", see where we are (waterlevel and type, pmove->onground).
    pm_categorize_position(c);

    // Store off the starting water level
    c.pm.oldwaterlevel = c.pm.waterlevel;

    // If we are not on ground, store off how fast we are moving down
    if c.pm.onground == -1 {
        c.pm.fl_fall_velocity = -c.pm.velocity[2];
    }

    c.sh.g_onladder = false;

    // Don't run ladder code if dead or on a train
    if !c.pm.dead && (c.pm.flags & FL_ONTRAIN) == 0 {
        p_ladder = pm_ladder(c);

        if p_ladder.is_some() {
            c.sh.g_onladder = true;
        }
    }

    pm_duck(c);
    pm_update_step_sound(c);

    // Don't run ladder code if dead or on a train
    if !c.pm.dead && (c.pm.flags & FL_ONTRAIN) == 0 {
        if let Some(ladder) = p_ladder {
            pm_ladder_move(c, ladder);
        } else if c.pm.movetype != MOVETYPE_WALK && c.pm.movetype != MOVETYPE_NOCLIP {
            // Clear ladder stuff unless player is noclipping
            // it will be set immediately again next frame if necessary
            c.pm.movetype = MOVETYPE_WALK;
        }
    }

    // Handle movement
    match c.pm.movetype {
        MOVETYPE_NONE => {}

        MOVETYPE_NOCLIP => pm_noclip(c),

        MOVETYPE_TOSS | MOVETYPE_BOUNCE => pm_physics_toss(c),

        MOVETYPE_FLY => {
            pm_check_water(c);

            // Was jump button pressed?
            // If so, set velocity to 270 away from ladder.  This is currently wrong.
            // Also, set MOVE_TYPE to walk, too.
            if (i32::from(c.pm.cmd.buttons) & IN_JUMP) != 0 {
                if p_ladder.is_none() {
                    pm_jump(c);
                }
            } else {
                c.pm.oldbuttons &= !IN_JUMP;
            }

            // Perform the move accounting for any base velocity.
            c.pm.velocity += c.pm.basevelocity;
            pm_fly_move(c);
            c.pm.velocity = vector_subtract(c.pm.velocity, c.pm.basevelocity);
        }

        MOVETYPE_WALK => {
            if !pm_in_water(c) {
                pm_add_correct_gravity(c);
            }

            // If we are leaping out of the water, just update the counters.
            if c.pm.waterjumptime != 0.0 {
                pm_water_jump(c);
                pm_fly_move(c);

                // Make sure waterlevel is set correctly
                pm_check_water(c);
                return;
            }

            // If we are swimming in the water, see if we are nudging against a place we can jump up
            // out of, and, if so, start out jump.  Otherwise, if we are not moving up, then reset
            // jump timer to 0
            if c.pm.waterlevel >= 2 {
                if c.pm.waterlevel == 2 {
                    pm_check_water_jump(c);
                }

                // If we are falling again, then we must not trying to jump out of water any more.
                if c.pm.velocity[2] < 0.0 && c.pm.waterjumptime != 0.0 {
                    c.pm.waterjumptime = 0.0;
                }

                // Was jump button pressed?
                if (i32::from(c.pm.cmd.buttons) & IN_JUMP) != 0 {
                    pm_jump(c);
                } else {
                    c.pm.oldbuttons &= !IN_JUMP;
                }

                // Perform regular water movement
                pm_water_move(c);

                c.pm.velocity = vector_subtract(c.pm.velocity, c.pm.basevelocity);

                // Get a final position
                pm_categorize_position(c);
            }
            // Not underwater
            else {
                // Was jump button pressed?
                if (i32::from(c.pm.cmd.buttons) & IN_JUMP) != 0 {
                    if p_ladder.is_none() {
                        pm_jump(c);
                    }
                } else {
                    c.pm.oldbuttons &= !IN_JUMP;
                }

                // Fricion is handled before we add in any base velocity. That way, if we are on a
                // conveyor, we don't slow when standing still, relative to the conveyor.
                if c.pm.onground != -1 {
                    c.pm.velocity[2] = 0.0;
                    pm_friction(c);
                }

                // Make sure velocity is valid.
                pm_check_velocity(c);

                // Are we on ground now
                if c.pm.onground != -1 {
                    pm_walk_move(c);
                } else {
                    // Take into account movement when in air.
                    pm_air_move(c);
                }

                // Set final flags.
                pm_categorize_position(c);

                // Now pull the base velocity back out.
                // Base velocity is set if you are on a moving object, like
                // a conveyor (or maybe another monster?)
                c.pm.velocity = vector_subtract(c.pm.velocity, c.pm.basevelocity);

                // Make sure velocity is valid.
                pm_check_velocity(c);

                // Add any remaining gravitational component.
                if !pm_in_water(c) {
                    pm_fixup_gravity_velocity(c);
                }

                // If we are on ground, no downward velocity.
                if c.pm.onground != -1 {
                    c.pm.velocity[2] = 0.0;
                }

                // See if we landed on the ground with enough force to play a landing sound.
                pm_check_falling(c);
            }

            // Did we enter or leave the water?
            pm_play_water_sounds(c);
        }

        // Bogus movetype: the source only logs ("Bogus pmove player movetype").
        _ => {}
    }
}
