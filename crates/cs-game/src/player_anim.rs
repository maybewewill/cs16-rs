//! The animation of a player's third-person model: `CBasePlayer::SetAnimation`, the gait
//! (`StudioProcessGait` and what it calls: the pitch and yaw blends of the model's bones) and
//! `StudioFrameAdvance`, with the sequence lookups of `animation.cpp` they need. Ported from
//! ReGameDLL_CS `player.cpp`, `animation.cpp` and `animating.cpp`, read 2026-11-22, the vanilla
//! build (no `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`, `BUILD_LATEST`; `real_t` is `float`).
//!
//! The server never draws the model; what the animation decides is carried in the entity state
//! (`sequence`, `gaitsequence`, `frame`, `framerate`, `animtime`, `effects`, `blending`) for the
//! clients, and in the pose the engine traces a bullet against (`SV_StudioPlayerTracePose`,
//! which takes the pitch from `angles`, so the `blending` the game keeps matters to the clients
//! only).
//!
//! The model is read from the engine (`EngineApi::studio_model`, the `GET_MODEL_PTR` of the
//! original). Without a model (none loaded: the engine's model loader is not set, or the file is
//! missing) the lookups answer as the C ones do for a null `studiohdr_t`: `LookupSequence` and
//! `LookupActivity` 0, `GetSequenceInfo` leaves the rates alone, `GetSequenceFlags` 0.
//!
//! Not ported and named: the bot events (`TheBots->OnEvent`), the career tasks, and the
//! `m_flDeathThrowTime`/`m_iThrowDirection` consumers (`PlayerDeathThink` of the corpse's throw;
//! [`AnimState`] keeps the values `SetAnimation` writes).

use cs_assets::mdl::StudioPoseData;
use cs_engine::EngineApi;
use cs_engine::consts::{FL_CONVEYOR, FL_DUCKING, FL_FROZEN, FL_ONGROUND, FL_WATERJUMP};
use cs_sim::pm::consts::{IN_DUCK, IN_JUMP};
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use crate::GameState;
use crate::player::{
    ATTN_NORM, CHAN_VOICE, HITGROUP_CHEST, HITGROUP_GENERIC, HITGROUP_HEAD, HITGROUP_LEFTARM,
    HITGROUP_RIGHTARM, HITGROUP_SHIELD, HITGROUP_STOMACH, PITCH_NORM,
};
use crate::random_in;
use crate::weapons::core::PlayerAnim;

/// `Activity_s` (`activity.h`), the values `SetAnimation` and the gait use.
pub(crate) const ACT_INVALID: i32 = -1;
pub(crate) const ACT_IDLE: i32 = 1;
pub(crate) const ACT_WALK: i32 = 3;
pub(crate) const ACT_RUN: i32 = 4;
pub(crate) const ACT_FLY: i32 = 5;
pub(crate) const ACT_SWIM: i32 = 6;
pub(crate) const ACT_HOP: i32 = 7;
pub(crate) const ACT_LEAP: i32 = 8;
pub(crate) const ACT_CROUCH: i32 = 17;
pub(crate) const ACT_CROUCHIDLE: i32 = 18;
pub(crate) const ACT_RANGE_ATTACK1: i32 = 28;
pub(crate) const ACT_RANGE_ATTACK2: i32 = 29;
pub(crate) const ACT_RELOAD: i32 = 32;
pub(crate) const ACT_DIESIMPLE: i32 = 36;
pub(crate) const ACT_DIEBACKWARD: i32 = 37;
pub(crate) const ACT_DIEFORWARD: i32 = 38;
pub(crate) const ACT_HOVER: i32 = 50;
pub(crate) const ACT_DIE_HEADSHOT: i32 = 66;
pub(crate) const ACT_DIE_CHESTSHOT: i32 = 67;
pub(crate) const ACT_DIE_GUTSHOT: i32 = 68;
pub(crate) const ACT_DIE_BACKSHOT: i32 = 69;
pub(crate) const ACT_FLINCH: i32 = 77;
pub(crate) const ACT_LARGE_FLINCH: i32 = 78;
pub(crate) const ACT_HOLDBOMB: i32 = 79;

/// `THROW_*` (`player.h`): where a corpse is thrown.
const THROW_NONE: i32 = 0;
const THROW_FORWARD: i32 = 1;
const THROW_BACKWARD: i32 = 2;
const THROW_HITVEL: i32 = 3;
const THROW_BOMB: i32 = 4;
const THROW_GRENADE: i32 = 5;
const THROW_HITVEL_MINUS_AIRVEL: i32 = 6;

/// `STUDIO_LOOPING` (`studio.h`): a sequence flag.
const STUDIO_LOOPING: i32 = 0x0001;
/// `EF_NOINTERP` (`const.h`): the client does not interpolate the entity's next update.
const EF_NOINTERP: u32 = 32;

