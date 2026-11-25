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

/// All `TE_*` effect numbers (`common/const.h`, 59 active IDs `0..=127` skipping 16 and 26).
pub mod te {
    pub const BEAMPOINTS: u8 = 0;
    pub const BEAMENTPOINT: u8 = 1;
    pub const GUNSHOT: u8 = 2;
    pub const EXPLOSION: u8 = 3;
    pub const TAREXPLOSION: u8 = 4;
    pub const SMOKE: u8 = 5;
    pub const TRACER: u8 = 6;
    pub const LIGHTNING: u8 = 7;
    pub const BEAMENTS: u8 = 8;
    pub const SPARKS: u8 = 9;
    pub const LAVASPLASH: u8 = 10;
    pub const TELEPORT: u8 = 11;
    pub const EXPLOSION2: u8 = 12;
    pub const BSPDECAL: u8 = 13;
    pub const IMPLOSION: u8 = 14;
    pub const SPRITETRAIL: u8 = 15;
    pub const SPRITE: u8 = 17;
    pub const BEAMSPRITE: u8 = 18;
    pub const BEAMTORUS: u8 = 19;
    pub const BEAMDISK: u8 = 20;
    pub const BEAMCYLINDER: u8 = 21;
    pub const BEAMFOLLOW: u8 = 22;
    pub const GLOWSPRITE: u8 = 23;
    pub const BEAMRING: u8 = 24;
    pub const STREAK_SPLASH: u8 = 25;
    pub const DLIGHT: u8 = 27;
    pub const ELIGHT: u8 = 28;
    pub const TEXTMESSAGE: u8 = 29;
    pub const LINE: u8 = 30;
    pub const BOX: u8 = 31;
    pub const KILLBEAM: u8 = 99;
    pub const LARGEFUNNEL: u8 = 100;
    pub const BLOODSTREAM: u8 = 101;
    pub const SHOWLINE: u8 = 102;
    pub const BLOOD: u8 = 103;
    pub const DECAL: u8 = 104;
    pub const FIZZ: u8 = 105;
    pub const MODEL: u8 = 106;
    pub const EXPLODEMODEL: u8 = 107;
    pub const BREAKMODEL: u8 = 108;
    pub const GUNSHOTDECAL: u8 = 109;
    pub const SPRITE_SPRAY: u8 = 110;
    pub const ARMOR_RICOCHET: u8 = 111;
    pub const PLAYERDECAL: u8 = 112;
    pub const BUBBLES: u8 = 113;
    pub const BUBBLETRAIL: u8 = 114;
    pub const BLOODSPRITE: u8 = 115;
    pub const WORLDDECAL: u8 = 116;
    pub const WORLDDECALHIGH: u8 = 117;
    pub const DECALHIGH: u8 = 118;
    pub const PROJECTILE: u8 = 119;
    pub const SPRAY: u8 = 120;
    pub const PLAYERSPRITES: u8 = 121;
    pub const PARTICLEBURST: u8 = 122;
    pub const FIREFIELD: u8 = 123;
    pub const PLAYERATTACHMENT: u8 = 124;
    pub const KILLPLAYERATTACHMENTS: u8 = 125;
    pub const MULTIGUNSHOT: u8 = 126;
    pub const USERTRACER: u8 = 127;
}

