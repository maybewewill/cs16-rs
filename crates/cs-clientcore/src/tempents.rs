//! Temporary entities: the pool, the spawn recipes behind each `TE_*` the server sends, the
//! per-frame update, and the render list the shell draws.
//!
//! Ported from Xash3D FWGS `engine/client/cl_tent.c` (GPL-3.0-or-later), read 2026-11-23:
//! - the pool and its two priorities: `CL_TempEntAlloc`, `CL_TempEntAllocHigh`,
//!   `CL_FreeLowPriorityTempEnt`, `CL_PrepareTEnt` (defaults of a fresh entry);
//! - the recipes `R_DefaultSprite`, `R_Sprite_Explode`, `R_Sprite_Smoke`, `R_TempSprite`,
//!   `R_SparkShower`, `R_BloodSprite`, `R_BreakModel`, `R_TempModel`, `R_Explosion`;
//! - the dispatch of `CL_ParseTempEntity`, with the wire layouts read by
//!   `cs_proto48::tempent::TempEffect`;
//!
//! In Xash3D the per-frame update belongs to the game DLL, which this crate does not port.
//! [`TempEnts::update`] is a clean-room design, not the original client's code; behaviour
//! tuned to look like the original: gravity, a swept move against [`FxWorld`], a bounce that
//! keeps `bounce_factor` of the speed, a settle on near-horizontal floors, a hit sound at most
//! once per bounce, a fade-out and the sprite animation. The constants of the physics (settle
//! speed, fade window, sound throttle) are our own.
//!
//! Decals, dynamic lights, beams and particle bursts are not entities of the pool: they leave as
//! [`RenderEffect`]s for the shell. Sprites and models need a frame count and a trace against
//! the world, which the core does not have; [`FxWorld`] is where the shell provides them.

use std::collections::VecDeque;

use cs_proto48::tempent::TempEffect;
use glam::Vec3;

use crate::precache::PrecacheTables;
use crate::sound::{ATTN_NORM, PITCH_NORM, SoundCommand, SoundQueue, SoundStart, chan};

/// The size of the pool (`GI->max_tents`; 500 is the default of Xash3D's game info,
/// assumption).
pub const MAX_TEMP_ENTITIES: usize = 500;

/// Lifetime of a fresh entry (`CL_PrepareTEnt`: `die = cl.time + 0.75f`).
pub const DEFAULT_LIFE: f64 = 0.75;

/// The `FTENT_*` flags (`common/const.h`, the engine's public numbering).
pub mod ftent {
    pub const NONE: u32 = 0;
    pub const SINEWAVE: u32 = 0x0000_0001;
    pub const GRAVITY: u32 = 0x0000_0002;
    pub const ROTATE: u32 = 0x0000_0004;
    pub const SLOWGRAVITY: u32 = 0x0000_0008;
    pub const SMOKETRAIL: u32 = 0x0000_0010;
    pub const COLLIDEWORLD: u32 = 0x0000_0020;
    pub const FLICKER: u32 = 0x0000_0040;
    pub const FADEOUT: u32 = 0x0000_0080;
    pub const SPRANIMATE: u32 = 0x0000_0100;
    pub const HITSOUND: u32 = 0x0000_0200;
    pub const SPIRAL: u32 = 0x0000_0400;
    pub const SPRCYCLE: u32 = 0x0000_0800;
    pub const COLLIDEALL: u32 = 0x0000_1000;
    pub const PERSIST: u32 = 0x0000_2000;
    pub const COLLIDEKILL: u32 = 0x0000_4000;
    pub const PLYRATTACHMENT: u32 = 0x0000_8000;
    pub const SPRANIMATELOOP: u32 = 0x0001_0000;
    pub const SPARKSHOWER: u32 = 0x0002_0000;
    pub const NOMODEL: u32 = 0x0004_0000;
    pub const CLIENTCUSTOM: u32 = 0x0008_0000;
}

/// `BOUNCE_*` / `BREAK_*` (`common/const.h`): what a tempent sounds like when it hits the world.
pub mod bounce {
    pub const NONE: u8 = 0;
    pub const GLASS: u8 = 0x01;
    pub const METAL: u8 = 0x02;
    pub const FLESH: u8 = 0x04;
    pub const WOOD: u8 = 0x08;
    pub const SMOKE: u8 = 0x10;
    pub const TRANS: u8 = 0x20;
    pub const CONCRETE: u8 = 0x40;
    pub const TYPEMASK: u8 = 0x4F;
    pub const SHRAP: u8 = 0x10;
    pub const SHELL: u8 = 0x20;
    pub const SHOTSHELL: u8 = 0x80;
}

/// `TE_BOUNCE_*`: the sound type an ejected shell asks `R_TempModel` for.
pub mod te_bounce {
    pub const NULL: u8 = 0;
    pub const SHELL: u8 = 1;
    pub const SHOTSHELL: u8 = 2;
}

/// `TE_EXPLFLAG_*` bits the proto crate does not name.
pub mod explflag {
    pub use cs_proto48::tempent::explflag::*;
    pub const DRAWALPHA: u8 = 16;
    pub const ROTATE: u8 = 32;
}

/// `kRender*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    #[default]
    Normal,
    TransColor,
    TransTexture,
    Glow,
    TransAlpha,
    TransAdd,
}

/// `kRenderFx*` values the recipes use.
pub mod renderfx {
    pub const NONE: u8 = 0;
    pub const NO_DISSIPATION: u8 = 7;
    pub const CLAMP_MIN_SCALE: u8 = 21;
}

/// The model of a temp entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelRef {
    /// No model (`FTENT_NOMODEL`): a point that moves, a spark.
    #[default]
    None,
    /// An index of the model precache table: the server named it in the message.
    Precached(u16),
    /// A file the client loads by itself (`CL_LoadClientSprites`, the shell models).
    Client(&'static str),
}

/// The sprites the engine loads for its own effects (`cl_default_sprites`).
pub const CLIENT_SPRITES: [&str; 7] = [
    "sprites/muzzleflash1.spr",
    "sprites/muzzleflash2.spr",
    "sprites/muzzleflash3.spr",
    "sprites/dot.spr",
    "sprites/animglow01.spr",
    "sprites/richo1.spr",
    "sprites/shellchrome.spr",
];

/// A result of tracing a short move against the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxTrace {
    /// How much of the move was free, 0.0 to 1.0.
    pub fraction: f32,
    pub end: Vec3,
    /// The normal of the plane that was hit.
    pub normal: Vec3,
}

/// What temp entities need to know about the world and the loaded models. The core has neither
/// the map nor the model files; the shell implements this, or passes [`NoWorld`].
pub trait FxWorld {
    /// The frame count of a sprite, or the body count of a studio model (`model->numframes`).
    fn frame_count(&self, _model: &ModelRef) -> u32 {
        1
    }

    /// The first thing a point travelling from `from` to `to` hits, or `None` if it is free.
    fn trace_line(&self, _from: Vec3, _to: Vec3) -> Option<FxTrace> {
        None
    }

    /// Whether the point is inside solid world (`PM_CL_PointContents == CONTENTS_SOLID`).
    fn point_solid(&self, _point: Vec3) -> bool {
        false
    }
}

/// A world with nothing in it: no collisions, one frame per model.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoWorld;

impl FxWorld for NoWorld {}

/// The pseudo random numbers of the effects: xorshift64*, seeded, so a test replays exactly.
/// Xash3D uses its own generator (`COM_RandomLong`); effects do not depend on the sequence.
#[derive(Debug, Clone)]
pub struct FxRandom(u64);

impl FxRandom {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    /// An integer in `low..=high`; `low` when the range is empty (`COM_RandomLong`).
    pub fn long(&mut self, low: i32, high: i32) -> i32 {
        if high <= low {
            return low;
        }
        let span = (i64::from(high) - i64::from(low) + 1) as u64;
        low + (u64::from(self.next_u32()) % span) as i32
    }

    /// A float in `low..high` (`COM_RandomFloat`; the bounds may come in either order).
    pub fn float(&mut self, low: f32, high: f32) -> f32 {
        let t = self.next_u32() as f32 / (u32::MAX as f32 + 1.0);
        low + (high - low) * t
    }
}

impl Default for FxRandom {
    fn default() -> Self {
        Self::new(0x9E37_79B9_7F4A_7C15)
    }
}

/// `TENTPRIORITY_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Low,
    High,
}

/// One entry of the pool (`TEMPENTITY`). The fields Xash3D keeps in `baseline` as scratch are
/// named here: `velocity` is `baseline.origin`, `angular_velocity` is `baseline.angles`.
#[derive(Debug, Clone, PartialEq)]
pub struct TempEnt {
    pub flags: u32,
    pub priority: Priority,
    /// Priority: [`Priority::High`] survives when the pool is full.
    pub born: f64,
    pub die: f64,
    pub model: ModelRef,
    /// The last frame number (`frameMax`).
    pub frame_max: u32,
    pub frame: f32,
    pub framerate: f32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    pub render_mode: RenderMode,
    pub render_fx: u8,
    /// The opacity now, 0 to 255.
    pub render_amt: f32,
    /// The opacity the entry was given; a fade starts from it.
    pub base_render_amt: f32,
    pub render_color: [u8; 3],
    /// A palette index the shell turns into `render_color` (`clgame.palette[i]`); the core has
    /// no palette.
    pub palette_color: Option<u8>,
    pub scale: f32,
    pub body: u32,
    pub fade_speed: f32,
    pub bounce_factor: f32,
    /// A `bounce::*` value: the sound it makes on a hit.
    pub hit_sound: u8,
    /// A fade that starts late: from this client time the opacity falls by the second number
    /// per second (the smoke grenade's clouds, `events/createsmoke.sc`).
    pub late_fade: Option<(f64, f32)>,
    /// Removed when a new round starts (`events/decal_reset.sc`).
    pub round_scoped: bool,
    /// Bounces so far (our own bookkeeping, for the hit sound).
    pub bounces: u32,
    /// Client time of the last hit sound.
    pub last_hit_sound: f64,
    /// Attached entity index (`FTENT_PLYRATTACHMENT`).
    pub client_entity: u16,
    /// Vertical offset for `FTENT_PLYRATTACHMENT`.
    pub attachment_offset_z: f32,
}

impl TempEnt {
    /// A fresh entry, as `CL_PrepareTEnt` sets it up.
    fn prepared(model: ModelRef, frame_count: u32, now: f64) -> Self {
        let mut flags = ftent::NONE;
        if model == ModelRef::None {
            flags |= ftent::NOMODEL;
        }
        Self {
            flags,
            priority: Priority::Low,
            born: now,
            die: now + DEFAULT_LIFE,
            model,
            frame_max: frame_count.saturating_sub(1),
            frame: 0.0,
            framerate: 0.0,
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            render_mode: RenderMode::Normal,
            render_fx: renderfx::NONE,
            render_amt: 255.0,
            base_render_amt: 0.0,
            render_color: [255, 255, 255],
            palette_color: None,
            scale: 1.0,
            body: 0,
            fade_speed: 0.5,
            bounce_factor: 1.0,
            hit_sound: bounce::NONE,
            late_fade: None,
            round_scoped: false,
            bounces: 0,
            last_hit_sound: f64::NEG_INFINITY,
            client_entity: 0,
            attachment_offset_z: 0.0,
        }
    }

    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

/// Default Half-Life / GoldSrc `host.palette` RGB fallback (`valve/gfx/palette.lmp`) for
/// `R_BloodSprite` and other palette-indexed temp entities when the client VFS does not supply a
/// custom `gfx/palette.lmp`. Indices `247..=251` are the authentic dark-crimson blood ramp.
pub fn goldsrc_palette_rgb(index: u8) -> [u8; 3] {
    match index {
        // Human crimson blood ramp (`BLOOD_COLOR_RED = 247` + `1..=3` -> `247..=250`).
        247 => [103, 0, 0],
        248 => [139, 0, 0],
        249 => [179, 0, 0],
        250 => [215, 0, 0],
        251 => [255, 0, 0],
        252 => [255, 243, 147],
        253 => [255, 247, 199],
        254 => [255, 255, 255],
        255 => [159, 91, 83],
        244 => [0, 255, 0],
        245 => [171, 231, 255],
        246 => [215, 255, 255],
        // Alien / yellow blood (`BLOOD_COLOR_YELLOW = 195`, and HL `70..=73` green-yellow ramp).
        195 => [195, 195, 0],
        196 => [215, 215, 0],
        197 => [235, 235, 0],
        198 => [255, 255, 0],
        70 => [115, 131, 27],
        71 => [135, 151, 35],
        72 => [155, 171, 43],
        73 => [175, 191, 51],
        // Clean-room monotonic greyscale ramp for any other unmounted palette index.
        other => [other, other, other],
    }
}

/// A parsed `TE_TEXTMESSAGE` ready for HUD / channel rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct TeTextMessage {
    pub channel: u8,
    pub x: i16,
    pub y: i16,
    pub effect: u8,
    pub color1: [u8; 4],
    pub color2: [u8; 4],
    pub fadein: u16,
    pub fadeout: u16,
    pub holdtime: u16,
    pub fxtime: Option<u16>,
    pub text: String,
    pub received_at: f64,
}