/// The animation state of one player: the `CBasePlayer` and `CBaseAnimating` members the code
/// of this module reads and writes.
#[derive(Debug, Clone)]
pub(crate) struct AnimState {
    /// `m_Activity`: what the model is doing now (0 is `ACT_RESET`).
    pub(crate) activity: i32,
    /// `m_IdealActivity`: what `SetAnimation` last asked for.
    pub(crate) ideal_activity: i32,
    /// `m_flLastFired`: server time of the last shot's animation; -15 from a spawn.
    pub(crate) last_fired: f32,
    /// `m_flFlinchTime`: nothing in the sources sets it, it stays 0.
    pub(crate) flinch_time: f32,
    /// `m_flFrameRate`: frames per second of the sequence, in the units of `pev->frame` (256 a
    /// loop).
    pub(crate) frame_rate: f32,
    /// `m_flGroundSpeed`.
    pub(crate) ground_speed: f32,
    /// `m_flLastEventCheck`.
    pub(crate) last_event_check: f32,
    /// `m_fSequenceFinished`.
    pub(crate) sequence_finished: bool,
    /// `m_fSequenceLoops`.
    pub(crate) sequence_loops: bool,
    /// `m_flDeathThrowTime`.
    pub(crate) death_throw_time: f32,
    /// `m_iThrowDirection` (`THROW_*`).
    pub(crate) throw_direction: i32,
    /// `m_bHeadshotKilled`.
    pub(crate) headshot_killed: bool,
    /// `m_bKilledByBomb`: set by the bomb's explosion (phase 2E).
    pub(crate) killed_by_bomb: bool,
    /// `m_bKilledByGrenade`: set by the grenade's explosion.
    pub(crate) killed_by_grenade: bool,
    /// `m_iGaitsequence`: `pev->gaitsequence` at the end of the last `PostThink`.
    pub(crate) gait_sequence: i32,
    /// `m_flGaitframe`.
    pub(crate) gait_frame: f32,
    /// `m_flGaityaw`.
    pub(crate) gait_yaw: f32,
    /// `m_flGaitMovement`.
    pub(crate) gait_movement: f32,
    /// `m_prevgaitorigin`.
    pub(crate) prev_gait_origin: Vec3,
    /// `m_flYawModifier`.
    pub(crate) yaw_modifier: f32,
    /// `m_flPitch`: the pitch blend (`pev->blending[1]`).
    pub(crate) pitch: f32,
    /// `m_flYaw`: the yaw blend (`pev->blending[0]`).
    pub(crate) yaw: f32,
}

impl AnimState {
    /// A player as the zeroed memory of the C++ object has it.
    pub(crate) fn new() -> Self {
        Self {
            activity: 0,
            ideal_activity: 0,
            last_fired: 0.0,
            flinch_time: 0.0,
            frame_rate: 0.0,
            ground_speed: 0.0,
            last_event_check: 0.0,
            sequence_finished: false,
            sequence_loops: false,
            death_throw_time: 0.0,
            throw_direction: THROW_NONE,
            headshot_killed: false,
            killed_by_bomb: false,
            killed_by_grenade: false,
            gait_sequence: 0,
            gait_frame: 0.0,
            gait_yaw: 0.0,
            gait_movement: 0.0,
            prev_gait_origin: Vec3::ZERO,
            yaw_modifier: 0.0,
            pitch: 0.0,
            yaw: 0.0,
        }
    }
}

/// `LookupSequence` (`animation.cpp`, read 2026-11-22): the sequence called `label` (case
/// blind), -1 when the model has none, 0 without a model.
pub(crate) fn lookup_sequence(model: Option<&StudioPoseData>, label: &str) -> i32 {
    let Some(model) = model else {
        return 0;
    };
    model
        .sequences
        .iter()
        .position(|s| s.label.eq_ignore_ascii_case(label))
        .map_or(ACT_INVALID, |i| i as i32)
}

/// `LookupActivity` (`animation.cpp`, read 2026-11-22): a sequence of `activity`, picked at
/// random by the sequences' weights (`RANDOM_LONG(0, total - 1)`), or among all of them when the
/// weights add up to 0; -1 when none has it, 0 without a model. `rng` is the game's generator
/// (the engine's `RANDOM_LONG` is not reproduced).
pub(crate) fn lookup_activity(model: Option<&StudioPoseData>, activity: i32, rng: &mut u32) -> i32 {
    let Some(model) = model else {
        return 0;
    };
    let mut weight_total = 0i32;
    let mut count = 0i32;
    for s in model.sequences.iter().filter(|s| s.activity == activity) {
        weight_total = weight_total.wrapping_add(s.actweight);
        count += 1;
    }
    if count == 0 {
        return ACT_INVALID;
    }
    if weight_total != 0 {
        let which = random_in(rng, 0, weight_total.wrapping_sub(1));
        let mut weight = 0i32;
        for (i, s) in model.sequences.iter().enumerate() {
            if s.activity == activity {
                weight = weight.wrapping_add(s.actweight);
                if weight > which {
                    return i as i32;
                }
            }
        }
    } else {
        let mut select = random_in(rng, 0, count - 1);
        for (i, s) in model.sequences.iter().enumerate() {
            if s.activity == activity {
                if select == 0 {
                    return i as i32;
                }
                select -= 1;
            }
        }
    }
    ACT_INVALID
}

/// `GetSequenceInfo` (`animation.cpp`, read 2026-11-22): the frame rate and the ground speed of
/// sequence `sequence`. A sequence outside the model gives 0 and 0, a sequence of one frame
/// 256 and 0; without a model the rates stay as they are (`None`). An `int` index below 0 reads
/// before the table in C; here it is taken as outside it (assumption).
fn get_sequence_info(model: Option<&StudioPoseData>, sequence: i32) -> Option<(f32, f32)> {
    let model = model?;
    let Some(seq) = usize::try_from(sequence).ok().and_then(|i| model.sequences.get(i)) else {
        return Some((0.0, 0.0));
    };
    if seq.numframes <= 1 {
        return Some((256.0, 0.0));
    }
    let frames = (seq.numframes - 1) as f32;
    let rate = seq.fps * 256.0 / frames;
    let [x, y, z] = seq.linearmovement;
    let speed = (x * x + y * y + z * z).sqrt();
    Some((rate, speed * seq.fps / frames))
}

