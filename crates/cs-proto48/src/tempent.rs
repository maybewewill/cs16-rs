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

use glam::Vec3;

use crate::bitbuf::MsgWriter;

/// The `TE_*` effect numbers used here (`common/const.h`).
pub mod te {
    pub const GUNSHOT: u8 = 2;
    pub const EXPLOSION: u8 = 3;
    pub const SMOKE: u8 = 5;
    pub const SPARKS: u8 = 9;
    pub const SPRITE: u8 = 17;
    pub const BEAMFOLLOW: u8 = 22;
    pub const GLOWSPRITE: u8 = 23;
    pub const STREAK_SPLASH: u8 = 25;
    pub const DLIGHT: u8 = 27;
    pub const BLOODSTREAM: u8 = 101;
    pub const DECAL: u8 = 104;
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

/// `TE_DLIGHT`: UNVERIFIED layout, 12 bytes. `coord x3`, `byte` radius in tens, `byte x3` colour, `byte` life in tenths,
/// `byte` decay rate in tenths.
///
/// Unverified: `const.h` lists an extra `byte (brightness)` after the colour (13 bytes), while
/// the client parser of the HL engine reads radius, r, g, b, life and decay (recalled from
/// memory, not fetched or checked against a capture). The 12-byte layout is what this builds.
/// Nothing in the weapon set sends `TE_DLIGHT`, and the round-trip test in
/// `cs-engine/tests/tempents.rs` only shows that the builder and the parser agree, not that
/// either is right.
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
