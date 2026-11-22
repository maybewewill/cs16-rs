//! Types of the `pm_shared` port: `playermove_t` and friends, the world trait and the per-game
//! state. Interfaces as in the plan (`docs/superpowers/plans/2026-11-22-pm-shared-port.md`, Task 4).

use glam::Vec3;

use super::consts::{MAX_CLIENTS, PLAYER_HULL_MAXS, PLAYER_HULL_MINS, PM_NORMAL};
use super::materials::TextureTable;

/// `movevars_t` without the sky fields. `Default` is the ReHLDS cvar defaults (including
/// `sv_rollspeed "0.0"`, ReHLDS rh_sv_user.cpp:50, verified by Task 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveVars {
    pub gravity: f32,
    pub stopspeed: f32,
    pub maxspeed: f32,
    pub spectatormaxspeed: f32,
    pub accelerate: f32,
    pub airaccelerate: f32,
    pub wateraccelerate: f32,
    pub friction: f32,
    pub edgefriction: f32,
    pub waterfriction: f32,
    pub entgravity: f32,
    pub bounce: f32,
    pub stepsize: f32,
    pub maxvelocity: f32,
    pub zmax: f32,
    pub wave_height: f32,
    pub footsteps: bool,
    pub rollangle: f32,
    pub rollspeed: f32,
}

impl Default for MoveVars {
    fn default() -> Self {
        Self {
            gravity: 800.0,
            stopspeed: 100.0,
            maxspeed: 320.0,
            spectatormaxspeed: 500.0,
            accelerate: 10.0,
            airaccelerate: 10.0,
            wateraccelerate: 10.0,
            friction: 4.0,
            edgefriction: 2.0,
            waterfriction: 1.0,
            entgravity: 1.0,
            bounce: 1.0,
            stepsize: 18.0,
            maxvelocity: 2000.0,
            zmax: 4096.0,
            wave_height: 0.0,
            footsteps: true,
            rollangle: 0.0,
            rollspeed: 0.0,
        }
    }
}

/// `usercmd_t` as `pm_shared` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PmCmd {
    pub msec: u8,
    pub viewangles: Vec3,
    pub forwardmove: f32,
    pub sidemove: f32,
    pub upmove: f32,
    pub buttons: u16,
    pub impulse: u8,
}

/// `pmtrace_t`. A trace that hit nothing has fraction 1, `ent` -1 and a zero plane normal (the
/// quirk that makes `onground` -1 in `PM_CategorizePosition`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PmTrace {
    pub allsolid: bool,
    pub startsolid: bool,
    pub inopen: bool,
    pub inwater: bool,
    pub fraction: f32,
    pub endpos: Vec3,
    pub plane_normal: Vec3,
    pub plane_dist: f32,
    pub ent: i32,
    pub deltavelocity: Vec3,
    pub hitgroup: i32,
}

impl PmTrace {
    /// A trace that hit nothing and ended at `end`.
    pub fn miss(end: Vec3) -> Self {
        Self {
            allsolid: false,
            startsolid: false,
            inopen: true,
            inwater: false,
            fraction: 1.0,
            endpos: end,
            plane_normal: Vec3::ZERO,
            plane_dist: 0.0,
            ent: -1,
            deltavelocity: Vec3::ZERO,
            hitgroup: 0,
        }
    }
}

impl Default for PmTrace {
    fn default() -> Self {
        Self::miss(Vec3::ZERO)
    }
}

/// `playermove_t` (pm_defs.h) minus physents/visents/callbacks; field names are the snake_case of
/// the C names. `numtouch` is `touchindex.len()`.
#[derive(Debug, Clone)]
pub struct PlayerMove {
    pub player_index: i32,
    pub server: bool,
    pub multiplayer: bool,
    pub time: f32,
    pub frametime: f32,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub origin: Vec3,
    pub angles: Vec3,
    pub oldangles: Vec3,
    pub velocity: Vec3,
    pub movedir: Vec3,
    pub basevelocity: Vec3,
    pub view_ofs: Vec3,
    pub fl_duck_time: f32,
    pub b_in_duck: bool,
    pub fl_time_step_sound: i32,
    pub i_step_left: i32,
    pub fl_fall_velocity: f32,
    pub punchangle: Vec3,
    pub fl_swim_time: f32,
    pub fl_next_primary_attack: f32,
    pub effects: i32,
    pub flags: i32,
    pub usehull: i32,
    pub gravity: f32,
    pub friction: f32,
    pub oldbuttons: i32,
    pub waterjumptime: f32,
    pub dead: bool,
    pub deadflag: i32,
    pub spectator: i32,
    pub movetype: i32,
    pub onground: i32,
    pub waterlevel: i32,
    pub watertype: i32,
    pub oldwaterlevel: i32,
    pub sztexturename: String,
    pub chtexturetype: u8,
    pub maxspeed: f32,
    pub clientmaxspeed: f32,
    pub iuser1: i32,
    pub iuser2: i32,
    pub iuser3: i32,
    pub iuser4: i32,
    pub fuser1: f32,
    pub fuser2: f32,
    pub fuser3: f32,
    pub fuser4: f32,
    pub vuser1: Vec3,
    pub vuser2: Vec3,
    pub vuser3: Vec3,
    pub vuser4: Vec3,
    pub cmd: PmCmd,
    pub touchindex: Vec<PmTrace>,
    pub movevars: MoveVars,
    pub player_mins: [Vec3; 4],
    pub player_maxs: [Vec3; 4],
    pub runfuncs: bool,
}

