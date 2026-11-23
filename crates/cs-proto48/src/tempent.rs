//! Builders for the `svc_temp_entity` effects the weapons and grenades use: one function per
//! `TE_*`, each returning the effect byte and the payload bytes after it, in the byte layout
//! of the engine's temp entity parser.
//!
//! Layouts follow the comments of `TE_*` in HL SDK `common/const.h`, read through ReGameDLL_CS
//! `regamedll/common/const.h` on 2026-11-22, and the way ReGameDLL_CS writes each message
//! (`regamedll/dlls/util.cpp`: `UTIL_BloodStream`, `UTIL_BloodDrips`, `UTIL_DecalTrace`,
//! `UTIL_GunshotDecalTrace`, `UTIL_Sparks`; `ggrenade.cpp`: `Explode`, `SmokeEffect`;
//! `player.cpp`: the helmet `TE_STREAK_SPLASH`, read 2026-11-22). A coordinate is a signed
//! 16-bit fixed point `f * 8` (`MSG_WriteCoord`, see [`MsgWriter::write_coord`]); a `short` is
//! little endian.
//!
//! Send the result with `EngineApi::send_temp_entity` or `EngineApi::temp_entity`.
//!
//! The client side is [`TempEffect::parse`]: it decodes the `te_type` and payload of a
//! received `SvcMessage::TempEntity` into a typed [`TempEffect`]. The layouts were checked
//! against Xash3D FWGS `engine/client/cl_tent.c` `CL_ParseTempEntity` (GPL-3.0-or-later, read
//! 2026-11-23). An effect without a typed variant keeps its payload in
//! [`TempEffect::Other`]; its length is known to the message parser, which keeps the stream
//! in step.

use glam::Vec3;

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};

/// The `TE_*` effect numbers used here (`common/const.h`).
pub mod te {
    pub const BEAMPOINTS: u8 = 0;
    pub const BEAMENTPOINT: u8 = 1;
    pub const GUNSHOT: u8 = 2;
    pub const EXPLOSION: u8 = 3;
    pub const SMOKE: u8 = 5;
    pub const TRACER: u8 = 6;
    pub const BEAMENTS: u8 = 8;
    pub const SPARKS: u8 = 9;
    pub const EXPLOSION2: u8 = 12;
    pub const BSPDECAL: u8 = 13;
    pub const SPRITE: u8 = 17;
    pub const BEAMFOLLOW: u8 = 22;
    pub const GLOWSPRITE: u8 = 23;
    pub const STREAK_SPLASH: u8 = 25;
    pub const DLIGHT: u8 = 27;
    pub const ELIGHT: u8 = 28;
    pub const KILLBEAM: u8 = 99;
    pub const BLOODSTREAM: u8 = 101;
    pub const DECAL: u8 = 104;
    pub const FIZZ: u8 = 105;
    pub const BREAKMODEL: u8 = 108;
    pub const GUNSHOTDECAL: u8 = 109;
    pub const BLOODSPRITE: u8 = 115;
    pub const WORLDDECAL: u8 = 116;
    pub const WORLDDECALHIGH: u8 = 117;
    pub const DECALHIGH: u8 = 118;
}

/// `TE_EXPLFLAG_*`: the `flags` byte of `TE_EXPLOSION`.
pub mod explflag {
    /// All flags clear: the default Half-Life explosion.
    pub const NONE: u8 = 0;
    pub const NOADDITIVE: u8 = 1;
    pub const NODLIGHTS: u8 = 2;
    pub const NOSOUND: u8 = 4;
    pub const NOPARTICLES: u8 = 8;
}

/// One temp entity: the `TE_*` byte and the payload that follows it in `svc_temp_entity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempEntity {
    pub te_type: u8,
    pub payload: Vec<u8>,
}

impl TempEntity {
    fn new(te_type: u8, build: impl FnOnce(&mut MsgWriter)) -> Self {
        let mut w = MsgWriter::new();
        build(&mut w);
        Self {
            te_type,
            payload: w.into_bytes(),
        }
    }
}

fn coords(w: &mut MsgWriter, v: Vec3) {
    w.write_coord(v.x);
    w.write_coord(v.y);
    w.write_coord(v.z);
}

/// `TE_GUNSHOT`: particles and a ricochet sound. `coord x3` (position).
pub fn gunshot(position: Vec3) -> TempEntity {
    TempEntity::new(te::GUNSHOT, |w| coords(w, position))
}