/// Kinds of particle burst the shell draws.
#[derive(Debug, Clone, PartialEq)]
pub enum ParticleEffect {
    /// `TE_GUNSHOT`: `R_RunParticleEffect( pos, vec3_origin, 0, 20 )`.
    Gunshot { position: Vec3, count: u16 },
    /// `R_FlickerParticles`, from an explosion or `FTENT_FLICKER`.
    Flicker { position: Vec3 },
    /// `R_ParticleExplosion2`.
    Explosion2 {
        position: Vec3,
        color: u8,
        count: u8,
    },
    /// `R_BloodStream`.
    BloodStream {
        origin: Vec3,
        direction: Vec3,
        color: u8,
        count: u8,
    },
    /// `TE_BLOOD`.
    Blood {
        origin: Vec3,
        direction: Vec3,
        color: u8,
        speed: u8,
    },
    /// `R_StreakSplash`.
    StreakSplash {
        start: Vec3,
        direction: Vec3,
        color: u8,
        count: u16,
        speed: u16,
        random_velocity: u16,
    },
    /// `R_BulletImpactParticles`, from `TE_GUNSHOTDECAL`.
    BulletImpact { position: Vec3 },
    /// `TE_SPARKS` / `FTENT_SPARKSHOWER` micro-sparks.
    Sparks { position: Vec3 },
    /// `TE_TAREXPLOSION`.
    TarExplosion { position: Vec3 },
    /// `TE_LAVASPLASH`.
    LavaSplash { position: Vec3 },
    /// `TE_TELEPORT`.
    Teleport { position: Vec3 },
    /// `TE_IMPLOSION`.
    Implosion {
        position: Vec3,
        radius: u8,
        count: u8,
        life: f32,
    },
    /// `TE_LARGEFUNNEL`.
    LargeFunnel { position: Vec3, flags: u16 },
    /// `TE_ARMOR_RICOCHET`.
    ArmorRicochet { position: Vec3 },
    /// `TE_PARTICLEBURST`.
    ParticleBurst {
        position: Vec3,
        radius: u16,
        color: u8,
        duration: f32,
    },
    /// `TE_USERTRACER`.
    UserTracer {
        origin: Vec3,
        velocity: Vec3,
        life: f32,
        color: u8,
        length: u8,
    },
    /// `TE_MULTIGUNSHOT`.
    MultiGunshot {
        origin: Vec3,
        direction: Vec3,
        x_noise: f32,
        y_noise: f32,
        count: u8,
    },
    /// `TE_LINE`.
    Line {
        start: Vec3,
        end: Vec3,
        life: f32,
        color: [u8; 3],
    },
    /// `TE_BOX`.
    Box {
        min: Vec3,
        max: Vec3,
        life: f32,
        color: [u8; 3],
    },
    /// `TE_SHOWLINE`.
    ShowLine { start: Vec3, end: Vec3 },
    /// `svc_particle`.
    SvcParticle {
        origin: Vec3,
        direction: Vec3,
        count: u32,
        color: u8,
    },
    /// `FTENT_SMOKETRAIL` smoke puff particle.
    SmokeTrail { position: Vec3 },
}

/// A dynamic light (`dlight_t`): a point light that decays.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicLight {
    /// 0 for a light of `TE_DLIGHT` or an explosion; the entity of `TE_ELIGHT`.
    pub key: u16,
    /// An entity light (`TE_ELIGHT`) as opposed to a world light.
    pub entity_light: bool,
    pub origin: Vec3,
    pub radius: f32,
    pub color: [u8; 3],
    /// Client time the light dies at.
    pub die: f64,
    /// Radius lost per second.
    pub decay: f32,
}

/// What a `TE_*` asks the renderer for that is not an entry of the pool.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderEffect {
    /// `CL_DecalShoot`: a decal on the world or on an entity.
    Decal {
        position: Vec3,
        /// The decal index the server sent (high variants already add 256).
        decal: u16,
        /// The name from the decal precache table, when it has the index.
        decal_name: Option<String>,
        entity: u16,
        /// The model index of the entity, for `TE_BSPDECAL`; 0 for the world.
        model: u16,
        /// `FDECAL_PERMANENT` (`TE_BSPDECAL`).
        permanent: bool,
    },
    Particles(ParticleEffect),
    Light(DynamicLight),
    /// A beam, a killed beam, a tracer or a fizz: the effect as decoded, with the time it came.
    Beam {
        time: f64,
        effect: TempEffect,
    },
    /// `TE_PLAYERDECAL`: spray decal on wall/entity from a player (`tempdecal.wad`).
    PlayerDecal {
        player: u8,
        position: Vec3,
        entity: u16,
        decal: u8,
    },
    /// `TE_TEXTMESSAGE`: on-screen text message.
    TextMessage(TeTextMessage),
}

/// The time of one update of the pool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempEntClock {
    /// Client time now.
    pub time: f64,
    /// Seconds since the last update.
    pub frametime: f32,
    /// `movevars.gravity`.
    pub gravity: f32,
}

/// Sprite path used by CS 1.6 32-bit layered smoke clouds (`events/createsmoke.sc`).
pub const GAS_PUFF_SPRITE: &str = "sprites/gas_puff_01.spr";

/// Client CVar/event options controlling 32-bit CS 1.6 smoke cloud spawning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmokeCloudConfig {
    /// `bparam1 != 0`: another smoke exploded within 250u, producing slightly lighter grey puffs.
    pub light_smoke: bool,
    /// `max_smokepuffs` (default `120`): hard cap on total active `gas_puff_01.spr` tempents.
    pub max_smokepuffs: usize,
    /// `fastsprites` (`0` = 20 layered puffs, `1` = 10 simpler puffs, `2` = 5 coarse puffs).
    pub fastsprites: u8,
}

impl Default for SmokeCloudConfig {
    fn default() -> Self {
        Self {
            light_smoke: false,
            max_smokepuffs: 120,
            fastsprites: 0,
        }
    }
}

/// The bounded queue of effects waiting for the shell.
const MAX_PENDING_EFFECTS: usize = 256;

/// The pool of temp entities and the effects not kept in it.
#[derive(Debug, Clone)]
pub struct TempEnts {
    /// Active entries, oldest first. Xash3D's active list is a stack: the newest is at its
    /// head, and `CL_FreeLowPriorityTempEnt` takes the first low priority entry from the head.
    active: Vec<TempEnt>,
    capacity: usize,
    effects: VecDeque<RenderEffect>,
    /// Allocations refused because the pool was full.
    pub overflows: u32,
    /// Recipes skipped because the server named a model it never precached.
    pub missing_models: u32,
}

impl Default for TempEnts {
    fn default() -> Self {
        Self::with_capacity(MAX_TEMP_ENTITIES)
    }
}

/// The arguments the recipes share.
struct Spawn<'a> {
    now: f64,
    world: &'a dyn FxWorld,
    precache: &'a PrecacheTables,
    rng: &'a mut FxRandom,
    sounds: &'a mut SoundQueue,
}

