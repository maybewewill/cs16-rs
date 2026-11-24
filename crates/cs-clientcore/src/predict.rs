//! Movement prediction: the commands the server has not answered yet are run again, on top of
//! the newest server frame, through the same `pm_shared` code the server runs ([`cs_pm::pm`]).
//!
//! Ported from Xash3D FWGS (GPL-3.0-or-later; read 2026-11-23) `engine/client/dll_int/cl_pmove.c`:
//! `CL_PredictMovement` (the replay loop and the final blend), `CL_RunUsercmd` (a command longer
//! than 50 ms is run as two halves), `CL_SetupPMove` and `CL_FinishPMove` (the mapping between the
//! frame's player state and `playermove_t`), `CL_CheckPredictionError`, `CL_PlayerTeleported`,
//! `CL_SetUpPlayerPrediction`, `CL_SetSolidPlayers`, `CL_AddLinksToPmove`,
//! `CL_CopyEntityToPhysEnt` (the physents of the frame) and `CL_SetSolidEntities`. The movement is
//! `cs-pm`; the world it walks in is the BSP ([`cs_pm::collision::WorldCollision`], given by the
//! shell) plus the entities of the newest frame, so doors, platforms and other players are
//! solid for the prediction as they are for the server.
//!
//! What differs from Xash3D, on purpose (each is also named where it happens):
//! - Commands are numbered by the netchan packet they travel in (the server acknowledges a
//!   packet, and a packet carries up to 16 commands; Xash3D sends one command per packet). A
//!   command not sent yet belongs to the next packet. The error check compares the server's
//!   origin with the predicted origin after the last command of the acknowledged packet.
//! - Loss: the predictor assumes every command of an acknowledged packet ran. When a client
//!   packet is lost the server replays only `min(dropped, 2)` backups (and repeats its last
//!   command beyond that), and the client cannot tell a lost packet from a lost server frame
//!   (both show as an acknowledgement that skips). The original client has the same blind
//!   spot: the error check flags it once (bounded by the commands of the lost packet) and the
//!   next frame, which the prediction restarts from, is exact again. Accepted transient,
//!   tested by `prediction_recovers_from_loss`.
//! - `TxferPredictionData`: only the part that is the movement's own state survives a server
//!   frame (`oldbuttons`, `flFallVelocity`, `iStepLeft`: the server never sends them). The
//!   client-DLL overrides of `clientdata` (`maxspeed`, `deadflag`, `fuser2`, ...) are not
//!   applied, the server's values win; they belong with the client weapon code (phase 6) and
//!   keeping the server's copy is what makes the CS velocity modifier and the weapon speed
//!   reach the prediction.
//! - Weapon prediction is bookkeeping (plan, Global Constraints): the clip, animation and
//!   timers come from `weapon_data`/`clientdata`, time passes through the timers by each
//!   command's `msec` ([`WeaponState::post_run_cmd`]); no `ItemPostFrame` runs on the client.
//! - `PM_TraceTexture` answers nothing (the footstep material falls back to concrete): the
//!   collision data has no texture names. Assumption: only the sound of a step differs.
//! - `SV_Active()` of the original ("a local server does not smooth the correction") is
//!   [`InterpSettings::remote_is_local`](crate::interp::InterpSettings).
//! - `cl.intermission`, `cls.spectator` and the demo paths are not modelled: no intermission is
//!   tracked ([`PredictInputs::intermission`] is the caller's), and a spectator is not predicted.
//!
//! The behaviour of the weapon post-run bookkeeping comes from the survey of the client DLL's
//! data flow (read-only; the Valve-headed `cs_weapons.cpp` is not copied).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use cs_pm::collision::{NO_COLLISION, WorldCollision, in_world};
use cs_pm::physent::{
    self, CONTENTS_LADDER, MAX_MOVEENTS, MAX_PHYSENTS, PM_PLAYER_MAXS, PM_PLAYER_MINS, PhysEnt,
    PhysEntView, PhysModel, PmPhysEnts, PmTraceResult, world_physent,
};
use cs_pm::pm::consts::{CONTENTS_EMPTY, FL_DUCKING, FL_ONGROUND, MOVETYPE_WALK};
use cs_pm::pm::{MoveVars, PlayerMove, PmCmd, PmShared, PmTrace, PmWorld, pm_init, pm_move};
use cs_proto48::delta::{GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::messages::MoveVarsMsg;
use glam::Vec3;

use crate::cvars::ClientCvars;
use crate::entities::{EntityStore, GAME_FPS, ModelKind};
use crate::frames::{ClientFrames, Frame, UPDATE_BACKUP, UPDATE_MASK};
use crate::interp::compute_player_origin;
use crate::precache::PrecacheTables;
use crate::weapons_predict::{
    PLAYER_HOLDING_SHIELD, PredictWeaponExtras, PredictedWeaponEvent, predict_weapon_step,
};

/// `MAX_PREDICTION_ERROR`: a larger error is a teleport (or a pause): nothing is smoothed.
pub const MAX_PREDICTION_ERROR: f32 = 64.0;
/// `MIN_CORRECTION_DISTANCE`: a smaller error is not worth a correction.
pub const MIN_CORRECTION_DISTANCE: f32 = 0.25;
/// A command longer than this is run as two halves (`CL_RunUsercmd`).
pub const MAX_USERCMD_MSEC: u8 = 50;
/// Commands the predictor keeps while the server does not answer; the oldest go first.
pub const MAX_PENDING_CMDS: usize = 512;
/// Sounds waiting for the shell; the oldest go first.
pub const MAX_PENDING_SOUNDS: usize = 256;
/// Prediction errors [`Predictor::checks`] keeps.
pub const CHECK_LOG_CAPACITY: usize = 512;

const SOLID_NOT: u8 = 0;
const SOLID_TRIGGER: u8 = 1;
const SOLID_BBOX: u8 = 2;
const SOLID_SLIDEBOX: u8 = 3;
const SOLID_BSP: u8 = 4;

/// `CL_RunUsercmd`'s split: a command over [`MAX_USERCMD_MSEC`] becomes two of half the `msec`
/// (integer division), the second without `impulse` and `weaponselect`; each half is split the
/// same way. The server's `SV_RunCmd` splits alike (`cs_engine::usercmd::split_usercmd`), so the
/// two simulate the same time.
pub fn split_usercmd(cmd: &GoldSrcUserCmd) -> Vec<GoldSrcUserCmd> {
    fn push(cmd: GoldSrcUserCmd, out: &mut Vec<GoldSrcUserCmd>) {
        if cmd.msec <= MAX_USERCMD_MSEC {
            out.push(cmd);
            return;
        }
        let mut half = cmd;
        half.msec /= 2;
        push(half.clone(), out);
        half.impulse = 0;
        half.weaponselect = 0;
        push(half, out);
    }
    let mut out = Vec::new();
    push(cmd.clone(), &mut out);
    out
}

/// The movevars the movement reads, field by field (the sky is not the movement's).
pub fn movevars_from_msg(m: &MoveVarsMsg) -> MoveVars {
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

/// `cl_nopred`, `cl_nosmooth`, `cl_smoothtime` and `cl_solid_players`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictSettings {
    pub nopred: bool,
    pub nosmooth: bool,
    pub smoothtime: f32,
    /// `cl_solid_players`: the other players are solid for the prediction.
    pub solid_players: bool,
}

impl Default for PredictSettings {
    fn default() -> Self {
        Self {
            nopred: false,
            nosmooth: false,
            smoothtime: 0.1,
            solid_players: true,
        }
    }
}

impl PredictSettings {
    pub fn from_cvars(cvars: &ClientCvars) -> Self {
        Self {
            nopred: cvars.float("cl_nopred") != 0.0,
            nosmooth: cvars.float("cl_nosmooth") != 0.0,
            smoothtime: cvars.float("cl_smoothtime"),
            solid_players: cvars.float("cl_solid_players") != 0.0,
        }
    }
}

/// Where another player is drawn this frame (`CL_SetUpPlayerPrediction`): the prediction
/// collides with it there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerPose {
    pub number: u16,
    pub origin: Vec3,
    pub angles: Vec3,
}