/// `TE_SPARKS` (`UTIL_Sparks`): `coord x3` (position).
pub fn sparks(position: Vec3) -> TempEntity {
    TempEntity::new(te::SPARKS, |w| coords(w, position))
}

/// `TE_EXPLOSION` (`CGrenade::Explode`): `coord x3`, `short` sprite index, `byte` scale in
/// tenths, `byte` frame rate, `byte` flags (see [`explflag`]).
pub fn explosion(position: Vec3, sprite: u16, scale: u8, framerate: u8, flags: u8) -> TempEntity {
    TempEntity::new(te::EXPLOSION, |w| {
        coords(w, position);
        w.write_u16(sprite);
        w.write_u8(scale);
        w.write_u8(framerate);
        w.write_u8(flags);
    })
}

/// `TE_SMOKE` (`CGrenade::SmokeEffect`): `coord x3`, `short` sprite index, `byte` scale in
/// tenths, `byte` frame rate.
pub fn smoke(position: Vec3, sprite: u16, scale: u8, framerate: u8) -> TempEntity {
    TempEntity::new(te::SMOKE, |w| {
        coords(w, position);
        w.write_u16(sprite);
        w.write_u8(scale);
        w.write_u8(framerate);
    })
}

/// `TE_BEAMFOLLOW`: `short` entity to follow, `short` sprite index, `byte` life in tenths of a
/// second, `byte` line width in tenths, `byte x3` colour, `byte` brightness.
pub fn beam_follow(
    entity: u16,
    sprite: u16,
    life: u8,
    width: u8,
    color: [u8; 3],
    brightness: u8,
) -> TempEntity {
    TempEntity::new(te::BEAMFOLLOW, |w| {
        w.write_u16(entity);
        w.write_u16(sprite);
        w.write_u8(life);
        w.write_u8(width);
        w.write_bytes(&color);
        w.write_u8(brightness);
    })
}

/// `TE_DLIGHT`, 12 bytes: `coord x3`, `byte` radius in tens, `byte x3` colour, `byte` life in
/// tenths, `byte` decay rate in tens.
///
/// `const.h` lists an extra `byte (brightness)` after the colour (13 bytes), but the Xash3D
/// FWGS client parser (`CL_ParseTempEntity`, read 2026-11-23) reads radius, r, g, b, life and
/// decay only, which is the 12-byte layout built here. Nothing in the weapon set sends
/// `TE_DLIGHT`, and no capture of the stock server carries one.
pub fn dlight(position: Vec3, radius: u8, color: [u8; 3], life: u8, decay: u8) -> TempEntity {
    TempEntity::new(te::DLIGHT, |w| {
        coords(w, position);
        w.write_u8(radius);
        w.write_bytes(&color);
        w.write_u8(life);
        w.write_u8(decay);
    })
}

/// `TE_STREAK_SPLASH` (the helmet hit in `CBasePlayer::TraceAttack`): `coord x3` start,
/// `coord x3` direction, `byte` colour, `short` count, `short` base speed, `short` random
/// velocity.
pub fn streak_splash(
    start: Vec3,
    direction: Vec3,
    color: u8,
    count: u16,
    speed: u16,
    random_velocity: u16,
) -> TempEntity {
    TempEntity::new(te::STREAK_SPLASH, |w| {
        coords(w, start);
        coords(w, direction);
        w.write_u8(color);
        w.write_u16(count);
        w.write_u16(speed);
        w.write_u16(random_velocity);
    })
}

/// `TE_BLOODSPRITE` (`UTIL_BloodDrips`): `coord x3`, `short` spray sprite, `short` drop
/// sprite, `byte` colour, `byte` scale.
pub fn blood_sprite(position: Vec3, spray: u16, drop: u16, color: u8, scale: u8) -> TempEntity {
    TempEntity::new(te::BLOODSPRITE, |w| {
        coords(w, position);
        w.write_u16(spray);
        w.write_u16(drop);
        w.write_u8(color);
        w.write_u8(scale);
    })
}

/// `TE_BLOODSTREAM` (`UTIL_BloodStream`): `coord x3` start, `coord x3` spray vector, `byte`
/// colour, `byte` speed (the amount, capped at 255 by the caller).
pub fn blood_stream(origin: Vec3, direction: Vec3, color: u8, speed: u8) -> TempEntity {
    TempEntity::new(te::BLOODSTREAM, |w| {
        coords(w, origin);
        coords(w, direction);
        w.write_u8(color);
        w.write_u8(speed);
    })
}

