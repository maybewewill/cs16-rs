//! 1-line strongly-typed Protocol 48 `TE_*` VFX builder helpers (`VfxOp`).
//!
//! Works identically across retail `hl.exe`, native `cs-client`, `cs-web`, and native unit tests.

use cs_mod_api::VfxOp;

use crate::bridge::HostBridge;

/// `TE_BEAMCYLINDER` expanding shockwave ring (Frost Nade, level-up, heal wave).
#[allow(clippy::too_many_arguments)]
pub fn shockwave_ring(
    host: &mut impl HostBridge,
    origin: [f32; 3],
    radius: f32,
    rgba: [u8; 4],
    life_decisec: u8,
    width: u8,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::ShockwaveRing {
        origin,
        radius,
        rgba,
        life_decisec,
        width,
        slot,
    })
}

/// `TE_BEAMPOINTS` laser / railgun / LaserMine tripwire beam.
#[allow(clippy::too_many_arguments)]
pub fn laser_beam(
    host: &mut impl HostBridge,
    start: [f32; 3],
    end: [f32; 3],
    rgba: [u8; 4],
    life_decisec: u8,
    width: u8,
    noise: u8,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::LaserBeam {
        start,
        end,
        rgba,
        life_decisec,
        width,
        noise,
        slot,
    })
}

/// `TE_BEAMFOLLOW` colored ribbon trail attached to an entity index (e.g. thrown grenade).
pub fn follow_trail(
    host: &mut impl HostBridge,
    ent_index: u16,
    rgba: [u8; 4],
    life_decisec: u8,
    width: u8,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::FollowTrail {
        ent_index,
        rgba,
        life_decisec,
        width,
        slot,
    })
}

/// `TE_KILLBEAM` removes all attached beams from `ent_index`.
pub fn kill_beam(host: &mut impl HostBridge, ent_index: u16, slot: Option<u8>) -> i32 {
    host.emit_vfx(VfxOp::KillBeam { ent_index, slot })
}

/// `TE_DLIGHT` dynamic point glow (VIP/leader/zombie aura).
#[allow(clippy::too_many_arguments)]
pub fn aura_light(
    host: &mut impl HostBridge,
    origin: [f32; 3],
    radius: u8,
    rgb: [u8; 3],
    life_decisec: u8,
    decay: u8,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::AuraLight {
        origin,
        radius,
        rgb,
        life_decisec,
        decay,
        slot,
    })
}

/// `TE_SPARKS` metallic spark shower.
pub fn sparks(host: &mut impl HostBridge, origin: [f32; 3], slot: Option<u8>) -> i32 {
    host.emit_vfx(VfxOp::Sparks { origin, slot })
}

/// `TE_IMPLOSION` inward-moving particle burst.
pub fn implosion(
    host: &mut impl HostBridge,
    origin: [f32; 3],
    radius: u8,
    count: u8,
    life_decisec: u8,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::Implosion {
        origin,
        radius,
        count,
        life_decisec,
        slot,
    })
}

/// `TE_TELEPORT` classic GoldSrc teleport splash particles.
pub fn teleport_splash(host: &mut impl HostBridge, origin: [f32; 3], slot: Option<u8>) -> i32 {
    host.emit_vfx(VfxOp::TeleportSplash { origin, slot })
}

/// `TE_LAVASPLASH` classic red/orange particle fountain.
pub fn lava_splash(host: &mut impl HostBridge, origin: [f32; 3], slot: Option<u8>) -> i32 {
    host.emit_vfx(VfxOp::LavaSplash { origin, slot })
}

/// `TE_EXPLOSION` fireball + particles + dynamic light.
pub fn explosion(
    host: &mut impl HostBridge,
    origin: [f32; 3],
    scale: u8,
    flags: u8,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::Explosion {
        origin,
        scale,
        flags,
        slot,
    })
}

/// `TE_BLOODSTREAM` directional blood jet.
pub fn blood_stream(
    host: &mut impl HostBridge,
    origin: [f32; 3],
    dir: [f32; 3],
    color: u8,
    speed: u8,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::BloodStream {
        origin,
        dir,
        color,
        speed,
        slot,
    })
}

/// `TE_WORLDDECAL` permanent brush decal by WAD decal name (e.g. `"{scorch1"`).
pub fn world_decal(
    host: &mut impl HostBridge,
    origin: [f32; 3],
    decal_name: impl Into<String>,
    slot: Option<u8>,
) -> i32 {
    host.emit_vfx(VfxOp::WorldDecal {
        origin,
        decal_name: decal_name.into(),
        slot,
    })
}