/// `GetSequenceFlags` (`animation.cpp`, read 2026-11-22).
fn get_sequence_flags(model: Option<&StudioPoseData>, sequence: i32) -> i32 {
    model
        .and_then(|m| usize::try_from(sequence).ok().and_then(|i| m.sequences.get(i)))
        .map_or(0, |s| s.flags)
}

/// The variables of `pev` the animation reads and writes, taken from the edict for the length of
/// one call and written back when it is done.
struct Pev {
    sequence: i32,
    gaitsequence: i32,
    frame: f32,
    animtime: f32,
    framerate: f32,
    effects: u32,
    flags: u32,
    velocity: Vec3,
    waterlevel: i32,
    health: f32,
    angles: Vec3,
}

impl Pev {
    fn read(v: &cs_engine::EntVars) -> Self {
        Self {
            sequence: i32::from(v.sequence),
            // The `int` of C: a -1 (`ACT_INVALID`) stored there comes back as -1.
            gaitsequence: i32::from(v.gaitsequence as i16),
            frame: v.frame,
            animtime: v.animtime,
            framerate: v.framerate,
            effects: v.effects,
            flags: v.flags,
            velocity: v.velocity,
            waterlevel: v.waterlevel,
            health: v.health,
            angles: v.angles,
        }
    }

    fn write(&self, v: &mut cs_engine::EntVars) {
        v.sequence = self.sequence as u16;
        v.gaitsequence = self.gaitsequence as u16;
        v.frame = self.frame;
        v.animtime = self.animtime;
        v.framerate = self.framerate;
        v.effects = self.effects;
    }
}

/// What `SetAnimation` works on: the model, the clock, the game's generator and the player's
/// state besides the edict's.
struct Anim<'a> {
    model: Option<&'a StudioPoseData>,
    time: f32,
    rng: &'a mut u32,
    pev: Pev,
    st: &'a mut AnimState,
    ext: &'a str,
    has_shield: bool,
    last_hit_group: i32,
    high_damage: bool,
    blast_vector: Vec3,
    /// The sample of the `DeathSound` the call made, as the `RANDOM_LONG(1, 4)` picked it.
    death_sound: Option<i32>,
}

/// `AnimationType` of `GetAnimDesired`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationType {
    Normal,
    Crouch,
}

