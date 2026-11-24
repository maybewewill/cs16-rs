//! Texture filtering modes (`gl_texturemode` + `gl_ansio`), POT mipmap generation, dynamic
//! light pool (`TE_DLIGHT`, `TE_ELIGHT`, muzzleflashes), and flashlight spotlight raycasts.

use bytemuck::{Pod, Zeroable};
use cs_assets::gamma::GammaTables;
use cs_clientcore::tempents::DynamicLight;
use cs_pm::collision::{Hull, WorldCollision};
use glam::Vec3;

pub const MAX_DLIGHTS: usize = 32;
pub const MAX_ELIGHTS: usize = 64;
pub const MAX_GPU_DLIGHTS: usize = 16;
pub const MAX_GPU_SPOTLIGHTS: usize = 4;

/// All 6 GoldSrc `gl_texturemode` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextureFilterMode {
    Nearest,
    Linear,
    NearestMipmapNearest,
    LinearMipmapNearest,
    NearestMipmapLinear,
    #[default]
    LinearMipmapLinear,
}

impl TextureFilterMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "GL_NEAREST" => Self::Nearest,
            "GL_LINEAR" => Self::Linear,
            "GL_NEAREST_MIPMAP_NEAREST" => Self::NearestMipmapNearest,
            "GL_LINEAR_MIPMAP_NEAREST" => Self::LinearMipmapNearest,
            "GL_NEAREST_MIPMAP_LINEAR" => Self::NearestMipmapLinear,
            _ => Self::LinearMipmapLinear,
        }
    }

    pub fn filters(self) -> (wgpu::FilterMode, wgpu::FilterMode, wgpu::MipmapFilterMode) {
        use wgpu::FilterMode::{Linear, Nearest};
        use wgpu::MipmapFilterMode as Mip;
        match self {
            Self::Nearest => (Nearest, Nearest, Mip::Nearest),
            Self::Linear => (Linear, Linear, Mip::Nearest),
            Self::NearestMipmapNearest => (Nearest, Nearest, Mip::Nearest),
            Self::LinearMipmapNearest => (Linear, Linear, Mip::Nearest),
            Self::NearestMipmapLinear => (Nearest, Nearest, Mip::Linear),
            Self::LinearMipmapLinear => (Linear, Linear, Mip::Linear),
        }
    }
}

/// Creates a WebGPU/WebGL2-valid sampler; forces `anisotropy_clamp = 1` unless all three filters are Linear.
pub fn create_sampler(
    device: &wgpu::Device,
    label: &str,
    mode: TextureFilterMode,
    anisotropy: u16,
    clamp_edge: bool,
) -> wgpu::Sampler {
    let (mag_filter, min_filter, mipmap_filter) = mode.filters();
    let addr = if clamp_edge {
        wgpu::AddressMode::ClampToEdge
    } else {
        wgpu::AddressMode::Repeat
    };
    let all_linear = mag_filter == wgpu::FilterMode::Linear
        && min_filter == wgpu::FilterMode::Linear
        && mipmap_filter == wgpu::MipmapFilterMode::Linear;
    let aniso = if all_linear {
        anisotropy.clamp(1, 16)
    } else {
        1
    };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: addr,
        address_mode_v: addr,
        address_mode_w: addr,
        mag_filter,
        min_filter,
        mipmap_filter,
        anisotropy_clamp: aniso,
        ..Default::default()
    })
}

/// GoldSrc texture upload scaling and gamma configuration matching `gl_round_down`,
/// `gl_picmip`, `gl_max_size`, and `LUMP_TEXGAMMA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureUploadConfig {
    pub gl_round_down: i32,
    pub gl_picmip: i32,
    pub gl_max_size: u32,
    pub apply_texgamma: bool,
}

impl Default for TextureUploadConfig {
    fn default() -> Self {
        Self {
            gl_round_down: 3,
            gl_picmip: 0,
            gl_max_size: 512,
            apply_texgamma: true,
        }
    }
}