impl TempEnts {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            active: Vec::new(),
            capacity,
            effects: VecDeque::new(),
            overflows: 0,
            missing_models: 0,
        }
    }

    /// The live entries, oldest first.
    pub fn active(&self) -> &[TempEnt] {
        &self.active
    }

    /// The entries the shell draws: those with a model.
    pub fn render_list(&self) -> impl Iterator<Item = &TempEnt> {
        self.active.iter().filter(|t| t.model != ModelRef::None)
    }

    /// Takes the decals, lights, beams and particle bursts asked for since the last call.
    pub fn take_effects(&mut self) -> Vec<RenderEffect> {
        self.effects.drain(..).collect()
    }

    /// Forgets everything (`CL_ClearTempEnts`, at a map change).
    pub fn clear(&mut self) {
        self.active.clear();
        self.effects.clear();
    }

    fn push_effect(&mut self, effect: RenderEffect) {
        if self.effects.len() >= MAX_PENDING_EFFECTS {
            self.effects.pop_front();
        }
        self.effects.push_back(effect);
    }

    /// `CL_TempEntAlloc`: a low priority entry; `None` when the pool is full.
    pub fn alloc(
        &mut self,
        origin: Vec3,
        model: ModelRef,
        world: &dyn FxWorld,
        now: f64,
    ) -> Option<&mut TempEnt> {
        if self.active.len() >= self.capacity {
            self.overflows = self.overflows.saturating_add(1);
            return None;
        }
        Some(self.insert(origin, model, Priority::Low, world, now))
    }

    /// `CL_TempEntAllocHigh`: a high priority entry. A full pool gives up the newest low
    /// priority entry; `None` when every entry is high priority.
    pub fn alloc_high(
        &mut self,
        origin: Vec3,
        model: ModelRef,
        world: &dyn FxWorld,
        now: f64,
    ) -> Option<&mut TempEnt> {
        if self.active.len() >= self.capacity {
            // `CL_FreeLowPriorityTempEnt`: the head of the list is the newest entry.
            let victim = self
                .active
                .iter()
                .rposition(|t| t.priority == Priority::Low)?;
            self.active.remove(victim);
        }
        Some(self.insert(origin, model, Priority::High, world, now))
    }

    fn insert(
        &mut self,
        origin: Vec3,
        model: ModelRef,
        priority: Priority,
        world: &dyn FxWorld,
        now: f64,
    ) -> &mut TempEnt {
        let frames = if model == ModelRef::None {
            0
        } else {
            world.frame_count(&model)
        };
        let mut tent = TempEnt::prepared(model, frames, now);
        tent.priority = priority;
        tent.origin = origin;
        self.active.push(tent);
        self.active.last_mut().expect("an entry was just pushed")
    }

    // ---- the recipes -------------------------------------------------------------------

    /// `R_TempModel`: a shell or a gib with gravity, a bounce sound and a fade. `kind` is a
    /// `te_bounce::*` value. Returns whether an entry was allocated.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_temp_model(
        &mut self,
        position: Vec3,
        velocity: Vec3,
        angles: Vec3,
        life: f32,
        model: ModelRef,
        kind: u8,
        world: &dyn FxWorld,
        rng: &mut FxRandom,
        now: f64,
    ) -> bool {
        let Some(t) = self.alloc(position, model, world, now) else {
            return false;
        };
        t.flags = ftent::COLLIDEWORLD | ftent::GRAVITY;
        // Ours: a brass shell keeps half its speed in a bounce.
        t.bounce_factor = SHELL_BOUNCE_FACTOR;
        t.velocity = velocity;
        t.angles = angles;
        match kind {
            te_bounce::SHELL => {
                t.hit_sound = bounce::SHELL;
                t.angular_velocity = Vec3::new(
                    rng.float(-512.0, 511.0),
                    rng.float(-255.0, 255.0),
                    rng.float(-255.0, 255.0),
                );
                t.flags |= ftent::ROTATE;
            }
            te_bounce::SHOTSHELL => {
                t.hit_sound = bounce::SHOTSHELL;
                t.angular_velocity = Vec3::new(
                    rng.float(-512.0, 511.0),
                    rng.float(-255.0, 255.0),
                    rng.float(-255.0, 255.0),
                );
                t.flags |= ftent::ROTATE | ftent::SLOWGRAVITY;
            }
            _ => {}
        }
        // A studio model picks a body; the sprite branch of the original picks a frame. The
        // model type is not known here, so the body is the one that is set (assumption).
        t.body = rng.long(0, t.frame_max as i32) as u32;
        t.die = now + f64::from(life);
        true
    }

    /// Removes what a new round clears (`events/decal_reset.sc`).
    pub fn clear_round(&mut self) {
        self.active.retain(|t| !t.round_scoped);
    }

    /// Removes active player attachments for `entity` (`TE_KILLPLAYERATTACHMENTS`).
    pub fn kill_player_attachments(&mut self, entity: u8) {
        let target = u16::from(entity);
        self.active
            .retain(|t| !(t.has(ftent::PLYRATTACHMENT) && t.client_entity == target));
    }

    /// Spawns particles from `svc_particle`.
    pub fn spawn_particle_msg(&mut self, msg: &cs_proto48::messages::ParticleMsg) {
        self.push_effect(RenderEffect::Particles(ParticleEffect::SvcParticle {
            origin: msg.origin,
            direction: msg.direction(),
            count: msg.effective_count(),
            color: msg.color,
        }));
    }

    /// Spawns a shell casing from a `Brass` usermsg payload.
    pub fn spawn_brass_usermsg(
        &mut self,
        payload: &[u8],
        now: f64,
        precache: &PrecacheTables,
        world: &dyn FxWorld,
        rng: &mut FxRandom,
    ) {
        // ReGameDLL writes `[te_model(106)?, coord x3 origin, coord x3 vel, coord/angle? yaw, short model, byte soundtype, ...]`
        // Accept both 17-byte `TE_MODEL` layout or 18-byte prefixed `[106, ...]` layout safely.
        let slice = if payload.len() >= 18 && payload[0] == cs_proto48::tempent::te::MODEL {
            &payload[1..]
        } else {
            payload
        };
        if let Ok(TempEffect::Model {
            position,
            velocity,
            angle,
            model,
            sound_type,
            life,
        }) = TempEffect::parse(cs_proto48::tempent::te::MODEL, slice)
        {
            let yaw = f32::from(angle) * (360.0 / 256.0);
            let mref = if precache.models.contains_key(&model) {
                ModelRef::Precached(model)
            } else {
                ModelRef::Client("models/rshell.mdl")
            };
            self.spawn_temp_model(
                position,
                velocity,
                Vec3::new(0.0, yaw, 0.0),
                f32::from(life.max(1)) * 0.1,
                mref,
                sound_type,
                world,
                rng,
                now,
            );
        }
    }

    /// Count active smoke puff tempents (`sprites/gas_puff_01.spr`).
    pub fn active_smoke_puff_count(&self) -> usize {
        self.active
            .iter()
            .filter(|t| t.model == ModelRef::Client(GAS_PUFF_SPRITE))
            .count()
    }

    /// The clouds of a smoke grenade (`EV_CreateSmoke`, `events/event_createsmoke.cpp`,
    /// GPL-2.0-or-later, read 2026-11-23): layered 32-bit grey puffs using `sprites/gas_puff_01.spr`
    /// that hold ~25s and fade over the final 5s (`~115-130 u` effective cloud radius), honoring
    /// `max_smokepuffs` and `fastsprites`.
    pub fn spawn_smoke_clouds(
        &mut self,
        origin: Vec3,
        world: &dyn FxWorld,
        rng: &mut FxRandom,
        now: f64,
    ) {
        self.spawn_smoke_clouds_ex(origin, SmokeCloudConfig::default(), world, rng, now);
    }

    /// Extended smoke cloud spawner honoring `max_smokepuffs`, `fastsprites`, and `light_smoke`.
    pub fn spawn_smoke_clouds_ex(
        &mut self,
        origin: Vec3,
        cfg: SmokeCloudConfig,
        world: &dyn FxWorld,
        rng: &mut FxRandom,
        now: f64,
    ) {
        let target_count = match cfg.fastsprites {
            0 => 20usize,
            1 => 10usize,
            _ => 5usize,
        };
        let active_puffs = self.active_smoke_puff_count();
        let budget = cfg.max_smokepuffs.saturating_sub(active_puffs);
        let spawn_count = target_count.min(budget);
        if spawn_count == 0 {
            return;
        }
        let base_scale = match cfg.fastsprites {
            0 => 2.9,
            1 => 3.6,
            _ => 4.4,
        };
        let (grey_lo, grey_hi) = if cfg.light_smoke {
            (160, 195)
        } else {
            (100, 145)
        };
        for i in 0..spawn_count {
            let mut org = origin;
            if i != 0 {
                // Polar scatter within ~72 u horizontal radius so puffs of scale ~2.9 (64x64)
                // layer densely across a ~115-130 u smoke cloud radius.
                let angle = rng.float(0.0, std::f32::consts::TAU);
                let dist = rng.float(15.0, 72.0);
                org.x += angle.cos() * dist;
                org.y += angle.sin() * dist;
                org.z += rng.float(18.0, 54.0);
            } else {
                org.z += 30.0;
            }
            let c = rng.long(grey_lo, grey_hi) as u8;
            let grey = [c, c, c];
            let velocity = Vec3::new(rng.float(-5.0, 5.0), rng.float(-5.0, 5.0), 0.0);
            let Some(t) = self.alloc(org, ModelRef::Client(GAS_PUFF_SPRITE), world, now) else {
                return;
            };
            if cfg.fastsprites >= 2 {
                t.flags |= ftent::COLLIDEWORLD | ftent::CLIENTCUSTOM;
                t.frame = 0.0;
                t.framerate = 0.0;
            } else {
                t.flags |= ftent::SPRANIMATELOOP | ftent::COLLIDEWORLD | ftent::CLIENTCUSTOM;
                t.frame = rng.long(0, t.frame_max as i32) as f32;
                t.framerate = if cfg.fastsprites == 0 { 2.0 } else { 0.0 };
            }
            t.flags &= !ftent::NOMODEL;
            t.die = now + 30.0;
            // 25s hold at full opacity + 5s linear fadeout (255 / 5 = 51.0 per second).
            t.late_fade = Some((now + 25.0, 51.0));
            t.render_amt = 255.0;
            t.base_render_amt = 255.0;
            t.render_mode = RenderMode::TransAlpha;
            t.render_color = grey;
            t.scale = base_scale
                + if cfg.fastsprites == 0 {
                    rng.float(-0.25, 0.3)
                } else {
                    0.0
                };
            t.velocity = velocity;
            t.round_scoped = true;
        }
    }

    /// `R_DefaultSprite`: an animated sprite that dies at the end of its animation.
    fn default_sprite(
        &mut self,
        s: &mut Spawn,
        position: Vec3,
        sprite: u16,
        framerate: f32,
    ) -> Option<usize> {
        if !s.precache.models.contains_key(&sprite) {
            self.missing_models = self.missing_models.saturating_add(1);
            return None;
        }
        let now = s.now;
        let t = self.alloc(position, ModelRef::Precached(sprite), s.world, now)?;
        t.scale = 1.0;
        t.flags |= ftent::SPRANIMATE;
        let framerate = if framerate == 0.0 { 10.0 } else { framerate };
        t.framerate = framerate;
        t.die = now + f64::from(t.frame_max as f32 / framerate);
        t.frame = 0.0;
        Some(self.active.len() - 1)
    }

    /// `R_TempSprite`: a sprite with a velocity, a render mode and a life.
    #[allow(clippy::too_many_arguments)]
    fn temp_sprite(
        &mut self,
        s: &mut Spawn,
        position: Vec3,
        direction: Vec3,
        scale: f32,
        sprite: u16,
        mode: RenderMode,
        fx: u8,
        alpha: f32,
        life: f32,
        flags: u32,
    ) {
        if !s.precache.models.contains_key(&sprite) {
            self.missing_models = self.missing_models.saturating_add(1);
            return;
        }
        let now = s.now;
        let Some(t) = self.alloc(position, ModelRef::Precached(sprite), s.world, now) else {
            return;
        };
        t.framerate = 10.0;
        t.render_mode = mode;
        t.render_fx = fx;
        t.scale = scale;
        t.base_render_amt = alpha * 255.0;
        t.render_amt = alpha * 255.0;
        t.flags |= flags;
        t.velocity = direction;
        t.die = if life != 0.0 {
            now + f64::from(life)
        } else {
            now + f64::from(t.frame_max as f32 * 0.1) + 1.0
        };
        t.frame = 0.0;
    }

    /// `R_SparkShower`.
    fn spark_shower(&mut self, s: &mut Spawn, position: Vec3) {
        let now = s.now;
        let Some(t) = self.alloc(position, ModelRef::None, s.world, now) else {
            return;
        };
        t.velocity = Vec3::new(
            s.rng.float(-300.0, 300.0),
            s.rng.float(-300.0, 300.0),
            s.rng.float(-200.0, 200.0),
        );
        t.flags |= ftent::SLOWGRAVITY | ftent::COLLIDEWORLD | ftent::SPARKSHOWER;
        t.framerate = s.rng.float(0.5, 1.5);
        t.scale = now as f32;
        t.die = now + 0.5;
    }

    /// `R_BloodSprite`: one large high priority sprite and a spatter of drops.
    fn blood_sprite(
        &mut self,
        s: &mut Spawn,
        position: Vec3,
        color: u8,
        sprite: u16,
        drop_sprite: u16,
        size: f32,
    ) {
        let color = color.wrapping_add(s.rng.long(1, 3) as u8);
        let impact_index = color;
        let spatter_index = color.wrapping_sub(1);
        let now = s.now;

        if s.precache.models.contains_key(&sprite) {
            let mut pos = position;
            pos.z += s.rng.float(2.0, 4.0);
            let scale = s.rng.float(size / 25.0, size / 35.0);
            let angle = s.rng.long(0, 360) as f32;
            if let Some(t) = self.alloc_high(pos, ModelRef::Precached(sprite), s.world, now) {
                t.render_mode = RenderMode::TransTexture;
                t.render_fx = renderfx::CLAMP_MIN_SCALE;
                t.scale = scale;
                t.flags = ftent::SPRANIMATE;
                t.palette_color = Some(impact_index);
                t.render_color = goldsrc_palette_rgb(impact_index);
                t.base_render_amt = 250.0;
                t.render_amt = 250.0;
                t.framerate = t.frame_max as f32 * 4.0;
                // The original divides by the frame rate even when it is 0; a single frame
                // sprite then has no life at all (assumption: a quarter second instead).
                t.die = if t.framerate > 0.0 {
                    now + f64::from(t.frame_max as f32 / t.framerate)
                } else {
                    now + 0.25
                };
                t.frame = 0.0;
                t.bounce_factor = 0.0;
                t.angles.z = angle;
            }
        } else if sprite != 0 {
            self.missing_models = self.missing_models.saturating_add(1);
        }

        if s.precache.models.contains_key(&drop_sprite) {
            let splatter = size as i32 + s.rng.long(1, 8) + s.rng.long(1, 8);
            for _ in 0..splatter {
                let velocity = Vec3::new(
                    s.rng.float(-96.0, 95.0),
                    s.rng.float(-96.0, 95.0),
                    s.rng.float(-32.0, 95.0),
                );
                let avelocity = Vec3::new(
                    s.rng.float(-256.0, -255.0),
                    s.rng.float(-256.0, -255.0),
                    s.rng.float(-256.0, -255.0),
                );
                let scale = s.rng.float(size / 15.0, size / 25.0);
                let life = s.rng.float(1.0, 3.0);
                let angle = s.rng.float(0.0, 360.0);
                let frame_max = s
                    .world
                    .frame_count(&ModelRef::Precached(drop_sprite))
                    .saturating_sub(1);
                let frame_pick = s.rng.long(1, frame_max as i32);
                let Some(t) = self.alloc(position, ModelRef::Precached(drop_sprite), s.world, now)
                else {
                    break;
                };
                t.render_mode = RenderMode::TransTexture;
                t.render_fx = renderfx::CLAMP_MIN_SCALE;
                t.scale = scale;
                t.flags = ftent::ROTATE | ftent::SLOWGRAVITY | ftent::COLLIDEWORLD;
                t.palette_color = Some(spatter_index);
                t.render_color = goldsrc_palette_rgb(spatter_index);
                t.base_render_amt = 250.0;
                t.render_amt = 250.0;
                t.velocity = velocity;
                t.angular_velocity = avelocity;
                t.die = now + f64::from(life);
                // `COM_RandomLong( 1, frameMax )`, and the last frame past frame 8.
                t.frame = if frame_pick > 8 {
                    t.frame_max as f32
                } else {
                    frame_pick as f32
                };
                t.angles.z = angle;
                t.bounce_factor = 0.0;
            }
        } else if drop_sprite != 0 {
            self.missing_models = self.missing_models.saturating_add(1);
        }
    }

    /// `R_BreakModel`: shards filling a box.
    #[allow(clippy::too_many_arguments)]
    fn break_model(
        &mut self,
        s: &mut Spawn,
        position: Vec3,
        size: Vec3,
        direction: Vec3,
        random: f32,
        life: f32,
        mut count: i32,
        model: u16,
        flags: u8,
    ) {
        /// `SHARD_VOLUME`.
        const SHARD_VOLUME: f32 = 12.0;
        if !s.precache.models.contains_key(&model) {
            self.missing_models = self.missing_models.saturating_add(1);
            return;
        }
        let kind = flags & bounce::TYPEMASK;
        if count == 0 {
            // Assume a surface, not a volume.
            count = ((size.x * size.y + size.y * size.z + size.z * size.x)
                / (3.0 * SHARD_VOLUME * SHARD_VOLUME)) as i32;
        }
        count = count.min(100);
        let now = s.now;
        for _ in 0..count {
            let mut spot = None;
            for _ in 0..32 {
                let candidate = position
                    + Vec3::new(
                        s.rng.float(-0.5, 0.5) * size.x,
                        s.rng.float(-0.5, 0.5) * size.y,
                        s.rng.float(-0.5, 0.5) * size.z,
                    );
                if !s.world.point_solid(candidate) {
                    spot = Some(candidate);
                    break;
                }
            }
            // A piece completely stuck in the wall is dropped.
            let Some(spot) = spot else {
                continue;
            };
            let rotates = s.rng.long(0, 255) < 200;
            let avelocity = Vec3::new(
                s.rng.float(-256.0, 255.0),
                s.rng.float(-256.0, 255.0),
                s.rng.float(-256.0, 255.0),
            );
            let smokes = s.rng.long(0, 255) < 100 && flags & bounce::SMOKE != 0;
            let velocity = Vec3::new(
                direction.x + s.rng.float(-random, random),
                direction.y + s.rng.float(-random, random),
                direction.z + s.rng.float(0.0, random),
            );
            let extra_life = s.rng.float(0.0, 1.0);
            let body_max = s
                .world
                .frame_count(&ModelRef::Precached(model))
                .saturating_sub(1);
            let body = s.rng.long(0, body_max as i32) as u32;
            let Some(t) = self.alloc(spot, ModelRef::Precached(model), s.world, now) else {
                return;
            };
            // Keeps the kind so the bounce sound is the right one.
            t.hit_sound = kind;
            // Frame of a sprite, body of a studio model: see `spawn_temp_model`.
            t.body = body;
            t.flags |= ftent::COLLIDEWORLD | ftent::FADEOUT | ftent::SLOWGRAVITY;
            if rotates {
                t.flags |= ftent::ROTATE;
                t.angular_velocity = avelocity;
            }
            if smokes {
                t.flags |= ftent::SMOKETRAIL;
            }
            if kind == bounce::GLASS || flags & bounce::TRANS != 0 {
                t.render_mode = RenderMode::TransTexture;
                t.render_amt = 128.0;
                t.base_render_amt = 128.0;
            } else {
                t.render_mode = RenderMode::Normal;
                t.render_amt = 255.0;
                t.base_render_amt = 255.0;
            }
            t.velocity = velocity;
            t.die = now + f64::from(life) + f64::from(extra_life);
        }
    }

    /// `R_Explosion`: the sprite, the light and the sound of an explosion.
    fn explosion(
        &mut self,
        s: &mut Spawn,
        position: Vec3,
        sprite: u16,
        scale: f32,
        framerate: f32,
        flags: u8,
    ) {
        if scale != 0.0 {
            if let Some(i) = self.default_sprite(s, position, sprite, framerate) {
                // `R_Sprite_Explode`.
                let t = &mut self.active[i];
                t.scale = scale;
                t.velocity.z = 8.0;
                t.origin.z += 10.0;
                if flags & explflag::ROTATE != 0 {
                    t.angles.z = s.rng.float(0.0, 360.0);
                }
                let no_additive = flags & explflag::NOADDITIVE != 0;
                t.render_mode = if no_additive {
                    RenderMode::Normal
                } else if flags & explflag::DRAWALPHA != 0 {
                    RenderMode::TransAlpha
                } else {
                    RenderMode::TransAdd
                };
                t.render_amt = if no_additive { 255.0 } else { 180.0 };
                t.render_fx = 0;
                t.render_color = [0, 0, 0];
            }
            if flags & explflag::NOPARTICLES == 0 {
                self.push_effect(RenderEffect::Particles(ParticleEffect::Flicker {
                    position,
                }));
            }
            if flags & explflag::NODLIGHTS == 0 {
                // The big flash, then the red glow.
                self.push_effect(RenderEffect::Light(DynamicLight {
                    key: 0,
                    entity_light: false,
                    origin: position,
                    radius: 200.0,
                    color: [250, 250, 150],
                    die: s.now + 0.01,
                    decay: 800.0,
                }));
                self.push_effect(RenderEffect::Light(DynamicLight {
                    key: 0,
                    entity_light: false,
                    origin: position,
                    radius: 150.0,
                    color: [255, 190, 40],
                    die: s.now + 1.0,
                    decay: 200.0,
                }));
            }
        }
        if flags & explflag::NOSOUND == 0 {
            let name = EXPLODE_SOUNDS[s.rng.long(0, EXPLODE_SOUNDS.len() as i32 - 1) as usize];
            s.sounds.push(SoundCommand::Ambient(SoundStart::by_name(
                name,
                0,
                chan::STATIC,
                position,
                1.0,
                0.3,
                PITCH_NORM,
            )));
        }
    }

    // ---- svc_temp_entity ---------------------------------------------------------------

    /// `CL_ParseTempEntity`: spawns what one decoded `svc_temp_entity` asks for. An effect
    /// this client has no recipe for is ignored; its bytes were consumed by the stream parser.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_effect(
        &mut self,
        effect: &TempEffect,
        now: f64,
        precache: &PrecacheTables,
        world: &dyn FxWorld,
        rng: &mut FxRandom,
        sounds: &mut SoundQueue,
    ) {
        let mut s = Spawn {
            now,
            world,
            precache,
            rng,
            sounds,
        };
        match effect {
            TempEffect::Gunshot { position } => {
                ricochet_sound(&mut s, *position);
                self.push_effect(RenderEffect::Particles(ParticleEffect::Gunshot {
                    position: *position,
                    count: 20,
                }));
            }
            TempEffect::Explosion {
                position,
                sprite,
                scale,
                framerate,
                flags,
            } => self.explosion(
                &mut s,
                *position,
                *sprite,
                f32::from(*scale) * 0.1,
                f32::from(*framerate),
                *flags,
            ),
            TempEffect::Smoke {
                position,
                sprite,
                scale,
                framerate,
            } => {
                if let Some(i) =
                    self.default_sprite(&mut s, *position, *sprite, f32::from(*framerate))
                {
                    // `R_Sprite_Smoke`.
                    let grey = s.rng.long(20, 35) as u8;
                    let t = &mut self.active[i];
                    t.render_mode = RenderMode::TransAlpha;
                    t.render_fx = renderfx::NONE;
                    t.velocity.z = 30.0;
                    t.render_color = [grey; 3];
                    t.origin.z += 20.0;
                    t.scale = f32::from(*scale) * 0.1;
                }
            }
            TempEffect::Sparks { position } => self.spark_shower(&mut s, *position),
            TempEffect::Explosion2 {
                position,
                color,
                count,
            } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::Explosion2 {
                    position: *position,
                    color: *color,
                    count: *count,
                }));
                // The light of the original sets no colour (assumption: it stays black).
                self.push_effect(RenderEffect::Light(DynamicLight {
                    key: 0,
                    entity_light: false,
                    origin: *position,
                    radius: 350.0,
                    color: [0, 0, 0],
                    die: now + 0.5,
                    decay: 300.0,
                }));
                s.sounds.push(SoundCommand::Start(SoundStart::by_name(
                    EXPLODE_SOUNDS[0],
                    0,
                    chan::AUTO,
                    *position,
                    1.0,
                    1.0,
                    PITCH_NORM,
                )));
            }
            TempEffect::BspDecal {
                position,
                decal,
                entity,
                model,
            } => self.push_effect(decal_effect(
                precache,
                *position,
                *decal,
                *entity,
                model.unwrap_or(0),
                true,
            )),
            TempEffect::Decal {
                position,
                decal,
                entity,
            } => self.push_effect(decal_effect(
                precache,
                *position,
                u16::from(*decal),
                *entity,
                0,
                false,
            )),
            TempEffect::DecalHigh {
                position,
                decal,
                entity,
            } => self.push_effect(decal_effect(
                precache,
                *position,
                u16::from(*decal) + 256,
                *entity,
                0,
                false,
            )),
            TempEffect::WorldDecal { position, decal } => self.push_effect(decal_effect(
                precache,
                *position,
                u16::from(*decal),
                0,
                0,
                false,
            )),
            TempEffect::WorldDecalHigh { position, decal } => self.push_effect(decal_effect(
                precache,
                *position,
                u16::from(*decal) + 256,
                0,
                0,
                false,
            )),
            TempEffect::GunshotDecal {
                position,
                entity,
                decal,
            } => {
                self.push_effect(decal_effect(
                    precache,
                    *position,
                    u16::from(*decal),
                    *entity,
                    0,
                    false,
                ));
                self.push_effect(RenderEffect::Particles(ParticleEffect::BulletImpact {
                    position: *position,
                }));
                // `flags = COM_RandomLong( 0, 0x7fff ); if( flags < 0x3fff ) ...`: about half
                // of the hits ricochet, with the sound `flags % count`.
                let roll = s.rng.long(0, 0x7fff);
                if roll < 0x3fff {
                    let name = RICOCHET_SOUNDS[roll as usize % RICOCHET_SOUNDS.len()];
                    s.sounds.push(SoundCommand::Start(SoundStart::by_name(
                        name,
                        0,
                        chan::AUTO,
                        *position,
                        1.0,
                        1.0,
                        100,
                    )));
                }
            }
            TempEffect::Sprite {
                position,
                sprite,
                scale,
                brightness,
            } => self.temp_sprite(
                &mut s,
                *position,
                Vec3::ZERO,
                f32::from(*scale) * 0.1,
                *sprite,
                RenderMode::TransAdd,
                renderfx::NONE,
                f32::from(*brightness) / 255.0,
                0.0,
                ftent::SPRANIMATE,
            ),
            TempEffect::GlowSprite {
                position,
                model,
                life,
                scale,
                brightness,
            } => self.temp_sprite(
                &mut s,
                *position,
                Vec3::ZERO,
                f32::from(*scale) * 0.1,
                *model,
                RenderMode::Glow,
                renderfx::NO_DISSIPATION,
                f32::from(*brightness) / 255.0,
                f32::from(*life) * 0.1,
                ftent::FADEOUT,
            ),
            TempEffect::StreakSplash {
                start,
                direction,
                color,
                count,
                speed,
                random_velocity,
            } => self.push_effect(RenderEffect::Particles(ParticleEffect::StreakSplash {
                start: *start,
                direction: *direction,
                color: *color,
                count: *count,
                speed: *speed,
                random_velocity: *random_velocity,
            })),
            TempEffect::Dlight {
                position,
                radius,
                color,
                life,
                decay,
            } => self.push_effect(RenderEffect::Light(DynamicLight {
                key: 0,
                entity_light: false,
                origin: *position,
                radius: f32::from(*radius) * 10.0,
                color: *color,
                die: now + f64::from(f32::from(*life) * 0.1),
                decay: f32::from(*decay) * 10.0,
            })),
            TempEffect::Elight {
                entity,
                position,
                radius,
                color,
                life,
                decay,
            } => {
                let life_s = f32::from(*life) * 0.1;
                self.push_effect(RenderEffect::Light(DynamicLight {
                    key: *entity,
                    entity_light: true,
                    origin: *position,
                    radius: *radius,
                    color: *color,
                    die: now + f64::from(life_s),
                    decay: if life_s != 0.0 {
                        *decay / life_s
                    } else {
                        *decay
                    },
                }));
            }
            TempEffect::BloodStream {
                origin,
                direction,
                color,
                count,
            } => self.push_effect(RenderEffect::Particles(ParticleEffect::BloodStream {
                origin: *origin,
                direction: *direction,
                color: *color,
                count: *count,
            })),
            TempEffect::BloodSprite {
                position,
                spray,
                drop,
                color,
                scale,
            } => self.blood_sprite(&mut s, *position, *color, *spray, *drop, f32::from(*scale)),
            TempEffect::BreakModel {
                position,
                size,
                velocity,
                random_velocity,
                model,
                count,
                life,
                flags,
            } => self.break_model(
                &mut s,
                *position,
                *size,
                *velocity,
                f32::from(*random_velocity) * 10.0,
                f32::from(*life) * 0.1,
                i32::from(*count),
                *model,
                *flags,
            ),
            TempEffect::BeamPoints { .. }
            | TempEffect::BeamEntPoint { .. }
            | TempEffect::BeamEnts { .. }
            | TempEffect::BeamFollow { .. }
            | TempEffect::KillBeam { .. }
            | TempEffect::Tracer { .. }
            | TempEffect::Fizz { .. }
            | TempEffect::Lightning { .. }
            | TempEffect::BeamSprite { .. }
            | TempEffect::BeamTorus { .. }
            | TempEffect::BeamDisk { .. }
            | TempEffect::BeamCylinder { .. }
            | TempEffect::BeamRing { .. } => self.push_effect(RenderEffect::Beam {
                time: now,
                effect: effect.clone(),
            }),
            TempEffect::TarExplosion { position } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::TarExplosion {
                    position: *position,
                }));
                s.sounds.push(SoundCommand::Start(SoundStart::by_name(
                    EXPLODE_SOUNDS[0],
                    0,
                    chan::AUTO,
                    *position,
                    1.0,
                    1.0,
                    PITCH_NORM,
                )));
            }
            TempEffect::LavaSplash { position } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::LavaSplash {
                    position: *position,
                }));
            }
            TempEffect::Teleport { position } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::Teleport {
                    position: *position,
                }));
            }
            TempEffect::Implosion {
                position,
                radius,
                count,
                life,
            } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::Implosion {
                    position: *position,
                    radius: *radius,
                    count: *count,
                    life: f32::from(*life) * 0.1,
                }));
            }
            TempEffect::SpriteTrail {
                start,
                end,
                sprite,
                count,
                life,
                scale,
                noise,
                speed,
            } => {
                if !s.precache.models.contains_key(sprite) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let dir = (*end - *start).normalize_or_zero();
                    let spd = f32::from(*speed) * 10.0;
                    let noise_f = f32::from(*noise) * 10.0;
                    let life_s = f32::from((*life).max(1)) * 0.1;
                    for _ in 0..(*count).min(64) {
                        let Some(t) =
                            self.alloc(*start, ModelRef::Precached(*sprite), s.world, now)
                        else {
                            break;
                        };
                        t.velocity = dir * spd
                            + Vec3::new(
                                s.rng.float(-noise_f, noise_f),
                                s.rng.float(-noise_f, noise_f),
                                s.rng.float(-noise_f, noise_f),
                            );
                        t.scale = (f32::from(*scale) * 0.1).max(0.1);
                        t.render_mode = RenderMode::TransAdd;
                        t.flags |= ftent::FADEOUT | ftent::SPRANIMATE | ftent::SPRANIMATELOOP;
                        t.framerate = 10.0;
                        t.die = now + f64::from(life_s) + f64::from(s.rng.float(0.0, 0.3));
                    }
                }
            }
            TempEffect::TextMessage {
                channel,
                x,
                y,
                effect: fx,
                color1,
                color2,
                fadein,
                fadeout,
                holdtime,
                fxtime,
                text,
            } => {
                self.push_effect(RenderEffect::TextMessage(TeTextMessage {
                    channel: *channel,
                    x: *x,
                    y: *y,
                    effect: *fx,
                    color1: *color1,
                    color2: *color2,
                    fadein: *fadein,
                    fadeout: *fadeout,
                    holdtime: *holdtime,
                    fxtime: *fxtime,
                    text: text.clone(),
                    received_at: now,
                }));
            }
            TempEffect::Line {
                start,
                end,
                life,
                color,
            } => self.push_effect(RenderEffect::Particles(ParticleEffect::Line {
                start: *start,
                end: *end,
                life: f32::from(*life) * 0.1,
                color: *color,
            })),
            TempEffect::Box {
                min,
                max,
                life,
                color,
            } => self.push_effect(RenderEffect::Particles(ParticleEffect::Box {
                min: *min,
                max: *max,
                life: f32::from(*life) * 0.1,
                color: *color,
            })),
            TempEffect::LargeFunnel {
                position,
                sprite,
                flags,
            } => {
                if s.precache.models.contains_key(sprite) {
                    self.temp_sprite(
                        &mut s,
                        *position,
                        Vec3::ZERO,
                        1.0,
                        *sprite,
                        RenderMode::TransAdd,
                        renderfx::NONE,
                        1.0,
                        1.0,
                        ftent::SPRANIMATE | ftent::FADEOUT,
                    );
                }
                self.push_effect(RenderEffect::Particles(ParticleEffect::LargeFunnel {
                    position: *position,
                    flags: *flags,
                }));
            }
            TempEffect::ShowLine { start, end } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::ShowLine {
                    start: *start,
                    end: *end,
                }));
            }
            TempEffect::Blood {
                origin,
                direction,
                color,
                speed,
            } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::Blood {
                    origin: *origin,
                    direction: *direction,
                    color: *color,
                    speed: *speed,
                }));
            }
            TempEffect::Model {
                position,
                velocity,
                angle,
                model,
                sound_type,
                life,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let yaw = f32::from(*angle) * (360.0 / 256.0);
                    self.spawn_temp_model(
                        *position,
                        *velocity,
                        Vec3::new(0.0, yaw, 0.0),
                        f32::from((*life).max(1)) * 0.1,
                        ModelRef::Precached(*model),
                        *sound_type,
                        s.world,
                        s.rng,
                        now,
                    );
                }
            }
            TempEffect::ExplodeModel {
                position,
                velocity,
                model,
                count,
                life,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let n = (*count).min(64);
                    let life_s = f32::from((*life).max(1)) * 0.1;
                    for _ in 0..n {
                        let vel = Vec3::new(
                            s.rng.float(-*velocity, *velocity),
                            s.rng.float(-*velocity, *velocity),
                            s.rng.float(0.0, (*velocity).abs()),
                        );
                        self.spawn_temp_model(
                            *position,
                            vel,
                            Vec3::new(0.0, s.rng.float(0.0, 360.0), 0.0),
                            life_s,
                            ModelRef::Precached(*model),
                            te_bounce::NULL,
                            s.world,
                            s.rng,
                            now,
                        );
                    }
                }
            }
            TempEffect::SpriteSpray {
                position,
                direction,
                sprite,
                count,
                speed,
                noise,
            } => {
                if !s.precache.models.contains_key(sprite) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let dir = direction.normalize_or_zero();
                    let spd = f32::from(*speed);
                    let noise_f = f32::from(*noise);
                    for _ in 0..(*count).min(64) {
                        let vel = dir * spd
                            + Vec3::new(
                                s.rng.float(-noise_f, noise_f),
                                s.rng.float(-noise_f, noise_f),
                                s.rng.float(-noise_f, noise_f),
                            );
                        if let Some(t) =
                            self.alloc(*position, ModelRef::Precached(*sprite), s.world, now)
                        {
                            t.velocity = vel;
                            t.render_mode = RenderMode::TransAdd;
                            t.flags |= ftent::FADEOUT | ftent::SPRANIMATE;
                            t.framerate = 10.0;
                            t.die = now + 0.6;
                        }
                    }
                }
            }
            TempEffect::ArmorRicochet { position, scale } => {
                if let Some(t) = self.alloc(
                    *position,
                    ModelRef::Client("sprites/richo1.spr"),
                    s.world,
                    now,
                ) {
                    t.scale = (f32::from(*scale) * 0.1).max(0.2);
                    t.render_mode = RenderMode::TransAdd;
                    t.flags |= ftent::SPRANIMATE | ftent::FADEOUT;
                    t.framerate = 20.0;
                    t.die = now + 0.2;
                }
                ricochet_sound(&mut s, *position);
                self.push_effect(RenderEffect::Particles(ParticleEffect::ArmorRicochet {
                    position: *position,
                }));
            }
            TempEffect::PlayerDecal {
                player,
                position,
                entity,
                decal,
            } => {
                self.push_effect(RenderEffect::PlayerDecal {
                    player: *player,
                    position: *position,
                    entity: *entity,
                    decal: *decal,
                });
            }
            TempEffect::Bubbles {
                mins,
                maxs,
                height,
                model,
                count,
                speed,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    for _ in 0..(*count).min(64) {
                        let pos = Vec3::new(
                            s.rng.float(mins.x, maxs.x),
                            s.rng.float(mins.y, maxs.y),
                            s.rng.float(mins.z, maxs.z),
                        );
                        if let Some(t) = self.alloc(pos, ModelRef::Precached(*model), s.world, now)
                        {
                            t.velocity = Vec3::new(0.0, 0.0, speed.max(20.0));
                            t.render_mode = RenderMode::TransAdd;
                            t.flags |= ftent::SINEWAVE | ftent::FADEOUT;
                            t.die = now + f64::from((*height / speed.max(20.0)).clamp(0.2, 4.0));
                        }
                    }
                }
            }
            TempEffect::BubbleTrail {
                mins,
                maxs,
                height,
                model,
                count,
                speed,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let n = (*count).clamp(1, 64);
                    for i in 0..n {
                        let frac = i as f32 / n as f32;
                        let pos = mins.lerp(*maxs, frac);
                        if let Some(t) = self.alloc(pos, ModelRef::Precached(*model), s.world, now)
                        {
                            t.velocity = Vec3::new(0.0, 0.0, speed.max(20.0));
                            t.render_mode = RenderMode::TransAdd;
                            t.flags |= ftent::SINEWAVE | ftent::FADEOUT;
                            t.die = now + f64::from((*height / speed.max(20.0)).clamp(0.2, 4.0));
                        }
                    }
                }
            }
            TempEffect::Projectile {
                position,
                velocity,
                model,
                life,
                owner: _,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else if let Some(t) =
                    self.alloc(*position, ModelRef::Precached(*model), s.world, now)
                {
                    t.velocity = *velocity;
                    t.flags |= ftent::COLLIDEWORLD | ftent::COLLIDEKILL;
                    t.die = now + f64::from((*life).max(1)) * 0.1;
                }
            }
            TempEffect::Spray {
                position,
                direction,
                model,
                count,
                speed,
                noise,
                rendermode,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let mode = match *rendermode {
                        1 => RenderMode::TransColor,
                        2 => RenderMode::TransTexture,
                        3 => RenderMode::Glow,
                        4 => RenderMode::TransAlpha,
                        5 => RenderMode::TransAdd,
                        _ => RenderMode::Normal,
                    };
                    let dir = direction.normalize_or_zero();
                    let spd = f32::from(*speed);
                    let noise_f = f32::from(*noise);
                    for _ in 0..(*count).min(64) {
                        let vel = dir * spd
                            + Vec3::new(
                                s.rng.float(-noise_f, noise_f),
                                s.rng.float(-noise_f, noise_f),
                                s.rng.float(-noise_f, noise_f),
                            );
                        if let Some(t) =
                            self.alloc(*position, ModelRef::Precached(*model), s.world, now)
                        {
                            t.velocity = vel;
                            t.render_mode = mode;
                            t.flags |= ftent::GRAVITY | ftent::FADEOUT;
                            t.die = now + 0.8;
                        }
                    }
                }
            }
            TempEffect::PlayerSprites {
                entity,
                model,
                count,
                variance,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let v = f32::from(*variance);
                    for _ in 0..(*count).min(32) {
                        if let Some(t) =
                            self.alloc(Vec3::ZERO, ModelRef::Precached(*model), s.world, now)
                        {
                            t.client_entity = *entity;
                            t.attachment_offset_z = 36.0;
                            t.velocity = Vec3::new(
                                s.rng.float(-v, v),
                                s.rng.float(-v, v),
                                s.rng.float(10.0, 10.0 + v),
                            );
                            t.render_mode = RenderMode::TransAdd;
                            t.flags |= ftent::PLYRATTACHMENT | ftent::FADEOUT;
                            t.die = now + 1.0;
                        }
                    }
                }
            }
            TempEffect::ParticleBurst {
                position,
                radius,
                color,
                duration,
            } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::ParticleBurst {
                    position: *position,
                    radius: *radius,
                    color: *color,
                    duration: f32::from(*duration) * 0.1,
                }));
            }
            TempEffect::FireField {
                position,
                radius,
                model,
                count,
                flags: _,
                life,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else {
                    let r = f32::from(*radius);
                    let life_s = f32::from((*life).max(1)) * 0.1;
                    for _ in 0..(*count).min(64) {
                        let pos = *position
                            + Vec3::new(
                                s.rng.float(-r, r),
                                s.rng.float(-r, r),
                                s.rng.float(0.0, r * 0.5),
                            );
                        if let Some(t) = self.alloc(pos, ModelRef::Precached(*model), s.world, now)
                        {
                            t.velocity = Vec3::new(0.0, 0.0, s.rng.float(15.0, 45.0));
                            t.render_mode = RenderMode::TransAdd;
                            t.flags |= ftent::SPRANIMATE | ftent::SPRANIMATELOOP | ftent::FADEOUT;
                            t.framerate = 12.0;
                            t.die = now + f64::from(life_s);
                        }
                    }
                }
            }
            TempEffect::PlayerAttachment {
                entity,
                offset_z,
                model,
                life,
            } => {
                if !s.precache.models.contains_key(model) {
                    self.missing_models = self.missing_models.saturating_add(1);
                } else if let Some(t) =
                    self.alloc(Vec3::ZERO, ModelRef::Precached(*model), s.world, now)
                {
                    t.client_entity = u16::from(*entity);
                    t.attachment_offset_z = *offset_z;
                    t.render_mode = RenderMode::TransAdd;
                    t.flags |= ftent::PLYRATTACHMENT;
                    t.die = now + f64::from((*life).max(1)) * 0.1;
                }
            }
            TempEffect::KillPlayerAttachments { entity } => {
                self.kill_player_attachments(*entity);
            }
            TempEffect::MultiGunshot {
                origin,
                direction,
                x_noise,
                y_noise,
                count,
                decal: _,
            } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::MultiGunshot {
                    origin: *origin,
                    direction: *direction,
                    x_noise: *x_noise,
                    y_noise: *y_noise,
                    count: *count,
                }));
            }
            TempEffect::UserTracer {
                origin,
                velocity,
                life,
                color,
                length,
            } => {
                self.push_effect(RenderEffect::Particles(ParticleEffect::UserTracer {
                    origin: *origin,
                    velocity: *velocity,
                    life: f32::from(*life) * 0.1,
                    color: *color,
                    length: *length,
                }));
            }
            TempEffect::Other { .. } => {}
        }
    }

    // ---- the per-frame update ----------------------------------------------------------

    /// One step of every live entry (clean-room design, not the original client's code;
    /// behaviour tuned to look like the original).
    pub fn update(
        &mut self,
        clock: TempEntClock,
        world: &dyn FxWorld,
        rng: &mut FxRandom,
        sounds: &mut SoundQueue,
    ) {
        let TempEntClock {
            time,
            frametime,
            gravity,
        } = clock;
        let dt = if frametime.is_finite() {
            frametime.max(0.0)
        } else {
            0.0
        };
        let gravity = if gravity.is_finite() { gravity } else { 800.0 };
        let mut emitted: Vec<RenderEffect> = Vec::new();
        self.active.retain_mut(|t| {
            if !t.has(ftent::PERSIST) && t.die < time {
                return false;
            }
            if (t.has(ftent::SPRANIMATE) || t.has(ftent::SPRCYCLE)) && t.frame_max > 0 {
                let rate = if t.framerate > 0.0 { t.framerate } else { 10.0 };
                t.frame += dt * rate;
                if t.frame >= t.frame_max as f32 {
                    if t.has(ftent::SPRANIMATELOOP) || t.has(ftent::SPRCYCLE) {
                        t.frame = t.frame.rem_euclid((t.frame_max + 1) as f32);
                    } else {
                        return false;
                    }
                }
            }
            if t.has(ftent::ROTATE) {
                t.angles += t.angular_velocity * dt;
            }
            if t.has(ftent::FLICKER) && rng.long(0, 7) == 0 {
                emitted.push(RenderEffect::Particles(ParticleEffect::Flicker {
                    position: t.origin,
                }));
            }
            if t.has(ftent::SMOKETRAIL) && rng.long(0, 3) == 0 {
                emitted.push(RenderEffect::Particles(ParticleEffect::SmokeTrail {
                    position: t.origin,
                }));
            }
            if t.has(ftent::SPARKSHOWER) && t.velocity != Vec3::ZERO && rng.long(0, 2) == 0 {
                emitted.push(RenderEffect::Particles(ParticleEffect::Sparks {
                    position: t.origin,
                }));
            }
            step_motion(t, time, dt, gravity, world, rng, sounds);
            step_fade(t, time, dt);
            true
        });
        for fx in emitted {
            self.push_effect(fx);
        }
    }
}