/// `CL_SetUpPlayerPrediction` for every player of `frames` but the local one: the interpolated
/// origin and angles (`CL_ComputePlayerOrigin`), taken from the entity's current state.
pub fn player_poses(frames: &mut ClientFrames) -> Vec<PlayerPose> {
    let ClientFrames {
        ring,
        clock,
        entities,
        settings,
        parsecount,
        ..
    } = frames;
    let Some(frame) = ring.get(*parsecount).filter(|f| f.valid) else {
        return Vec::new();
    };
    let local = entities.local_entity_number();
    let max_clients = entities.max_clients;
    let mut poses = Vec::new();
    for fe in &frame.entities {
        let number = fe.state.number;
        if number == local || number == 0 || number > u16::from(max_clients) {
            continue;
        }
        if fe.state.modelindex == 0 {
            continue;
        }
        let Some(ent) = entities.ents.get_mut(&number) else {
            continue;
        };
        ent.origin = ent.curstate.origin;
        ent.angles = ent.curstate.angles;
        compute_player_origin(ent, clock.time, settings);
        poses.push(PlayerPose {
            number,
            origin: ent.origin,
            angles: ent.angles,
        });
    }
    poses
}

/// The weapon bookkeeping of the prediction: what the server's `clientdata` and `weapon_data`
/// say, with time passing through the timers. See the module doc: no client-side
/// `ItemPostFrame`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WeaponState {
    /// `clientdata.m_flNextAttack`: the player cannot attack until it is 0.
    pub next_attack: f32,
    /// `clientdata.m_iId`: the weapon in hand.
    pub weapon_id: i32,
    pub viewmodel: i32,
    /// `clientdata.weaponanim`.
    pub weaponanim: i32,
    pub fov: f32,
    /// `weapon_data_t` by weapon index.
    pub weapons: BTreeMap<u8, GoldSrcWeaponData>,
}

impl WeaponState {
    /// The timers one command lets run out: each is lowered by `msec / 1000`, down to its
    /// floor (the lowest a timer is allowed to read): `m_flNextPrimaryAttack` -1.0,
    /// `m_flNextSecondaryAttack`, `m_flTimeWeaponIdle`, `m_flNextReload` and
    /// `m_flNextAttack` -0.001, `m_fNextAimBonus` -1.0. Assumption: `m_flPumpTime` is left
    /// alone (the survey says it "wraps" without saying how); the server resets it on its own.
    pub fn post_run_cmd(&mut self, msec: u8) {
        let dt = f32::from(msec) / 1000.0;
        let lower = |v: &mut f32, floor: f32| {
            if *v > floor {
                *v = (*v - dt).max(floor);
            }
        };
        lower(&mut self.next_attack, -0.001);
        for w in self.weapons.values_mut() {
            lower(&mut w.m_fl_next_primary_attack, -1.0);
            lower(&mut w.m_fl_next_secondary_attack, -0.001);
            lower(&mut w.m_fl_time_weapon_idle, -0.001);
            lower(&mut w.m_fl_next_reload, -0.001);
            lower(&mut w.m_f_next_aim_bonus, -1.0);
        }
    }

    /// Whether the weapon in hand may attack now: the player's timer and the weapon's primary
    /// timer have run out.
    pub fn can_attack(&self) -> bool {
        self.next_attack <= 0.0
            && u8::try_from(self.weapon_id)
                .ok()
                .and_then(|id| self.weapons.get(&id))
                .is_none_or(|w| w.m_fl_next_primary_attack <= 0.0)
    }
}

/// `local_state_t`: the part of the player the movement reads and writes between commands
/// (`entity_state_t` of the player plus `clientdata_t`) and the weapon bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalState {
    pub origin: Vec3,
    pub velocity: Vec3,
    /// `playerstate.angles` / `pmove->angles`: the view angles the command gave, with the roll
    /// the movement adds.
    pub angles: Vec3,
    pub basevelocity: Vec3,
    pub view_ofs: Vec3,
    pub punchangle: Vec3,
    pub flags: i32,
    pub movetype: i32,
    pub onground: i32,
    pub usehull: i32,
    pub waterlevel: i32,
    pub watertype: i32,
    pub effects: i32,
    pub friction: f32,
    pub gravity: f32,
    /// Not sent by the server; kept from the prediction (`TxferPredictionData`).
    pub oldbuttons: i32,
    pub fl_fall_velocity: f32,
    pub i_step_left: i32,
    /// `clientdata`'s `flDuckTime`, `flSwimTime` and `waterjumptime` are `int`: the movement's
    /// float is truncated each time it is stored (`CL_FinishPMove`).
    pub fl_duck_time: i32,
    pub fl_swim_time: i32,
    pub waterjumptime: i32,
    pub b_in_duck: bool,
    pub fl_time_step_sound: i32,
    pub maxspeed: f32,
    pub deadflag: i32,
    pub health: f32,
    pub iuser: [i32; 4],
    pub fuser: [f32; 4],
    pub vuser: [Vec3; 4],
    pub weapon: WeaponState,
    pub weapon_extras: BTreeMap<i32, PredictWeaponExtras>,
}

impl Default for LocalState {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            angles: Vec3::ZERO,
            basevelocity: Vec3::ZERO,
            view_ofs: Vec3::new(0.0, 0.0, 17.0),
            punchangle: Vec3::ZERO,
            flags: 0,
            movetype: MOVETYPE_WALK,
            onground: -1,
            usehull: 0,
            waterlevel: 0,
            watertype: CONTENTS_EMPTY,
            effects: 0,
            friction: 1.0,
            gravity: 1.0,
            oldbuttons: 0,
            fl_fall_velocity: 0.0,
            i_step_left: 0,
            fl_duck_time: 0,
            fl_swim_time: 0,
            waterjumptime: 0,
            b_in_duck: false,
            fl_time_step_sound: 0,
            maxspeed: 0.0,
            deadflag: 0,
            health: 100.0,
            iuser: [0; 4],
            fuser: [0.0; 4],
            vuser: [Vec3::ZERO; 4],
            weapon: WeaponState::default(),
            weapon_extras: BTreeMap::new(),
        }
    }
}

/// What the prediction keeps of its own result across a server frame.
#[derive(Debug, Clone, Default)]
pub struct Carried {
    pub oldbuttons: i32,
    pub fl_fall_velocity: f32,
    pub i_step_left: i32,
    pub weapon_extras: BTreeMap<i32, PredictWeaponExtras>,
}

