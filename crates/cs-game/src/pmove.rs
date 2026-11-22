//! The player's movement command as the engine and the game run it together: the part of
//! `SV_RunCmd` that the game owns in this architecture. Ported from ReHLDS
//! `rehlds/engine/sv_user.cpp` (`SV_SetupPMove`, `SV_FinishPMove`, `SV_ProcessImpacts`,
//! `SV_RunCmd`), `rehlds/engine/sv_pmove.cpp` (`PM_SV_TraceTexture`, `PM_SV_PlaySound`) and
//! ReGameDLL_CS `regamedll/dlls/client.cpp` (`CmdStart`, `CmdEnd`), all read 2026-11-22; the
//! movement itself is [`cs_sim::pm`].
//!
//! What the engine does around the game call (`SV_CheckMovingGround`, the lag-compensated
//! positions of the other players, the relink and the delivery of the touches) is in
//! `cs-engine`'s `usercmd.rs`. The order inside [`crate::player::run_usercmd`] is that of
//! `SV_RunCmd`: `CmdStart`, `oldangles`/`v_angle`, `PlayerPreThink`, [`run_pmove`] (setup,
//! `PM_Move`, finish, impacts queued); the engine relinks and touches; then `PlayerPostThink`
//! and [`cmd_end`].
//!
//! Deviations, on purpose:
//! - ReHLDS keeps one static `playermove_t` for all the players, so `onground`, `waterlevel`,
//!   `watertype` and `oldwaterlevel` are what the previous player's command left in it. Here
//!   every command starts from the player's own edict (`waterlevel` and `watertype` from the
//!   entvars, `onground` from `FL_ONGROUND` and `groundentity`, `oldwaterlevel` = `waterlevel`).
//!   `PM_CategorizePosition` computes all of them again before anything reads them, so the
//!   result is the same.
//! - `SV_ProcessImpacts` sets `v.velocity = tr.deltavelocity` for each touch before
//!   `SV_Impact`; the engine API reports only which entity was touched
//!   (`EngineApi::player_touched`), so the touches see the velocity the command ended with.
//! - `PlayerPostThink` and `CmdEnd` are a second game call (`GameDll::post_usercmd`) after
//!   the engine's `SV_ProcessImpacts` (relink, touches), so a game call never nests a touch.
//! - `Sys_FloatTime` is the engine's frame real time (`EngineApi::realtime`).
//! - `RandomLong` is the game's own generator ([`GameState`] `random_range`); the engine offers
//!   the game none. The server's and the client's streams are independent in the original too,
//!   so the footstep variants never need to match.

use cs_engine::consts::{FL_DUCKING, FL_ONGROUND};
use cs_engine::{EdictId, EngineApi, EntVars, PmPhysEnts, PmTraceResult};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::MoveVarsMsg;
use cs_sim::pm::{MoveVars, PlayerMove, PmCmd, PmTrace, PmWorld, pm_move};
use glam::Vec3;

use crate::GameState;

/// `VEC_DUCK_HULL_MIN` and `VEC_DUCK_HULL_MAX` (`util.h`): the entity size of a ducking player
/// (note the top, 32, where the movement's ducking hull has 18).
const VEC_DUCK_HULL_MIN: Vec3 = Vec3::new(-16.0, -16.0, -18.0);
const VEC_DUCK_HULL_MAX: Vec3 = Vec3::new(16.0, 16.0, 32.0);

/// `FIXANGLE_NONE`.
const FIXANGLE_NONE: i32 = 0;

/// The movevars the engine hands the movement, field by field (`SV_SetMoveVars`); the sky is
/// not the movement's.
pub(crate) fn movevars_from_msg(m: &MoveVarsMsg) -> MoveVars {
    MoveVars {
        gravity: m.gravity,
        stopspeed: m.stopspeed,
        maxspeed: m.maxspeed,
        spectatormaxspeed: m.spectatormaxspeed,
        accelerate: m.accelerate,
        airaccelerate: m.airaccelerate,
        wateraccelerate: m.wateraccelerate,
        friction: m.friction,
        edgefriction: m.edgefriction,
        waterfriction: m.waterfriction,
        entgravity: m.entgravity,
        bounce: m.bounce,
        stepsize: m.stepsize,
        maxvelocity: m.maxvelocity,
        zmax: m.zmax,
        wave_height: m.wave_height,
        footsteps: m.footsteps,
        rollangle: m.rollangle,
        rollspeed: m.rollspeed,
    }
}