/// Seconds before `die` over which `FTENT_FADEOUT` ramps the opacity to 0 (ours).
pub const FADE_WINDOW: f64 = 0.5;
/// The share of speed a shell keeps in a bounce (ours).
pub const SHELL_BOUNCE_FACTOR: f32 = 0.5;
/// A floor needs at least this normal.z to hold a resting entry (ours; about 45 degrees).
const FLOOR_NORMAL_Z: f32 = 0.7;
/// Below this speed after a bounce on a floor, the entry settles (ours).
const SETTLE_SPEED: f32 = 40.0;
/// How far off the plane a bounced entry is put, so the next trace does not start inside it.
const SURFACE_PUSH: f32 = 0.05;
/// The least time between two hit sounds of one entry (ours).
const HIT_SOUND_GAP: f64 = 0.08;

fn step_fade(t: &mut TempEnt, time: f64, dt: f32) {
    if t.has(ftent::FADEOUT) {
        let remaining = (t.die - time).max(0.0);
        if remaining < FADE_WINDOW {
            let base = if t.base_render_amt > 0.0 {
                t.base_render_amt
            } else {
                255.0
            };
            let target = base * (remaining / FADE_WINDOW) as f32;
            t.render_amt = t.render_amt.min(target).max(0.0);
        }
    }
    if let Some((start, rate)) = t.late_fade
        && time >= start
    {
        t.render_amt = (t.render_amt - rate * dt).max(0.0);
    }
}