impl LocalState {
    /// The player as the server's frame says: `clientdata` (origin, velocity, timers, ...) and
    /// the local player's entity state (`usehull`, `friction`, `gravity`, `movetype`, ...),
    /// with what the prediction carries (`TxferPredictionData`). `None` without `clientdata`.
    /// A frame that does not hold the local player's entity state gets sensible defaults, as
    /// Xash3D's workaround for a server that "refuses to send our local player in delta".
    pub fn from_frame(frame: &Frame, local: u16, carried: Option<&Carried>) -> Option<Self> {
        let cd = frame.clientdata.as_ref()?;
        let ps: Option<&GoldSrcEntityState> = frame.entity(local).map(|e| &e.state);
        let flags = cd.flags as i32;
        let carried = carried.cloned().unwrap_or_default();
        Some(Self {
            origin: cd.origin,
            velocity: cd.velocity,
            angles: ps.map_or(Vec3::ZERO, |p| p.angles),
            basevelocity: ps.map_or(Vec3::ZERO, |p| p.basevelocity),
            view_ofs: cd.view_ofs,
            punchangle: cd.punchangle,
            flags,
            movetype: ps.map_or(MOVETYPE_WALK, |p| i32::from(p.movetype)),
            // The server does not send it; the movement finds the ground again before it
            // reads it (`PM_CategorizePosition`). The world is the likeliest ground.
            onground: if flags & FL_ONGROUND != 0 { 0 } else { -1 },
            usehull: ps.map_or(i32::from(flags & FL_DUCKING != 0), |p| i32::from(p.usehull)),
            waterlevel: cd.waterlevel,
            watertype: CONTENTS_EMPTY,
            effects: ps.map_or(0, |p| p.effects as i32),
            friction: ps.map_or(1.0, |p| p.friction),
            gravity: ps.map_or(1.0, |p| p.gravity),
            oldbuttons: carried.oldbuttons,
            fl_fall_velocity: carried.fl_fall_velocity,
            i_step_left: carried.i_step_left,
            fl_duck_time: cd.fl_duck_time,
            fl_swim_time: cd.fl_swim_time,
            waterjumptime: cd.waterjumptime,
            b_in_duck: cd.b_in_duck != 0,
            fl_time_step_sound: cd.fl_time_step_sound,
            maxspeed: cd.maxspeed,
            deadflag: cd.deadflag,
            health: cd.health,
            iuser: [cd.iuser1, cd.iuser2, cd.iuser3, cd.iuser4],
            fuser: [cd.fuser1, cd.fuser2, cd.fuser3, 0.0],
            vuser: [Vec3::ZERO, cd.vuser2, cd.vuser3, cd.vuser4],
            weapon: WeaponState {
                next_attack: cd.m_fl_next_attack,
                weapon_id: cd.m_i_id,
                viewmodel: cd.viewmodel,
                weaponanim: cd.weaponanim,
                fov: cd.fov,
                weapons: frame.weapon_data.clone(),
            },
            weapon_extras: carried.weapon_extras,
        })
    }

    fn carried(&self) -> Carried {
        Carried {
            oldbuttons: self.oldbuttons,
            fl_fall_velocity: self.fl_fall_velocity,
            i_step_left: self.i_step_left,
            weapon_extras: self.weapon_extras.clone(),
        }
    }
}

/// A sound the movement asked for (`PM_PlaySound`): footsteps, water, jumps. Played by the
/// shell, only for a command the prediction runs for the first time.
#[derive(Debug, Clone, PartialEq)]
pub struct PredictedSound {
    pub channel: i32,
    pub sample: String,
    pub volume: f32,
    pub attenuation: f32,
    pub flags: i32,
    pub pitch: i32,
}

/// One check of the prediction against the server (`CL_CheckPredictionError`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictionCheck {
    /// The packet the server acknowledged.
    pub ack: u32,
    /// The distance between the server's origin and the one predicted for that packet.
    pub error: f32,
    /// The error was larger than [`MAX_PREDICTION_ERROR`]: a teleport, nothing is smoothed.
    pub teleport: bool,
}

/// What the prediction left for the view and the renderer (`cl.simorg` and friends).
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    /// The values come from the prediction; otherwise they are the server's.
    pub predicted: bool,
    /// `cl.simorg`: where the local player is.
    pub simorg: Vec3,
    pub simvel: Vec3,
    pub punchangle: Vec3,
    /// `cl.viewheight`: `view_ofs`.
    pub viewheight: Vec3,
    pub onground: i32,
    /// The entity number of the ground (`cl.local.lastground`); -1 in the air.
    pub lastground: i32,
    /// The ground moves in the plane (`cl.local.moving`): the player rides it.
    pub moving: bool,
    pub usehull: i32,
    pub waterlevel: i32,
    pub viewmodel: i32,
    /// `cl.local.prediction_error`: the server's origin minus the predicted one at the last
    /// check (0 after a teleport).
    pub prediction_error: Vec3,
    /// The player moved further in one command than `maxvelocity` allows in a frame
    /// (`CL_PlayerTeleported`): the view takes the new place at once.
    pub teleported: bool,
    /// The weapon bookkeeping after the last predicted command.
    pub weapon: WeaponState,
    /// Whether the local player holds a tactical shield (`PLAYER_HOLDING_SHIELD` in `iuser3`).
    pub holding_shield: bool,
}

impl Default for Prediction {
    fn default() -> Self {
        Self {
            predicted: false,
            simorg: Vec3::ZERO,
            simvel: Vec3::ZERO,
            punchangle: Vec3::ZERO,
            viewheight: Vec3::ZERO,
            onground: -1,
            lastground: -1,
            moving: false,
            usehull: 0,
            waterlevel: 0,
            viewmodel: 0,
            prediction_error: Vec3::ZERO,
            teleported: false,
            weapon: WeaponState::default(),
            holding_shield: false,
        }
    }
}

impl Prediction {
    /// The entity the local player rides, for `EmitInputs::standing_on`
    /// (`cl.local.moving && cl.local.onground`).
    pub fn standing_on(&self) -> Option<u16> {
        if self.moving && self.onground > 0 {
            u16::try_from(self.lastground).ok()
        } else {
            None
        }
    }
}

/// A command the prediction knows: from [`Predictor::push_cmd`] until the server acknowledged
/// the packet it went out in.
#[derive(Debug, Clone)]
struct PendingCmd {
    cmd: GoldSrcUserCmd,
    /// When the command was made (`senttime`).
    created_at: f64,
    /// The netchan sequence of the packet it went out in; `None` until it is sent.
    sequence: Option<u32>,
    /// Shared RNG seed (`select_cmds_with_seeds`), initialized on send.
    random_seed: u32,
    /// `processedfuncs`: the command has run once, so its sounds and weapon events were played.
    processed: bool,
}

/// What the caller hands the predictor each frame.
pub struct PredictInputs<'a> {
    pub now: f64,
    /// `cls.netchan.incoming_acknowledged`.
    pub incoming_acknowledged: u32,
    /// `cls.netchan.outgoing_sequence`.
    pub outgoing_sequence: u32,
    /// The newest frame, when it is usable (`cl.validsequence`, `frame->valid`).
    pub frame: Option<&'a Frame>,
    /// The local player's slot (`cl.playernum`).
    pub playernum: u8,
    pub max_clients: u8,
    pub precache: &'a PrecacheTables,
    pub movevars: MoveVars,
    /// `clgame.movevars.maxvelocity`.
    pub max_velocity: f32,
    pub poses: &'a [PlayerPose],
    /// The entity store, to see whether the ground the player stands on moves.
    pub entities: Option<&'a EntityStore>,
    /// The server runs in this process (`SV_Active()`): a wrong prediction is not smoothed.
    pub remote_is_local: bool,
    /// `cl.intermission`.
    pub intermission: bool,
}

