//! Texture filtering modes (`gl_texturemode` + `gl_ansio`), POT mipmap generation, dynamic
//! light pool (`TE_DLIGHT`, `TE_ELIGHT`, muzzleflashes), and flashlight spotlight raycasts.

use bytemuck::{Pod, Zeroable};
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

/// Generate a full 2x2 box-filtered mip chain when both dimensions are powers of two. Non-POT
/// textures return only mip level 0 to maintain strict WebGL2 compatibility.
pub fn generate_mipmaps_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let w = width.max(1);
    let h = height.max(1);
    let expected = (w as usize) * (h as usize) * 4;
    let level0 = if rgba.len() >= expected {
        rgba[..expected].to_vec()
    } else {
        vec![255u8; expected]
    };
    let mut mips = vec![(w, h, level0)];
    if !w.is_power_of_two() || !h.is_power_of_two() {
        return mips;
    }

    let mut cur_w = w;
    let mut cur_h = h;
    while cur_w > 1 || cur_h > 1 {
        let next_w = (cur_w / 2).max(1);
        let next_h = (cur_h / 2).max(1);
        let prev = &mips.last().expect("at least mip 0").2;
        let mut next = Vec::with_capacity((next_w as usize) * (next_h as usize) * 4);
        for y in 0..next_h {
            let y0 = (y * 2).min(cur_h - 1);
            let y1 = (y0 + 1).min(cur_h - 1);
            for x in 0..next_w {
                let x0 = (x * 2).min(cur_w - 1);
                let x1 = (x0 + 1).min(cur_w - 1);
                let mut sum = [0u32; 4];
                for (sx, sy) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                    let off = ((sy * cur_w + sx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += u32::from(prev[off + c]);
                    }
                }
                next.push((sum[0] / 4) as u8);
                next.push((sum[1] / 4) as u8);
                next.push((sum[2] / 4) as u8);
                next.push((sum[3] / 4) as u8);
            }
        }
        mips.push((next_w, next_h, next));
        cur_w = next_w;
        cur_h = next_h;
    }
    mips
}

/// Upload an RGBA8 2D texture with automatic POT mipmap chain and return its view.
pub fn upload_rgba_texture_mipmapped(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::TextureView {
    let mips = generate_mipmaps_rgba(width, height, rgba);
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
        let npot = generate_mipmaps_rgba(6, 10, &[128; 6 * 10 * 4]);
        assert_eq!(npot.len(), 1);
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