/// `TE_GUNSHOTDECAL` (`UTIL_GunshotDecalTrace`): a decal and a ricochet sound. `coord x3`,
/// `short` entity index, `byte` decal index.
pub fn gunshot_decal(position: Vec3, entity: u16, decal: u8) -> TempEntity {
    TempEntity::new(te::GUNSHOTDECAL, |w| {
        coords(w, position);
        w.write_u16(entity);
        w.write_u8(decal);
    })
}

/// `TE_DECAL` (`UTIL_DecalTrace` on a brush entity): `coord x3`, `byte` decal index, `short`
/// entity index.
pub fn decal(position: Vec3, decal: u8, entity: u16) -> TempEntity {
    TempEntity::new(te::DECAL, |w| {
        coords(w, position);
        w.write_u8(decal);
        w.write_u16(entity);
    })
}

/// `TE_DECALHIGH`: [`decal`] for a decal index above 255; `decal` is the index minus 256.
pub fn decal_high(position: Vec3, decal: u8, entity: u16) -> TempEntity {
    TempEntity::new(te::DECALHIGH, |w| {
        coords(w, position);
        w.write_u8(decal);
        w.write_u16(entity);
    })
}

/// `UTIL_DecalTrace` / `UTIL_GunshotDecalTrace`'s choice for a decal on a brush entity:
/// `TE_DECAL` for an index below 256, else `TE_DECALHIGH` with `index - 256`. `None` for a
/// negative index (the original sends nothing) or one over 511, which a byte cannot carry.
pub fn decal_for(position: Vec3, index: i32, entity: u16) -> Option<TempEntity> {
    match index {
        0..=255 => Some(decal(position, index as u8, entity)),
        256..=511 => Some(decal_high(position, (index - 256) as u8, entity)),
        _ => None,
    }
}

/// [`decal_for`] for the world: `TE_WORLDDECAL` or `TE_WORLDDECALHIGH`.
pub fn world_decal_for(position: Vec3, index: i32) -> Option<TempEntity> {
    match index {
        0..=255 => Some(world_decal(position, index as u8)),
        256..=511 => Some(world_decal_high(position, (index - 256) as u8)),
        _ => None,
    }
}

/// `TE_WORLDDECAL` (`UTIL_DecalTrace` on the world): `coord x3`, `byte` decal index.
pub fn world_decal(position: Vec3, decal: u8) -> TempEntity {
    TempEntity::new(te::WORLDDECAL, |w| {
        coords(w, position);
        w.write_u8(decal);
    })
}

/// `TE_WORLDDECALHIGH`: [`world_decal`] for a decal index above 255; `decal` is the index
/// minus 256.
pub fn world_decal_high(position: Vec3, decal: u8) -> TempEntity {
    TempEntity::new(te::WORLDDECALHIGH, |w| {
        coords(w, position);
        w.write_u8(decal);
    })
}

/// `TE_SPRITE` (`CGrenade::Explode2`): `coord x3`, `short` sprite index, `byte` scale in tenths,
/// `byte` brightness.
pub fn sprite(position: Vec3, sprite: u16, scale: u8, brightness: u8) -> TempEntity {
    TempEntity::new(te::SPRITE, |w| {
        coords(w, position);
        w.write_u16(sprite);
        w.write_u8(scale);
        w.write_u8(brightness);
    })
}

/// `TE_GLOWSPRITE` (`CGrenade::C4Think`): `coord x3`, `short` model index, `byte` life/scale in
/// tenths, `byte` size, `byte` alpha (`ggrenade.cpp` 1492-1501).
pub fn glow_sprite(position: Vec3, model: u16, scale: u8, size: u8, alpha: u8) -> TempEntity {
    TempEntity::new(te::GLOWSPRITE, |w| {
        coords(w, position);
        w.write_u16(model);
        w.write_u8(scale);
        w.write_u8(size);
        w.write_u8(alpha);
    })
}

/// The beam parameters shared by `TE_BEAMPOINTS`, `TE_BEAMENTPOINT` and `TE_BEAMENTS`, in the
/// wire units: frame rate in tenths, life in tenths of a second, width in tenths, noise in
/// hundredths, brightness `0..=255`, scroll speed in tenths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeamParams {
    pub sprite: u16,
    pub start_frame: u8,
    pub frame_rate: u8,
    pub life: u8,
    pub width: u8,
    pub noise: u8,
    pub color: [u8; 3],
    pub brightness: u8,
    pub speed: u8,
}