/// The movement prediction of the local player.
pub struct Predictor {
    settings: PredictSettings,
    world: Option<Arc<WorldCollision>>,
    pm_shared: PmShared,
    now: f64,
    last_update: f64,
    cmds: VecDeque<PendingCmd>,
    /// The local state after the last command of packet `s`, by `s & UPDATE_MASK` (the sequence is
    /// kept to tell a stale slot).
    predicted: Vec<Option<(u32, LocalState)>>,
    /// `cl.local.predicted_origins`: `(sequence, origin)` by `sequence & UPDATE_MASK`.
    predicted_origins: Vec<Option<(u32, Vec3)>>,
    carried: Option<Carried>,
    /// The sequence of the frame the last update was based on.
    last_frame: u32,
    /// When the last command the server acknowledged was made.
    last_acked_time: f64,
    out: Prediction,
    /// `cls.correction_time` and `cl.local.lastorigin`.
    correction_time: f32,
    lastorigin: Vec3,
    rng: u32,
    sounds: Vec<PredictedSound>,
    weapon_events: Vec<PredictedWeaponEvent>,
    /// Mirrors `cl_lw` (`1` by default): when true, weapon prediction emits local events/sounds.
    pub cl_lw: bool,
    checks: VecDeque<PredictionCheck>,
    /// Test hook: every command the movement ran, in order (split halves apart), as `(msec,
    /// impulse)`; empty unless [`record_steps`](Self::record_steps) is on.
    steps: Vec<(u8, u8)>,
    record_steps: bool,
}

impl Default for Predictor {
    fn default() -> Self {
        Self::new()
    }
}

impl Predictor {
    pub fn new() -> Self {
        Self {
            settings: PredictSettings::default(),
            world: None,
            pm_shared: pm_init(None),
            now: 0.0,
            last_update: 0.0,
            cmds: VecDeque::new(),
            predicted: vec![None; UPDATE_BACKUP],
            predicted_origins: vec![None; UPDATE_BACKUP],
            carried: None,
            last_frame: 0,
            last_acked_time: 0.0,
            out: Prediction::default(),
            correction_time: 0.0,
            lastorigin: Vec3::ZERO,
            rng: 0x2545_f491,
            sounds: Vec::new(),
            weapon_events: Vec::new(),
            cl_lw: true,
            checks: VecDeque::new(),
            steps: Vec::new(),
            record_steps: false,
        }
    }

    /// Forgets everything that belongs to the map, the world's collision included: the shell
    /// gives the new map's with [`set_world`](Self::set_world), and until it does the player is
    /// not predicted (a prediction in empty space would be wrong). The settings and the
    /// materials stay.
    pub fn reset(&mut self) {
        let settings = self.settings;
        let cl_lw = self.cl_lw;
        let pm_shared = std::mem::replace(&mut self.pm_shared, pm_init(None));
        let record = self.record_steps;
        *self = Self::new();
        self.settings = settings;
        self.cl_lw = cl_lw;
        self.pm_shared = pm_shared;
        self.record_steps = record;
    }

    /// The map's collision (the BSP's hulls): the world the prediction walks in. Without it the
    /// player is not predicted (the server's state is shown).
    pub fn set_world(&mut self, world: Option<Arc<WorldCollision>>) {
        self.world = world;
    }

    /// Whether the map's collision is known, so the player is predicted.
    pub fn has_world(&self) -> bool {
        self.world.is_some()
    }

    /// `sound/materials.txt`: the texture table the footsteps read (`PM_Init`).
    pub fn set_materials(&mut self, materials_txt: &[u8]) {
        self.pm_shared = pm_init(Some(materials_txt));
    }

    pub fn apply_cvars(&mut self, cvars: &ClientCvars) {
        self.settings = PredictSettings::from_cvars(cvars);
        self.cl_lw = cvars.float("cl_lw") != 0.0;
    }

