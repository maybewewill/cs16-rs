//! Material bullet impact, ricochet, and weapon reload sounds (`pm_materials.h` + `cs16-client`).

use cs_clientcore::SoundStart;
use cs_clientcore::sound::chan;
use cs_pm::pm::consts::{
    CHAR_TEX_COMPUTER, CHAR_TEX_CONCRETE, CHAR_TEX_DIRT, CHAR_TEX_FLESH, CHAR_TEX_GLASS,
    CHAR_TEX_GRATE, CHAR_TEX_METAL, CHAR_TEX_SLOSH, CHAR_TEX_SNOW, CHAR_TEX_TILE, CHAR_TEX_VENT,
    CHAR_TEX_WOOD,
};
use glam::Vec3;

/// Returns `(sample_path_without_sound_prefix, volume, pitch)` matching GoldSrc surface material impacts.
pub fn material_impact_sound(tex_type: u8, rng_seed: u32) -> (&'static str, f32, u8) {
    let r = (rng_seed as usize) % 12;
    let pitch = 96 + ((rng_seed >> 4) % 9) as u8;
    match tex_type {
        CHAR_TEX_METAL | CHAR_TEX_VENT | CHAR_TEX_GRATE => {
            const SAMPLES: [&str; 6] = [
                "debris/metal1.wav",
                "debris/metal2.wav",
                "debris/metal3.wav",
                "debris/metal4.wav",
                "debris/metal5.wav",
                "debris/metal6.wav",
            ];
            (SAMPLES[r % SAMPLES.len()], 0.85, pitch)
        }
        CHAR_TEX_WOOD => {
            const SAMPLES: [&str; 4] = [
                "debris/wood1.wav",
                "debris/wood2.wav",
                "debris/wood3.wav",
                "debris/wood4.wav",
            ];
            (SAMPLES[r % SAMPLES.len()], 0.85, pitch)
        }
        CHAR_TEX_GLASS | CHAR_TEX_COMPUTER => {
            const SAMPLES: [&str; 4] = [
                "debris/glass1.wav",
                "debris/glass2.wav",
                "debris/glass3.wav",
                "debris/glass4.wav",
            ];
            (SAMPLES[r % SAMPLES.len()], 0.80, pitch)
        }
        CHAR_TEX_DIRT | CHAR_TEX_SNOW => {
            const SAMPLES: [&str; 4] = [
                "player/pl_dirt1.wav",
                "player/pl_dirt2.wav",
                "player/pl_dirt3.wav",
                "player/pl_dirt4.wav",
            ];
            (SAMPLES[r % SAMPLES.len()], 0.75, pitch)
        }
        CHAR_TEX_TILE => {
            const SAMPLES: [&str; 4] = [
                "player/pl_tile1.wav",
                "player/pl_tile2.wav",
                "player/pl_tile3.wav",
                "player/pl_tile4.wav",
            ];
            (SAMPLES[r % SAMPLES.len()], 0.75, pitch)
        }
        CHAR_TEX_SLOSH => {
            const SAMPLES: [&str; 4] = [
                "player/pl_slosh1.wav",
                "player/pl_slosh2.wav",
                "player/pl_slosh3.wav",
                "player/pl_slosh4.wav",
            ];
            (SAMPLES[r % SAMPLES.len()], 0.75, pitch)
        }
        CHAR_TEX_FLESH => {
            const SAMPLES: [&str; 2] = ["debris/bustflesh1.wav", "debris/bustflesh2.wav"];
            (SAMPLES[r % SAMPLES.len()], 0.80, pitch)
        }
        // `CHAR_TEX_CONCRETE` and default stone/brick surfaces.
        _ => {
            const SAMPLES: [&str; 7] = [
                "weapons/ric1.wav",
                "weapons/ric2.wav",
                "weapons/ric3.wav",
                "weapons/ric4.wav",
                "weapons/ric5.wav",
                "weapons/ric_conc-1.wav",
                "weapons/ric_conc-2.wav",
            ];
            (SAMPLES[r % SAMPLES.len()], 0.75, pitch)
        }
    }
}

/// Optional secondary ricochet whine on hard metal/concrete impacts (~1/4 chance).
pub fn ricochet_sound(tex_type: u8, rng_seed: u32) -> Option<(&'static str, f32, u8)> {
    if !(rng_seed >> 8).is_multiple_of(4) {
        return None;
    }
    let idx = (rng_seed as usize) >> 2;
    let pitch = 94 + ((rng_seed >> 12) % 13) as u8;
    match tex_type {
        CHAR_TEX_METAL | CHAR_TEX_VENT | CHAR_TEX_GRATE => {
            const SAMPLES: [&str; 2] = ["weapons/ric_metal-1.wav", "weapons/ric_metal-2.wav"];
            Some((SAMPLES[idx % SAMPLES.len()], 0.70, pitch))
        }
        CHAR_TEX_CONCRETE | CHAR_TEX_TILE => {
            const SAMPLES: [&str; 2] = ["weapons/ric_conc-1.wav", "weapons/ric_conc-2.wav"];
            Some((SAMPLES[idx % SAMPLES.len()], 0.65, pitch))
        }
        _ => None,
    }
}

/// Build `SoundStart` commands for a bullet impact point on `tex_type`.
pub fn build_impact_sounds(origin: Vec3, tex_type: u8, rng_seed: u32) -> Vec<SoundStart> {
    let mut out = Vec::with_capacity(2);
    let (sample, vol, pitch) = material_impact_sound(tex_type, rng_seed);
    out.push(SoundStart::by_name(sample, 0, chan::AUTO, origin, vol, 1.2, pitch));
    if let Some((ric, rvol, rpitch)) = ricochet_sound(tex_type, rng_seed) {
        out.push(SoundStart::by_name(ric, 0, chan::AUTO, origin, rvol, 1.0, rpitch));
    }
    out
}

/// Maps `ReloadSound` message `kind` byte (`0` rifle/pistol, `1` shotgun shell) to retail WAV path.
pub fn reload_sound_path(kind: u8) -> &'static str {
    if kind == 1 {
        "weapons/generic_shot_reload.wav"
    } else {
        "weapons/generic_reload.wav"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_surface_material_and_reload_kind() {
        for mat in [
            CHAR_TEX_CONCRETE,
            CHAR_TEX_METAL,
            CHAR_TEX_VENT,
            CHAR_TEX_GRATE,
            CHAR_TEX_TILE,
            CHAR_TEX_WOOD,
            CHAR_TEX_COMPUTER,
            CHAR_TEX_GLASS,
            CHAR_TEX_DIRT,
            CHAR_TEX_SLOSH,
            CHAR_TEX_SNOW,
            CHAR_TEX_FLESH,
        ] {
            let (sample, vol, pitch) = material_impact_sound(mat, 1024);
            assert!(sample.ends_with(".wav"));
            assert!(vol > 0.0 && pitch >= 96);
        }
        assert_eq!(reload_sound_path(0), "weapons/generic_reload.wav");
        assert_eq!(reload_sound_path(1), "weapons/generic_shot_reload.wav");
    }
}