/// The world as the movement sees it: the engine's physents and callbacks for one player.
pub(crate) struct EnginePmWorld<'a, 'e> {
    eng: &'a mut EngineApi<'e>,
    physents: &'a PmPhysEnts,
    player: EdictId,
    rng: &'a mut u32,
}

fn to_pm_trace(t: PmTraceResult) -> PmTrace {
    PmTrace {
        allsolid: t.allsolid,
        startsolid: t.startsolid,
        inopen: t.inopen,
        inwater: t.inwater,
        fraction: t.fraction,
        endpos: t.endpos,
        plane_normal: t.plane_normal,
        plane_dist: t.plane_dist,
        ent: t.ent,
        deltavelocity: Vec3::ZERO,
        hitgroup: t.hitgroup,
    }
}

impl PmWorld for EnginePmWorld<'_, '_> {
    fn player_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        usehull: i32,
        trace_flags: i32,
        ignore_pe: i32,
    ) -> PmTrace {
        to_pm_trace(self.eng.pm_player_trace(
            self.physents,
            start,
            end,
            usehull,
            trace_flags,
            ignore_pe,
        ))
    }

    fn test_player_position(&mut self, pm_origin: Vec3, pos: Vec3, usehull: i32) -> (i32, PmTrace) {
        let (ent, trace) = self
            .eng
            .pm_test_player_position(self.physents, pm_origin, pos, usehull);
        (ent, to_pm_trace(trace))
    }

    fn point_contents(&mut self, p: Vec3) -> (i32, i32) {
        self.eng.pm_point_contents(self.physents, p)
    }

    fn num_moveents(&self) -> usize {
        self.physents.num_moveents()
    }

    fn ladder_contains(&mut self, moveent: usize, origin: Vec3, usehull: i32) -> bool {
        self.eng
            .pm_ladder_contains(self.physents, moveent, origin, usehull)
    }

    fn moveent_bounds(&self, moveent: usize) -> (Vec3, Vec3) {
        self.physents
            .moveent(moveent)
            .map_or((Vec3::ZERO, Vec3::ZERO), |m| (m.model_mins, m.model_maxs))
    }

    fn moveent_origin(&self, moveent: usize) -> Vec3 {
        self.physents
            .moveent(moveent)
            .map_or(Vec3::ZERO, |m| m.origin)
    }

    fn trace_model(&mut self, moveent: usize, start: Vec3, end: Vec3) -> (f32, Vec3) {
        self.eng.pm_trace_model(self.physents, moveent, start, end)
    }

    /// `PM_SV_TraceTexture` (rh_sv_pmove.cpp, read 2026-11-22): nothing for a physent index out
    /// of range, a physent without a model or one whose `info` is not an edict; else the
    /// engine's `TraceTexture` on that physent's edict.
    fn trace_texture(&mut self, ground: i32, start: Vec3, end: Vec3) -> Option<String> {
        let pe = self.physents.physent(ground)?;
        if !pe.is_brush_model {
            return None;
        }
        let edict = self.physents.edict_of(ground)?;
        self.eng.trace_texture(edict, start, end)
    }

    /// `PM_SV_PlaySound`: everybody that hears the player but the player.
    fn play_sound(
        &mut self,
        channel: i32,
        sample: &str,
        volume: f32,
        attenuation: f32,
        flags: i32,
        pitch: i32,
    ) {
        self.eng.pm_play_sound(
            self.player,
            channel,
            sample,
            volume,
            attenuation,
            flags,
            pitch,
        );
    }

    fn random_long(&mut self, low: i32, high: i32) -> i32 {
        crate::random_in(self.rng, low, high)
    }

    fn sys_float_time(&mut self) -> f64 {
        self.eng.realtime()
    }

    fn physent_by_info(&self, info: i32) -> Option<(Vec3, Vec3)> {
        (0..self.physents.num_physents())
            .filter_map(|i| self.physents.physent(i as i32))
            .find(|pe| pe.info == info)
            .map(|pe| (pe.origin, pe.angles))
    }

    /// `physents[ent].player != 0` (`PM_CheckStuck`).
    fn physent_is_player(&self, ent: i32) -> bool {
        self.physents.physent(ent).is_some_and(|pe| pe.player != 0)
    }

    /// `physents[ent].model != NULL` (`PM_CheckStuck`).
    fn physent_has_model(&self, ent: i32) -> bool {
        self.physents
            .physent(ent)
            .is_some_and(|pe| pe.is_brush_model)
    }
}