    pub fn settings(&self) -> PredictSettings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: PredictSettings) {
        self.settings = settings;
    }

    /// The client's clock of the frame (`host.realtime`), the time a command made now gets.
    pub fn set_now(&mut self, now: f64) {
        self.now = now;
    }

    /// A command the input layer made; the prediction runs it at once, before it is sent.
    pub fn push_cmd(&mut self, cmd: GoldSrcUserCmd) {
        if self.cmds.len() >= MAX_PENDING_CMDS {
            self.cmds.pop_front();
        }
        self.cmds.push_back(PendingCmd {
            cmd,
            created_at: self.now,
            sequence: None,
            random_seed: 0,
            processed: false,
        });
    }

    /// `take` queued commands went out in the packet `sequence`; when the packet carried none
    /// the connection sent `placeholder` (the last command again, `msec` 0), which the server
    /// runs as a command of its own.
    pub fn packet_sent(
        &mut self,
        sequence: u32,
        take: usize,
        placeholder: Option<GoldSrcUserCmd>,
    ) {
        let count = self
            .cmds
            .iter()
            .filter(|c| c.sequence.is_none())
            .take(take)
            .count();
        for (idx, c) in self
            .cmds
            .iter_mut()
            .filter(|c| c.sequence.is_none())
            .take(take)
            .enumerate()
        {
            c.sequence = Some(sequence);
            c.random_seed = sequence.wrapping_sub((count - 1 - idx) as u32);
        }
        if let Some(cmd) = placeholder {
            if self.cmds.len() >= MAX_PENDING_CMDS {
                self.cmds.pop_front();
            }
            self.cmds.push_back(PendingCmd {
                cmd,
                created_at: self.now,
                sequence: Some(sequence),
                random_seed: sequence,
                processed: false,
            });
        }
    }

    /// What the last update computed.
    pub fn prediction(&self) -> &Prediction {
        &self.out
    }

    /// The commands the server has not acknowledged (sent or not).
    pub fn unacked(&self) -> usize {
        self.cmds.len()
    }

    /// The checks against the server, oldest first (the last [`CHECK_LOG_CAPACITY`]).
    pub fn checks(&self) -> &VecDeque<PredictionCheck> {
        &self.checks
    }

    /// The sounds of the commands run for the first time since the last call.
    pub fn take_sounds(&mut self) -> Vec<PredictedSound> {
        std::mem::take(&mut self.sounds)
    }

    /// Local weapon events emitted for `runfuncs && cl_lw` steps since the last call.
    pub fn take_weapon_events(&mut self) -> Vec<PredictedWeaponEvent> {
        std::mem::take(&mut self.weapon_events)
    }

    /// Predicted `m_iWeaponState` bits of the active weapon (`WPNSTATE_*`).
    pub fn predicted_weapon_state(&self) -> i32 {
        u8::try_from(self.out.weapon.weapon_id)
            .ok()
            .and_then(|id| self.out.weapon.weapons.get(&id))
            .map_or(0, |w| w.m_i_weapon_state)
    }

    /// Whether the predicted local state is carrying a tactical shield (`PLAYER_HOLDING_SHIELD`).
    pub fn predicted_holding_shield(&self) -> bool {
        self.out.holding_shield
    }

    /// Predicted active weapon FOV (`clientdata.fov`).
    pub fn predicted_fov(&self) -> f32 {
        self.out.weapon.fov
    }

    /// Predicted clip count of the active weapon.
    pub fn predicted_clip(&self) -> i32 {
        u8::try_from(self.out.weapon.weapon_id)
            .ok()
            .and_then(|id| self.out.weapon.weapons.get(&id))
            .map_or(0, |w| w.m_i_clip)
    }

    /// The origin predicted after the last command of packet `sequence`, while the ring holds it.
    pub fn predicted_origin(&self, sequence: u32) -> Option<Vec3> {
        match self.predicted_origins[(sequence & UPDATE_MASK) as usize] {
            Some((s, o)) if s == sequence => Some(o),
            _ => None,
        }
    }

    /// Test hook: keeps the `(msec, impulse)` of every step the movement runs.
    #[doc(hidden)]
    pub fn record_steps(&mut self, on: bool) {
        self.record_steps = on;
        self.steps.clear();
    }

    /// The steps recorded since [`record_steps`](Self::record_steps) or the last call.
    #[doc(hidden)]
    pub fn take_steps(&mut self) -> Vec<(u8, u8)> {
        std::mem::take(&mut self.steps)
    }

    fn is_predicted(&self, intermission: bool) -> bool {
        !self.settings.nopred && !intermission
    }

    /// One frame: checks the server's newest frame against the earlier prediction, then replays
    /// the unacknowledged commands on it (`CL_CheckPredictionError`, `CL_PredictMovement`).
    pub fn update(&mut self, inp: &PredictInputs<'_>) {
        let raw_dt = inp.now - self.last_update;
        let frametime = if raw_dt.is_finite() {
            raw_dt.clamp(0.0, 0.25) as f32
        } else {
            0.0
        };
        self.last_update = inp.now;
        self.now = inp.now;
        let Some(frame) = inp.frame else {
            self.out.predicted = false;
            return;
        };
        if frame.sequence != self.last_frame {
            self.on_new_frame(inp, frame);
            self.last_frame = frame.sequence;
        }
        self.predict(inp, frame, frametime);
    }

    /// A newer frame arrived: the error check of the old prediction against it, what survives
    /// (`TxferPredictionData`), and the commands it acknowledged are done with.
    fn on_new_frame(&mut self, inp: &PredictInputs<'_>, frame: &Frame) {
        let ack = inp.incoming_acknowledged;
        let predicted_state = match &self.predicted[(ack & UPDATE_MASK) as usize] {
            Some((s, st)) if *s == ack => Some(st.clone()),
            _ => None,
        };
        if let (Some(cd), true) = (frame.clientdata.as_ref(), self.is_predicted(inp.intermission))
        {
            self.check_prediction_error(ack, cd.origin, inp.remote_is_local);
        }
        // `HUD_TxferPredictionData`: what the server never sends is kept from the prediction.
        self.carried = predicted_state.as_ref().map(LocalState::carried);

        while let Some(front) = self.cmds.front() {
            match front.sequence {
                Some(s) if s <= ack => {
                    self.last_acked_time = front.created_at;
                    self.cmds.pop_front();
                }
                _ => break,
            }
        }
    }

    /// `CL_CheckPredictionError`.
    fn check_prediction_error(&mut self, ack: u32, server_origin: Vec3, remote_is_local: bool) {
        let Some(predicted) = self.predicted_origin(ack) else {
            return;
        };
        let delta = server_origin - predicted;
        let dist = delta.length();
        let teleport = dist > MAX_PREDICTION_ERROR;
        if teleport {
            // A teleport or something, or a paused game.
            self.out.prediction_error = Vec3::ZERO;
        } else {
            self.predicted_origins[(ack & UPDATE_MASK) as usize] = Some((ack, server_origin));
            self.out.prediction_error = delta;
            // GoldSrc checks for single player; Xash3D for a local server.
            if dist > MIN_CORRECTION_DISTANCE && !remote_is_local {
                self.correction_time = self.settings.smoothtime;
            }
        }
        if self.checks.len() >= CHECK_LOG_CAPACITY {
            self.checks.pop_front();
        }
        self.checks.push_back(PredictionCheck {
            ack,
            error: dist,
            teleport,
        });
    }

    /// `CL_PredictMovement(repredicting = false)`.
    fn predict(&mut self, inp: &PredictInputs<'_>, frame: &Frame, frametime: f32) {
        let ack = inp.incoming_acknowledged;
        // "net hasn't delivered packets in a long time"
        if inp.outgoing_sequence.wrapping_sub(ack) >= UPDATE_MASK || inp.intermission {
            return;
        }
        let local = u16::from(inp.playernum) + 1;
        let Some(base) = LocalState::from_frame(frame, local, self.carried.as_ref()) else {
            self.out.predicted = false;
            return;
        };

        if !self.is_predicted(inp.intermission) || self.world.is_none() {
            // The server's word: `CL_PredictMovement` copies the frame's clientdata.
            self.set_from_server(&base);
            return;
        }

        let world = self.world.clone();
        let coll: &WorldCollision = world.as_deref().unwrap_or(&NO_COLLISION);
        let physents = build_physents(inp, frame, coll, world.is_some(), self.settings.solid_players);
        let ctx = RunCtx {
            coll,
            physents: &physents,
            player_index: i32::from(inp.playernum),
            multiplayer: inp.max_clients > 1,
            movevars: inp.movevars,
        };

        let mut time = frame.time;
        let mut from = base.clone();
        let mut prev = base.clone();
        let mut prev_time = self.last_acked_time;
        let mut to_time = self.last_acked_time;
        let mut ran = false;
        let mut lastground = self.out.lastground;
        for i in 0..self.cmds.len() {
            let (cmd, runfuncs, random_seed, created_at) = {
                let c = &self.cmds[i];
                (c.cmd.clone(), !c.processed, c.random_seed, c.created_at)
            };
            let to = self.run_usercmd(
                &ctx,
                &from,
                &cmd,
                runfuncs,
                random_seed,
                &mut time,
                &mut lastground,
            );
            if let Some(seq) = self.cmds[i].sequence {
                let slot = (seq & UPDATE_MASK) as usize;
                self.predicted_origins[slot] = Some((seq, to.origin));
                self.predicted[slot] = Some((seq, to.clone()));
            }
            self.cmds[i].processed = true;
            prev = std::mem::replace(&mut from, to);
            prev_time = to_time;
            to_time = created_at;
            ran = true;
        }
        let to = from;
        if !ran {
            // Nothing is in flight: the server's state stands.
            self.set_from_server(&base);
            return;
        }
        self.out.lastground = lastground;
        self.finish(inp, &prev, &to, prev_time, to_time, frametime);
    }

    /// `cl.simorg` and the rest from the server's frame.
    fn set_from_server(&mut self, s: &LocalState) {
        self.out.predicted = false;
        self.out.simorg = s.origin;
        self.out.simvel = s.velocity;
        self.out.punchangle = s.punchangle;
        self.out.viewheight = s.view_ofs;
        self.out.usehull = s.usehull;
        self.out.waterlevel = s.waterlevel;
        self.out.onground = if s.flags & FL_ONGROUND != 0 { s.onground } else { -1 };
        self.out.moving = false;
        self.out.teleported = false;
        self.out.viewmodel = s.weapon.viewmodel;
        self.out.weapon = s.weapon.clone();
        self.out.holding_shield = s.iuser[2] & PLAYER_HOLDING_SHIELD != 0;
        self.lastorigin = s.origin;
    }

    /// The end of `CL_PredictMovement`: a blend between the last two states by the time since
    /// the last-but-one command was made, the ground, and the smoothing of a correction.
    fn finish(
        &mut self,
        inp: &PredictInputs<'_>,
        from: &LocalState,
        to: &LocalState,
        from_time: f64,
        to_time: f64,
        frametime: f32,
    ) {
        let f = if to_time != from_time {
            ((inp.now - from_time) / (to_time - from_time)).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        let teleported = player_teleported(from, to, inp.max_velocity);
        self.out.predicted = true;
        self.out.teleported = teleported;
        if teleported {
            self.out.simvel = to.velocity;
            self.out.simorg = to.origin;
            self.out.punchangle = to.punchangle;
            self.out.viewheight = to.view_ofs;
        } else {
            self.out.simorg = from.origin.lerp(to.origin, f);
            self.out.simvel = from.velocity.lerp(to.velocity, f);
            self.out.punchangle = from.punchangle.lerp(to.punchangle, f);
            self.out.viewheight = if from.usehull == to.usehull {
                from.view_ofs.lerp(to.view_ofs, f)
            } else {
                to.view_ofs
            };
        }
        self.out.waterlevel = to.waterlevel;
        self.out.usehull = to.usehull;
        self.out.viewmodel = to.weapon.viewmodel;
        self.out.weapon = to.weapon.clone();
        self.out.holding_shield = to.iuser[2] & PLAYER_HOLDING_SHIELD != 0;

        if to.flags & FL_ONGROUND != 0 {
            self.out.onground = self.out.lastground;
            self.out.moving = false;
            let ground = u16::try_from(self.out.lastground)
                .ok()
                .and_then(|n| inp.entities.and_then(|e| e.get(n)));
            if let Some(ent) = ground {
                let dx = ent.curstate.origin.x - ent.prevstate.origin.x;
                let dy = ent.curstate.origin.y - ent.prevstate.origin.y;
                if (dx * dx + dy * dy).sqrt() > 0.0 {
                    self.correction_time = 0.0;
                    self.out.moving = true;
                }
            }
        } else {
            self.out.onground = -1;
            self.out.moving = false;
        }

        if self.correction_time > 0.0
            && !self.settings.nosmooth
            && self.settings.smoothtime.is_finite()
            && self.settings.smoothtime != 0.0
        {
            // Only decay the timer once per frame.
            self.correction_time -= if frametime.is_finite() {
                frametime.max(0.0)
            } else {
                0.0
            };
            // Make sure smoothtime is positive and bounded.
            let smooth = if !self.settings.smoothtime.is_finite() || self.settings.smoothtime <= 0.0 {
                0.1
            } else {
                self.settings.smoothtime.min(10.0)
            };
            self.correction_time = self.correction_time.clamp(0.0, smooth);
            // How far along the correction is, and how much of the error is left to make up.
            let frac = 1.0 - self.correction_time / smooth;
            let delta = (self.out.simorg - self.lastorigin) * frac;
            self.out.simorg = self.lastorigin + delta;
        }
        self.lastorigin = self.out.simorg;
    }

    /// `CL_RunUsercmd`: one command from `from`; a long one is run as halves.
    #[allow(clippy::too_many_arguments)]
    fn run_usercmd(
        &mut self,
        ctx: &RunCtx<'_>,
        from: &LocalState,
        cmd: &GoldSrcUserCmd,
        runfuncs: bool,
        random_seed: u32,
        time: &mut f64,
        lastground: &mut i32,
    ) -> LocalState {
        let mut state = from.clone();
        for (idx, step) in split_usercmd(cmd).into_iter().enumerate() {
            state = self.run_step(
                ctx,
                &state,
                &step,
                runfuncs && idx == 0,
                random_seed,
                time,
                lastground,
            );
        }
        state
    }

    /// One command of at most 50 ms: setup, `PM_Move`, finish, client weapon prediction (`cl_lw`),
    /// timer decay, and advancing time.
    #[allow(clippy::too_many_arguments)]
    fn run_step(
        &mut self,
        ctx: &RunCtx<'_>,
        from: &LocalState,
        cmd: &GoldSrcUserCmd,
        runfuncs: bool,
        random_seed: u32,
        time: &mut f64,
        lastground: &mut i32,
    ) -> LocalState {
        if self.record_steps {
            self.steps.push((cmd.msec, cmd.impulse));
        }
        let old_buttons = from.oldbuttons as u16;
        let mut to = from.clone();
        let mut pm = setup_pmove(ctx, from, cmd, runfuncs, *time);
        {
            let mut world = ClientPmWorld {
                coll: ctx.coll,
                physents: ctx.physents,
                rng: &mut self.rng,
                now: self.now,
                runfuncs,
                sounds: &mut self.sounds,
            };
            pm_move(&mut pm, &mut self.pm_shared, &mut world, false);
        }
        finish_pmove(&pm, &mut to);
        *lastground = match usize::try_from(pm.onground) {
            Ok(i) if i > 0 && i < ctx.physents.num_physents() => ctx
                .physents
                .physent(pm.onground)
                .map_or(pm.onground, |p| p.info),
            _ => pm.onground,
        };
        let on_ground = to.flags & FL_ONGROUND != 0;
        let ducking = to.flags & FL_DUCKING != 0;
        let mut weapon_extras = std::mem::take(&mut to.weapon_extras);
        predict_weapon_step(
            &mut to,
            &mut weapon_extras,
            cmd,
            old_buttons,
            random_seed,
            on_ground,
            ducking,
            runfuncs,
            self.cl_lw,
            &mut self.weapon_events,
            &mut self.sounds,
        );
        to.weapon_extras = weapon_extras;
        // `pfnPostRunCmd`: decay timers after `ItemPostFrame`.
        to.weapon.post_run_cmd(cmd.msec);
        *time += f64::from(cmd.msec) / 1000.0;
        to
    }
}