fn write_beam(w: &mut MsgWriter, b: &BeamParams) {
    w.write_u16(b.sprite);
    w.write_u8(b.start_frame);
    w.write_u8(b.frame_rate);
    w.write_u8(b.life);
    w.write_u8(b.width);
    w.write_u8(b.noise);
    w.write_bytes(&b.color);
    w.write_u8(b.brightness);
    w.write_u8(b.speed);
}

fn read_beam(r: &mut MsgReader<'_>) -> Result<BeamParams, ProtoError> {
    Ok(BeamParams {
        sprite: r.read_u16()?,
        start_frame: r.read_u8()?,
        frame_rate: r.read_u8()?,
        life: r.read_u8()?,
        width: r.read_u8()?,
        noise: r.read_u8()?,
        color: read_rgb(r)?,
        brightness: r.read_u8()?,
        speed: r.read_u8()?,
    })
}

/// `TE_TRACER`: `coord x3` start, `coord x3` end.
pub fn tracer(start: Vec3, end: Vec3) -> TempEntity {
    TempEntity::new(te::TRACER, |w| {
        coords(w, start);
        coords(w, end);
    })
}

/// `TE_BEAMPOINTS`: `coord x3` start, `coord x3` end, then the [`BeamParams`].
pub fn beam_points(start: Vec3, end: Vec3, beam: &BeamParams) -> TempEntity {
    TempEntity::new(te::BEAMPOINTS, |w| {
        coords(w, start);
        coords(w, end);
        write_beam(w, beam);
    })
}

/// `TE_BEAMENTPOINT`: `short` start entity (with the attachment in the high bits), `coord x3`
/// end, then the [`BeamParams`].
pub fn beam_ent_point(start_entity: u16, end: Vec3, beam: &BeamParams) -> TempEntity {
    TempEntity::new(te::BEAMENTPOINT, |w| {
        w.write_u16(start_entity);
        coords(w, end);
        write_beam(w, beam);
    })
}

/// `TE_BEAMENTS`: `short` start entity, `short` end entity, then the [`BeamParams`].
pub fn beam_ents(start_entity: u16, end_entity: u16, beam: &BeamParams) -> TempEntity {
    TempEntity::new(te::BEAMENTS, |w| {
        w.write_u16(start_entity);
        w.write_u16(end_entity);
        write_beam(w, beam);
    })
}

/// `TE_KILLBEAM`: `short` entity whose beams stop.
pub fn kill_beam(entity: u16) -> TempEntity {
    TempEntity::new(te::KILLBEAM, |w| w.write_u16(entity))
}

/// `TE_EXPLOSION2`: `coord x3`, `byte` first colour, `byte` count.
pub fn explosion2(position: Vec3, color: u8, count: u8) -> TempEntity {
    TempEntity::new(te::EXPLOSION2, |w| {
        coords(w, position);
        w.write_u8(color);
        w.write_u8(count);
    })
}

/// `TE_BSPDECAL`: `coord x3`, `short` decal index, `short` entity, and the `short` model index
/// of the entity when the entity is not 0.
pub fn bsp_decal(position: Vec3, decal: u16, entity: u16, model: Option<u16>) -> TempEntity {
    TempEntity::new(te::BSPDECAL, |w| {
        coords(w, position);
        w.write_u16(decal);
        w.write_u16(entity);
        if entity != 0 {
            w.write_u16(model.unwrap_or(0));
        }
    })
}

/// `TE_ELIGHT`: `short` entity, `coord x3`, `coord` radius, `byte x3` colour, `byte` life in
/// tenths, `coord` decay.
pub fn elight(
    entity: u16,
    position: Vec3,
    radius: f32,
    color: [u8; 3],
    life: u8,
    decay: f32,
) -> TempEntity {
    TempEntity::new(te::ELIGHT, |w| {
        w.write_u16(entity);
        coords(w, position);
        w.write_coord(radius);
        w.write_bytes(&color);
        w.write_u8(life);
        w.write_coord(decay);
    })
}

/// `TE_FIZZ`: `short` entity, `short` model index, `byte` density.
pub fn fizz(entity: u16, model: u16, density: u8) -> TempEntity {
    TempEntity::new(te::FIZZ, |w| {
        w.write_u16(entity);
        w.write_u16(model);
        w.write_u8(density);
    })
}