impl Anim<'_> {
    fn random_long(&mut self, lo: i32, hi: i32) -> i32 {
        random_in(self.rng, lo, hi)
    }

    fn lookup_activity(&mut self, activity: i32) -> i32 {
        lookup_activity(self.model, activity, self.rng)
    }

    fn lookup_sequence(&self, label: &str) -> i32 {
        lookup_sequence(self.model, label)
    }

    /// `CBasePlayer::GetAnimDesired`: `crouch_` (when asked for and ducking) or `ref_`, the
    /// name and the weapon's animation set, into a 128-byte buffer.
    fn get_anim_desired(&self, name: &str, kind: AnimationType) -> i32 {
        let prefix = if kind == AnimationType::Crouch && self.pev.flags & FL_DUCKING != 0 {
            "crouch_"
        } else {
            "ref_"
        };
        let mut label = format!("{prefix}{name}_{}", self.ext);
        label.truncate(127);
        self.lookup_sequence(&label)
    }

    /// `CBaseAnimating::ResetSequenceInfo`.
    fn reset_sequence_info(&mut self) {
        if let Some((rate, speed)) = get_sequence_info(self.model, self.pev.sequence) {
            self.st.frame_rate = rate;
            self.st.ground_speed = speed;
        }
        self.st.sequence_loops =
            get_sequence_flags(self.model, self.pev.sequence) & STUDIO_LOOPING != 0;
        self.pev.animtime = self.time;
        self.pev.framerate = 1.0;
        self.st.sequence_finished = false;
        self.st.last_event_check = self.time;
    }

    /// `DeathSound` as `SetAnimation(PLAYER_DIE)` calls it (vanilla): `RANDOM_LONG(1, 4)` now,
    /// the sound is played by the caller.
    fn death_sound(&mut self) {
        self.death_sound = Some(self.random_long(1, 4));
    }

    /// `CBasePlayer::SetAnimation`, from `m_flFlinchTime`'s test to the commit of the sequence,
    /// in the order of the source. Every `return` of the C function is a `return` here.
    ///
    /// QUIRKS kept: the vanilla `PLAYER_DIE` plays a death sound here as well as the one
    /// `Killed` plays at its end; the flinch case sets `m_Activity` before it tests the shield;
    /// `LookupActivity` draws a random number on every call, so `hopSeq`/`leapSeq` cost two
    /// draws per call whatever the animation; `m_flFlinchTime` is never set (0), so the test
    /// that would drop an animation during a flinch never fires.
    fn run(&mut self, mut anim: PlayerAnim) {
        let time = self.time;
        if matches!(anim, PlayerAnim::Flinch | PlayerAnim::LargeFlinch) && self.has_shield {
            return;
        }
        if !matches!(anim, PlayerAnim::Flinch | PlayerAnim::LargeFlinch)
            && self.st.flinch_time > time
            && self.pev.health > 0.0
        {
            return;
        }

        let mut speed = Vec3::new(self.pev.velocity.x, self.pev.velocity.y, 0.0).length();
        if self.pev.flags & FL_FROZEN != 0 {
            speed = 0.0;
            anim = PlayerAnim::Idle;
        }

        let hop_seq = self.lookup_activity(ACT_HOP);
        let leap_seq = self.lookup_activity(ACT_LEAP);

        match anim {
            PlayerAnim::Jump => {
                if matches!(self.st.activity, ACT_SWIM | ACT_DIESIMPLE | ACT_HOVER) {
                    self.st.ideal_activity = self.st.activity;
                } else {
                    self.st.ideal_activity = ACT_HOP;
                }
            }
            PlayerAnim::SuperJump => {
                if matches!(self.st.activity, ACT_SWIM | ACT_DIESIMPLE | ACT_HOVER) {
                    self.st.ideal_activity = self.st.activity;
                } else {
                    self.st.ideal_activity = ACT_LEAP;
                }
            }
            PlayerAnim::Die => {
                self.st.ideal_activity = ACT_DIESIMPLE;
                self.death_sound();
            }
            PlayerAnim::Attack1 => {
                if matches!(self.st.activity, ACT_SWIM | ACT_DIESIMPLE | ACT_HOVER) {
                    self.st.ideal_activity = self.st.activity;
                } else {
                    self.st.ideal_activity = ACT_RANGE_ATTACK1;
                }
            }
            PlayerAnim::Attack2 => {
                if matches!(self.st.activity, ACT_SWIM | ACT_DIESIMPLE | ACT_HOVER) {
                    self.st.ideal_activity = self.st.activity;
                } else {
                    self.st.ideal_activity = ACT_RANGE_ATTACK2;
                }
            }
            PlayerAnim::Reload => {
                if matches!(self.st.activity, ACT_SWIM | ACT_DIESIMPLE | ACT_HOVER) {
                    self.st.ideal_activity = self.st.activity;
                } else {
                    self.st.ideal_activity = ACT_RELOAD;
                }
            }
            PlayerAnim::Idle | PlayerAnim::Walk => {
                if self.pev.flags & FL_ONGROUND != 0
                    || (self.st.activity != ACT_HOP && self.st.activity != ACT_LEAP)
                {
                    if self.pev.waterlevel <= 1 {
                        self.st.ideal_activity = ACT_WALK;
                    } else if speed == 0.0 {
                        self.st.ideal_activity = ACT_HOVER;
                    } else {
                        self.st.ideal_activity = ACT_SWIM;
                    }
                } else {
                    self.st.ideal_activity = self.st.activity;
                }
            }
            PlayerAnim::HoldBomb => self.st.ideal_activity = ACT_HOLDBOMB,
            PlayerAnim::Flinch => self.st.ideal_activity = ACT_FLINCH,
            PlayerAnim::LargeFlinch => self.st.ideal_activity = ACT_LARGE_FLINCH,
        }

        let mut anim_desired = 0i32;
        match self.st.ideal_activity {
            ACT_HOP | ACT_LEAP => {
                if self.st.activity == self.st.ideal_activity {
                    return;
                }
                let ref_anim = match self.st.activity {
                    ACT_RANGE_ATTACK1 => "shoot",
                    ACT_RANGE_ATTACK2 => "shoot2",
                    ACT_RELOAD => "reload",
                    _ => "aim",
                };
                anim_desired = self.get_anim_desired(ref_anim, AnimationType::Normal);
                if anim_desired == -1 {
                    anim_desired = 0;
                }
                if self.pev.sequence != anim_desired || !self.st.sequence_loops {
                    self.pev.frame = 0.0;
                }
                if !self.st.sequence_loops {
                    self.pev.effects |= EF_NOINTERP;
                }
                self.pev.gaitsequence = if self.st.ideal_activity == ACT_LEAP {
                    self.lookup_activity(ACT_LEAP)
                } else {
                    self.lookup_activity(ACT_HOP)
                };
                self.st.activity = self.st.ideal_activity;
            }
            ACT_RANGE_ATTACK1 => {
                self.st.last_fired = time;
                anim_desired = self.get_anim_desired("shoot", AnimationType::Crouch);
                if anim_desired == -1 {
                    anim_desired = 0;
                }
                self.pev.sequence = anim_desired;
                self.pev.frame = 0.0;
                self.reset_sequence_info();
                self.st.activity = self.st.ideal_activity;
            }
            ACT_RANGE_ATTACK2 => {
                self.st.last_fired = time;
                anim_desired = self.get_anim_desired("shoot2", AnimationType::Crouch);
                if anim_desired == -1 {
                    anim_desired = 0;
                }
                self.pev.sequence = anim_desired;
                self.pev.frame = 0.0;
                self.reset_sequence_info();
                self.st.activity = self.st.ideal_activity;
            }
            ACT_RELOAD => {
                anim_desired = self.get_anim_desired("reload", AnimationType::Crouch);
                if anim_desired == -1 {
                    anim_desired = 0;
                }
                if self.pev.sequence != anim_desired || !self.st.sequence_loops {
                    self.pev.frame = 0.0;
                }
                if !self.st.sequence_loops {
                    self.pev.effects |= EF_NOINTERP;
                }
                self.st.activity = self.st.ideal_activity;
            }
            ACT_HOLDBOMB => {
                anim_desired = self.get_anim_desired("aim", AnimationType::Crouch);
                if anim_desired == -1 {
                    anim_desired = 0;
                }
                self.st.activity = self.st.ideal_activity;
            }
            ACT_WALK => {
                let a = self.st.activity;
                let finished = self.st.sequence_finished;
                if (a != ACT_RANGE_ATTACK1 || finished)
                    && (a != ACT_RANGE_ATTACK2 || finished)
                    && (a != ACT_FLINCH || finished)
                    && (a != ACT_LARGE_FLINCH || finished)
                    && (a != ACT_RELOAD || finished)
                {
                    // `m_flLastFired + 4.0 >= gpGlobals->time`: a double sum.
                    if speed <= 135.0 || f64::from(self.st.last_fired) + 4.0 >= f64::from(time) {
                        anim_desired = self.get_anim_desired("aim", AnimationType::Crouch);
                        if anim_desired == -1 {
                            anim_desired = 0;
                        }
                        self.st.activity = ACT_WALK;
                    } else {
                        anim_desired = self.get_anim_desired("run", AnimationType::Normal);
                        if anim_desired == -1 {
                            anim_desired = self.get_anim_desired("aim", AnimationType::Crouch);
                            if anim_desired == -1 {
                                anim_desired = 0;
                            }
                            self.st.activity = ACT_RUN;
                            self.pev.gaitsequence = self.lookup_activity(ACT_RUN);
                        } else {
                            self.st.activity = ACT_RUN;
                            self.pev.gaitsequence = anim_desired;
                        }
                    }
                } else {
                    anim_desired = self.pev.sequence;
                }
                self.pev.gaitsequence = if speed > 135.0 {
                    self.lookup_activity(ACT_RUN)
                } else {
                    self.lookup_activity(ACT_WALK)
                };
            }
            ACT_FLINCH | ACT_LARGE_FLINCH => {
                self.st.activity = self.st.ideal_activity;
                // "TODO: why? this condition was checked!" (vanilla).
                if matches!(anim, PlayerAnim::Flinch | PlayerAnim::LargeFlinch) && self.has_shield {
                    return;
                }
                anim_desired = match self.last_hit_group {
                    HITGROUP_GENERIC => {
                        if self.random_long(0, 1) != 0 {
                            self.lookup_sequence("head_flinch")
                        } else {
                            self.lookup_sequence("gut_flinch")
                        }
                    }
                    HITGROUP_HEAD | HITGROUP_CHEST => self.lookup_sequence("head_flinch"),
                    HITGROUP_SHIELD => 0,
                    _ => self.lookup_sequence("gut_flinch"),
                };
                if anim_desired == -1 {
                    anim_desired = 0;
                }
            }
            ACT_DIESIMPLE => {
                if self.st.activity == self.st.ideal_activity {
                    return;
                }
                self.st.activity = self.st.ideal_activity;
                self.st.death_throw_time = 0.0;
                self.st.throw_direction = THROW_NONE;
                self.die_animation(&mut anim_desired);
                return;
            }
            _ => {
                if self.st.activity == self.st.ideal_activity {
                    return;
                }
                self.st.activity = self.st.ideal_activity;
                anim_desired = self.lookup_activity(self.st.ideal_activity);
                if self.pev.sequence != anim_desired {
                    self.pev.gaitsequence = 0;
                    self.pev.sequence = anim_desired;
                    self.pev.frame = 0.0;
                    self.reset_sequence_info();
                }
                return;
            }
        }

        if self.pev.gaitsequence != hop_seq && self.pev.gaitsequence != leap_seq {
            if self.pev.flags & FL_DUCKING != 0 {
                self.pev.gaitsequence = if speed != 0.0 {
                    self.lookup_activity(ACT_CROUCH)
                } else {
                    self.lookup_activity(ACT_CROUCHIDLE)
                };
            } else if speed > 135.0 {
                // `m_flLastFired + 4.0f < gpGlobals->time`: a float sum.
                if self.st.last_fired + 4.0 < time
                    && self.st.activity != ACT_FLINCH
                    && self.st.activity != ACT_LARGE_FLINCH
                {
                    anim_desired = self.get_anim_desired("run", AnimationType::Normal);
                    if anim_desired == -1 {
                        anim_desired = self.get_anim_desired("aim", AnimationType::Crouch);
                    } else {
                        self.pev.gaitsequence = anim_desired;
                    }
                    self.st.activity = ACT_RUN;
                }
                self.pev.gaitsequence = self.lookup_activity(ACT_RUN);
            } else if speed > 0.0 {
                self.pev.gaitsequence = self.lookup_activity(ACT_WALK);
            } else {
                self.pev.gaitsequence = self.lookup_activity(ACT_IDLE);
            }
        }
        if self.pev.sequence != anim_desired {
            self.pev.sequence = anim_desired;
            self.pev.frame = 0.0;
            self.reset_sequence_info();
        }
    }

    /// The `ACT_DIESIMPLE` case of `SetAnimation` after `m_flDeathThrowTime` and
    /// `m_iThrowDirection` are cleared: the death sequence by the hit group (and a random pick
    /// for a generic one), a ducking player's `crouch_die`, and a blast's.
    fn die_animation(&mut self, anim_desired: &mut i32) {
        match self.last_hit_group {
            HITGROUP_GENERIC => match self.random_long(0, 8) {
                0 => {
                    *anim_desired = self.lookup_activity(ACT_DIE_HEADSHOT);
                    self.st.throw_direction = THROW_BACKWARD;
                }
                1 => *anim_desired = self.lookup_activity(ACT_DIE_GUTSHOT),
                2 => {
                    *anim_desired = self.lookup_activity(ACT_DIE_BACKSHOT);
                    self.st.throw_direction = THROW_HITVEL;
                }
                4 => {
                    *anim_desired = self.lookup_activity(ACT_DIEBACKWARD);
                    self.st.throw_direction = THROW_HITVEL;
                }
                5 => {
                    *anim_desired = self.lookup_activity(ACT_DIEFORWARD);
                    self.st.throw_direction = THROW_FORWARD;
                }
                6 => *anim_desired = self.lookup_activity(ACT_DIE_CHESTSHOT),
                7 => *anim_desired = self.lookup_activity(ACT_DIE_GUTSHOT),
                8 => *anim_desired = self.lookup_activity(ACT_DIE_HEADSHOT),
                _ => *anim_desired = self.lookup_activity(ACT_DIESIMPLE),
            },
            HITGROUP_HEAD => {
                let mut random = self.random_long(0, 8);
                self.st.headshot_killed = true;
                if self.high_damage {
                    random += 1;
                }
                self.st.throw_direction = match random {
                    1 | 2 => THROW_BACKWARD,
                    3 | 4 => THROW_FORWARD,
                    5 | 6 => THROW_HITVEL,
                    _ => THROW_NONE,
                };
                *anim_desired = self.lookup_activity(ACT_DIE_HEADSHOT);
            }
            HITGROUP_CHEST => *anim_desired = self.lookup_activity(ACT_DIE_CHESTSHOT),
            HITGROUP_STOMACH => *anim_desired = self.lookup_activity(ACT_DIE_GUTSHOT),
            HITGROUP_LEFTARM => *anim_desired = self.lookup_sequence("left"),
            HITGROUP_RIGHTARM => {
                self.st.throw_direction = if self.random_long(0, 1) != 0 {
                    THROW_HITVEL
                } else {
                    THROW_HITVEL_MINUS_AIRVEL
                };
                *anim_desired = self.lookup_sequence("right");
            }
            _ => *anim_desired = self.lookup_activity(ACT_DIESIMPLE),
        }

        if self.pev.flags & FL_DUCKING != 0 {
            *anim_desired = self.lookup_sequence("crouch_die");
            self.st.throw_direction = THROW_BACKWARD;
        } else if self.st.killed_by_bomb || self.st.killed_by_grenade {
            let (forward, _, _) = angle_vectors(self.pev.angles);
            if forward.dot(self.blast_vector) > 0.0 {
                *anim_desired = self.lookup_sequence("left");
            } else if self.random_long(0, 1) != 0 {
                *anim_desired = self.lookup_sequence("crouch_die");
            } else {
                *anim_desired = self.lookup_activity(ACT_DIE_HEADSHOT);
            }
            if self.st.killed_by_bomb {
                self.st.throw_direction = THROW_BOMB;
            } else if self.st.killed_by_grenade {
                self.st.throw_direction = THROW_GRENADE;
            }
        }

        if *anim_desired == -1 {
            *anim_desired = 0;
        }
        if self.pev.sequence != *anim_desired {
            self.pev.gaitsequence = 0;
            self.pev.sequence = *anim_desired;
            self.pev.frame = 0.0;
            self.reset_sequence_info();
        }
    }
}