/// What a replay shares between its commands.
struct RunCtx<'a> {
    coll: &'a WorldCollision,
    physents: &'a PmPhysEnts,
    player_index: i32,
    multiplayer: bool,
    movevars: MoveVars,
}

/// `CL_PlayerTeleported`: the command moved the player further than `maxvelocity` allows in a
/// frame. Both lengths are `int` in the source (truncated).
fn player_teleported(from: &LocalState, to: &LocalState, max_velocity: f32) -> bool {
    let delta = to.origin - from.origin;
    let maxlen = (max_velocity * (1.0 / GAME_FPS)) as i32;
    let len = delta.length() as i32;
    len > maxlen
}

/// `CL_SetupPMove`: the movement's state from the player's.
fn setup_pmove(
    ctx: &RunCtx<'_>,
    from: &LocalState,
    cmd: &GoldSrcUserCmd,
    runfuncs: bool,
    time: f64,
) -> PlayerMove {
    PlayerMove {
        player_index: ctx.player_index,
        multiplayer: ctx.multiplayer,
        runfuncs,
        // `pmove->time = time * 1000.0f` (a float).
        time: (time * 1000.0) as f32,
        frametime: f32::from(cmd.msec) / 1000.0,
        origin: from.origin,
        angles: from.angles,
        oldangles: from.angles,
        velocity: from.velocity,
        basevelocity: from.basevelocity,
        view_ofs: from.view_ofs,
        movedir: Vec3::ZERO,
        fl_duck_time: from.fl_duck_time as f32,
        b_in_duck: from.b_in_duck,
        usehull: from.usehull,
        fl_time_step_sound: from.fl_time_step_sound,
        i_step_left: from.i_step_left,
        fl_fall_velocity: from.fl_fall_velocity,
        fl_swim_time: from.fl_swim_time as f32,
        punchangle: from.punchangle,
        effects: from.effects,
        flags: from.flags,
        gravity: from.gravity,
        friction: from.friction,
        oldbuttons: from.oldbuttons,
        waterjumptime: from.waterjumptime as f32,
        dead: from.health <= 0.0,
        deadflag: from.deadflag,
        spectator: 0,
        movetype: from.movetype,
        onground: from.onground,
        waterlevel: from.waterlevel,
        watertype: from.watertype,
        oldwaterlevel: from.waterlevel,
        maxspeed: ctx.movevars.maxspeed,
        clientmaxspeed: from.maxspeed,
        iuser1: from.iuser[0],
        iuser2: from.iuser[1],
        iuser3: from.iuser[2],
        iuser4: from.iuser[3],
        fuser1: from.fuser[0],
        fuser2: from.fuser[1],
        fuser3: from.fuser[2],
        fuser4: from.fuser[3],
        vuser1: from.vuser[0],
        vuser2: from.vuser[1],
        vuser3: from.vuser[2],
        vuser4: from.vuser[3],
        cmd: PmCmd {
            msec: cmd.msec,
            viewangles: cmd.viewangles,
            forwardmove: cmd.forwardmove,
            sidemove: cmd.sidemove,
            upmove: cmd.upmove,
            buttons: cmd.buttons,
            impulse: cmd.impulse,
        },
        movevars: ctx.movevars,
        server: false,
        ..PlayerMove::default()
    }
}