/// `TE_BREAKMODEL`: `coord x3` position, `coord x3` size, `coord x3` velocity, `byte` random
/// velocity in tens, `short` model index, `byte` count, `byte` life in tenths, `byte` flags.
#[allow(clippy::too_many_arguments)]
pub fn break_model(
    position: Vec3,
    size: Vec3,
    velocity: Vec3,
    random_velocity: u8,
    model: u16,
    count: u8,
    life: u8,
    flags: u8,
) -> TempEntity {
    TempEntity::new(te::BREAKMODEL, |w| {
        coords(w, position);
        coords(w, size);
        coords(w, velocity);
        w.write_u8(random_velocity);
        w.write_u16(model);
        w.write_u8(count);
        w.write_u8(life);
        w.write_u8(flags);
    })
}

/// A decoded `svc_temp_entity`. Fields keep the wire units; each variant names the builder
/// that writes it and carries the same fields in the same order. A `TE_*` without a variant
/// here is [`TempEffect::Other`].
#[derive(Debug, Clone, PartialEq)]
pub enum TempEffect {
    BeamPoints {
        start: Vec3,
        end: Vec3,
        beam: BeamParams,
    },
    BeamEntPoint {
        start_entity: u16,
        end: Vec3,
        beam: BeamParams,
    },
    BeamEnts {
        start_entity: u16,
        end_entity: u16,
        beam: BeamParams,
    },
    Gunshot {
        position: Vec3,
    },
    Explosion {
        position: Vec3,
        sprite: u16,
        scale: u8,
        framerate: u8,
        flags: u8,
    },
    Smoke {
        position: Vec3,
        sprite: u16,
        scale: u8,
        framerate: u8,
    },
    Tracer {
        start: Vec3,
        end: Vec3,
    },
    Sparks {
        position: Vec3,
    },
    Explosion2 {
        position: Vec3,
        color: u8,
        count: u8,
    },
    BspDecal {
        position: Vec3,
        decal: u16,
        entity: u16,
        model: Option<u16>,
    },
    Sprite {
        position: Vec3,
        sprite: u16,
        scale: u8,
        brightness: u8,
    },
    BeamFollow {
        entity: u16,
        sprite: u16,
        life: u8,
        width: u8,
        color: [u8; 3],
        brightness: u8,
    },
    GlowSprite {
        position: Vec3,
        model: u16,
        scale: u8,
        size: u8,
        alpha: u8,
    },
    StreakSplash {
        start: Vec3,
        direction: Vec3,
        color: u8,
        count: u16,
        speed: u16,
        random_velocity: u16,
    },
    Dlight {
        position: Vec3,
        radius: u8,
        color: [u8; 3],
        life: u8,
        decay: u8,
    },
    Elight {
        entity: u16,
        position: Vec3,
        radius: f32,
        color: [u8; 3],
        life: u8,
        decay: f32,
    },
    KillBeam {
        entity: u16,
    },
    BloodStream {
        origin: Vec3,
        direction: Vec3,
        color: u8,
        speed: u8,
    },
    Decal {
        position: Vec3,
        decal: u8,
        entity: u16,
    },
    Fizz {
        entity: u16,
        model: u16,
        density: u8,
    },
    BreakModel {
        position: Vec3,
        size: Vec3,
        velocity: Vec3,
        random_velocity: u8,
        model: u16,
        count: u8,
        life: u8,
        flags: u8,
    },
    GunshotDecal {
        position: Vec3,
        entity: u16,
        decal: u8,
    },
    BloodSprite {
        position: Vec3,
        spray: u16,
        drop: u16,
        color: u8,
        scale: u8,
    },
    WorldDecal {
        position: Vec3,
        decal: u8,
    },
    WorldDecalHigh {
        position: Vec3,
        decal: u8,
    },
    DecalHigh {
        position: Vec3,
        decal: u8,
        entity: u16,
    },
    /// An effect this reader has no typed variant for; the payload is complete.
    Other { te_type: u8, payload: Vec<u8> },
}

fn read_vec(r: &mut MsgReader<'_>) -> Result<Vec3, ProtoError> {
    Ok(Vec3::new(r.read_coord()?, r.read_coord()?, r.read_coord()?))
}

fn read_rgb(r: &mut MsgReader<'_>) -> Result<[u8; 3], ProtoError> {
    Ok([r.read_u8()?, r.read_u8()?, r.read_u8()?])
}