/// `CBasePlayer::SetAnimation(anim)` for the player of `slot`: picks the sequence of the model's
/// animation set for what the player does and writes it, the frame and the gait into the
/// entity. Nothing happens for a player with no model.
pub(crate) fn set_animation(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, anim: PlayerAnim) {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    if v.modelindex == 0 {
        return;
    }
    let model = eng.studio_model(v.modelindex);
    let pev = Pev::read(v);
    let time = eng.time() as f32;
    let GameState { players, rng, .. } = g;
    let Some(p) = players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    let (has_shield, last_hit_group, high_damage, blast_vector) =
        (p.owns_shield, p.last_hit_group, p.high_damage, p.blast_vector);
    let ext = p.anim_extension.clone();
    let mut a = Anim {
        model: model.as_deref(),
        time,
        rng,
        pev,
        st: &mut p.anim,
        ext: &ext,
        has_shield,
        last_hit_group,
        high_damage,
        blast_vector,
        death_sound: None,
    };
    a.run(anim);
    let (pev, death_sound) = (a.pev, a.death_sound);
    if let Some(v) = eng.ent_mut(id) {
        pev.write(v);
    }
    if let Some(sample) = death_sound {
        play_death_sound(eng, id, sample);
    }
}

/// `CBasePlayer::Jump` (`player.cpp` 4500-4552, read 2026-11-22), called from `PreThink` of a
/// living player with `IN_JUMP` held: on the first command of the press, on the ground, out of
/// deep water and not in a water jump, `SetAnimation(PLAYER_JUMP)`, and a player standing on a
/// conveyor, a train or a vehicle takes its velocity along. (The long jump of a ducking player
/// is `m_fLongJump`, which only the `slj` physics key of a mod sets and `Spawn` clears, so
/// `PLAYER_SUPERJUMP` is never asked for here.)
pub(crate) fn jump(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    if v.flags & FL_WATERJUMP != 0 || v.waterlevel >= 2 {
        return;
    }
    // "If this isn't the first frame pressing the jump button, break out" (no pogo stick).
    let pressed = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| i32::from(p.buttons_pressed) & IN_JUMP != 0);
    if !pressed {
        return;
    }
    let Some(ground) = v.groundentity else {
        return;
    };
    if v.flags & FL_ONGROUND == 0 {
        return;
    }
    set_animation(g, eng, slot, PlayerAnim::Jump);
    let Some(ground_vars) = eng.ent(ground) else {
        return;
    };
    // "If you're standing on a conveyor, add its velocity to yours (for momentum)."
    let conveyor = ground_vars.flags & FL_CONVEYOR != 0;
    let train = matches!(
        ground_vars.classname.as_str(),
        "func_tracktrain" | "func_train" | "func_vehicle"
    );
    let ground_velocity = ground_vars.velocity;
    if let Some(v) = eng.ent_mut(id) {
        if conveyor {
            v.velocity += v.basevelocity;
        }
        if train {
            v.velocity += ground_velocity;
        }
    }
}