fn step_motion(
    t: &mut TempEnt,
    time: f64,
    dt: f32,
    sv_gravity: f32,
    world: &dyn FxWorld,
    rng: &mut FxRandom,
    sounds: &mut SoundQueue,
) {
    if dt <= 0.0 {
        return;
    }
    if t.has(ftent::SINEWAVE) {
        let phase = (time - t.born) as f32 * 6.0;
        t.origin.x += phase.sin() * 8.0 * dt;
        t.origin.y += phase.cos() * 8.0 * dt;
    }
    if t.has(ftent::SPIRAL) {
        let phase = (time - t.born) as f32 * 10.0;
        t.origin.x += phase.cos() * 14.0 * dt;
        t.origin.y += phase.sin() * 14.0 * dt;
    }
    if t.has(ftent::GRAVITY) {
        t.velocity.z -= sv_gravity * dt;
    } else if t.has(ftent::SLOWGRAVITY) {
        t.velocity.z -= sv_gravity * 0.5 * dt;
    }
    if t.velocity == Vec3::ZERO {
        return;
    }
    let target = t.origin + t.velocity * dt;
    if !t.has(ftent::COLLIDEWORLD) {
        t.origin = target;
        return;
    }
    let Some(hit) = world.trace_line(t.origin, target) else {
        t.origin = target;
        return;
    };
    let normal = hit.normal.try_normalize().unwrap_or(Vec3::Z);
    let fraction = hit.fraction.clamp(0.0, 1.0);
    let impact_speed = t.velocity.length();
    t.origin = (t.origin + (target - t.origin) * fraction) + normal * SURFACE_PUSH;
    if t.has(ftent::COLLIDEKILL) {
        t.die = time;
        return;
    }
    let into = t.velocity.dot(normal);
    let reflected = t.velocity - normal * (2.0 * into);
    t.velocity = reflected * t.bounce_factor.max(0.0);
    t.angular_velocity *= t.bounce_factor.clamp(0.0, 1.0);
    t.bounces += 1;
    play_hit_sound(t, impact_speed, time, rng, sounds);
    if normal.z >= FLOOR_NORMAL_Z && t.velocity.length() < SETTLE_SPEED {
        t.velocity = Vec3::ZERO;
        t.angular_velocity = Vec3::ZERO;
        t.flags &= !(ftent::GRAVITY | ftent::SLOWGRAVITY | ftent::ROTATE);
    }
}