impl TempEffect {
    /// Decodes the payload that follows the `te_type` byte, as `SvcMessage::TempEntity` holds
    /// it (`CL_ParseTempEntity`). An effect with a typed variant must use its payload exactly:
    /// a short payload is `UnexpectedEof`, a long one `InvalidPayload`.
    pub fn parse(te_type: u8, payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = MsgReader::new(payload);
        let r = &mut r;
        let effect = match te_type {
            te::BEAMPOINTS => Self::BeamPoints {
                start: read_vec(r)?,
                end: read_vec(r)?,
                beam: read_beam(r)?,
            },
            te::BEAMENTPOINT => Self::BeamEntPoint {
                start_entity: r.read_u16()?,
                end: read_vec(r)?,
                beam: read_beam(r)?,
            },
            te::BEAMENTS => Self::BeamEnts {
                start_entity: r.read_u16()?,
                end_entity: r.read_u16()?,
                beam: read_beam(r)?,
            },
            te::GUNSHOT => Self::Gunshot {
                position: read_vec(r)?,
            },
            te::EXPLOSION => Self::Explosion {
                position: read_vec(r)?,
                sprite: r.read_u16()?,
                scale: r.read_u8()?,
                framerate: r.read_u8()?,
                flags: r.read_u8()?,
            },
            te::SMOKE => Self::Smoke {
                position: read_vec(r)?,
                sprite: r.read_u16()?,
                scale: r.read_u8()?,
                framerate: r.read_u8()?,
            },
            te::TRACER => Self::Tracer {
                start: read_vec(r)?,
                end: read_vec(r)?,
            },
            te::SPARKS => Self::Sparks {
                position: read_vec(r)?,
            },
            te::EXPLOSION2 => Self::Explosion2 {
                position: read_vec(r)?,
                color: r.read_u8()?,
                count: r.read_u8()?,
            },
            te::BSPDECAL => {
                let position = read_vec(r)?;
                let decal = r.read_u16()?;
                let entity = r.read_u16()?;
                let model = if entity != 0 {
                    Some(r.read_u16()?)
                } else {
                    None
                };
                Self::BspDecal {
                    position,
                    decal,
                    entity,
                    model,
                }
            }
            te::SPRITE => Self::Sprite {
                position: read_vec(r)?,
                sprite: r.read_u16()?,
                scale: r.read_u8()?,
                brightness: r.read_u8()?,
            },
            te::BEAMFOLLOW => Self::BeamFollow {
                entity: r.read_u16()?,
                sprite: r.read_u16()?,
                life: r.read_u8()?,
                width: r.read_u8()?,
                color: read_rgb(r)?,
                brightness: r.read_u8()?,
            },
            te::GLOWSPRITE => Self::GlowSprite {
                position: read_vec(r)?,
                model: r.read_u16()?,
                scale: r.read_u8()?,
                size: r.read_u8()?,
                alpha: r.read_u8()?,
            },
            te::STREAK_SPLASH => Self::StreakSplash {
                start: read_vec(r)?,
                direction: read_vec(r)?,
                color: r.read_u8()?,
                count: r.read_u16()?,
                speed: r.read_u16()?,
                random_velocity: r.read_u16()?,
            },
            te::DLIGHT => Self::Dlight {
                position: read_vec(r)?,
                radius: r.read_u8()?,
                color: read_rgb(r)?,
                life: r.read_u8()?,
                decay: r.read_u8()?,
            },
            te::ELIGHT => Self::Elight {
                entity: r.read_u16()?,
                position: read_vec(r)?,
                radius: r.read_coord()?,
                color: read_rgb(r)?,
                life: r.read_u8()?,
                decay: r.read_coord()?,
            },
            te::KILLBEAM => Self::KillBeam {
                entity: r.read_u16()?,
            },
            te::BLOODSTREAM => Self::BloodStream {
                origin: read_vec(r)?,
                direction: read_vec(r)?,
                color: r.read_u8()?,
                speed: r.read_u8()?,
            },
            te::DECAL => Self::Decal {
                position: read_vec(r)?,
                decal: r.read_u8()?,
                entity: r.read_u16()?,
            },
            te::FIZZ => Self::Fizz {
                entity: r.read_u16()?,
                model: r.read_u16()?,
                density: r.read_u8()?,
            },
            te::BREAKMODEL => Self::BreakModel {
                position: read_vec(r)?,
                size: read_vec(r)?,
                velocity: read_vec(r)?,
                random_velocity: r.read_u8()?,
                model: r.read_u16()?,
                count: r.read_u8()?,
                life: r.read_u8()?,
                flags: r.read_u8()?,
            },
            te::GUNSHOTDECAL => Self::GunshotDecal {
                position: read_vec(r)?,
                entity: r.read_u16()?,
                decal: r.read_u8()?,
            },
            te::BLOODSPRITE => Self::BloodSprite {
                position: read_vec(r)?,
                spray: r.read_u16()?,
                drop: r.read_u16()?,
                color: r.read_u8()?,
                scale: r.read_u8()?,
            },
            te::WORLDDECAL => Self::WorldDecal {
                position: read_vec(r)?,
                decal: r.read_u8()?,
            },
            te::WORLDDECALHIGH => Self::WorldDecalHigh {
                position: read_vec(r)?,
                decal: r.read_u8()?,
            },
            te::DECALHIGH => Self::DecalHigh {
                position: read_vec(r)?,
                decal: r.read_u8()?,
                entity: r.read_u16()?,
            },
            other => {
                return Ok(Self::Other {
                    te_type: other,
                    payload: payload.to_vec(),
                });
            }
        };
        if r.remaining() != 0 {
            return Err(ProtoError::InvalidPayload(format!(
                "TE_* {te_type}: {} bytes left after the effect",
                r.remaining()
            )));
        }
        Ok(effect)
    }
}