/// `CL_FinishPMove`. Quirks kept: `flDuckTime`, `flSwimTime` and `waterjumptime` are stored as
/// `int` (truncated); `oldbuttons` is the command's buttons.
fn finish_pmove(pm: &PlayerMove, to: &mut LocalState) {
    to.flags = pm.flags;
    to.b_in_duck = pm.b_in_duck;
    to.fl_time_step_sound = pm.fl_time_step_sound;
    to.fl_duck_time = pm.fl_duck_time as i32;
    to.fl_swim_time = pm.fl_swim_time as i32;
    to.waterjumptime = pm.waterjumptime as i32;
    to.watertype = pm.watertype;
    to.waterlevel = pm.waterlevel;
    to.maxspeed = pm.clientmaxspeed;
    to.deadflag = pm.deadflag;
    to.velocity = pm.velocity;
    to.view_ofs = pm.view_ofs;
    to.origin = pm.origin;
    to.angles = pm.angles;
    to.basevelocity = pm.basevelocity;
    to.punchangle = pm.punchangle;
    to.oldbuttons = i32::from(pm.cmd.buttons);
    to.friction = pm.friction;
    to.movetype = pm.movetype;
    to.onground = pm.onground;
    to.effects = pm.effects;
    to.usehull = pm.usehull;
    to.i_step_left = pm.i_step_left;
    to.fl_fall_velocity = pm.fl_fall_velocity;
    to.iuser = [pm.iuser1, pm.iuser2, pm.iuser3, pm.iuser4];
    to.fuser = [pm.fuser1, pm.fuser2, pm.fuser3, pm.fuser4];
    to.vuser = [pm.vuser1, pm.vuser2, pm.vuser3, pm.vuser4];
}

/// `CL_SetSolidEntities`, `CL_AddLinksToPmove` and `CL_SetSolidPlayers`: the world, then the
/// solid entities of the frame, then the other players where they are drawn. The kinds an
/// entity may be follow `SV_AddLinksToPM` of the server (`cs-engine`), so both sides collide with
/// the same things.
fn build_physents(
    inp: &PredictInputs<'_>,
    frame: &Frame,
    coll: &WorldCollision,
    has_world: bool,
    solid_players: bool,
) -> PmPhysEnts {
    let mut out = PmPhysEnts::default();
    out.push_physent(world_physent(has_world.then_some(coll)));
    let local = u16::from(inp.playernum) + 1;
    let max_clients = u16::from(inp.max_clients);

    for fe in &frame.entities {
        let state = &fe.state;
        let number = state.number;
        // Players are added below, at their drawn positions.
        if (1..=max_clients).contains(&number) {
            continue;
        }
        if state.modelindex == 0 {
            continue;
        }
        let Some(name) = inp.precache.model(state.modelindex) else {
            continue;
        };
        let Some(kind) = ModelKind::from_name(name) else {
            continue;
        };
        // An entity the local player owns (its own shots) is not in its way.
        if state.owner != 0 && state.owner == local {
            continue;
        }
        let skin = i32::from(state.skin);
        if state.solid == SOLID_TRIGGER || (state.solid == SOLID_NOT && skin >= CONTENTS_EMPTY) {
            continue;
        }
        if !matches!(state.solid, SOLID_BSP | SOLID_BBOX | SOLID_SLIDEBOX | SOLID_NOT) {
            continue;
        }
        // A dead body.
        if state.mins.z == 0.0 && state.maxs.z == 1.0 {
            continue;
        }
        // Can't collide with a zeroed hull.
        if state.mins == Vec3::ZERO && state.maxs == Vec3::ZERO {
            continue;
        }
        let brush = match (kind, name.strip_prefix('*').and_then(|n| n.parse::<usize>().ok())) {
            (ModelKind::Brush, Some(m)) => Some(m),
            (ModelKind::Brush, None) if state.modelindex == 1 => Some(0),
            _ => None,
        };
        let model = match brush {
            Some(m) if has_world && m < coll.model_count() => PhysModel::Brush(m),
            Some(_) | None if matches!(state.solid, SOLID_BSP | SOLID_NOT) => PhysModel::Other,
            _ => PhysModel::None,
        };
        // A brush model the map does not have has no hull: nothing to collide with.
        if kind == ModelKind::Brush && model == PhysModel::Other {
            continue;
        }
        let (mins, maxs) = match state.solid {
            SOLID_BSP | SOLID_NOT => (Vec3::ZERO, Vec3::ZERO),
            _ => (state.mins, state.maxs),
        };
        let (model_mins, model_maxs) = match model {
            PhysModel::Brush(m) => coll.model_bounds(m).unwrap_or((Vec3::ZERO, Vec3::ZERO)),
            _ => (Vec3::ZERO, Vec3::ZERO),
        };
        let pe = PhysEnt {
            view: PhysEntView {
                info: i32::from(number),
                player: 0,
                origin: state.origin,
                angles: state.angles,
                mins,
                maxs,
                solid: i32::from(state.solid),
                skin,
                rendermode: i32::from(state.rendermode),
                movetype: i32::from(state.movetype),
                is_brush_model: matches!(model, PhysModel::Brush(_)),
                model_mins,
                model_maxs,
            },
            model,
        };
        if state.solid == SOLID_NOT && skin == CONTENTS_LADDER {
            if out.num_moveents() < MAX_MOVEENTS {
                out.push_moveent(pe);
            }
        } else if out.num_physents() + usize::from(inp.max_clients) < MAX_PHYSENTS {
            // Slots are reserved for all the clients.
            out.push_physent(pe);
        }
    }

    for pose in inp.poses.iter().filter(|_| solid_players) {
        let Some(fe) = frame.entity(pose.number) else {
            continue;
        };
        let state = &fe.state;
        if state.modelindex == 0 || state.solid == SOLID_NOT {
            // Not present, or a dead body.
            continue;
        }
        if out.num_physents() >= MAX_PHYSENTS {
            break;
        }
        let hull = usize::from(state.usehull).min(PM_PLAYER_MINS.len() - 1);
        out.push_physent(PhysEnt {
            view: PhysEntView {
                info: i32::from(pose.number),
                player: i32::from(pose.number),
                origin: pose.origin,
                angles: pose.angles,
                mins: PM_PLAYER_MINS[hull],
                maxs: PM_PLAYER_MAXS[hull],
                solid: i32::from(state.solid),
                skin: i32::from(state.skin),
                rendermode: i32::from(state.rendermode),
                movetype: i32::from(state.movetype),
                is_brush_model: false,
                model_mins: Vec3::ZERO,
                model_maxs: Vec3::ZERO,
            },
            model: PhysModel::None,
        });
    }
    out
}

/// The world as the movement sees it: the physents of the frame and the callbacks.
struct ClientPmWorld<'a> {
    coll: &'a WorldCollision,
    physents: &'a PmPhysEnts,
    rng: &'a mut u32,
    now: f64,
    runfuncs: bool,
    sounds: &'a mut Vec<PredictedSound>,
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

/// A xorshift step: the client's own stream of `RandomLong` (the server's and the client's
/// streams are independent in the original too: only the variant of a footstep sound).
fn random_in(state: &mut u32, low: i32, high: i32) -> i32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    if high <= low {
        return low;
    }
    let span = (i64::from(high) - i64::from(low) + 1) as u64;
    (i64::from(low) + i64::from(x) % span as i64) as i32
}