/// The files of each hit sound type are the stock game's (assumption).
fn hit_sound_list(kind: u8) -> Option<&'static [&'static str]> {
    Some(match kind {
        bounce::GLASS => &[
            "debris/glass1.wav",
            "debris/glass2.wav",
            "debris/glass3.wav",
        ],
        bounce::METAL => &[
            "debris/metal1.wav",
            "debris/metal2.wav",
            "debris/metal3.wav",
        ],
        bounce::FLESH => &[
            "debris/flesh1.wav",
            "debris/flesh2.wav",
            "debris/flesh3.wav",
        ],
        bounce::WOOD => &["debris/wood1.wav", "debris/wood2.wav", "debris/wood3.wav"],
        bounce::SHRAP => &RICOCHET_SOUNDS,
        bounce::CONCRETE => &[
            "debris/concrete1.wav",
            "debris/concrete2.wav",
            "debris/concrete3.wav",
        ],
        bounce::SHELL => &[
            "player/pl_shell1.wav",
            "player/pl_shell2.wav",
            "player/pl_shell3.wav",
        ],
        bounce::SHOTSHELL => &[
            "weapons/sshell1.wav",
            "weapons/sshell2.wav",
            "weapons/sshell3.wav",
        ],
        _ => return None,
    })
}

/// A hit sound at most once per bounce, and not more often than [`HIT_SOUND_GAP`]; a shell is
/// always heard on its first bounce and then one time in three; the volume follows the speed.
fn play_hit_sound(
    t: &mut TempEnt,
    impact_speed: f32,
    time: f64,
    rng: &mut FxRandom,
    sounds: &mut SoundQueue,
) {
    let Some(list) = hit_sound_list(t.hit_sound) else {
        return;
    };
    if time - t.last_hit_sound < HIT_SOUND_GAP || impact_speed < 20.0 {
        return;
    }
    let shell = matches!(t.hit_sound, bounce::SHELL | bounce::SHOTSHELL);
    let thin_out = if shell { t.bounces > 1 } else { true };
    if thin_out && rng.long(0, 2) != 0 {
        return;
    }
    t.last_hit_sound = time;
    let base = if t.hit_sound == bounce::SHOTSHELL {
        0.5
    } else {
        0.8
    };
    let volume = (base * (impact_speed / 350.0).min(1.0)).max(0.05);
    let name = list[rng.long(0, list.len() as i32 - 1) as usize];
    sounds.push(SoundCommand::Start(SoundStart::by_name(
        name,
        0,
        chan::BODY,
        t.origin,
        volume,
        ATTN_NORM,
        PITCH_NORM,
    )));
}

/// `R_RicochetSound`: `S_StartSound( pos, 0, CHAN_AUTO, handle, VOL_NORM, 1.0, 100, 0 )`.
fn ricochet_sound(s: &mut Spawn, position: Vec3) {
    let name = RICOCHET_SOUNDS[s.rng.long(0, RICOCHET_SOUNDS.len() as i32 - 1) as usize];
    s.sounds.push(SoundCommand::Start(SoundStart::by_name(
        name,
        0,
        chan::AUTO,
        position,
        1.0,
        1.0,
        100,
    )));
}

fn decal_effect(
    precache: &PrecacheTables,
    position: Vec3,
    decal: u16,
    entity: u16,
    model: u16,
    permanent: bool,
) -> RenderEffect {
    RenderEffect::Decal {
        position,
        decal,
        decal_name: precache.decals.get(&decal).map(|e| e.name.clone()),
        entity,
        model,
        permanent,
    }
}