impl TempEntity {
    /// The typed effect of this temp entity, see [`TempEffect::parse`].
    pub fn decode(&self) -> Result<TempEffect, ProtoError> {
        TempEffect::parse(self.te_type, &self.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{SvcMessage, SvcParseContext, parse_svc_stream, write_svc_choke, write_svc_temp_entity};

    const P: Vec3 = Vec3::new(1.0, -2.5, 100.0);
    const Q: Vec3 = Vec3::new(-8.0, 0.125, 64.0);
    const BEAM: BeamParams = BeamParams {
        sprite: 300,
        start_frame: 1,
        frame_rate: 2,
        life: 3,
        width: 4,
        noise: 5,
        color: [6, 7, 8],
        brightness: 9,
        speed: 10,
    };

    /// Every builder, with the effect it must decode to.
    fn cases() -> Vec<(TempEntity, TempEffect)> {
        vec![
            (gunshot(P), TempEffect::Gunshot { position: P }),
            (
                explosion(P, 513, 30, 15, explflag::NOSOUND),
                TempEffect::Explosion {
                    position: P,
                    sprite: 513,
                    scale: 30,
                    framerate: 15,
                    flags: 4,
                },
            ),
            (
                smoke(P, 7, 25, 5),
                TempEffect::Smoke {
                    position: P,
                    sprite: 7,
                    scale: 25,
                    framerate: 5,
                },
            ),
            (sparks(P), TempEffect::Sparks { position: P }),
            (
                sprite(P, 9, 8, 200),
                TempEffect::Sprite {
                    position: P,
                    sprite: 9,
                    scale: 8,
                    brightness: 200,
                },
            ),
            (
                beam_follow(33, 44, 5, 6, [1, 2, 3], 4),
                TempEffect::BeamFollow {
                    entity: 33,
                    sprite: 44,
                    life: 5,
                    width: 6,
                    color: [1, 2, 3],
                    brightness: 4,
                },
            ),
            (
                glow_sprite(P, 70, 10, 20, 30),
                TempEffect::GlowSprite {
                    position: P,
                    model: 70,
                    scale: 10,
                    size: 20,
                    alpha: 30,
                },
            ),
            (
                streak_splash(P, Q, 5, 20, 300, 400),
                TempEffect::StreakSplash {
                    start: P,
                    direction: Q,
                    color: 5,
                    count: 20,
                    speed: 300,
                    random_velocity: 400,
                },
            ),
            (
                dlight(P, 12, [1, 2, 3], 4, 5),
                TempEffect::Dlight {
                    position: P,
                    radius: 12,
                    color: [1, 2, 3],
                    life: 4,
                    decay: 5,
                },
            ),
            (
                blood_stream(P, Q, 70, 200),
                TempEffect::BloodStream {
                    origin: P,
                    direction: Q,
                    color: 70,
                    speed: 200,
                },
            ),
            (
                decal(P, 12, 3),
                TempEffect::Decal {
                    position: P,
                    decal: 12,
                    entity: 3,
                },
            ),
            (
                decal_high(P, 12, 3),
                TempEffect::DecalHigh {
                    position: P,
                    decal: 12,
                    entity: 3,
                },
            ),
            (
                gunshot_decal(P, 5, 9),
                TempEffect::GunshotDecal {
                    position: P,
                    entity: 5,
                    decal: 9,
                },
            ),
            (
                blood_sprite(P, 11, 12, 247, 6),
                TempEffect::BloodSprite {
                    position: P,
                    spray: 11,
                    drop: 12,
                    color: 247,
                    scale: 6,
                },
            ),
            (
                world_decal(P, 40),
                TempEffect::WorldDecal {
                    position: P,
                    decal: 40,
                },
            ),
            (
                world_decal_high(P, 40),
                TempEffect::WorldDecalHigh {
                    position: P,
                    decal: 40,
                },
            ),
            (
                tracer(P, Q),
                TempEffect::Tracer { start: P, end: Q },
            ),
            (
                beam_points(P, Q, &BEAM),
                TempEffect::BeamPoints {
                    start: P,
                    end: Q,
                    beam: BEAM,
                },
            ),
            (
                beam_ent_point(0x1005, Q, &BEAM),
                TempEffect::BeamEntPoint {
                    start_entity: 0x1005,
                    end: Q,
                    beam: BEAM,
                },
            ),
            (
                beam_ents(4, 5, &BEAM),
                TempEffect::BeamEnts {
                    start_entity: 4,
                    end_entity: 5,
                    beam: BEAM,
                },
            ),
            (kill_beam(77), TempEffect::KillBeam { entity: 77 }),
            (
                explosion2(P, 3, 4),
                TempEffect::Explosion2 {
                    position: P,
                    color: 3,
                    count: 4,
                },
            ),
            (
                bsp_decal(P, 12, 0, None),
                TempEffect::BspDecal {
                    position: P,
                    decal: 12,
                    entity: 0,
                    model: None,
                },
            ),
            (
                bsp_decal(P, 12, 9, Some(33)),
                TempEffect::BspDecal {
                    position: P,
                    decal: 12,
                    entity: 9,
                    model: Some(33),
                },
            ),
            (
                elight(6, P, 120.0, [9, 8, 7], 20, 30.5),
                TempEffect::Elight {
                    entity: 6,
                    position: P,
                    radius: 120.0,
                    color: [9, 8, 7],
                    life: 20,
                    decay: 30.5,
                },
            ),
            (
                fizz(3, 4, 5),
                TempEffect::Fizz {
                    entity: 3,
                    model: 4,
                    density: 5,
                },
            ),
            (
                break_model(P, Q, Vec3::new(1.0, 2.0, 3.0), 25, 600, 7, 8, 9),
                TempEffect::BreakModel {
                    position: P,
                    size: Q,
                    velocity: Vec3::new(1.0, 2.0, 3.0),
                    random_velocity: 25,
                    model: 600,
                    count: 7,
                    life: 8,
                    flags: 9,
                },
            ),
        ]
    }

    #[test]
    fn every_builder_decodes_to_what_it_was_given() {
        for (te, expected) in cases() {
            assert_eq!(te.decode().unwrap(), expected, "te {}", te.te_type);
        }
    }

    #[test]
    fn every_builder_survives_the_message_parser_and_the_typed_reader() {
        for (te, expected) in cases() {
            let mut w = MsgWriter::new();
            write_svc_temp_entity(&mut w, te.te_type, &te.payload);
            write_svc_choke(&mut w);
            let msgs = parse_svc_stream(w.as_slice(), &mut SvcParseContext::new()).unwrap();
            assert_eq!(msgs.len(), 2, "te {}", te.te_type);
            let SvcMessage::TempEntity { te_type, raw } = &msgs[0] else {
                panic!("te {}: {:?}", te.te_type, msgs[0]);
            };
            assert_eq!(*te_type, te.te_type);
            assert_eq!(TempEffect::parse(*te_type, raw).unwrap(), expected);
            assert_eq!(msgs[1], SvcMessage::Choke);
        }
    }

    #[test]
    fn a_short_or_long_payload_is_an_error() {
        for (te, _) in cases() {
            let mut short = te.payload.clone();
            short.pop();
            assert!(TempEffect::parse(te.te_type, &short).is_err(), "short {}", te.te_type);
            let mut long = te.payload.clone();
            long.push(0);
            assert!(TempEffect::parse(te.te_type, &long).is_err(), "long {}", te.te_type);
        }
    }

    #[test]
    fn an_untyped_effect_keeps_its_payload() {
        assert_eq!(
            TempEffect::parse(111, &[1, 2, 3, 4, 5, 6, 7]).unwrap(),
            TempEffect::Other {
                te_type: 111,
                payload: vec![1, 2, 3, 4, 5, 6, 7],
            }
        );
    }

    #[test]
    fn dlight_is_twelve_bytes() {
        assert_eq!(dlight(P, 1, [2, 3, 4], 5, 6).payload.len(), 12);
    }
}