/// `TE_EXPLFLAG_*`: the `flags` byte of `TE_EXPLOSION`.
pub mod explflag {
    /// All flags clear: the default Half-Life explosion.
    pub const NONE: u8 = 0;
    pub const NOADDITIVE: u8 = 1;
    pub const NODLIGHTS: u8 = 2;
    pub const NOSOUND: u8 = 4;
    pub const NOPARTICLES: u8 = 8;
    pub const DRAWALPHA: u8 = 16;
    pub const ROTATE: u8 = 32;
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
/// colour, `byte` count (the amount, capped at 255 by the caller; Xash `cl_tent.c` reads it as `count`).
pub fn blood_stream(origin: Vec3, direction: Vec3, color: u8, count: u8) -> TempEntity {
    TempEntity::new(te::BLOODSTREAM, |w| {
        coords(w, origin);
        coords(w, direction);
        w.write_u8(color);
        w.write_u8(count);
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

/// `TE_GLOWSPRITE` (`CGrenade::C4Think`): `coord x3`, `short` model index, `byte` life in
/// tenths of a second, `byte` scale in tenths, `byte` brightness (`ggrenade.cpp` 1492-1501;
/// Xash `cl_tent.c` 2061-2072).
pub fn glow_sprite(position: Vec3, model: u16, life: u8, scale: u8, brightness: u8) -> TempEntity {
    TempEntity::new(te::GLOWSPRITE, |w| {
        coords(w, position);
        w.write_u16(model);
        w.write_u8(life);
        w.write_u8(scale);
        w.write_u8(brightness);
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

/// `TE_TAREXPLOSION`: `coord x3` (position).
pub fn tar_explosion(position: Vec3) -> TempEntity {
    TempEntity::new(te::TAREXPLOSION, |w| coords(w, position))
}

/// `TE_LIGHTNING`: `coord x3` start, `coord x3` end, `byte` life, `byte` width, `byte` amplitude,
/// `short` sprite model index.
pub fn lightning(
    start: Vec3,
    end: Vec3,
    life: u8,
    width: u8,
    amplitude: u8,
    sprite: u16,
) -> TempEntity {
    TempEntity::new(te::LIGHTNING, |w| {
        coords(w, start);
        coords(w, end);
        w.write_u8(life);
        w.write_u8(width);
        w.write_u8(amplitude);
        w.write_u16(sprite);
    })
}

/// `TE_LAVASPLASH`: `coord x3` (position).
pub fn lava_splash(position: Vec3) -> TempEntity {
    TempEntity::new(te::LAVASPLASH, |w| coords(w, position))
}

/// `TE_TELEPORT`: `coord x3` (position).
pub fn teleport(position: Vec3) -> TempEntity {
    TempEntity::new(te::TELEPORT, |w| coords(w, position))
}

/// `TE_IMPLOSION`: `coord x3` position, `byte` radius, `byte` count, `byte` life in tenths.
pub fn implosion(position: Vec3, radius: u8, count: u8, life: u8) -> TempEntity {
    TempEntity::new(te::IMPLOSION, |w| {
        coords(w, position);
        w.write_u8(radius);
        w.write_u8(count);
        w.write_u8(life);
    })
}

/// `TE_SPRITETRAIL`: `coord x3` start, `coord x3` end, `short` sprite, `byte` count, `byte` life,
/// `byte` scale, `byte` noise, `byte` speed in tens.
#[allow(clippy::too_many_arguments)]
pub fn sprite_trail(
    start: Vec3,
    end: Vec3,
    sprite: u16,
    count: u8,
    life: u8,
    scale: u8,
    noise: u8,
    speed: u8,
) -> TempEntity {
    TempEntity::new(te::SPRITETRAIL, |w| {
        coords(w, start);
        coords(w, end);
        w.write_u16(sprite);
        w.write_u8(count);
        w.write_u8(life);
        w.write_u8(scale);
        w.write_u8(noise);
        w.write_u8(speed);
    })
}

/// `TE_BEAMSPRITE`: `coord x3` start, `coord x3` end, `short` beam sprite, `short` end sprite (16B).
pub fn beam_sprite(start: Vec3, end: Vec3, beam_sprite: u16, end_sprite: u16) -> TempEntity {
    TempEntity::new(te::BEAMSPRITE, |w| {
        coords(w, start);
        coords(w, end);
        w.write_u16(beam_sprite);
        w.write_u16(end_sprite);
    })
}

/// `TE_BEAMTORUS`: `coord x3` center, `coord x3` axis+radius, then [`BeamParams`].
pub fn beam_torus(center: Vec3, axis_radius: Vec3, beam: &BeamParams) -> TempEntity {
    TempEntity::new(te::BEAMTORUS, |w| {
        coords(w, center);
        coords(w, axis_radius);
        write_beam(w, beam);
    })
}

/// `TE_BEAMDISK`: `coord x3` center, `coord x3` axis+radius, then [`BeamParams`].
pub fn beam_disk(center: Vec3, axis_radius: Vec3, beam: &BeamParams) -> TempEntity {
    TempEntity::new(te::BEAMDISK, |w| {
        coords(w, center);
        coords(w, axis_radius);
        write_beam(w, beam);
    })
}

/// `TE_BEAMCYLINDER`: `coord x3` center, `coord x3` axis+radius, then [`BeamParams`].
pub fn beam_cylinder(center: Vec3, axis_radius: Vec3, beam: &BeamParams) -> TempEntity {
    TempEntity::new(te::BEAMCYLINDER, |w| {
        coords(w, center);
        coords(w, axis_radius);
        write_beam(w, beam);
    })
}

/// `TE_BEAMRING`: `short` start entity, `short` end entity, then [`BeamParams`].
pub fn beam_ring(start_entity: u16, end_entity: u16, beam: &BeamParams) -> TempEntity {
    TempEntity::new(te::BEAMRING, |w| {
        w.write_u16(start_entity);
        w.write_u16(end_entity);
        write_beam(w, beam);
    })
}

/// `TE_TEXTMESSAGE`: HUD text channel message.
#[allow(clippy::too_many_arguments)]
pub fn text_message(
    channel: u8,
    x: i16,
    y: i16,
    effect: u8,
    color1: [u8; 4],
    color2: [u8; 4],
    fadein: u16,
    fadeout: u16,
    holdtime: u16,
    fxtime: Option<u16>,
    text: &str,
) -> TempEntity {
    TempEntity::new(te::TEXTMESSAGE, |w| {
        w.write_u8(channel);
        w.write_i16(x);
        w.write_i16(y);
        w.write_u8(effect);
        w.write_bytes(&color1);
        w.write_bytes(&color2);
        w.write_u16(fadein);
        w.write_u16(fadeout);
        w.write_u16(holdtime);
        if effect == 2 {
            w.write_u16(fxtime.unwrap_or(0));
        }
        w.write_str(text);
    })
}

/// `TE_LINE`: `coord x3` start, `coord x3` end, `short` life in tenths, `byte x3` rgb.
pub fn line(start: Vec3, end: Vec3, life: u16, color: [u8; 3]) -> TempEntity {
    TempEntity::new(te::LINE, |w| {
        coords(w, start);
        coords(w, end);
        w.write_u16(life);
        w.write_bytes(&color);
    })
}

/// `TE_BOX`: `coord x3` min, `coord x3` max, `short` life in tenths, `byte x3` rgb.
pub fn box_fx(min: Vec3, max: Vec3, life: u16, color: [u8; 3]) -> TempEntity {
    TempEntity::new(te::BOX, |w| {
        coords(w, min);
        coords(w, max);
        w.write_u16(life);
        w.write_bytes(&color);
    })
}

/// `TE_LARGEFUNNEL`: `coord x3` origin, `short` sprite, `short` flags.
pub fn large_funnel(position: Vec3, sprite: u16, flags: u16) -> TempEntity {
    TempEntity::new(te::LARGEFUNNEL, |w| {
        coords(w, position);
        w.write_u16(sprite);
        w.write_u16(flags);
    })
}

/// `TE_SHOWLINE`: `coord x3` start, `coord x3` end.
pub fn show_line(start: Vec3, end: Vec3) -> TempEntity {
    TempEntity::new(te::SHOWLINE, |w| {
        coords(w, start);
        coords(w, end);
    })
}

/// `TE_BLOOD`: `coord x3` origin, `coord x3` direction, `byte` colour, `byte` speed.
pub fn blood(origin: Vec3, direction: Vec3, color: u8, speed: u8) -> TempEntity {
    TempEntity::new(te::BLOOD, |w| {
        coords(w, origin);
        coords(w, direction);
        w.write_u8(color);
        w.write_u8(speed);
    })
}

/// `TE_MODEL`: `coord x3` position, `coord x3` velocity, `byte` yaw angle (`degrees * 256 / 360` or scaled),
/// `short` model index, `byte` sound type, `byte` life in tenths (17B).
pub fn model(
    position: Vec3,
    velocity: Vec3,
    angle: u8,
    model: u16,
    sound_type: u8,
    life: u8,
) -> TempEntity {
    TempEntity::new(te::MODEL, |w| {
        coords(w, position);
        coords(w, velocity);
        w.write_u8(angle);
        w.write_u16(model);
        w.write_u8(sound_type);
        w.write_u8(life);
    })
}

/// `TE_EXPLODEMODEL`: `coord x3` origin, `coord` velocity, `short` model, `short` count, `byte` life (13B).
pub fn explode_model(
    position: Vec3,
    velocity: f32,
    model: u16,
    count: u16,
    life: u8,
) -> TempEntity {
    TempEntity::new(te::EXPLODEMODEL, |w| {
        coords(w, position);
        w.write_coord(velocity);
        w.write_u16(model);
        w.write_u16(count);
        w.write_u8(life);
    })
}

/// `TE_SPRITE_SPRAY`: `coord x3` pos, `coord x3` dir, `short` sprite, `byte` count, `byte` speed, `byte` noise.
pub fn sprite_spray(
    position: Vec3,
    direction: Vec3,
    sprite: u16,
    count: u8,
    speed: u8,
    noise: u8,
) -> TempEntity {
    TempEntity::new(te::SPRITE_SPRAY, |w| {
        coords(w, position);
        coords(w, direction);
        w.write_u16(sprite);
        w.write_u8(count);
        w.write_u8(speed);
        w.write_u8(noise);
    })
}

/// `TE_ARMOR_RICOCHET`: `coord x3` pos, `byte` scale in tenths.
pub fn armor_ricochet(position: Vec3, scale: u8) -> TempEntity {
    TempEntity::new(te::ARMOR_RICOCHET, |w| {
        coords(w, position);
        w.write_u8(scale);
    })
}

/// `TE_PLAYERDECAL`: `byte` player index, `coord x3` position, `short` entity, `byte` decal index (10B).
pub fn player_decal(player: u8, position: Vec3, entity: u16, decal: u8) -> TempEntity {
    TempEntity::new(te::PLAYERDECAL, |w| {
        w.write_u8(player);
        coords(w, position);
        w.write_u16(entity);
        w.write_u8(decal);
    })
}

/// `TE_BUBBLES`: `coord x3` mins, `coord x3` maxs, `coord` height, `short` model, `byte` count, `coord` speed.
pub fn bubbles(
    mins: Vec3,
    maxs: Vec3,
    height: f32,
    model: u16,
    count: u8,
    speed: f32,
) -> TempEntity {
    TempEntity::new(te::BUBBLES, |w| {
        coords(w, mins);
        coords(w, maxs);
        w.write_coord(height);
        w.write_u16(model);
        w.write_u8(count);
        w.write_coord(speed);
    })
}

/// `TE_BUBBLETRAIL`: `coord x3` mins, `coord x3` maxs, `coord` height, `short` model, `byte` count, `coord` speed.
pub fn bubble_trail(
    mins: Vec3,
    maxs: Vec3,
    height: f32,
    model: u16,
    count: u8,
    speed: f32,
) -> TempEntity {
    TempEntity::new(te::BUBBLETRAIL, |w| {
        coords(w, mins);
        coords(w, maxs);
        w.write_coord(height);
        w.write_u16(model);
        w.write_u8(count);
        w.write_coord(speed);
    })
}

/// `TE_PROJECTILE`: `coord x3` origin, `coord x3` velocity, `short` model, `byte` life, `byte` owner.
pub fn projectile(
    position: Vec3,
    velocity: Vec3,
    model: u16,
    life: u8,
    owner: u8,
) -> TempEntity {
    TempEntity::new(te::PROJECTILE, |w| {
        coords(w, position);
        coords(w, velocity);
        w.write_u16(model);
        w.write_u8(life);
        w.write_u8(owner);
    })
}

/// `TE_SPRAY`: `coord x3` pos, `coord x3` dir, `short` model, `byte` count, `byte` speed, `byte` noise, `byte` rendermode.
#[allow(clippy::too_many_arguments)]
pub fn spray(
    position: Vec3,
    direction: Vec3,
    model: u16,
    count: u8,
    speed: u8,
    noise: u8,
    rendermode: u8,
) -> TempEntity {
    TempEntity::new(te::SPRAY, |w| {
        coords(w, position);
        coords(w, direction);
        w.write_u16(model);
        w.write_u8(count);
        w.write_u8(speed);
        w.write_u8(noise);
        w.write_u8(rendermode);
    })
}

/// `TE_PLAYERSPRITES`: `short` player entity, `short` model, `byte` count, `byte` variance.
pub fn player_sprites(entity: u16, model: u16, count: u8, variance: u8) -> TempEntity {
    TempEntity::new(te::PLAYERSPRITES, |w| {
        w.write_u16(entity);
        w.write_u16(model);
        w.write_u8(count);
        w.write_u8(variance);
    })
}

/// `TE_PARTICLEBURST`: `coord x3` origin, `short` radius, `byte` color, `byte` duration in tenths.
pub fn particle_burst(position: Vec3, radius: u16, color: u8, duration: u8) -> TempEntity {
    TempEntity::new(te::PARTICLEBURST, |w| {
        coords(w, position);
        w.write_u16(radius);
        w.write_u8(color);
        w.write_u8(duration);
    })
}

/// `TE_FIREFIELD`: `coord x3` origin, `short` radius, `short` model, `byte` count, `byte` flags, `byte` life in tenths.
pub fn fire_field(
    position: Vec3,
    radius: u16,
    model: u16,
    count: u8,
    flags: u8,
    life: u8,
) -> TempEntity {
    TempEntity::new(te::FIREFIELD, |w| {
        coords(w, position);
        w.write_u16(radius);
        w.write_u16(model);
        w.write_u8(count);
        w.write_u8(flags);
        w.write_u8(life);
    })
}

/// `TE_PLAYERATTACHMENT`: `byte` entity, `coord` vertical offset, `short` model, `short` life in tenths.
pub fn player_attachment(entity: u8, offset_z: f32, model: u16, life: u16) -> TempEntity {
    TempEntity::new(te::PLAYERATTACHMENT, |w| {
        w.write_u8(entity);
        w.write_coord(offset_z);
        w.write_u16(model);
        w.write_u16(life);
    })
}

/// `TE_KILLPLAYERATTACHMENTS`: `byte` entity.
pub fn kill_player_attachments(entity: u8) -> TempEntity {
    TempEntity::new(te::KILLPLAYERATTACHMENTS, |w| w.write_u8(entity))
}

/// `TE_MULTIGUNSHOT`: `coord x3` origin, `coord x3` dir, `coord` x_noise, `coord` y_noise, `byte` count, `byte` decal index.
pub fn multi_gunshot(
    origin: Vec3,
    direction: Vec3,
    x_noise: f32,
    y_noise: f32,
    count: u8,
    decal: u8,
) -> TempEntity {
    TempEntity::new(te::MULTIGUNSHOT, |w| {
        coords(w, origin);
        coords(w, direction);
        w.write_coord(x_noise);
        w.write_coord(y_noise);
        w.write_u8(count);
        w.write_u8(decal);
    })
}

/// `TE_USERTRACER`: `coord x3` origin, `coord x3` velocity, `byte` life, `byte` color index, `byte` length.
pub fn user_tracer(
    origin: Vec3,
    velocity: Vec3,
    life: u8,
    color: u8,
    length: u8,
) -> TempEntity {
    TempEntity::new(te::USERTRACER, |w| {
        coords(w, origin);
        coords(w, velocity);
        w.write_u8(life);
        w.write_u8(color);
        w.write_u8(length);
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
        /// Life in tenths of a second.
        life: u8,
        /// Scale in tenths.
        scale: u8,
        brightness: u8,
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
        count: u8,
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
    TarExplosion {
        position: Vec3,
    },
    Lightning {
        start: Vec3,
        end: Vec3,
        life: u8,
        width: u8,
        amplitude: u8,
        sprite: u16,
    },
    LavaSplash {
        position: Vec3,
    },
    Teleport {
        position: Vec3,
    },
    Implosion {
        position: Vec3,
        radius: u8,
        count: u8,
        life: u8,
    },
    SpriteTrail {
        start: Vec3,
        end: Vec3,
        sprite: u16,
        count: u8,
        life: u8,
        scale: u8,
        noise: u8,
        speed: u8,
    },
    BeamSprite {
        start: Vec3,
        end: Vec3,
        beam_sprite: u16,
        end_sprite: u16,
    },
    BeamTorus {
        center: Vec3,
        axis_radius: Vec3,
        beam: BeamParams,
    },
    BeamDisk {
        center: Vec3,
        axis_radius: Vec3,
        beam: BeamParams,
    },
    BeamCylinder {
        center: Vec3,
        axis_radius: Vec3,
        beam: BeamParams,
    },
    BeamRing {
        start_entity: u16,
        end_entity: u16,
        beam: BeamParams,
    },
    TextMessage {
        channel: u8,
        x: i16,
        y: i16,
        effect: u8,
        color1: [u8; 4],
        color2: [u8; 4],
        fadein: u16,
        fadeout: u16,
        holdtime: u16,
        fxtime: Option<u16>,
        text: String,
    },
    Line {
        start: Vec3,
        end: Vec3,
        life: u16,
        color: [u8; 3],
    },
    Box {
        min: Vec3,
        max: Vec3,
        life: u16,
        color: [u8; 3],
    },
    LargeFunnel {
        position: Vec3,
        sprite: u16,
        flags: u16,
    },
    ShowLine {
        start: Vec3,
        end: Vec3,
    },
    Blood {
        origin: Vec3,
        direction: Vec3,
        color: u8,
        speed: u8,
    },
    Model {
        position: Vec3,
        velocity: Vec3,
        angle: u8,
        model: u16,
        sound_type: u8,
        life: u8,
    },
    ExplodeModel {
        position: Vec3,
        velocity: f32,
        model: u16,
        count: u16,
        life: u8,
    },
    SpriteSpray {
        position: Vec3,
        direction: Vec3,
        sprite: u16,
        count: u8,
        speed: u8,
        noise: u8,
    },
    ArmorRicochet {
        position: Vec3,
        scale: u8,
    },
    PlayerDecal {
        player: u8,
        position: Vec3,
        entity: u16,
        decal: u8,
    },
    Bubbles {
        mins: Vec3,
        maxs: Vec3,
        height: f32,
        model: u16,
        count: u8,
        speed: f32,
    },
    BubbleTrail {
        mins: Vec3,
        maxs: Vec3,
        height: f32,
        model: u16,
        count: u8,
        speed: f32,
    },
    Projectile {
        position: Vec3,
        velocity: Vec3,
        model: u16,
        life: u8,
        owner: u8,
    },
    Spray {
        position: Vec3,
        direction: Vec3,
        model: u16,
        count: u8,
        speed: u8,
        noise: u8,
        rendermode: u8,
    },
    PlayerSprites {
        entity: u16,
        model: u16,
        count: u8,
        variance: u8,
    },
    ParticleBurst {
        position: Vec3,
        radius: u16,
        color: u8,
        duration: u8,
    },
    FireField {
        position: Vec3,
        radius: u16,
        model: u16,
        count: u8,
        flags: u8,
        life: u8,
    },
    PlayerAttachment {
        entity: u8,
        offset_z: f32,
        model: u16,
        life: u16,
    },
    KillPlayerAttachments {
        entity: u8,
    },
    MultiGunshot {
        origin: Vec3,
        direction: Vec3,
        x_noise: f32,
        y_noise: f32,
        count: u8,
        decal: u8,
    },
    UserTracer {
        origin: Vec3,
        velocity: Vec3,
        life: u8,
        color: u8,
        length: u8,
    },
    /// An effect this reader has no typed variant for; the payload is complete. Valid only
    /// for types that passed the stream parser (`parse_svc_stream`), which knows the length of
    /// every `TE_*` it accepts; `parse` itself does not check the length of an untyped effect.
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
                life: r.read_u8()?,
                scale: r.read_u8()?,
                brightness: r.read_u8()?,
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
                count: r.read_u8()?,
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
            te::TAREXPLOSION => Self::TarExplosion {
                position: read_vec(r)?,
            },
            te::LIGHTNING => Self::Lightning {
                start: read_vec(r)?,
                end: read_vec(r)?,
                life: r.read_u8()?,
                width: r.read_u8()?,
                amplitude: r.read_u8()?,
                sprite: r.read_u16()?,
            },
            te::LAVASPLASH => Self::LavaSplash {
                position: read_vec(r)?,
            },
            te::TELEPORT => Self::Teleport {
                position: read_vec(r)?,
            },
            te::IMPLOSION => Self::Implosion {
                position: read_vec(r)?,
                radius: r.read_u8()?,
                count: r.read_u8()?,
                life: r.read_u8()?,
            },
            te::SPRITETRAIL => Self::SpriteTrail {
                start: read_vec(r)?,
                end: read_vec(r)?,
                sprite: r.read_u16()?,
                count: r.read_u8()?,
                life: r.read_u8()?,
                scale: r.read_u8()?,
                noise: r.read_u8()?,
                speed: r.read_u8()?,
            },
            te::BEAMSPRITE => Self::BeamSprite {
                start: read_vec(r)?,
                end: read_vec(r)?,
                beam_sprite: r.read_u16()?,
                end_sprite: r.read_u16()?,
            },
            te::BEAMTORUS => Self::BeamTorus {
                center: read_vec(r)?,
                axis_radius: read_vec(r)?,
                beam: read_beam(r)?,
            },
            te::BEAMDISK => Self::BeamDisk {
                center: read_vec(r)?,
                axis_radius: read_vec(r)?,
                beam: read_beam(r)?,
            },
            te::BEAMCYLINDER => Self::BeamCylinder {
                center: read_vec(r)?,
                axis_radius: read_vec(r)?,
                beam: read_beam(r)?,
            },
            te::BEAMRING => Self::BeamRing {
                start_entity: r.read_u16()?,
                end_entity: r.read_u16()?,
                beam: read_beam(r)?,
            },
            te::TEXTMESSAGE => {
                let channel = r.read_u8()?;
                let x = r.read_i16()?;
                let y = r.read_i16()?;
                let effect = r.read_u8()?;
                let color1 = [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?];
                let color2 = [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?];
                let fadein = r.read_u16()?;
                let fadeout = r.read_u16()?;
                let holdtime = r.read_u16()?;
                let fxtime = if effect == 2 {
                    Some(r.read_u16()?)
                } else {
                    None
                };
                if !r.remaining_slice().contains(&0) {
                    return Err(ProtoError::UnexpectedEof);
                }
                let text = r.read_str()?;
                Self::TextMessage {
                    channel,
                    x,
                    y,
                    effect,
                    color1,
                    color2,
                    fadein,
                    fadeout,
                    holdtime,
                    fxtime,
                    text,
                }
            }
            te::LINE => Self::Line {
                start: read_vec(r)?,
                end: read_vec(r)?,
                life: r.read_u16()?,
                color: read_rgb(r)?,
            },
            te::BOX => Self::Box {
                min: read_vec(r)?,
                max: read_vec(r)?,
                life: r.read_u16()?,
                color: read_rgb(r)?,
            },
            te::LARGEFUNNEL => Self::LargeFunnel {
                position: read_vec(r)?,
                sprite: r.read_u16()?,
                flags: r.read_u16()?,
            },
            te::SHOWLINE => Self::ShowLine {
                start: read_vec(r)?,
                end: read_vec(r)?,
            },
            te::BLOOD => Self::Blood {
                origin: read_vec(r)?,
                direction: read_vec(r)?,
                color: r.read_u8()?,
                speed: r.read_u8()?,
            },
            te::MODEL => Self::Model {
                position: read_vec(r)?,
                velocity: read_vec(r)?,
                angle: r.read_u8()?,
                model: r.read_u16()?,
                sound_type: r.read_u8()?,
                life: r.read_u8()?,
            },
            te::EXPLODEMODEL => Self::ExplodeModel {
                position: read_vec(r)?,
                velocity: r.read_coord()?,
                model: r.read_u16()?,
                count: r.read_u16()?,
                life: r.read_u8()?,
            },
            te::SPRITE_SPRAY => Self::SpriteSpray {
                position: read_vec(r)?,
                direction: read_vec(r)?,
                sprite: r.read_u16()?,
                count: r.read_u8()?,
                speed: r.read_u8()?,
                noise: r.read_u8()?,
            },
            te::ARMOR_RICOCHET => Self::ArmorRicochet {
                position: read_vec(r)?,
                scale: r.read_u8()?,
            },
            te::PLAYERDECAL => Self::PlayerDecal {
                player: r.read_u8()?,
                position: read_vec(r)?,
                entity: r.read_u16()?,
                decal: r.read_u8()?,
            },
            te::BUBBLES => Self::Bubbles {
                mins: read_vec(r)?,
                maxs: read_vec(r)?,
                height: r.read_coord()?,
                model: r.read_u16()?,
                count: r.read_u8()?,
                speed: r.read_coord()?,
            },
            te::BUBBLETRAIL => Self::BubbleTrail {
                mins: read_vec(r)?,
                maxs: read_vec(r)?,
                height: r.read_coord()?,
                model: r.read_u16()?,
                count: r.read_u8()?,
                speed: r.read_coord()?,
            },
            te::PROJECTILE => Self::Projectile {
                position: read_vec(r)?,
                velocity: read_vec(r)?,
                model: r.read_u16()?,
                life: r.read_u8()?,
                owner: r.read_u8()?,
            },
            te::SPRAY => Self::Spray {
                position: read_vec(r)?,
                direction: read_vec(r)?,
                model: r.read_u16()?,
                count: r.read_u8()?,
                speed: r.read_u8()?,
                noise: r.read_u8()?,
                rendermode: r.read_u8()?,
            },
            te::PLAYERSPRITES => Self::PlayerSprites {
                entity: r.read_u16()?,
                model: r.read_u16()?,
                count: r.read_u8()?,
                variance: r.read_u8()?,
            },
            te::PARTICLEBURST => Self::ParticleBurst {
                position: read_vec(r)?,
                radius: r.read_u16()?,
                color: r.read_u8()?,
                duration: r.read_u8()?,
            },
            te::FIREFIELD => Self::FireField {
                position: read_vec(r)?,
                radius: r.read_u16()?,
                model: r.read_u16()?,
                count: r.read_u8()?,
                flags: r.read_u8()?,
                life: r.read_u8()?,
            },
            te::PLAYERATTACHMENT => Self::PlayerAttachment {
                entity: r.read_u8()?,
                offset_z: r.read_coord()?,
                model: r.read_u16()?,
                life: r.read_u16()?,
            },
            te::KILLPLAYERATTACHMENTS => Self::KillPlayerAttachments {
                entity: r.read_u8()?,
            },
            te::MULTIGUNSHOT => Self::MultiGunshot {
                origin: read_vec(r)?,
                direction: read_vec(r)?,
                x_noise: r.read_coord()?,
                y_noise: r.read_coord()?,
                count: r.read_u8()?,
                decal: r.read_u8()?,
            },
            te::USERTRACER => Self::UserTracer {
                origin: read_vec(r)?,
                velocity: read_vec(r)?,
                life: r.read_u8()?,
                color: r.read_u8()?,
                length: r.read_u8()?,
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
                    life: 10,
                    scale: 20,
                    brightness: 30,
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
                    count: 200,
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
            (
                tar_explosion(P),
                TempEffect::TarExplosion { position: P },
            ),
            (
                lightning(P, Q, 10, 20, 30, 55),
                TempEffect::Lightning {
                    start: P,
                    end: Q,
                    life: 10,
                    width: 20,
                    amplitude: 30,
                    sprite: 55,
                },
            ),
            (lava_splash(P), TempEffect::LavaSplash { position: P }),
            (teleport(P), TempEffect::Teleport { position: P }),
            (
                implosion(P, 64, 30, 12),
                TempEffect::Implosion {
                    position: P,
                    radius: 64,
                    count: 30,
                    life: 12,
                },
            ),
            (
                sprite_trail(P, Q, 42, 12, 8, 5, 3, 25),
                TempEffect::SpriteTrail {
                    start: P,
                    end: Q,
                    sprite: 42,
                    count: 12,
                    life: 8,
                    scale: 5,
                    noise: 3,
                    speed: 25,
                },
            ),
            (
                beam_sprite(P, Q, 11, 22),
                TempEffect::BeamSprite {
                    start: P,
                    end: Q,
                    beam_sprite: 11,
                    end_sprite: 22,
                },
            ),
            (
                beam_torus(P, Q, &BEAM),
                TempEffect::BeamTorus {
                    center: P,
                    axis_radius: Q,
                    beam: BEAM,
                },
            ),
            (
                beam_disk(P, Q, &BEAM),
                TempEffect::BeamDisk {
                    center: P,
                    axis_radius: Q,
                    beam: BEAM,
                },
            ),
            (
                beam_cylinder(P, Q, &BEAM),
                TempEffect::BeamCylinder {
                    center: P,
                    axis_radius: Q,
                    beam: BEAM,
                },
            ),
            (
                beam_ring(1, 2, &BEAM),
                TempEffect::BeamRing {
                    start_entity: 1,
                    end_entity: 2,
                    beam: BEAM,
                },
            ),
            (
                text_message(
                    1,
                    -4096,
                    2048,
                    2,
                    [255, 200, 100, 255],
                    [255, 0, 0, 255],
                    64,
                    128,
                    512,
                    Some(256),
                    "PHASE 8",
                ),
                TempEffect::TextMessage {
                    channel: 1,
                    x: -4096,
                    y: 2048,
                    effect: 2,
                    color1: [255, 200, 100, 255],
                    color2: [255, 0, 0, 255],
                    fadein: 64,
                    fadeout: 128,
                    holdtime: 512,
                    fxtime: Some(256),
                    text: "PHASE 8".to_string(),
                },
            ),
            (
                text_message(
                    0,
                    0,
                    0,
                    0,
                    [255, 255, 255, 255],
                    [0, 0, 0, 0],
                    10,
                    20,
                    30,
                    None,
                    "FADE",
                ),
                TempEffect::TextMessage {
                    channel: 0,
                    x: 0,
                    y: 0,
                    effect: 0,
                    color1: [255, 255, 255, 255],
                    color2: [0, 0, 0, 0],
                    fadein: 10,
                    fadeout: 20,
                    holdtime: 30,
                    fxtime: None,
                    text: "FADE".to_string(),
                },
            ),
            (
                line(P, Q, 15, [255, 128, 0]),
                TempEffect::Line {
                    start: P,
                    end: Q,
                    life: 15,
                    color: [255, 128, 0],
                },
            ),
            (
                box_fx(P, Q, 20, [0, 255, 64]),
                TempEffect::Box {
                    min: P,
                    max: Q,
                    life: 20,
                    color: [0, 255, 64],
                },
            ),
            (
                large_funnel(P, 77, 1),
                TempEffect::LargeFunnel {
                    position: P,
                    sprite: 77,
                    flags: 1,
                },
            ),
            (show_line(P, Q), TempEffect::ShowLine { start: P, end: Q }),
            (
                blood(P, Q, 247, 120),
                TempEffect::Blood {
                    origin: P,
                    direction: Q,
                    color: 247,
                    speed: 120,
                },
            ),
            (
                model(P, Q, 90, 333, 2, 150),
                TempEffect::Model {
                    position: P,
                    velocity: Q,
                    angle: 90,
                    model: 333,
                    sound_type: 2,
                    life: 150,
                },
            ),
            (
                explode_model(P, 200.0, 444, 12, 30),
                TempEffect::ExplodeModel {
                    position: P,
                    velocity: 200.0,
                    model: 444,
                    count: 12,
                    life: 30,
                },
            ),
            (
                sprite_spray(P, Q, 88, 16, 40, 20),
                TempEffect::SpriteSpray {
                    position: P,
                    direction: Q,
                    sprite: 88,
                    count: 16,
                    speed: 40,
                    noise: 20,
                },
            ),
            (
                armor_ricochet(P, 8),
                TempEffect::ArmorRicochet {
                    position: P,
                    scale: 8,
                },
            ),
            (
                player_decal(3, P, 0, 14),
                TempEffect::PlayerDecal {
                    player: 3,
                    position: P,
                    entity: 0,
                    decal: 14,
                },
            ),
            (
                bubbles(P, Q, 128.0, 19, 25, 8.0),
                TempEffect::Bubbles {
                    mins: P,
                    maxs: Q,
                    height: 128.0,
                    model: 19,
                    count: 25,
                    speed: 8.0,
                },
            ),
            (
                bubble_trail(P, Q, 64.0, 19, 10, 6.0),
                TempEffect::BubbleTrail {
                    mins: P,
                    maxs: Q,
                    height: 64.0,
                    model: 19,
                    count: 10,
                    speed: 6.0,
                },
            ),
            (
                projectile(P, Q, 95, 30, 1),
                TempEffect::Projectile {
                    position: P,
                    velocity: Q,
                    model: 95,
                    life: 30,
                    owner: 1,
                },
            ),
            (
                spray(P, Q, 45, 8, 50, 12, 5),
                TempEffect::Spray {
                    position: P,
                    direction: Q,
                    model: 45,
                    count: 8,
                    speed: 50,
                    noise: 12,
                    rendermode: 5,
                },
            ),
            (
                player_sprites(2, 66, 12, 4),
                TempEffect::PlayerSprites {
                    entity: 2,
                    model: 66,
                    count: 12,
                    variance: 4,
                },
            ),
            (
                particle_burst(P, 200, 70, 5),
                TempEffect::ParticleBurst {
                    position: P,
                    radius: 200,
                    color: 70,
                    duration: 5,
                },
            ),
            (
                fire_field(P, 150, 52, 20, 3, 25),
                TempEffect::FireField {
                    position: P,
                    radius: 150,
                    model: 52,
                    count: 20,
                    flags: 3,
                    life: 25,
                },
            ),
            (
                player_attachment(1, 35.0, 71, 15),
                TempEffect::PlayerAttachment {
                    entity: 1,
                    offset_z: 35.0,
                    model: 71,
                    life: 15,
                },
            ),
            (
                kill_player_attachments(1),
                TempEffect::KillPlayerAttachments { entity: 1 },
            ),
            (
                multi_gunshot(P, Q, 0.25, 0.5, 8, 3),
                TempEffect::MultiGunshot {
                    origin: P,
                    direction: Q,
                    x_noise: 0.25,
                    y_noise: 0.5,
                    count: 8,
                    decal: 3,
                },
            ),
            (
                user_tracer(P, Q, 10, 4, 30),
                TempEffect::UserTracer {
                    origin: P,
                    velocity: Q,
                    life: 10,
                    color: 4,
                    length: 30,
                },
            ),
        ]
    }

    fn assert_category_roundtrip(filter: fn(u8) -> bool) -> std::collections::BTreeSet<u8> {
        let mut seen = std::collections::BTreeSet::new();
        for (te, expected) in cases() {
            if !filter(te.te_type) {
                continue;
            }
            seen.insert(te.te_type);
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
        seen
    }

    fn is_beam_or_line_te(id: u8) -> bool {
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

    fn is_explosion_or_light_te(id: u8) -> bool {
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

    fn is_gunshot_blood_or_tracer_te(id: u8) -> bool {
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

    fn is_decal_sprite_hud_or_attachment_te(id: u8) -> bool {
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
    fn te_beams_and_lines_roundtrip_and_match_skip() {
        let seen = assert_category_roundtrip(is_beam_or_line_te);
        assert_eq!(seen.len(), 14);
    }

    #[test]
    fn te_explosions_sparks_and_lights_roundtrip_and_match_skip() {
        let seen = assert_category_roundtrip(is_explosion_or_light_te);
        assert_eq!(seen.len(), 14);
    }

    #[test]
    fn te_gunshots_blood_and_tracers_roundtrip_and_match_skip() {
        let seen = assert_category_roundtrip(is_gunshot_blood_or_tracer_te);
        assert_eq!(seen.len(), 15);
    }

    #[test]
    fn te_decals_sprites_hud_and_players_roundtrip_and_match_skip() {
        let seen = assert_category_roundtrip(is_decal_sprite_hud_or_attachment_te);
        assert_eq!(seen.len(), 16);
    }

    #[test]
    fn temp_entities_cover_all_59_active_ids() {
        let mut union = std::collections::BTreeSet::new();
        for f in [
            is_beam_or_line_te as fn(u8) -> bool,
            is_explosion_or_light_te,
            is_gunshot_blood_or_tracer_te,
            is_decal_sprite_hud_or_attachment_te,
        ] {
            let cat: std::collections::BTreeSet<u8> = cases()
                .into_iter()
                .filter_map(|(te, _)| f(te.te_type).then_some(te.te_type))
                .collect();
            for id in cat {
                assert!(union.insert(id), "duplicate category for TE {id}");
            }
        }
        assert_eq!(union.len(), 59, "expected all 59 active TE_* IDs, got {union:?}");
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
            TempEffect::parse(200, &[1, 2, 3, 4, 5, 6, 7]).unwrap(),
            TempEffect::Other {
                te_type: 200,
                payload: vec![1, 2, 3, 4, 5, 6, 7],
            }
        );
    }

    /// The tail bytes mean life, scale, brightness (glow sprite) and count (blood stream), as
    /// Xash `cl_tent.c` reads them (read 2026-11-23).
    #[test]
    fn glow_sprite_and_blood_stream_tail_semantics() {
        let mut payload = vec![0u8; 6];
        payload.extend_from_slice(&[9, 0, 4, 5, 6]); // model 9, life 4, scale 5, brightness 6
        assert_eq!(
            TempEffect::parse(te::GLOWSPRITE, &payload).unwrap(),
            TempEffect::GlowSprite {
                position: Vec3::ZERO,
                model: 9,
                life: 4,
                scale: 5,
                brightness: 6,
            }
        );
        let mut payload = vec![0u8; 12];
        payload.extend_from_slice(&[70, 33]); // colour 70, count 33
        assert_eq!(
            TempEffect::parse(te::BLOODSTREAM, &payload).unwrap(),
            TempEffect::BloodStream {
                origin: Vec3::ZERO,
                direction: Vec3::ZERO,
                color: 70,
                count: 33,
            }
        );
    }

    #[test]
    fn dlight_is_twelve_bytes() {
        assert_eq!(dlight(P, 1, [2, 3, 4], 5, 6).payload.len(), 12);
    }
}