impl Default for PlayerMove {
    /// A standing, grounded-unknown walker: `onground` -1, `gravity` and `friction` 1 (the entity
    /// values `pev->gravity`/`pev->friction` default to), the engine's four hulls and default
    /// movevars; everything else zero.
    fn default() -> Self {
        Self {
            player_index: 0,
            server: true,
            multiplayer: true,
            time: 0.0,
            frametime: 0.0,
            forward: Vec3::ZERO,
            right: Vec3::ZERO,
            up: Vec3::ZERO,
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            oldangles: Vec3::ZERO,
            velocity: Vec3::ZERO,
            movedir: Vec3::ZERO,
            basevelocity: Vec3::ZERO,
            view_ofs: Vec3::new(0.0, 0.0, 17.0),
            fl_duck_time: 0.0,
            b_in_duck: false,
            fl_time_step_sound: 0,
            i_step_left: 0,
            fl_fall_velocity: 0.0,
            punchangle: Vec3::ZERO,
            fl_swim_time: 0.0,
            fl_next_primary_attack: 0.0,
            effects: 0,
            flags: 0,
            usehull: 0,
            gravity: 1.0,
            friction: 1.0,
            oldbuttons: 0,
            waterjumptime: 0.0,
            dead: false,
            deadflag: 0,
            spectator: 0,
            movetype: super::consts::MOVETYPE_WALK,
            onground: -1,
            waterlevel: 0,
            watertype: super::consts::CONTENTS_EMPTY,
            oldwaterlevel: 0,
            sztexturename: String::new(),
            chtexturetype: 0,
            maxspeed: 320.0,
            clientmaxspeed: 0.0,
            iuser1: 0,
            iuser2: 0,
            iuser3: 0,
            iuser4: 0,
            fuser1: 0.0,
            fuser2: 0.0,
            fuser3: 0.0,
            fuser4: 0.0,
            vuser1: Vec3::ZERO,
            vuser2: Vec3::ZERO,
            vuser3: Vec3::ZERO,
            vuser4: Vec3::ZERO,
            cmd: PmCmd::default(),
            touchindex: Vec::new(),
            movevars: MoveVars::default(),
            player_mins: PLAYER_HULL_MINS,
            player_maxs: PLAYER_HULL_MAXS,
            runfuncs: true,
        }
    }
}

impl PlayerMove {
    /// `usehull` as an index into `player_mins`/`player_maxs`. The engine only ever sets 0..=3; an
    /// out-of-range value is clamped instead of indexing out of bounds.
    pub fn hull_index(&self) -> usize {
        self.usehull.clamp(0, 3) as usize
    }
}