/// Port of `GL_SetTextureDimensions` (`ref/gl/gl_image.c:431-517`, read 2026-11-24).
///
/// Rounds non-POT sizes to power-of-two according to `gl_round_down` (`0..=3`), applies
/// `gl_picmip`, and halves both axes until `<= gl_max_size`.
pub fn gl_set_texture_dimensions(
    width: u32,
    height: u32,
    gl_round_down: i32,
    gl_picmip: i32,
    gl_max_size: u32,
) -> (u32, u32) {
    let w_in = width.max(1);
    let h_in = height.max(1);
    let step = gl_round_down.clamp(0, 4);

    let mut scaled_w = 1u32;
    while scaled_w < w_in {
        scaled_w <<= 1;
    }
    if step > 0
        && w_in < scaled_w
        && (step == 1 || (scaled_w - w_in) > (scaled_w >> step as u32))
    {
        scaled_w >>= 1;
    }

    let mut scaled_h = 1u32;
    while scaled_h < h_in {
        scaled_h <<= 1;
    }
    if step > 0
        && h_in < scaled_h
        && (step == 1 || (scaled_h - h_in) > (scaled_h >> step as u32))
    {
        scaled_h >>= 1;
    }

    let picmip = gl_picmip.clamp(0, 4) as u32;
    if picmip > 0 {
        scaled_w = (scaled_w >> picmip).max(1);
        scaled_h = (scaled_h >> picmip).max(1);
    }

    let max_size = gl_max_size.max(16);
    while scaled_w > max_size || scaled_h > max_size {
        scaled_w = (scaled_w >> 1).max(1);
        scaled_h = (scaled_h >> 1).max(1);
    }

    (scaled_w.max(1), scaled_h.max(1))
}

fn resample_rgba32_lerp_line(src: &[u8], dst: &mut [u8], in_w: usize, out_w: usize) {
    let fstep = ((in_w as f32) * 65536.0 / (out_w as f32)) as i32;
    let endx = in_w.saturating_sub(1);
    let mut f = 0i32;
    for x in 0..out_w {
        let xi = ((f >> 16) as usize).min(endx);
        let in_off = xi * 4;
        let out_off = x * 4;
        if xi < endx {
            let lerp = f & 0xFFFF;
            for c in 0..4 {
                let a = i32::from(src[in_off + c]);
                let b = i32::from(src[in_off + 4 + c]);
                dst[out_off + c] = ((((b - a) * lerp) >> 16) + a) as u8;
            }
        } else {
            dst[out_off..out_off + 4].copy_from_slice(&src[in_off..in_off + 4]);
        }
        f += fstep;
    }
}

/// Port of `Image_Resample32Lerp` (`engine/common/imagelib/img_utils.c:631-799`, read 2026-11-24).
pub fn resample_rgba32_lerp(
    indata: &[u8],
    in_w: u32,
    in_h: u32,
    out_w: u32,
    out_h: u32,
) -> Vec<u8> {
    let iw = in_w.max(1) as usize;
    let ih = in_h.max(1) as usize;
    let ow = out_w.max(1) as usize;
    let oh = out_h.max(1) as usize;
    if iw == ow && ih == oh && indata.len() >= iw * ih * 4 {
        return indata[..iw * ih * 4].to_vec();
    }
    let in_row_bytes = iw * 4;
    let out_row_bytes = ow * 4;
    let mut out = vec![0u8; ow * oh * 4];
    let fstep = ((ih as f32) * 65536.0 / (oh as f32)) as i32;
    let endy = ih.saturating_sub(1);

    let mut row1 = vec![0u8; out_row_bytes];
    let mut row2 = vec![0u8; out_row_bytes];
    let row_slice = |y: usize| -> &[u8] {
        let s = y.min(endy) * in_row_bytes;
        &indata[s..s + in_row_bytes]
    };

    resample_rgba32_lerp_line(row_slice(0), &mut row1, iw, ow);
    resample_rgba32_lerp_line(row_slice(1.min(endy)), &mut row2, iw, ow);

    let mut old_y = 0usize;
    let mut f = 0i32;
    for y in 0..oh {
        let yi = ((f >> 16) as usize).min(endy);
        let dst = &mut out[y * out_row_bytes..(y + 1) * out_row_bytes];
        if yi < endy {
            let lerp = f & 0xFFFF;
            if yi != old_y {
                if yi == old_y + 1 {
                    row1.copy_from_slice(&row2);
                } else {
                    resample_rgba32_lerp_line(row_slice(yi), &mut row1, iw, ow);
                }
                resample_rgba32_lerp_line(row_slice(yi + 1), &mut row2, iw, ow);
                old_y = yi;
            }
            for k in 0..out_row_bytes {
                let a = i32::from(row1[k]);
                let b = i32::from(row2[k]);
                dst[k] = ((((b - a) * lerp) >> 16) + a) as u8;
            }
        } else {
            if yi != old_y {
                if yi == old_y + 1 {
                    row1.copy_from_slice(&row2);
                } else {
                    resample_rgba32_lerp_line(row_slice(yi), &mut row1, iw, ow);
                }
                old_y = yi;
            }
            dst.copy_from_slice(&row1);
        }
        f += fstep;
    }

    out
}