// The sound lists of Xash3D's `SoundList_*`. The files are the game's own; their names are not
// in the fetched sources and are the ones of the stock game (assumption).
const RICOCHET_SOUNDS: [&str; 5] = [
    "weapons/ric1.wav",
    "weapons/ric2.wav",
    "weapons/ric3.wav",
    "weapons/ric4.wav",
    "weapons/ric5.wav",
];
const EXPLODE_SOUNDS: [&str; 3] = [
    "weapons/explode3.wav",
    "weapons/explode4.wav",
    "weapons/explode5.wav",
];

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::messages::ResourceEntry;
    use cs_proto48::tempent as wire;

    fn precache_with_models(indices: &[u16]) -> PrecacheTables {
        let mut t = PrecacheTables::default();
        let entries: Vec<ResourceEntry> = indices
            .iter()
            .map(|&i| ResourceEntry {
                res_type: crate::resources::res_type::MODEL,
                filename: format!("sprites/s{i}.spr"),
                index: i,
                download_size: 0,
                flags: 0,
                md5: None,
                reserved: None,
            })
            .collect();
        t.add(&entries);
        t
    }

    /// A world whose floor is the plane `z = 0` and whose sprites have `frames` frames.
    struct Floor {
        frames: u32,
    }

    impl FxWorld for Floor {
        fn frame_count(&self, _m: &ModelRef) -> u32 {
            self.frames
        }

        fn trace_line(&self, from: Vec3, to: Vec3) -> Option<FxTrace> {
            if from.z > 0.0 && to.z <= 0.0 {
                let fraction = from.z / (from.z - to.z);
                Some(FxTrace {
                    fraction,
                    end: from + (to - from) * fraction,
                    normal: Vec3::Z,
                })
            } else {
                None
            }
        }
    }

    fn clock(time: f64) -> TempEntClock {
        TempEntClock {
            time,
            frametime: 0.05,
            gravity: 800.0,
        }
    }

    fn spawn(
        ents: &mut TempEnts,
        effect: &TempEffect,
        now: f64,
        precache: &PrecacheTables,
        world: &dyn FxWorld,
    ) -> SoundQueue {
        let mut sounds = SoundQueue::default();
        let mut rng = FxRandom::new(7);
        ents.spawn_effect(effect, now, precache, world, &mut rng, &mut sounds);
        sounds
    }

    #[test]
    fn tempents_expire() {
        let precache = precache_with_models(&[5]);
        let world = Floor { frames: 4 };
        let mut ents = TempEnts::default();
        // A glow sprite lives `life` tenths of a second.
        spawn(
            &mut ents,
            &TempEffect::GlowSprite {
                position: Vec3::new(1.0, 2.0, 3.0),
                model: 5,
                life: 10,
                scale: 5,
                brightness: 255,
            },
            100.0,
            &precache,
            &world,
        );
        assert_eq!(ents.active().len(), 1);
        let t = &ents.active()[0];
        assert_eq!(t.die, 101.0);
        assert!(t.has(ftent::FADEOUT));
        assert_eq!(t.render_mode, RenderMode::Glow);
        assert_eq!(t.scale, 0.5);

        let mut rng = FxRandom::new(1);
        let mut sounds = SoundQueue::default();
        // Alive until `die`.
        ents.update(clock(100.9), &world, &mut rng, &mut sounds);
        assert_eq!(ents.active().len(), 1);
        // The last half second fades the glow.
        assert!(ents.active()[0].render_amt < 255.0);
        assert!(ents.active()[0].render_amt > 0.0);
        // Past `die` it is gone.
        ents.update(clock(101.1), &world, &mut rng, &mut sounds);
        assert!(ents.active().is_empty());
    }

    #[test]
    fn an_entry_without_fade_dies_at_its_time() {
        let world = NoWorld;
        let mut ents = TempEnts::default();
        let t = ents
            .alloc(Vec3::ZERO, ModelRef::Client("models/x.mdl"), &world, 10.0)
            .unwrap();
        assert_eq!(t.die, 10.0 + DEFAULT_LIFE);
        let mut rng = FxRandom::default();
        let mut sounds = SoundQueue::default();
        ents.update(clock(10.5), &world, &mut rng, &mut sounds);
        assert_eq!(ents.active().len(), 1);
        ents.update(clock(10.8), &world, &mut rng, &mut sounds);
        assert!(ents.active().is_empty());
    }

    #[test]
    fn persistent_entries_outlive_their_time() {
        let world = NoWorld;
        let mut ents = TempEnts::default();
        let t = ents
            .alloc(Vec3::ZERO, ModelRef::Client("models/x.mdl"), &world, 0.0)
            .unwrap();
        t.flags |= ftent::PERSIST;
        ents.update(
            clock(500.0),
            &world,
            &mut FxRandom::default(),
            &mut SoundQueue::default(),
        );
        assert_eq!(ents.active().len(), 1);
    }

    #[test]
    fn a_full_pool_refuses_low_and_evicts_for_high() {
        let world = NoWorld;
        let mut ents = TempEnts::with_capacity(3);
        for i in 0..3 {
            assert!(
                ents.alloc(Vec3::splat(i as f32), ModelRef::None, &world, 0.0)
                    .is_some()
            );
        }
        assert!(
            ents.alloc(Vec3::ZERO, ModelRef::None, &world, 0.0)
                .is_none()
        );
        assert_eq!(ents.overflows, 1);
        // The newest low priority entry is the one given up (the head of Xash3D's list).
        let high = ents.alloc_high(Vec3::splat(9.0), ModelRef::None, &world, 0.0);
        assert!(high.is_some());
        let origins: Vec<f32> = ents.active().iter().map(|t| t.origin.x).collect();
        assert_eq!(origins, vec![0.0, 1.0, 9.0]);
        // Now one is high; two lows left. Fill with highs until none can be evicted.
        assert!(
            ents.alloc_high(Vec3::ZERO, ModelRef::None, &world, 0.0)
                .is_some()
        );
        assert!(
            ents.alloc_high(Vec3::ZERO, ModelRef::None, &world, 0.0)
                .is_some()
        );
        assert!(
            ents.alloc_high(Vec3::ZERO, ModelRef::None, &world, 0.0)
                .is_none()
        );
        assert!(ents.active().iter().all(|t| t.priority == Priority::High));
    }

    #[test]
    fn a_sprite_effect_needs_a_precached_model() {
        let world = Floor { frames: 2 };
        let mut ents = TempEnts::default();
        spawn(
            &mut ents,
            &TempEffect::Sprite {
                position: Vec3::ZERO,
                sprite: 9,
                scale: 10,
                brightness: 128,
            },
            0.0,
            &PrecacheTables::default(),
            &world,
        );
        assert!(ents.active().is_empty());
        assert_eq!(ents.missing_models, 1);

        spawn(
            &mut ents,
            &TempEffect::Sprite {
                position: Vec3::ZERO,
                sprite: 9,
                scale: 10,
                brightness: 128,
            },
            0.0,
            &precache_with_models(&[9]),
            &world,
        );
        let t = &ents.active()[0];
        assert_eq!(t.render_mode, RenderMode::TransAdd);
        assert_eq!(t.scale, 1.0);
        assert!((t.render_amt - 128.0).abs() < 1e-3);
        assert_eq!(t.frame_max, 1);
        // `R_TempSprite` with no life: the animation, plus a second.
        assert!((t.die - (0.1 + 1.0)).abs() < 1e-6);
    }

    #[test]
    fn an_explosion_makes_a_sprite_two_lights_and_a_sound() {
        let world = Floor { frames: 10 };
        let precache = precache_with_models(&[3]);
        let mut ents = TempEnts::default();
        let sounds = spawn(
            &mut ents,
            &TempEffect::Explosion {
                position: Vec3::new(0.0, 0.0, 100.0),
                sprite: 3,
                scale: 30,
                framerate: 15,
                flags: 0,
            },
            5.0,
            &precache,
            &world,
        );
        let t = &ents.active()[0];
        assert_eq!(t.render_mode, RenderMode::TransAdd);
        assert_eq!(t.render_amt, 180.0);
        assert_eq!(t.origin.z, 110.0);
        assert_eq!(t.velocity.z, 8.0);
        assert_eq!(t.scale, 3.0);
        assert!((t.die - (5.0 + 9.0 / 15.0)).abs() < 1e-6);
        let effects = ents.take_effects();
        let lights = effects
            .iter()
            .filter(|e| matches!(e, RenderEffect::Light(_)))
            .count();
        assert_eq!(lights, 2);
        assert!(matches!(sounds.pending()[0], SoundCommand::Ambient(_)));

        // The no-sound, no-light, no-particle flags.
        let mut quiet = TempEnts::default();
        let sounds = spawn(
            &mut quiet,
            &TempEffect::Explosion {
                position: Vec3::ZERO,
                sprite: 3,
                scale: 30,
                framerate: 15,
                flags: explflag::NOSOUND | explflag::NODLIGHTS | explflag::NOPARTICLES,
            },
            5.0,
            &precache,
            &world,
        );
        assert!(sounds.pending().is_empty());
        assert!(quiet.take_effects().is_empty());
    }

    #[test]
    fn a_sprite_animation_ends_the_entry() {
        let world = Floor { frames: 5 };
        let precache = precache_with_models(&[4]);
        let mut ents = TempEnts::default();
        spawn(
            &mut ents,
            &TempEffect::Smoke {
                position: Vec3::ZERO,
                sprite: 4,
                scale: 20,
                framerate: 10,
            },
            0.0,
            &precache,
            &world,
        );
        let t = &ents.active()[0];
        assert_eq!(t.render_mode, RenderMode::TransAlpha);
        assert_eq!(t.velocity.z, 30.0);
        assert_eq!(t.origin.z, 20.0);
        assert!((20..=35).contains(&t.render_color[0]));
        let mut rng = FxRandom::default();
        let mut sounds = SoundQueue::default();
        // Four frames to play at 10 per second: 0.4 s.
        for step in 1..=7 {
            ents.update(clock(f64::from(step) * 0.05), &world, &mut rng, &mut sounds);
        }
        assert_eq!(ents.active().len(), 1);
        ents.update(clock(0.5), &world, &mut rng, &mut sounds);
        assert!(ents.active().is_empty());
    }

    #[test]
    fn a_shell_with_no_velocity_only_falls() {
        let world = Floor { frames: 1 };
        let mut ents = TempEnts::default();
        let mut rng = FxRandom::new(3);
        let mut sounds = SoundQueue::default();
        assert!(ents.spawn_temp_model(
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::ZERO,
            Vec3::ZERO,
            2.5,
            ModelRef::Client("models/rshell.mdl"),
            te_bounce::SHELL,
            &world,
            &mut rng,
            0.0,
        ));
        ents.update(clock(1.0), &world, &mut rng, &mut sounds);
        assert!(ents.active()[0].origin.z < 100.0);
        assert!(ents.active()[0].origin.z > 0.0);
        assert!(sounds.pending().is_empty());
        ents.update(clock(2.6), &world, &mut rng, &mut sounds);
        assert!(ents.active().is_empty());
    }

    #[test]
    fn blood_sprite_makes_one_large_sprite_and_a_spatter() {
        let world = Floor { frames: 6 };
        let precache = precache_with_models(&[10, 11]);
        let mut ents = TempEnts::default();
        spawn(
            &mut ents,
            &TempEffect::BloodSprite {
                position: Vec3::new(0.0, 0.0, 10.0),
                spray: 10,
                drop: 11,
                color: 247,
                scale: 4,
            },
            0.0,
            &precache,
            &world,
        );
        let big: Vec<_> = ents
            .active()
            .iter()
            .filter(|t| t.priority == Priority::High)
            .collect();
        assert_eq!(big.len(), 1);
        assert_eq!(big[0].render_mode, RenderMode::TransTexture);
        assert_eq!(big[0].bounce_factor, 0.0);
        assert!((248..=250).contains(&big[0].palette_color.unwrap()));
        assert!((big[0].die - 0.25).abs() < 1e-6);
        let drops = ents.active().len() - 1;
        // `size + two d8`: 4 + 2..=16.
        assert!((6..=20).contains(&drops), "{drops}");
        let d = ents
            .active()
            .iter()
            .find(|t| t.priority == Priority::Low)
            .unwrap();
        assert!(d.has(ftent::ROTATE | ftent::SLOWGRAVITY | ftent::COLLIDEWORLD));
        assert!(d.frame >= 1.0 && d.frame <= 5.0);
    }

    #[test]
    fn break_model_fills_a_box_with_shards() {
        let world = Floor { frames: 3 };
        let precache = precache_with_models(&[20]);
        let mut ents = TempEnts::default();
        spawn(
            &mut ents,
            &TempEffect::BreakModel {
                position: Vec3::new(0.0, 0.0, 100.0),
                size: Vec3::new(40.0, 40.0, 40.0),
                velocity: Vec3::new(0.0, 0.0, 50.0),
                random_velocity: 3,
                model: 20,
                count: 12,
                life: 20,
                flags: bounce::GLASS,
            },
            0.0,
            &precache,
            &world,
        );
        assert_eq!(ents.active().len(), 12);
        for t in ents.active() {
            assert_eq!(t.render_mode, RenderMode::TransTexture);
            assert_eq!(t.base_render_amt, 128.0);
            assert_eq!(t.hit_sound, bounce::GLASS);
            assert!(t.has(ftent::FADEOUT | ftent::COLLIDEWORLD | ftent::SLOWGRAVITY));
            assert!(t.die >= 2.0 && t.die <= 3.0);
            assert!((t.origin - Vec3::new(0.0, 0.0, 100.0)).abs().max_element() <= 20.0);
        }
    }

    #[test]
    fn sparks_are_a_modelless_shower() {
        let mut ents = TempEnts::default();
        spawn(
            &mut ents,
            &TempEffect::Sparks {
                position: Vec3::new(1.0, 1.0, 1.0),
            },
            2.0,
            &PrecacheTables::default(),
            &NoWorld,
        );
        let t = &ents.active()[0];
        assert!(t.has(ftent::SPARKSHOWER | ftent::NOMODEL | ftent::SLOWGRAVITY));
        assert_eq!(t.die, 2.5);
        assert_eq!(ents.render_list().count(), 0);
    }

    #[test]
    fn decals_lights_and_beams_leave_as_effects() {
        let mut precache = PrecacheTables::default();
        precache.add(&[ResourceEntry {
            res_type: crate::resources::res_type::DECAL,
            filename: "{shot1".into(),
            index: 42,
            download_size: 0,
            flags: 0,
            md5: None,
            reserved: None,
        }]);
        let mut ents = TempEnts::default();
        let mut sounds = SoundQueue::default();
        let mut rng = FxRandom::new(11);
        let p = Vec3::new(8.0, 16.0, 24.0);
        for effect in [
            TempEffect::GunshotDecal {
                position: p,
                entity: 5,
                decal: 42,
            },
            TempEffect::DecalHigh {
                position: p,
                decal: 1,
                entity: 0,
            },
            TempEffect::Dlight {
                position: p,
                radius: 20,
                color: [1, 2, 3],
                life: 5,
                decay: 4,
            },
            TempEffect::Tracer { start: p, end: p },
        ] {
            ents.spawn_effect(&effect, 10.0, &precache, &NoWorld, &mut rng, &mut sounds);
        }
        let effects = ents.take_effects();
        assert!(ents.take_effects().is_empty());
        assert_eq!(
            effects[0],
            RenderEffect::Decal {
                position: p,
                decal: 42,
                decal_name: Some("{shot1".into()),
                entity: 5,
                model: 0,
                permanent: false,
            }
        );
        assert!(matches!(
            effects[1],
            RenderEffect::Particles(ParticleEffect::BulletImpact { .. })
        ));
        assert!(matches!(
            &effects[2],
            RenderEffect::Decal { decal: 257, .. }
        ));
        let RenderEffect::Light(l) = &effects[3] else {
            panic!("{:?}", effects[3]);
        };
        assert_eq!((l.radius, l.decay, l.color), (200.0, 40.0, [1, 2, 3]));
        assert!((l.die - 10.5).abs() < 1e-6);
        assert!(matches!(effects[4], RenderEffect::Beam { .. }));
    }

    #[test]
    fn wire_effects_decode_into_recipes() {
        // The same path the stream takes: bytes, `TempEffect::parse`, the recipe.
        let te = wire::explosion(Vec3::new(8.0, 8.0, 8.0), 3, 20, 10, 0);
        let effect = TempEffect::parse(te.te_type, &te.payload).unwrap();
        let mut ents = TempEnts::default();
        spawn(
            &mut ents,
            &effect,
            0.0,
            &precache_with_models(&[3]),
            &Floor { frames: 4 },
        );
        assert_eq!(ents.active().len(), 1);
        assert_eq!(ents.active()[0].scale, 2.0);
    }

    #[test]
    fn smoke_clouds_live_30_seconds_and_go_with_the_round() {
        let world = NoWorld;
        let mut ents = TempEnts::default();
        let mut rng = FxRandom::new(6);
        ents.spawn_smoke_clouds(Vec3::new(10.0, 20.0, 0.0), &world, &mut rng, 100.0);
        assert_eq!(ents.active().len(), 20);
        assert_eq!(ents.active()[0].origin, Vec3::new(10.0, 20.0, 30.0));
        assert!(
            ents.active()
                .iter()
                .all(|t| t.die == 130.0 && t.round_scoped)
        );
        assert!(
            ents.active()
                .iter()
                .all(|t| t.late_fade == Some((125.0, 51.0)))
        );
        assert!(ents.active().iter().all(|t| {
            t.render_mode == RenderMode::TransAlpha
                && t.render_color[0] <= 150
                && t.model == ModelRef::Client(GAS_PUFF_SPRITE)
        }));
        let mut sounds = SoundQueue::default();
        ents.update(clock(124.0), &world, &mut rng, &mut sounds);
        assert_eq!(ents.active().len(), 20);
        assert!(
            ents.active()
                .iter()
                .all(|t| (t.render_amt - 255.0).abs() < 1e-4)
        );
        // Fade window: 25s..30s -> at 127.5s (~2.5s into fade), alpha is roughly half.
        ents.update(
            TempEntClock {
                time: 127.5,
                frametime: 2.5,
                gravity: 800.0,
            },
            &world,
            &mut rng,
            &mut sounds,
        );
        assert!(
            ents.active()
                .iter()
                .all(|t| t.render_amt > 100.0 && t.render_amt < 150.0)
        );
        ents.clear_round();
        assert!(ents.active().is_empty());
    }

    #[test]
    fn smoke_clouds_honor_max_smokepuffs_and_fastsprites() {
        let world = NoWorld;
        let mut rng = FxRandom::new(9);
        let mut ents = TempEnts::default();
        ents.spawn_smoke_clouds_ex(
            Vec3::ZERO,
            SmokeCloudConfig {
                light_smoke: false,
                max_smokepuffs: 7,
                fastsprites: 0,
            },
            &world,
            &mut rng,
            10.0,
        );
        assert_eq!(ents.active_smoke_puff_count(), 7);

        let mut fast1 = TempEnts::default();
        fast1.spawn_smoke_clouds_ex(
            Vec3::ZERO,
            SmokeCloudConfig {
                light_smoke: false,
                max_smokepuffs: 120,
                fastsprites: 1,
            },
            &world,
            &mut rng,
            10.0,
        );
        assert_eq!(fast1.active_smoke_puff_count(), 10);

        let mut fast2 = TempEnts::default();
        fast2.spawn_smoke_clouds_ex(
            Vec3::ZERO,
            SmokeCloudConfig {
                light_smoke: true,
                max_smokepuffs: 120,
                fastsprites: 2,
            },
            &world,
            &mut rng,
            10.0,
        );
        assert_eq!(fast2.active_smoke_puff_count(), 5);
        assert!(fast2.active().iter().all(|t| t.render_color[0] >= 160));
    }

    #[test]
    fn the_generator_is_deterministic_and_bounded() {
        let mut a = FxRandom::new(5);
        let mut b = FxRandom::new(5);
        for _ in 0..100 {
            let x = a.long(-3, 4);
            assert_eq!(x, b.long(-3, 4));
            assert!((-3..=4).contains(&x));
            let f = a.float(1.0, 2.0);
            assert_eq!(f, b.float(1.0, 2.0));
            assert!((1.0..2.0).contains(&f));
        }
        assert_eq!(a.long(5, 5), 5);
        assert_eq!(a.long(5, 1), 5);
    }

    fn shell_ent(ents: &mut TempEnts, world: &dyn FxWorld, pos: Vec3, vel: Vec3) {
        let mut rng = FxRandom::new(3);
        assert!(ents.spawn_temp_model(
            pos,
            vel,
            Vec3::ZERO,
            10.0,
            ModelRef::Client("models/rshell.mdl"),
            te_bounce::SHELL,
            world,
            &mut rng,
            0.0,
        ));
    }

    fn run(ents: &mut TempEnts, world: &dyn FxWorld, from: f64, steps: usize) -> SoundQueue {
        let mut rng = FxRandom::new(9);
        let mut sounds = SoundQueue::default();
        for i in 0..steps {
            ents.update(clock(from + 0.05 * i as f64), world, &mut rng, &mut sounds);
        }
        sounds
    }

    #[test]
    fn a_shell_falls_and_rests_on_the_floor() {
        let world = Floor { frames: 1 };
        let mut ents = TempEnts::default();
        shell_ent(
            &mut ents,
            &world,
            Vec3::new(0.0, 0.0, 20.0),
            Vec3::new(30.0, 0.0, 50.0),
        );
        let start_z = ents.active()[0].origin.z;
        run(&mut ents, &world, 0.0, 120);
        let t = &ents.active()[0];
        assert!(t.origin.z < start_z);
        assert!(
            t.origin.z >= -0.01 && t.origin.z < 1.0,
            "z = {}",
            t.origin.z
        );
        assert_eq!(t.velocity, Vec3::ZERO);
        assert!(!t.has(ftent::GRAVITY));
        assert!(t.origin.x > 0.0);
    }

    #[test]
    fn a_bounce_scales_speed_by_the_bounce_factor() {
        let world = Floor { frames: 1 };
        let mut ents = TempEnts::default();
        let mut rng = FxRandom::new(1);
        let mut sounds = SoundQueue::default();
        let t = ents
            .alloc(Vec3::new(0.0, 0.0, 1.0), ModelRef::None, &world, 0.0)
            .unwrap();
        t.flags = ftent::COLLIDEWORLD;
        t.bounce_factor = 0.4;
        t.velocity = Vec3::new(0.0, 0.0, -500.0);
        t.die = 100.0;
        ents.update(clock(0.0), &world, &mut rng, &mut sounds);
        let t = &ents.active()[0];
        assert!((t.velocity.z - 200.0).abs() < 0.01, "{:?}", t.velocity);
    }

    #[test]
    fn no_world_means_no_collision() {
        let mut ents = TempEnts::default();
        shell_ent(&mut ents, &NoWorld, Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        run(&mut ents, &NoWorld, 0.0, 20);
        let t = &ents.active()[0];
        assert!(t.origin.z < -10.0);
        assert!(t.velocity.z < 0.0);
    }

    #[test]
    fn fade_reaches_zero_at_die() {
        let world = NoWorld;
        let mut ents = TempEnts::default();
        let t = ents.alloc(Vec3::ZERO, ModelRef::None, &world, 0.0).unwrap();
        t.flags |= ftent::FADEOUT;
        t.die = 2.0;
        let mut rng = FxRandom::new(1);
        let mut sounds = SoundQueue::default();
        ents.update(clock(1.0), &world, &mut rng, &mut sounds);
        assert_eq!(ents.active()[0].render_amt, 255.0);
        ents.update(clock(1.75), &world, &mut rng, &mut sounds);
        let mid = ents.active()[0].render_amt;
        assert!(mid > 0.0 && mid < 255.0);
        ents.update(clock(2.0), &world, &mut rng, &mut sounds);
        assert_eq!(ents.active()[0].render_amt, 0.0);
    }

    #[test]
    fn a_shell_clinks_on_its_first_bounce() {
        let world = Floor { frames: 1 };
        let mut ents = TempEnts::default();
        shell_ent(
            &mut ents,
            &world,
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::new(0.0, 0.0, -300.0),
        );
        let sounds = run(&mut ents, &world, 0.0, 4);
        assert!(!sounds.pending().is_empty());
    }

    #[test]
    fn degenerate_inputs_give_no_nan() {
        struct ZeroNormal;
        impl FxWorld for ZeroNormal {
            fn trace_line(&self, from: Vec3, to: Vec3) -> Option<FxTrace> {
                Some(FxTrace {
                    fraction: 0.5,
                    end: (from + to) * 0.5,
                    normal: Vec3::ZERO,
                })
            }
        }
        let mut rng = FxRandom::new(1);
        let mut sounds = SoundQueue::default();
        for world in [&ZeroNormal as &dyn FxWorld, &NoWorld] {
            let mut ents = TempEnts::default();
            shell_ent(
                &mut ents,
                world,
                Vec3::new(0.0, 0.0, 5.0),
                Vec3::new(10.0, 0.0, -10.0),
            );
            for ft in [0.0_f32, 0.05, f32::NAN] {
                let c = TempEntClock {
                    time: 0.1,
                    frametime: ft,
                    gravity: 800.0,
                };
                ents.update(c, world, &mut rng, &mut sounds);
            }
            let t = &ents.active()[0];
            assert!(t.origin.is_finite() && t.velocity.is_finite() && t.angles.is_finite());
        }
    }

    fn sample_59_temp_effects() -> Vec<wire::TempEntity> {
        let p = Vec3::new(0.0, 0.0, 16.0);
        let q = Vec3::new(32.0, 16.0, 24.0);
        let beam = wire::BeamParams {
            sprite: 1,
            start_frame: 0,
            frame_rate: 10,
            life: 10,
            width: 8,
            noise: 2,
            color: [255, 128, 64],
            brightness: 255,
            speed: 5,
        };
        vec![
            wire::beam_points(p, q, &beam),
            wire::beam_ent_point(1, q, &beam),
            wire::gunshot(p),
            wire::explosion(p, 1, 10, 15, 0),
            wire::tar_explosion(p),
            wire::smoke(p, 1, 10, 10),
            wire::tracer(p, q),
            wire::lightning(p, q, 5, 10, 4, 1),
            wire::beam_ents(1, 2, &beam),
            wire::sparks(p),
            wire::lava_splash(p),
            wire::teleport(p),
            wire::explosion2(p, 10, 8),
            wire::bsp_decal(p, 1, 0, None),
            wire::implosion(p, 32, 12, 5),
            wire::sprite_trail(p, q, 1, 3, 5, 4, 2, 10),
            wire::sprite(p, 1, 10, 255),
            wire::beam_sprite(p, q, 1, 2),
            wire::beam_torus(p, q, &beam),
            wire::beam_disk(p, q, &beam),
            wire::beam_cylinder(p, q, &beam),
            wire::beam_follow(1, 1, 5, 4, [255, 0, 0], 200),
            wire::glow_sprite(p, 1, 5, 5, 255),
            wire::beam_ring(1, 2, &beam),
            wire::streak_splash(p, q, 4, 6, 100, 50),
            wire::dlight(p, 20, [255, 255, 200], 5, 10),
            wire::elight(1, p, 80.0, [255, 0, 0], 5, 10.0),
            wire::text_message(1, 0, 0, 0, [255; 4], [255; 4], 10, 10, 50, None, "hi"),
            wire::line(p, q, 10, [255, 0, 0]),
            wire::box_fx(p, q, 10, [0, 255, 0]),
            wire::kill_beam(1),
            wire::large_funnel(p, 1, 0),
            wire::blood_stream(p, q, 70, 50),
            wire::show_line(p, q),
            wire::blood(p, q, 70, 80),
            wire::decal(p, 1, 0),
            wire::fizz(1, 1, 10),
            wire::model(p, Vec3::Z, 0, 1, 1, 10),
            wire::explode_model(p, 50.0, 1, 3, 10),
            wire::break_model(p, Vec3::splat(8.0), Vec3::Z, 10, 1, 2, 10, 0),
            wire::gunshot_decal(p, 0, 1),
            wire::sprite_spray(p, q, 1, 3, 20, 5),
            wire::armor_ricochet(p, 5),
            wire::player_decal(1, p, 0, 0),
            wire::bubbles(p, q, 64.0, 1, 3, 8.0),
            wire::bubble_trail(p, q, 64.0, 1, 3, 8.0),
            wire::blood_sprite(p, 1, 2, 70, 4),
            wire::world_decal(p, 1),
            wire::world_decal_high(p, 1),
            wire::decal_high(p, 1, 0),
            wire::projectile(p, Vec3::X * 50.0, 1, 10, 1),
            wire::spray(p, Vec3::Z, 1, 3, 20, 5, 5),
            wire::player_sprites(1, 1, 2, 4),
            wire::particle_burst(p, 40, 70, 5),
            wire::fire_field(p, 32, 1, 3, 0, 10),
            wire::player_attachment(1, 35.0, 1, 15),
            wire::multi_gunshot(p, Vec3::X, 0.1, 0.1, 3, 1),
            wire::user_tracer(p, Vec3::X * 100.0, 5, 4, 20),
            wire::kill_player_attachments(1),
        ]
    }

    fn spawn_category(filter: fn(u8) -> bool) -> (TempEnts, Vec<RenderEffect>, usize) {
        let precache = precache_with_models(&[1, 2, 3]);
        let world = Floor { frames: 8 };
        let mut ents = TempEnts::default();
        let mut sounds = SoundQueue::default();
        let mut rng = FxRandom::new(9);
        let mut count = 0usize;
        for item in &sample_59_temp_effects() {
            if !filter(item.te_type) {
                continue;
            }
            let eff = item.decode().unwrap();
            assert!(!matches!(eff, TempEffect::Other { .. }));
            ents.spawn_effect(&eff, 1.0, &precache, &world, &mut rng, &mut sounds);
            count += 1;
        }
        ents.update(clock(1.05), &world, &mut rng, &mut sounds);
        let effects = ents.take_effects();
        (ents, effects, count)
    }

    fn is_beam_or_line(id: u8) -> bool {
        use wire::te;
        matches!(
            id,
            te::BEAMPOINTS
                | te::BEAMENTPOINT
                | te::LIGHTNING
                | te::BEAMENTS
                | te::BEAMSPRITE
                | te::BEAMTORUS
                | te::BEAMDISK
                | te::BEAMCYLINDER
                | te::BEAMFOLLOW
                | te::BEAMRING
                | te::LINE
                | te::BOX
                | te::KILLBEAM
                | te::SHOWLINE
        )
    }

    fn is_explosion_or_light(id: u8) -> bool {
        use wire::te;
        matches!(
            id,
            te::EXPLOSION
                | te::TAREXPLOSION
                | te::SMOKE
                | te::SPARKS
                | te::LAVASPLASH
                | te::TELEPORT
                | te::EXPLOSION2
                | te::IMPLOSION
                | te::DLIGHT
                | te::ELIGHT
                | te::LARGEFUNNEL
                | te::FIZZ
                | te::EXPLODEMODEL
                | te::FIREFIELD
        )
    }

    fn is_gunshot_blood_or_tracer(id: u8) -> bool {
        use wire::te;
        matches!(
            id,
            te::GUNSHOT
                | te::TRACER
                | te::STREAK_SPLASH
                | te::BLOODSTREAM
                | te::BLOOD
                | te::MODEL
                | te::BREAKMODEL
                | te::GUNSHOTDECAL
                | te::ARMOR_RICOCHET
                | te::BLOODSPRITE
                | te::PROJECTILE
                | te::SPRAY
                | te::PARTICLEBURST
                | te::MULTIGUNSHOT
                | te::USERTRACER
        )
    }

    fn is_decal_sprite_hud_or_attachment(id: u8) -> bool {
        use wire::te;
        matches!(
            id,
            te::BSPDECAL
                | te::SPRITETRAIL
                | te::SPRITE
                | te::GLOWSPRITE
                | te::TEXTMESSAGE
                | te::DECAL
                | te::SPRITE_SPRAY
                | te::PLAYERDECAL
                | te::BUBBLES
                | te::BUBBLETRAIL
                | te::WORLDDECAL
                | te::WORLDDECALHIGH
                | te::DECALHIGH
                | te::PLAYERSPRITES
                | te::PLAYERATTACHMENT
                | te::KILLPLAYERATTACHMENTS
        )
    }

    #[test]
    fn te_beams_and_lines_produce_effects_and_kill_beam() {
        let (_ents, effects, count) = spawn_category(is_beam_or_line);
        assert_eq!(count, 14);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, RenderEffect::Beam { .. }))
        );
        assert!(effects.iter().any(|e| matches!(
            e,
            RenderEffect::Beam {
                effect: TempEffect::KillBeam { entity: 1 },
                ..
            }
        )));
    }

    #[test]
    fn te_explosions_sparks_and_lights_produce_effects_and_dlights() {
        let (_ents, effects, count) = spawn_category(is_explosion_or_light);
        assert_eq!(count, 14);
        assert!(effects.iter().any(|e| matches!(e, RenderEffect::Light(_))));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, RenderEffect::Particles(_)))
        );
    }

    #[test]
    fn te_gunshots_blood_tracers_and_brass_drive_tempents() {
        let (mut ents, effects, count) = spawn_category(is_gunshot_blood_or_tracer);
        assert_eq!(count, 15);
        assert!(!effects.is_empty());
        assert!(!ents.active().is_empty());

        let precache = precache_with_models(&[1, 2, 3]);
        let world = Floor { frames: 8 };
        let mut sounds = SoundQueue::default();
        let mut rng = FxRandom::new(9);
        let p = Vec3::new(0.0, 0.0, 16.0);
        let brass = wire::model(p, Vec3::Y * 40.0, 10, 1, 1, 10);
        ents.spawn_brass_usermsg(&brass.payload, 2.0, &precache, &world, &mut rng);
        ents.spawn_particle_msg(&cs_proto48::messages::ParticleMsg {
            origin: p,
            direction: [0, 0, 16],
            count: 25,
            color: 70,
        });
        ents.update(clock(2.1), &world, &mut rng, &mut sounds);
        assert!(!ents.take_effects().is_empty());
    }

    #[test]
    fn te_decals_sprites_hud_and_player_attachments_drive_tempents() {
        let (mut ents, effects, count) = spawn_category(is_decal_sprite_hud_or_attachment);
        assert_eq!(count, 16);
        assert!(!effects.is_empty());

        let precache = precache_with_models(&[1, 2, 3]);
        let world = Floor { frames: 8 };
        let mut sounds = SoundQueue::default();
        let mut rng = FxRandom::new(9);
        ents.spawn_effect(
            &TempEffect::PlayerAttachment {
                entity: 2,
                offset_z: 30.0,
                model: 1,
                life: 20,
            },
            2.0,
            &precache,
            &world,
            &mut rng,
            &mut sounds,
        );
        assert!(ents.active().iter().any(|t| t.client_entity == 2));
        ents.kill_player_attachments(2);
        assert!(!ents.active().iter().any(|t| t.client_entity == 2));
    }

    #[test]
    fn temp_effects_cover_all_59_active_ids() {
        let mut seen = std::collections::BTreeSet::new();
        for f in [
            is_beam_or_line as fn(u8) -> bool,
            is_explosion_or_light,
            is_gunshot_blood_or_tracer,
            is_decal_sprite_hud_or_attachment,
        ] {
            for item in &sample_59_temp_effects() {
                if f(item.te_type) {
                    assert!(seen.insert(item.te_type), "duplicate TE {}", item.te_type);
                }
            }
        }
        assert_eq!(seen.len(), 59);
    }

    #[test]
    fn blood_sprite_populates_crimson_palette_rgb_not_white() {
        for idx in 247u8..=251u8 {
            let rgb = goldsrc_palette_rgb(idx);
            assert!(
                rgb[0] > 100 && rgb[1] < 30 && rgb[2] < 30,
                "expected crimson for palette index {idx}, got {rgb:?}"
            );
        }

        let mut ents = TempEnts::default();
        let precache = precache_with_models(&[10, 11]);
        let world = Floor { frames: 8 };
        let mut sounds = SoundQueue::default();
        let mut rng = FxRandom::new(42);
        ents.spawn_effect(
            &TempEffect::BloodSprite {
                position: Vec3::new(0.0, 0.0, 32.0),
                spray: 10,
                drop: 11,
                color: 247,
                scale: 12,
            },
            1.0,
            &precache,
            &world,
            &mut rng,
            &mut sounds,
        );
        assert!(!ents.active().is_empty(), "expected impact spray + drops");
        for t in ents.active() {
            assert!(
                t.palette_color.is_some(),
                "blood tempent must preserve palette_color"
            );
            assert!(
                t.render_color[0] > 100 && t.render_color[1] < 30 && t.render_color[2] < 30,
                "blood tempent render_color must be dark crimson, not white: {:?}",
                t.render_color
            );
        }
    }
}