impl PmWorld for ClientPmWorld<'_> {
    fn player_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        usehull: i32,
        trace_flags: i32,
        ignore_pe: i32,
    ) -> PmTrace {
        if !in_world(start) || !in_world(end) {
            return to_pm_trace(PmTraceResult::miss(if in_world(end) { end } else { Vec3::ZERO }));
        }
        to_pm_trace(physent::player_trace(
            self.coll,
            self.physents.physents_slice(),
            start,
            end,
            usehull,
            trace_flags,
            ignore_pe,
        ))
    }

    fn test_player_position(&mut self, pm_origin: Vec3, pos: Vec3, usehull: i32) -> (i32, PmTrace) {
        if !in_world(pm_origin) || !in_world(pos) {
            return (-1, to_pm_trace(PmTraceResult::miss(Vec3::ZERO)));
        }
        let (ent, trace) = physent::test_player_position(
            self.coll,
            self.physents.physents_slice(),
            pm_origin,
            pos,
            usehull,
        );
        (ent, to_pm_trace(trace))
    }

    fn point_contents(&mut self, p: Vec3) -> (i32, i32) {
        if !in_world(p) {
            return (CONTENTS_EMPTY, CONTENTS_EMPTY);
        }
        physent::point_contents(self.coll, self.physents.physents_slice(), p)
    }

    fn num_moveents(&self) -> usize {
        self.physents.num_moveents()
    }

    fn ladder_contains(&mut self, moveent: usize, origin: Vec3, usehull: i32) -> bool {
        if !in_world(origin) {
            return false;
        }
        self.physents
            .moveent_at(moveent)
            .is_some_and(|m| physent::ladder_contains(self.coll, m, origin, usehull))
    }

    fn moveent_bounds(&self, moveent: usize) -> (Vec3, Vec3) {
        self.physents
            .moveent(moveent)
            .map_or((Vec3::ZERO, Vec3::ZERO), |m| (m.model_mins, m.model_maxs))
    }

    fn trace_model(&mut self, moveent: usize, start: Vec3, end: Vec3) -> (f32, Vec3) {
        if !in_world(start) || !in_world(end) {
            return (1.0, Vec3::ZERO);
        }
        match self.physents.moveent_at(moveent) {
            Some(pe) => physent::trace_model(self.coll, pe, start, end),
            None => (1.0, Vec3::ZERO),
        }
    }

    /// Nothing (module doc: the collision data has no texture names).
    fn trace_texture(&mut self, _ground: i32, _start: Vec3, _end: Vec3) -> Option<String> {
        None
    }

    fn play_sound(
        &mut self,
        channel: i32,
        sample: &str,
        volume: f32,
        attenuation: f32,
        flags: i32,
        pitch: i32,
    ) {
        // `PM_PlaySound` plays through the world even when `runfuncs` is off in places (the
        // water code); a replayed command must not play it again.
        if self.runfuncs {
            if self.sounds.len() >= MAX_PENDING_SOUNDS {
                self.sounds.remove(0);
            }
            self.sounds.push(PredictedSound {
                channel,
                sample: sample.to_string(),
                volume,
                attenuation,
                flags,
                pitch,
            });
        }
    }

    fn random_long(&mut self, low: i32, high: i32) -> i32 {
        random_in(self.rng, low, high)
    }

    fn sys_float_time(&mut self) -> f64 {
        self.now
    }

    fn physent_by_info(&self, info: i32) -> Option<(Vec3, Vec3)> {
        (0..self.physents.num_physents())
            .filter_map(|i| self.physents.physent(i as i32))
            .find(|pe| pe.info == info)
            .map(|pe| (pe.origin, pe.angles))
    }

    fn physent_is_player(&self, ent: i32) -> bool {
        self.physents.physent(ent).is_some_and(|pe| pe.player != 0)
    }

    fn physent_has_model(&self, ent: i32) -> bool {
        self.physents.physent(ent).is_some_and(|pe| pe.is_brush_model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(msec: u8) -> GoldSrcUserCmd {
        GoldSrcUserCmd {
            msec,
            impulse: 7,
            weaponselect: 3,
            ..GoldSrcUserCmd::default()
        }
    }

    #[test]
    fn a_short_command_is_not_split() {
        let steps = split_usercmd(&cmd(50));
        assert_eq!(steps.len(), 1);
        assert_eq!((steps[0].msec, steps[0].impulse), (50, 7));
    }

    #[test]
    fn a_long_command_is_halved_and_only_the_first_half_keeps_the_impulse() {
        let steps = split_usercmd(&cmd(100));
        let got: Vec<_> = steps.iter().map(|c| (c.msec, c.impulse, c.weaponselect)).collect();
        assert_eq!(got, vec![(50, 7, 3), (50, 0, 0)]);
        // 255 is halved three times, an odd millisecond is lost at every level.
        let steps = split_usercmd(&cmd(255));
        assert!(steps.iter().all(|c| c.msec == 31));
        assert_eq!(steps.len(), 8);
        assert_eq!(steps.iter().filter(|c| c.impulse != 0).count(), 1);
    }

    #[test]
    fn weapon_timers_run_down_to_their_floors() {
        let mut w = WeaponState {
            next_attack: 0.015,
            weapon_id: 7,
            ..WeaponState::default()
        };
        w.weapons.insert(
            7,
            GoldSrcWeaponData {
                m_fl_next_primary_attack: 0.5,
                m_fl_next_secondary_attack: 0.004,
                m_fl_time_weapon_idle: -0.0005,
                m_fl_next_reload: 0.0,
                m_f_next_aim_bonus: -0.99,
                ..GoldSrcWeaponData::default()
            },
        );
        w.post_run_cmd(10);
        assert!((w.next_attack - 0.005).abs() < 1e-6);
        assert!(!w.can_attack());
        w.post_run_cmd(10);
        w.post_run_cmd(10);
        // Floors: -0.001 for the player's timer.
        assert_eq!(w.next_attack, -0.001);
        let d = &w.weapons[&7];
        assert!((d.m_fl_next_primary_attack - 0.47).abs() < 1e-5);
        assert_eq!(d.m_fl_next_secondary_attack, -0.001);
        assert_eq!(d.m_fl_time_weapon_idle, -0.001);
        assert_eq!(d.m_f_next_aim_bonus, -1.0);
    }

    #[test]
    fn the_teleport_test_uses_whole_units() {
        let from = LocalState::default();
        let at = |x: f32| LocalState {
            origin: Vec3::new(x, 0.0, 0.0),
            ..LocalState::default()
        };
        // maxvelocity 2000 at 20 fps: 100 units.
        assert!(!player_teleported(&from, &at(100.9), 2000.0));
        assert!(player_teleported(&from, &at(101.0), 2000.0));
    }

    #[test]
    fn random_stays_in_range() {
        let mut s = 1;
        for _ in 0..1000 {
            let v = random_in(&mut s, 2, 5);
            assert!((2..=5).contains(&v));
        }
    }

    #[test]
    fn finish_uses_to_state_when_timestamps_match_and_survives_non_finite_smooth() {
        let mut p = Predictor::new();
        p.lastorigin = Vec3::ZERO;
        p.correction_time = 0.05;
        p.settings.smoothtime = f32::INFINITY;
        let from = LocalState {
            origin: Vec3::new(10.0, 0.0, 0.0),
            ..LocalState::default()
        };
        let to = LocalState {
            origin: Vec3::new(20.0, 0.0, 0.0),
            ..LocalState::default()
        };
        let precache = PrecacheTables::default();
        let inp = PredictInputs {
            now: 5.0,
            incoming_acknowledged: 1,
            outgoing_sequence: 2,
            frame: None,
            playernum: 0,
            max_clients: 16,
            precache: &precache,
            movevars: MoveVars::default(),
            max_velocity: 2000.0,
            poses: &[],
            entities: None,
            remote_is_local: false,
            intermission: false,
        };
        p.finish(&inp, &from, &to, 5.0, 5.0, f32::NAN);
        assert_eq!(p.prediction().simorg, Vec3::new(20.0, 0.0, 0.0));
        assert!(p.prediction().simorg.is_finite());
    }
}