/// Port of `GL_BuildMipMap` 2x2 box filter (`ref/gl/gl_image.c:805-830`, read 2026-11-24).
pub fn build_mip_level_box(src: &[u8], src_w: u32, src_h: u32) -> (u32, u32, Vec<u8>) {
    let next_w = (src_w >> 1).max(1);
    let next_h = (src_h >> 1).max(1);
    let in_stride = (src_w as usize) * 4;
    let mut next = Vec::with_capacity((next_w as usize) * (next_h as usize) * 4);
    for y in 0..next_h {
        let row0 = ((y as usize) * 2) * in_stride;
        let has_next_y = ((y << 1) + 1) < src_h;
        let row1 = if has_next_y { row0 + in_stride } else { row0 };
        for x in 0..next_w {
            let x0 = (x as usize) * 8;
            if ((x << 1) + 1) < src_w {
                for c in 0..4 {
                    let s = u32::from(src[row0 + x0 + c])
                        + u32::from(src[row0 + x0 + 4 + c])
                        + u32::from(src[row1 + x0 + c])
                        + u32::from(src[row1 + x0 + 4 + c]);
                    next.push((s >> 2) as u8);
                }
            } else {
                for c in 0..4 {
                    let s = u32::from(src[row0 + x0 + c]) + u32::from(src[row1 + x0 + c]);
                    next.push((s >> 1) as u8);
                }
            }
        }
    }
    (next_w, next_h, next)
}

/// Apply `texgammatable` (`LUMP_TEXGAMMA`), resample NPOT dimensions to GoldSrc POT via
/// `GL_SetTextureDimensions` + `Image_Resample32Lerp`, and generate the complete `GL_BuildMipMap`
/// 2x2 box-filter chain.
pub fn prepare_texture_mips(
    width: u32,
    height: u32,
    rgba: &[u8],
    tables: Option<&GammaTables>,
    cfg: TextureUploadConfig,
) -> Vec<(u32, u32, Vec<u8>)> {
    let w = width.max(1);
    let h = height.max(1);
    let expected = (w as usize) * (h as usize) * 4;
    let mut src = if rgba.len() >= expected {
        rgba[..expected].to_vec()
    } else {
        vec![255u8; expected]
    };
    if cfg.apply_texgamma
        && let Some(t) = tables
    {
        t.apply_texgamma_rgba(&mut src);
    }

    let (target_w, target_h) =
        gl_set_texture_dimensions(w, h, cfg.gl_round_down, cfg.gl_picmip, cfg.gl_max_size);
    let level0 = if target_w != w || target_h != h {
        resample_rgba32_lerp(&src, w, h, target_w, target_h)
    } else {
        src
    };

    let mut mips = vec![(target_w, target_h, level0)];
    let mut cur_w = target_w;
    let mut cur_h = target_h;
    while cur_w > 1 || cur_h > 1 {
        let (next_w, next_h, next) = build_mip_level_box(&mips.last().expect("mip 0").2, cur_w, cur_h);
        mips.push((next_w, next_h, next));
        cur_w = next_w;
        cur_h = next_h;
    }
    mips
}

/// Generate a full 2x2 box-filtered mip chain for an already-POT (or NPOT via GoldSrc defaults)
/// texture (`GL_SetTextureDimensions` + `Image_Resample32Lerp` + `GL_BuildMipMap`).
pub fn generate_mipmaps_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    prepare_texture_mips(width, height, rgba, None, TextureUploadConfig::default())
}

/// Upload an RGBA8 2D texture with GoldSrc POT scaling, optional `texgammatable` conversion,
/// and full mipmap chain.
#[allow(clippy::too_many_arguments)]
pub fn upload_rgba_texture_mipmapped_with_config(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
    tables: Option<&GammaTables>,
    cfg: TextureUploadConfig,
) -> wgpu::TextureView {
    let mips = prepare_texture_mips(width, height, rgba, tables, cfg);
    let w = mips[0].0;
    let h = mips[0].1;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (mw, mh, data)) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * mw),
                rows_per_image: Some(*mh),
            },
            wgpu::Extent3d {
                width: *mw,
                height: *mh,
                depth_or_array_layers: 1,
            },
        );
    }
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Upload an RGBA8 2D texture with GoldSrc default POT resampling and full mipmap chain.
pub fn upload_rgba_texture_mipmapped(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::TextureView {
    upload_rgba_texture_mipmapped_with_config(
        device,
        queue,
        label,
        width,
        height,
        rgba,
        None,
        TextureUploadConfig::default(),
    )
}

/// Flashlight or directional cone light (`EF_DIMLIGHT` / `impulse 100`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpotLight {
    pub origin: Vec3,
    pub dir: Vec3,
    pub radius: f32,
    pub cos_half_angle: f32,
    pub color: [f32; 3],
}