/// `CBasePlayer::Duck` (`player.cpp` 4573-4580, read 2026-11-22): a player holding `IN_DUCK` is
/// walking, as far as the animation goes.
pub(crate) fn duck(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    if eng.ent(id).is_some_and(|v| v.button & IN_DUCK != 0) {
        set_animation(g, eng, slot, PlayerAnim::Walk);
    }
}

/// The sample `DeathSound` plays for the draw `RANDOM_LONG(1, 4)` gave.
pub(crate) fn play_death_sound(eng: &mut EngineApi<'_>, id: cs_engine::EdictId, draw: i32) {
    let sample = match draw {
        1 => "player/die1.wav",
        2 => "player/die2.wav",
        3 => "player/die3.wav",
        _ => "player/death6.wav",
    };
    eng.emit_sound(id, CHAN_VOICE, sample, 1.0, ATTN_NORM, PITCH_NORM);
}

/// The animation part of `CBasePlayer::Spawn` (`player.cpp` 5795-5798, 5860-5890 and 5986,
/// read 2026-11-22): the gait starts over, `m_flLastFired = -15`, the death flags clear, and the
/// sequence is an idle one (`pev->sequence = LookupActivity(ACT_IDLE)`). Called once the player
/// has its model.
pub(crate) fn on_spawn(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    let model = eng.studio_model(v.modelindex);
    let GameState { players, rng, .. } = g;
    let Some(p) = players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    let st = &mut p.anim;
    st.gait_sequence = 0;
    st.gait_frame = 0.0;
    st.gait_yaw = 0.0;
    st.gait_movement = 0.0;
    st.prev_gait_origin = Vec3::ZERO;
    st.killed_by_bomb = false;
    st.killed_by_grenade = false;
    st.last_fired = -15.0;
    st.headshot_killed = false;
    let sequence = lookup_activity(model.as_deref(), ACT_IDLE, rng);
    if let Some(v) = eng.ent_mut(id) {
        v.sequence = sequence as u16;
    }
}