/// The physent index of the player's `groundentity`, -1 when it is off the ground or the
/// entity is not among the physents.
fn onground_index(v: &EntVars, physents: &PmPhysEnts) -> i32 {
    if v.flags & FL_ONGROUND == 0 {
        return -1;
    }
    let Some(ground) = v.groundentity else {
        return -1;
    };
    (0..physents.num_physents() as i32)
        .find(|&i| physents.edict_of(i) == Some(ground))
        .unwrap_or(-1)
}

/// `SV_SetupPMove` (sv_user.cpp 767-829, read 2026-11-22): the movement state from the edict, the
/// command and the movevars. `physinfo` is not carried (the movement does not read it).
#[allow(clippy::too_many_arguments)]
fn setup_pmove(
    eng: &EngineApi<'_>,
    slot: u8,
    v: &EntVars,
    cmd: &GoldSrcUserCmd,
    old_angles: Vec3,
    movevars: MoveVars,
    physents: &PmPhysEnts,
) -> PlayerMove {
    // `(float)(ucmd->msec * 0.001)`: an int times a double, stored in a float.
    let frametime = (f64::from(cmd.msec) * 0.001) as f32;
    PlayerMove {
        server: true,
        multiplayer: eng.max_clients() > 1,
        // `(float)(1000.0 * client->svtimebase)`.
        time: (1000.0 * eng.time()) as f32,
        usehull: i32::from(v.flags & FL_DUCKING == FL_DUCKING),
        maxspeed: movevars.maxspeed,
        clientmaxspeed: v.max_speed,
        fl_duck_time: v.fl_duck_time as f32,
        b_in_duck: v.b_in_duck,
        fl_time_step_sound: v.fl_time_step_sound,
        i_step_left: v.i_step_left,
        fl_fall_velocity: v.fl_fall_velocity,
        fl_swim_time: v.fl_swim_time as f32,
        oldbuttons: v.oldbuttons,
        velocity: v.velocity,
        movedir: v.movedir,
        oldangles: old_angles,
        angles: v.v_angle,
        basevelocity: v.basevelocity,
        view_ofs: v.view_ofs,
        punchangle: v.punchangle,
        deadflag: v.deadflag,
        effects: v.effects as i32,
        gravity: v.gravity,
        friction: v.friction,
        spectator: 0,
        waterjumptime: v.teleport_time,
        cmd: PmCmd {
            msec: cmd.msec,
            viewangles: cmd.viewangles,
            forwardmove: cmd.forwardmove,
            sidemove: cmd.sidemove,
            upmove: cmd.upmove,
            buttons: cmd.buttons,
            impulse: cmd.impulse,
        },
        dead: v.health <= 0.0,
        movetype: i32::from(v.movetype),
        flags: v.flags as i32,
        player_index: i32::from(slot),
        iuser1: v.iuser1,
        iuser2: v.iuser2,
        iuser3: v.iuser3,
        iuser4: v.iuser4,
        fuser1: v.fuser1,
        fuser2: v.fuser2,
        fuser3: v.fuser3,
        fuser4: v.fuser4,
        vuser1: v.vuser1,
        vuser2: v.vuser2,
        vuser3: v.vuser3,
        vuser4: v.vuser4,
        origin: v.origin,
        runfuncs: true,
        frametime,
        movevars,
        // The per-player carry (see the module doc).
        onground: onground_index(v, physents),
        waterlevel: v.waterlevel,
        watertype: v.watertype,
        oldwaterlevel: v.waterlevel,
        ..PlayerMove::default()
    }
}