/// Compact uniform buffer holding active world dynamic lights and spotlights (720 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuLightsUniform {
    pub dlight_pos_rad: [[f32; 4]; MAX_GPU_DLIGHTS],
    pub dlight_col: [[f32; 4]; MAX_GPU_DLIGHTS],
    pub spot_pos_rad: [[f32; 4]; MAX_GPU_SPOTLIGHTS],
    pub spot_dir_cos: [[f32; 4]; MAX_GPU_SPOTLIGHTS],
    pub spot_col: [[f32; 4]; MAX_GPU_SPOTLIGHTS],
    pub counts: [u32; 4],
}

impl Default for GpuLightsUniform {
    fn default() -> Self {
        Self::zeroed()
    }
}

/// Client dynamic point-light & entity-light manager (`R_AddDynamicLights`).
#[derive(Debug, Clone, Default)]
pub struct DLightPool {
    pub dlights: Vec<DynamicLight>,
    pub elights: Vec<DynamicLight>,
}

impl DLightPool {
    pub fn clear(&mut self) {
        self.dlights.clear();
        self.elights.clear();
    }

    pub fn add_light(&mut self, light: DynamicLight) {
        if light.entity_light {
            if light.key != 0
                && let Some(slot) = self.elights.iter_mut().find(|l| l.key == light.key)
            {
                *slot = light;
                return;
            }
            if self.elights.len() >= MAX_ELIGHTS {
                self.elights.remove(0);
            }
            self.elights.push(light);
        } else {
            if light.key != 0
                && let Some(slot) = self.dlights.iter_mut().find(|l| l.key == light.key)
            {
                *slot = light;
                return;
            }
            if self.dlights.len() >= MAX_DLIGHTS {
                self.dlights.remove(0);
            }
            self.dlights.push(light);
        }
    }

    pub fn step(&mut self, now: f64, dt: f32) {
        let decay_dt = dt.max(0.0);
        for list in [&mut self.dlights, &mut self.elights] {
            list.retain_mut(|l| {
                l.radius -= l.decay * decay_dt;
                l.radius > 1.0 && l.die > now
            });
        }
    }

    /// Additional entity lighting at `origin` contributed by nearby active `elights` & `dlights`.
    pub fn sample_elights_at(&self, origin: Vec3) -> Vec3 {
        let mut add = Vec3::ZERO;
        for l in self.elights.iter().chain(self.dlights.iter()) {
            let r = l.radius.max(1.0);
            let dist = (l.origin - origin).length();
            if dist < r {
                let atten = 1.0 - (dist / r);
                add += Vec3::new(
                    f32::from(l.color[0]) / 255.0,
                    f32::from(l.color[1]) / 255.0,
                    f32::from(l.color[2]) / 255.0,
                ) * atten;
            }
        }
        add
    }

    pub fn pack_uniform(&self, cam_pos: Vec3, spotlights: &[SpotLight]) -> GpuLightsUniform {
        let mut out = GpuLightsUniform::default();
        let mut sorted: Vec<&DynamicLight> =
            self.dlights.iter().filter(|l| l.radius > 1.0).collect();
        sorted.sort_by(|a, b| {
            (a.origin - cam_pos)
                .length_squared()
                .total_cmp(&(b.origin - cam_pos).length_squared())
        });
        let nd = sorted.len().min(MAX_GPU_DLIGHTS);
        for (i, l) in sorted.into_iter().take(nd).enumerate() {
            out.dlight_pos_rad[i] = [l.origin.x, l.origin.y, l.origin.z, l.radius];
            out.dlight_col[i] = [
                f32::from(l.color[0]) / 255.0,
                f32::from(l.color[1]) / 255.0,
                f32::from(l.color[2]) / 255.0,
                1.0,
            ];
        }
        let ns = spotlights.len().min(MAX_GPU_SPOTLIGHTS);
        for (i, s) in spotlights.iter().take(ns).enumerate() {
            let dir = s.dir.normalize_or_zero();
            out.spot_pos_rad[i] = [s.origin.x, s.origin.y, s.origin.z, s.radius];
            out.spot_dir_cos[i] = [dir.x, dir.y, dir.z, s.cos_half_angle];
            out.spot_col[i] = [s.color[0], s.color[1], s.color[2], 1.0];
        }
        out.counts = [nd as u32, ns as u32, 0, 0];
        out
    }
}