/// `CBaseAnimating::StudioFrameAdvance(0)` for the player of `slot` (called from `PostThink`):
/// the frame moves on by the time since the last advance, at the sequence's rate; a frame past
/// the end of the sequence wraps (a looping one) or stops at 0 or 255 and finishes it.
pub(crate) fn studio_frame_advance(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let time = eng.time() as f32;
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    let mut interval = time - v.animtime;
    if interval <= 0.001 {
        v.animtime = time;
        return;
    }
    if v.animtime == 0.0 {
        interval = 0.0;
    }
    v.frame += interval * p.anim.frame_rate * v.framerate;
    v.animtime = time;
    if v.frame < 0.0 || f64::from(v.frame) >= 256.0 {
        if p.anim.sequence_loops {
            // `pev->frame -= int(pev->frame / 256.0) * 256.0`: double arithmetic.
            let frame = f64::from(v.frame);
            v.frame = (frame - f64::from((frame / 256.0) as i32) * 256.0) as f32;
        } else {
            v.frame = if v.frame < 0.0 { 0.0 } else { 255.0 };
        }
        p.anim.sequence_finished = true;
    }
}

/// `int64(angle / 360) * 360` as a `float`: the whole turns in `angle`. The product wraps (C's
/// overflow is undefined; a client can send any angle).
fn whole_turns(angle: f32) -> f32 {
    ((angle / 360.0) as i64).wrapping_mul(360) as f32
}

/// `CBasePlayer::StudioEstimateGait` (`player.cpp`, read 2026-11-22): how far the player moved
/// since the last call (`m_flGaitMovement`, 0 below 5 units a second) and which way its legs
/// face (`m_flGaityaw`): the direction of the movement, or turning towards the view while it
/// stands still.
///
/// QUIRKS kept: vanilla tests the yaw step as `abs(int64(step)) < 0.1`, so a step under one
/// degree is dropped; `real_t` is `float`; `Q_fmod` and `atan2` are the `double` functions.
fn studio_estimate_gait(st: &mut AnimState, origin: Vec3, angles_y: f32, frametime: f32) {
    let dt = frametime.clamp(0.0, 1.0);
    if dt == 0.0 {
        st.gait_movement = 0.0;
        return;
    }
    let mut est_velocity = origin - st.prev_gait_origin;
    st.prev_gait_origin = origin;
    st.gait_movement = est_velocity.length();
    if st.gait_movement / dt < 5.0 {
        st.gait_movement = 0.0;
        est_velocity.x = 0.0;
        est_velocity.y = 0.0;
    }
    if est_velocity.x == 0.0 && est_velocity.y == 0.0 {
        let mut yaw_diff = angles_y - st.gait_yaw;
        let mut yaw = (f64::from(yaw_diff) % 360.0) as f32;
        yaw_diff -= whole_turns(yaw_diff);
        if yaw_diff > 180.0 {
            yaw_diff -= 360.0;
        }
        if yaw_diff < -180.0 {
            yaw_diff += 360.0;
        }
        if yaw < -180.0 {
            yaw += 360.0;
        } else if yaw > 180.0 {
            yaw -= 360.0;
        }
        if yaw > -5.0 && yaw < 5.0 {
            st.yaw_modifier = 0.05;
        }
        if !(-90.0..=90.0).contains(&yaw) {
            st.yaw_modifier = 3.5;
        }
        if dt < 0.25 {
            yaw_diff *= dt * st.yaw_modifier;
        } else {
            yaw_diff *= dt;
        }
        if ((yaw_diff as i64).unsigned_abs() as f32) < 0.1 {
            yaw_diff = 0.0;
        }
        st.gait_yaw += yaw_diff;
        st.gait_yaw -= whole_turns(st.gait_yaw);
        st.gait_movement = 0.0;
    } else {
        st.gait_yaw = (f64::from(est_velocity.y).atan2(f64::from(est_velocity.x)) * 180.0
            / std::f64::consts::PI) as f32;
        st.gait_yaw = st.gait_yaw.clamp(-180.0, 180.0);
    }
}