/// `SV_FinishPMove` (sv_user.cpp 831-894, read 2026-11-22): the result of the movement back into
/// the edict. QUIRKS kept: `flDuckTime` and `flSwimTime` are stored as `int`, truncating; the
/// ground entity is left as it was when the player is in the air; `v.angles` is a third of the
/// pitch (negated), the yaw and the roll.
fn finish_pmove(v: &mut EntVars, pm: &PlayerMove, physents: &PmPhysEnts) {
    v.deadflag = pm.deadflag;
    v.effects = pm.effects as u32;
    v.teleport_time = pm.waterjumptime;
    v.waterlevel = pm.waterlevel;
    v.watertype = pm.watertype;
    v.flags = pm.flags as u32;
    v.friction = pm.friction;
    v.movetype = pm.movetype as u8;
    v.max_speed = pm.clientmaxspeed;
    v.i_step_left = pm.i_step_left;

    v.view_ofs = pm.view_ofs;
    v.movedir = pm.movedir;
    v.punchangle = pm.punchangle;

    if pm.onground == -1 {
        v.flags &= !FL_ONGROUND;
    } else {
        v.flags |= FL_ONGROUND;
        if let Some(ground) = physents.edict_of(pm.onground) {
            v.groundentity = Some(ground);
        }
    }

    v.origin = pm.origin;
    v.velocity = pm.velocity;
    v.basevelocity = pm.basevelocity;

    // The model shows a 1/3 the pitch and the whole yaw and roll.
    if v.fixangle == FIXANGLE_NONE {
        v.v_angle = pm.angles;
        v.angles = Vec3::new(
            (-f64::from(pm.angles.x) / 3.0) as f32,
            pm.angles.y,
            pm.angles.z,
        );
    }

    v.b_in_duck = pm.b_in_duck;
    v.fl_duck_time = pm.fl_duck_time as i32;
    v.fl_time_step_sound = pm.fl_time_step_sound;
    v.fl_fall_velocity = pm.fl_fall_velocity;
    v.fl_swim_time = pm.fl_swim_time as i32;
    v.oldbuttons = i32::from(pm.cmd.buttons);

    v.iuser1 = pm.iuser1;
    v.iuser2 = pm.iuser2;
    v.iuser3 = pm.iuser3;
    v.iuser4 = pm.iuser4;
    v.fuser1 = pm.fuser1;
    v.fuser2 = pm.fuser2;
    v.fuser3 = pm.fuser3;
    v.fuser4 = pm.fuser4;
    v.vuser1 = pm.vuser1;
    v.vuser2 = pm.vuser2;
    v.vuser3 = pm.vuser3;
    v.vuser4 = pm.vuser4;

    // `SetMinMaxSize(ent, player_mins[usehull], player_maxs[usehull])`: no relink.
    let hull = pm.hull_index();
    v.mins = pm.player_mins[hull];
    v.maxs = pm.player_maxs[hull];
    v.size = v.maxs - v.mins;
}

/// Runs the movement of one command of the player of `slot`: `SV_SetupPMove`, `PM_Move`,
/// `SV_FinishPMove` and the impacts (`SV_ProcessImpacts`: each entity the movement touched is
/// reported to the engine, which delivers the touches once the game call is over). The caller
/// has already run `PlayerPreThink` and set `v_angle`; `pmove->oldangles` is the player's
/// [`oldangles`](crate::player::Player) (the `v_angle` from before the command's).
pub(crate) fn run_pmove(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    cmd: &GoldSrcUserCmd,
) {
    let id = eng.client_edict(slot);
    let Some(old_angles) = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| p.oldangles)
    else {
        return;
    };
    let physents = eng.pm_physents(id);
    let movevars = movevars_from_msg(&eng.move_vars());
    let Some(v) = eng.ent(id) else {
        return;
    };
    let mut pm = setup_pmove(eng, slot, v, cmd, old_angles, movevars, &physents);

    {
        let mut world = EnginePmWorld {
            eng: &mut *eng,
            physents: &physents,
            player: id,
            rng: &mut g.rng,
        };
        pm_move(&mut pm, &mut g.pm_shared, &mut world);
    }

    if let Some(v) = eng.ent_mut(id) {
        finish_pmove(v, &pm, &physents);
    }
    // `SV_ProcessImpacts`: `touch = EDICT_NUM(physents[tr->ent].info)` for each touch.
    for tr in &pm.touchindex {
        if let Some(other) = physents.edict_of(tr.ent) {
            eng.player_touched(id, other);
        }
    }
}

/// `CmdEnd` (client.cpp 5168, read 2026-11-22): a ducking player's entity gets `VEC_DUCK_HULL`
/// (whose top, 32, is not the movement's ducking hull's 18).
pub(crate) fn cmd_end(eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    if eng.ent(id).is_some_and(|v| v.flags & FL_DUCKING != 0) {
        eng.set_size(id, VEC_DUCK_HULL_MIN, VEC_DUCK_HULL_MAX);
    }
}

/// `(info, physent_is_player, physent_has_model)` of every physent of the player of `slot`, as
/// the movement code asks the world. For tests only (cargo feature `test-hooks`).
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub(crate) fn world_flags(eng: &mut EngineApi<'_>, slot: u8) -> Vec<(i32, bool, bool)> {
    let id = eng.client_edict(slot);
    let physents = eng.pm_physents(id);
    let mut rng = 1;
    let world = EnginePmWorld {
        eng,
        physents: &physents,
        player: id,
        rng: &mut rng,
    };
    (0..physents.num_physents() as i32)
        .filter_map(|i| {
            let info = physents.physent(i)?.info;
            Some((info, world.physent_is_player(i), world.physent_has_model(i)))
        })
        .collect()
}
