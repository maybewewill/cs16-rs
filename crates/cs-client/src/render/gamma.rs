//! GoldSrc LUT-based gamma, texture gamma (`texgammatable`), lightmap gamma (`lightgammatable`),
//! `gl_overbright`, and optional software-renderer 4x4 Bayer RGB565 quantization (`r_software_mode`).
//!
//! Ported from Xash3D FWGS (`engine/client/gamma.c` `BuildGammaTable`, `ref/gl/gl_rsurf.c`
//! `R_BuildLightMap`), all read 2026-11-24.

pub use cs_assets::gamma::*;

/// Parameters governing GoldSrc LUT color response across world/model/sprite/sky/decal pipelines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GammaParams {
    pub gamma: f32,
    pub texgamma: f32,
    pub brightness: f32,
    pub lightgamma: f32,
    pub gl_overbright: bool,
    pub software_mode: bool,
}

impl Default for GammaParams {
    fn default() -> Self {
        Self {
            gamma: 2.5,
            texgamma: 2.0,
            brightness: 0.0,
            lightgamma: 2.5,
            gl_overbright: false,
            software_mode: false,
        }
    }
}

const BAYER_4X4: [[f32; 4]; 4] = [
    [0.0 / 16.0, 8.0 / 16.0, 2.0 / 16.0, 10.0 / 16.0],
    [12.0 / 16.0, 4.0 / 16.0, 14.0 / 16.0, 6.0 / 16.0],
    [3.0 / 16.0, 11.0 / 16.0, 1.0 / 16.0, 9.0 / 16.0],
    [15.0 / 16.0, 7.0 / 16.0, 13.0 / 16.0, 5.0 / 16.0],
];

/// Reference CPU evaluation of the GoldSrc LUT texture + lightmap + software-dither pipeline.
pub fn apply_goldsrc_gamma_rgb(
    albedo: [f32; 3],
    light: [f32; 3],
    px_x: u32,
    px_y: u32,
    params: &GammaParams,
) -> [f32; 3] {
    let tables = GammaTables::build(GoldSrcGammaConfig {
        gamma: params.gamma,
        brightness: params.brightness,
        texgamma: params.texgamma,
        lightgamma: params.lightgamma,
    });
    let lightscale = tables.lightscale(params.gl_overbright);
    let style_scale = lightstyle_f32_to_scale(1.0);
    let bl = [
        ((light[0].max(0.0) * 255.0).round() as u32) * style_scale,
        ((light[1].max(0.0) * 255.0).round() as u32) * style_scale,
        ((light[2].max(0.0) * 255.0).round() as u32) * style_scale,
    ];
    let lm_rgba = build_lightmap_pixel(bl, lightscale, &tables);
    let overbright_mul = if params.gl_overbright { 2.0 } else { 1.0 };
    let bayer = BAYER_4X4[(px_y & 3) as usize][(px_x & 3) as usize];
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let tex_in = (albedo[c].clamp(0.0, 1.0) * 255.0).round() as u8;
        let tex_f = f32::from(tables.texture_to_gamma(tex_in)) / 255.0;
        let lm_f = f32::from(lm_rgba[c]) / 255.0;
        let mut g = (tex_f * lm_f * overbright_mul).clamp(0.0, 1.0);
        if params.software_mode {
            let steps = if c == 1 { 63.0 } else { 31.0 };
            g = ((g * steps + bayer).floor() / steps).clamp(0.0, 1.0);
        }
        out[c] = g;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_texgamma_brightness_and_software_dither_work() {
        let base = GammaParams::default();
        let mid = apply_goldsrc_gamma_rgb([0.5; 3], [0.5; 3], 0, 0, &base);
        // LUT evaluation matches `BuildGammaTable` + `R_BuildLightMap` (`gamma=2.5, brightness=0.0`)
        assert!(mid[0] > 0.15 && mid[0] < 0.45, "{mid:?}");

        let brighter = apply_goldsrc_gamma_rgb(
            [0.5; 3],
            [0.5; 3],
            0,
            0,
            &GammaParams {
                brightness: 1.0,
                ..base
            },
        );
        assert!(brighter[0] > mid[0]);

        let higher_gamma = apply_goldsrc_gamma_rgb(
            [0.5; 3],
            [0.5; 3],
            0,
            0,
            &GammaParams {
                gamma: 3.0,
                ..base
            },
        );
        assert!(higher_gamma[0] > mid[0]);

        let dark_texgamma = apply_goldsrc_gamma_rgb(
            [0.5; 3],
            [0.5; 3],
            0,
            0,
            &GammaParams {
                texgamma: 3.0,
                ..base
            },
        );
        assert!(dark_texgamma[0] < mid[0]);

        let sw = apply_goldsrc_gamma_rgb(
            [0.37; 3],
            [0.5; 3],
            1,
            2,
            &GammaParams {
                software_mode: true,
                ..base
            },
        );
        assert!((sw[0] * 31.0 - (sw[0] * 31.0).round()).abs() < 1e-5);
        assert!((sw[1] * 63.0 - (sw[1] * 63.0).round()).abs() < 1e-5);
    }
}