/// Trace a GoldSrc flashlight ray from `origin` along `forward`, returning a spotlight plus a
/// soft bounce point light near the wall hit point.
pub fn compute_flashlight_spot(
    origin: Vec3,
    forward: Vec3,
    collision: Option<&WorldCollision>,
    now: f64,
) -> (SpotLight, Option<DynamicLight>) {
    let dir = forward.normalize_or_zero();
    let max_dist = 2048.0f32;
    let end = origin + dir * max_dist;
    let hit_dist = collision
        .map(|c| c.trace_world(origin, end, Hull::Point).fraction * max_dist)
        .unwrap_or(max_dist);
    let spot = SpotLight {
        origin,
        dir,
        radius: hit_dist.clamp(64.0, 1200.0),
        cos_half_angle: 22.0f32.to_radians().cos(),
        color: [0.95, 0.93, 0.85],
    };
    let bounce = (hit_dist < max_dist).then(|| DynamicLight {
        key: 0,
        entity_light: false,
        origin: origin + dir * (hit_dist - 4.0).max(0.0),
        radius: (128.0 - hit_dist * 0.05).clamp(48.0, 128.0),
        color: [220, 215, 195],
        die: now + 0.05,
        decay: 0.0,
    });
    (spot, bounce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_filter_modes_and_mipmaps_respect_pot_and_nearest_aniso() {
        assert_eq!(TextureFilterMode::parse("GL_NEAREST"), TextureFilterMode::Nearest);
        assert_eq!(
            TextureFilterMode::parse("gl_linear_mipmap_nearest"),
            TextureFilterMode::LinearMipmapNearest
        );
        let pot = generate_mipmaps_rgba(8, 8, &[255; 8 * 8 * 4]);
        assert_eq!(pot.len(), 4); // 8, 4, 2, 1
        assert_eq!((pot[3].0, pot[3].1), (1, 1));
        let dust_npot = generate_mipmaps_rgba(128, 240, &vec![180u8; 128 * 240 * 4]);
        assert_eq!((dust_npot[0].0, dust_npot[0].1), (128, 256));
        assert_eq!(dust_npot.len(), 9);
        assert_eq!(gl_set_texture_dimensions(192, 160, 3, 0, 512), (128, 128));

        // Verify texgammatable is applied BEFORE box-filter mipmap averaging.
        let tables = GammaTables::default();
        let src = [
            100u8, 100, 100, 255, 200, 200, 200, 255, //
            100, 100, 100, 255, 200, 200, 200, 255,
        ];
        let mips = prepare_texture_mips(2, 2, &src, Some(&tables), TextureUploadConfig::default());
        let g100 = u32::from(tables.texture_to_gamma(100));
        let g200 = u32::from(tables.texture_to_gamma(200));
        let expected_mip1 = ((g100 * 2 + g200 * 2) >> 2) as u8;
        assert_eq!(mips[1].2[0], expected_mip1);
    }

    #[test]
    fn dlight_pool_replaces_keyed_lights_decays_and_packs() {
        let mut pool = DLightPool::default();
        pool.add_light(DynamicLight {
            key: 7,
            entity_light: false,
            origin: Vec3::new(100.0, 0.0, 0.0),
            radius: 200.0,
            color: [255, 128, 0],
            die: 10.0,
            decay: 50.0,
        });
        pool.add_light(DynamicLight {
            key: 7,
            entity_light: false,
            origin: Vec3::new(50.0, 0.0, 0.0),
            radius: 180.0,
            color: [255, 200, 100],
            die: 10.0,
            decay: 50.0,
        });
        assert_eq!(pool.dlights.len(), 1);
        pool.step(2.0, 1.0);
        assert!((pool.dlights[0].radius - 130.0).abs() < 1e-4);

        let (spot, bounce) = compute_flashlight_spot(Vec3::ZERO, Vec3::X, None, 2.0);
        assert!(bounce.is_none());
        let u = pool.pack_uniform(Vec3::ZERO, &[spot]);
        assert_eq!(u.counts[0], 1);
        assert_eq!(u.counts[1], 1);
        assert!(u.dlight_col[0][0] > 0.9);
    }
}