/// `CBasePlayer::StudioPlayerBlend`: the blend weight of a pitch (255 looking up 15 degrees or
/// more, 0 looking down 15 or more, linear between).
pub(crate) fn studio_player_blend(pitch: f32) -> i32 {
    let range = ((pitch * 3.0) as i64) as f32;
    if range <= -45.0 {
        255
    } else if range >= 45.0 {
        0
    } else {
        ((45.0 - range) * (255.0 / 90.0)) as i64 as i32
    }
}

/// `CBasePlayer::CalculatePitchBlend`: `pev->blending[1]` from `pev->angles.x`.
fn calculate_pitch_blend(st: &mut AnimState, angles_x: f32) -> u8 {
    let blend = studio_player_blend(angles_x);
    st.pitch = blend as f32;
    blend as u8
}

/// `CBasePlayer::CalculateYawBlend`: `pev->blending[0]`, the player's view yaw against the way
/// the legs face (`StudioEstimateGait`), as 0 to 255.
fn calculate_yaw_blend(
    st: &mut AnimState,
    origin: Vec3,
    angles_y: f32,
    frametime: f32,
) -> u8 {
    const MAX_YAW: f32 = 255.0;
    studio_estimate_gait(st, origin, angles_y, frametime);
    // `Q_fmod(real_t(pev->angles.y - m_flGaityaw), 360)`.
    let mut yaw = (f64::from(angles_y - st.gait_yaw) % 360.0) as f32;
    if yaw < -180.0 {
        yaw += 360.0;
    } else if yaw > 180.0 {
        yaw -= 360.0;
    }
    if st.gait_movement != 0.0 {
        if yaw > 120.0 {
            st.gait_yaw -= 180.0;
            st.gait_movement = -st.gait_movement;
            yaw -= 180.0;
        } else if yaw < -120.0 {
            st.gait_yaw += 180.0;
            st.gait_movement = -st.gait_movement;
            yaw += 180.0;
        }
    }
    yaw = (yaw / 90.0) * 128.0 + 127.0;
    yaw = yaw.clamp(0.0, 255.0);
    let blend_yaw = MAX_YAW - yaw;
    st.yaw = blend_yaw;
    (blend_yaw as i64) as u8
}

/// `CBasePlayer::StudioProcessGait` (`player.cpp`, read 2026-11-22), the end of `PostThink`:
/// `pev->blending` from the view angles and the gait, and `m_flGaitframe` along the gait
/// sequence. A `gaitsequence` outside the model ends it before the frame (C reads past the
/// table; assumption).
pub(crate) fn studio_process_gait(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let frametime = eng.frametime();
    let Some(v) = eng.ent(id) else {
        return;
    };
    let (origin, angles, gait_sequence, framerate, modelindex) =
        (v.origin, v.angles, v.gaitsequence, v.framerate, v.modelindex);
    let model = eng.studio_model(modelindex);
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    // `m_iGaitsequence = pev->gaitsequence` comes just before the call in `PostThink`.
    p.anim.gait_sequence = i32::from(gait_sequence as i16);
    let dt = frametime.clamp(0.0, 1.0);
    let yaw = calculate_yaw_blend(&mut p.anim, origin, angles.y, frametime);
    let pitch = calculate_pitch_blend(&mut p.anim, angles.x);
    if let Some(v) = eng.ent_mut(id) {
        v.blending = [yaw, pitch];
    }
    let Some(model) = model else {
        return;
    };
    let Some(seq) = model.sequences.get(usize::from(gait_sequence)) else {
        return;
    };
    let st = &mut p.anim;
    if seq.linearmovement[0] > 0.0 {
        st.gait_frame += (st.gait_movement / seq.linearmovement[0]) * seq.numframes as f32;
    } else {
        st.gait_frame += framerate * seq.fps * dt;
    }
    if seq.numframes <= 0 {
        return;
    }
    // `m_flGaitframe -= int(m_flGaitframe / pseqdesc->numframes) * pseqdesc->numframes`.
    let frames = seq.numframes as f32;
    st.gait_frame -= ((st.gait_frame / frames) as i32 as f32) * frames;
    if st.gait_frame < 0.0 {
        st.gait_frame += frames;
    }
}