/// What `pm_shared` needs from the world (the engine's `pmove` callbacks and physents).
/// Everything the game or the client has to provide; `cs_sim::pm` never calls `EngineApi`.
pub trait PmWorld {
    /// `PM_PlayerTrace(start, end, trace_flags, ignore_pe)` with `pmove->usehull` = `usehull`.
    fn player_trace(&mut self, start: Vec3, end: Vec3, usehull: i32, trace_flags: i32, ignore_pe: i32) -> PmTrace;
    /// `PM_TestPlayerPosition(pos, &trace)`: the physent index the hull at `pos` is stuck in (-1 =
    /// free) and the trace. QUIRK of the engine: the returned trace starts at `pm_origin`
    /// (`pmove->origin`), not at `pos`.
    fn test_player_position(&mut self, pm_origin: Vec3, pos: Vec3, usehull: i32) -> (i32, PmTrace);
    /// `PM_PointContents(p, &truecontents)`: (contents, truecontents).
    fn point_contents(&mut self, p: Vec3) -> (i32, i32);
    /// `pmove->nummoveent`.
    fn num_moveents(&self) -> usize;
    /// The ladder test of `PM_Ladder` for moveent `moveent`: the model is a brush model with
    /// `skin == CONTENTS_LADDER` and the hull-point contents of `origin` in its hull `usehull`
    /// are not empty.
    fn ladder_contains(&mut self, moveent: usize, origin: Vec3, usehull: i32) -> bool;
    /// `PM_GetModelBounds` of the moveent's model: (mins, maxs).
    fn moveent_bounds(&self, moveent: usize) -> (Vec3, Vec3);
    /// `moveents[i].origin`.
    fn moveent_origin(&self, moveent: usize) -> Vec3;
    /// `PM_TraceModel(moveent, start, end, &trace)`: (fraction, plane normal).
    fn trace_model(&mut self, moveent: usize, start: Vec3, end: Vec3) -> (f32, Vec3);
    /// `PM_TraceTexture(ground, start, end)`: the texture name hit on physent `ground`.
    fn trace_texture(&mut self, ground: i32, start: Vec3, end: Vec3) -> Option<String>;
    /// `PM_PlaySound(channel, sample, volume, attenuation, flags, pitch)`.
    fn play_sound(&mut self, channel: i32, sample: &str, volume: f32, attenuation: f32, flags: i32, pitch: i32);
    /// `RandomLong(low, high)` (the engine's RNG; server and client streams are independent).
    fn random_long(&mut self, low: i32, high: i32) -> i32;
    /// `Sys_FloatTime()` in seconds.
    fn sys_float_time(&mut self) -> f64;
    /// Origin and angles of the first physent whose `info == info` (PM_SpectatorMove tracking).
    fn physent_by_info(&self, info: i32) -> Option<(Vec3, Vec3)>;
    /// `pmove->physents[ent].player != 0`. Added by Task 4 (the plan's `PM_CheckStuck` reads it);
    /// the default says "not a player".
    fn physent_is_player(&self, _ent: i32) -> bool {
        false
    }
    /// `pmove->physents[ent].model != NULL`. Added by Task 4 (client branch of `PM_CheckStuck`);
    /// the default says "no model".
    fn physent_has_model(&self, _ent: i32) -> bool {
        false
    }
}

/// pm_shared.cpp globals and statics: stuck table and per-client stuck state, texture table,
/// `iSkipStep` and the other function statics. One per game (server) or per client.
///
/// QUIRK kept: the C statics are process-wide (shared by every player of the DLL); here they live
/// in the instance, which is the same as long as one instance serves the whole game.
pub struct PmShared {
    /// `pm_shared_initialized`.
    pub(crate) initialized: bool,
    /// `rgv3tStuckTable[54]`.
    pub(crate) stuck_table: [Vec3; 54],
    /// `rgStuckLast[MAX_CLIENTS][2]`: next stuck-table index, indexed `[player][server as usize]`.
    pub(crate) stuck_last: [[i32; 2]; MAX_CLIENTS],
    /// `static float rgStuckCheckTime[MAX_CLIENTS][2]` of `PM_CheckStuck` (second index 0 = server, 1 = client).
    pub(crate) stuck_check_time: [[f32; 2]; MAX_CLIENTS],
    /// `pm_gcTextures`, `pm_grgszTextureName`, `pm_grgchTextureType` (materials.txt table).
    pub(crate) textures: TextureTable,
    /// `static int iSkipStep` of `PM_PlayStepSound` (wade-step skip counter). Read by Task 7.
    #[allow(dead_code)]
    pub(crate) i_skip_step: i32,
    /// `g_onladder`.
    pub(crate) g_onladder: bool,
}

/// The mutable context every `PM_*` function works on: `pmove`, the shared globals and the world.
pub(crate) struct PmCtx<'a> {
    pub pm: &'a mut PlayerMove,
    pub sh: &'a mut PmShared,
    pub w: &'a mut dyn PmWorld,
}

impl PmCtx<'_> {
    /// `pmove->PM_PlayerTrace(start, end, PM_NORMAL, -1)` with the player's current `usehull`.
    pub(crate) fn trace(&mut self, start: Vec3, end: Vec3) -> PmTrace {
        self.w.player_trace(start, end, self.pm.usehull, PM_NORMAL, -1)
    }

    /// `pmove->PM_TestPlayerPosition(pos, &trace)` with the player's current `usehull`.
    pub(crate) fn test_position(&mut self, pos: Vec3) -> (i32, PmTrace) {
        self.w.test_player_position(self.pm.origin, pos, self.pm.usehull)
    }
}
