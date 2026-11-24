//! GoldSrc display gamma, texture gamma (`texgamma`), brightness, overbright scaling, and
//! optional classic software-renderer 4x4 Bayer RGB565 quantization (`r_software_mode`).

/// Uniform parameters governing GoldSrc color response across BSP, MDL, Sprite, Sky, and Decal shaders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GammaParams {
    pub gamma: f32,
    pub texgamma: f32,
    pub brightness: f32,
    pub overbright: f32,
    pub software_mode: bool,
}

impl Default for GammaParams {
    fn default() -> Self {
        Self {
            gamma: 2.5,
            texgamma: 2.0,
            brightness: 1.0,
            overbright: 2.0,
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

/// Reference CPU implementation of the GoldSrc shader gamma + optional software-mode pipeline.
pub fn apply_goldsrc_gamma_rgb(
    albedo: [f32; 3],
    light: [f32; 3],
    px_x: u32,
    px_y: u32,
    params: &GammaParams,
) -> [f32; 3] {
    let tex_exp = params.texgamma.max(0.5) * 0.5;
    let bright = params.brightness.max(0.1);
    let gamma_exp = 2.5 / params.gamma.max(0.5);
    let bayer = BAYER_4X4[(px_y & 3) as usize][(px_x & 3) as usize];
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let a = albedo[c].clamp(0.0, 1.0).powf(tex_exp);
        let lit = (a * light[c].max(0.0) * params.overbright * bright).clamp(0.0, 1.0);
        let mut g = lit.powf(gamma_exp);
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
        // 0.5 * 0.5 * 2.0 = 0.5; gamma 2.5 => exp 1.0 => exact 0.5
        assert!((mid[0] - 0.5).abs() < 1e-5, "{mid:?}");

        let brighter = apply_goldsrc_gamma_rgb(
            [0.5; 3],
            [0.5; 3],
            0,
            0,
            &GammaParams {
                brightness: 1.5,
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
                gamma: 3.5,
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
        // R & B quantized to 31 steps, G to 63 steps.
        assert!((sw[0] * 31.0 - (sw[0] * 31.0).round()).abs() < 1e-5);
        assert!((sw[1] * 63.0 - (sw[1] * 63.0).round()).abs() < 1e-5);
    }
}
