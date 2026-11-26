//! Clean-room procedural GoldSrc `WAD3` / `MipTex` textures, index-alpha decals,
//! and 6-face uncompressed `TGA` skybox faces for `dm_rusthouse`.

use crate::wad::{DECAL_MIPTEX_TYPE, MIPTEX_TYPE, WAD3_MAGIC};

/// Texture indices embedded into `maps/dm_rusthouse.bsp` and `demo.wad`.
pub const TEX_FLOOR: usize = 0;
pub const TEX_SANDWALL: usize = 1;
pub const TEX_CRATE: usize = 2;
pub const TEX_RUSTTRIM: usize = 3;
pub const TEX_DECK: usize = 4;
pub const TEX_SPAWNCT: usize = 5;
pub const TEX_SPAWNT: usize = 6;
pub const TEX_SKY: usize = 7;

/// Embedded texture specs `(name, raw_miptex_bytes)` in fixed index order.
pub fn build_arena_miptex_list() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("RH_FLOOR", build_floor_miptex()),
        ("RH_SANDWALL", build_sandwall_miptex()),
        ("RH_CRATE", build_crate_miptex()),
        ("RH_RUSTTRIM", build_rusttrim_miptex()),
        ("RH_DECK", build_deck_miptex()),
        (
            "RH_SPAWNCT",
            build_spawn_miptex("RH_SPAWNCT", [66, 92, 118], [108, 148, 184]),
        ),
        (
            "RH_SPAWNT",
            build_spawn_miptex("RH_SPAWNT", [122, 76, 52], [186, 120, 72]),
        ),
        ("SKY", build_sky_miptex()),
    ]
}

/// Build a valid `WAD3` archive (`demo.wad`) containing all arena textures.
pub fn build_demo_wad(miptex_list: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let entries: Vec<(&str, u8, &[u8])> = miptex_list
        .iter()
        .map(|(name, bytes)| (*name, MIPTEX_TYPE, bytes.as_slice()))
        .collect();
    encode_wad3(&entries)
}

/// Build a clean-room `decals.wad` (`{shot1..5`, `{blood1..4`, `{scorch1..2`).
pub fn build_demo_decals_wad() -> Vec<u8> {
    let mut owned: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 1..=5 {
        owned.push((
            format!("{{shot{i}"),
            build_radial_decal(
                &format!("{{shot{i}"),
                16,
                [24, 22, 20],
                5.0 + (i as f32) * 0.35,
                i as u32,
            ),
        ));
    }
    for i in 1..=4 {
        owned.push((
            format!("{{blood{i}"),
            build_radial_decal(
                &format!("{{blood{i}"),
                16,
                [132, 16, 16],
                6.5,
                100 + i as u32,
            ),
        ));
    }
    for i in 1..=2 {
        owned.push((
            format!("{{scorch{i}"),
            build_radial_decal(
                &format!("{{scorch{i}"),
                32,
                [18, 16, 14],
                13.0,
                200 + i as u32,
            ),
        ));
    }
    let refs: Vec<(&str, u8, &[u8])> = owned
        .iter()
        .map(|(name, bytes)| (name.as_str(), DECAL_MIPTEX_TYPE, bytes.as_slice()))
        .collect();
    encode_wad3(&refs)
}

/// Generate the 6 clean-room uncompressed 24-bit TGA skybox faces (`gfx/env/rusthouse{suffix}.tga`).
pub fn build_skybox_tgas(skyname: &str) -> Vec<(String, Vec<u8>)> {
    const SIZE: u32 = 128;
    let suffixes = ["rt", "bk", "lf", "ft", "up", "dn"];
    let mut out = Vec::with_capacity(6);
    for suffix in suffixes {
        let mut rgb = vec![0u8; (SIZE * SIZE * 3) as usize];
        for y in 0..SIZE {
            let v = (y as f32 + 0.5) / SIZE as f32;
            for x in 0..SIZE {
                let u = (x as f32 + 0.5) / SIZE as f32;
                let col = sample_sky_face(suffix, u, v);
                let idx = ((y * SIZE + x) * 3) as usize;
                rgb[idx..idx + 3].copy_from_slice(&col);
            }
        }
        let path = format!("gfx/env/{skyname}{suffix}.tga");
        out.push((path, encode_tga_rgb24(SIZE as u16, SIZE as u16, &rgb)));
    }
    out
}

fn sample_sky_face(suffix: &str, u: f32, v: f32) -> [u8; 3] {
    // Direction vector matching `skybox.rs::build_goldsrc_sky_cube()` (`v = 0` top, `v = 1` bottom).
    let a = u * 2.0 - 1.0;
    let b = 1.0 - v * 2.0; // +1 at top, -1 at bottom
    let dir = match suffix {
        "rt" => glam::Vec3::new(1.0, -a, b),
        "bk" => glam::Vec3::new(a, 1.0, b),
        "lf" => glam::Vec3::new(-1.0, a, b),
        "ft" => glam::Vec3::new(-a, -1.0, b),
        "up" => glam::Vec3::new(a, -b, 1.0),
        "dn" => glam::Vec3::new(a, b, -1.0),
        _ => glam::Vec3::Z,
    }
    .normalize();

    // Warm Mediterranean late-afternoon desert sky matching the BSP sun vector.
    let sun_dir = glam::Vec3::new(0.45, -0.35, 0.82).normalize();
    let elev = dir.z;

    let mut rgb = if elev >= 0.0 {
        let t = elev.powf(0.55).clamp(0.0, 1.0);
        // Horizon warm golden haze -> soft azure -> deep cobalt zenith
        let horizon = [236.0f32, 212.0, 174.0];
        let mid = [118.0f32, 168.0, 214.0];
        let zenith = [48.0f32, 98.0, 162.0];
        if t < 0.35 {
            lerp3(horizon, mid, t / 0.35)
        } else {
            lerp3(mid, zenith, (t - 0.35) / 0.65)
        }
    } else {
        // Subtle sunlit ground haze below horizon
        let t = (-elev).clamp(0.0, 1.0);
        lerp3([214.0, 188.0, 152.0], [150.0, 128.0, 102.0], t)
    };

    // Subtle soft wisps of high clouds in the upper hemisphere
    if elev > 0.06 {
        let cx = dir.x / (elev + 0.35);
        let cy = dir.y / (elev + 0.35);
        let cloud = ((cx * 2.3).sin() * (cy * 1.9 + 0.7).cos() + 0.5 * (cx * 4.7 - cy * 3.1).sin())
            .clamp(0.0, 1.5);
        let mask = ((cloud - 0.55) * 0.9).clamp(0.0, 1.0) * (1.0 - elev * 0.55).clamp(0.0, 1.0);
        rgb = lerp3(rgb, [246.0, 238.0, 224.0], mask * 0.38);
    }

    // Distant warm sandstone ridge silhouette just above horizon
    let az = dir.y.atan2(dir.x);
    let ridge = 0.035 + 0.025 * (az * 3.0).sin() + 0.015 * (az * 7.0 + 1.2).cos();
    if elev < ridge && elev > -0.15 {
        let blend = ((ridge - elev) / 0.012).clamp(0.0, 1.0);
        rgb = lerp3(rgb, [176.0, 146.0, 114.0], blend * 0.72);
    }

    // Crisp sun disc + golden atmospheric halo
    let sdot = dir.dot(sun_dir).max(0.0);
    let halo = sdot.powf(14.0) * 0.55;
    let core = sdot.powf(260.0) * 1.1;
    let glow = (halo + core).min(1.0);
    rgb = lerp3(rgb, [255.0, 248.0, 220.0], glow);

    [
        rgb[0].clamp(0.0, 255.0) as u8,
        rgb[1].clamp(0.0, 255.0) as u8,
        rgb[2].clamp(0.0, 255.0) as u8,
    ]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Encode an uncompressed 24-bit BGR top-left TGA (`image_type = 2`, `descriptor = 0x20`).
pub fn encode_tga_rgb24(width: u16, height: u16, rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(18 + rgb.len());
    out.push(0); // id_len
    out.push(0); // color_map_type
    out.push(2); // uncompressed true-color
    out.extend_from_slice(&[0u8; 5]); // color map spec
    out.extend_from_slice(&0u16.to_le_bytes()); // x origin
    out.extend_from_slice(&0u16.to_le_bytes()); // y origin
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.push(24); // bpp
    out.push(0x20); // top-left origin
    for px in rgb.as_chunks::<3>().0 {
        out.push(px[2]);
        out.push(px[1]);
        out.push(px[0]);
    }
    out
}

/// Encode a `WAD3` archive from `(name, entry_type, bytes)` tuples.
pub fn encode_wad3(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(WAD3_MAGIC);
    data.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes()); // dir_offset patched below

    let mut dir_records: Vec<(u32, u32, u8, &str)> = Vec::with_capacity(entries.len());
    for &(name, entry_type, bytes) in entries {
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let off = data.len() as u32;
        data.extend_from_slice(bytes);
        dir_records.push((off, bytes.len() as u32, entry_type, name));
    }

    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let dir_offset = data.len() as u32;
    data[8..12].copy_from_slice(&dir_offset.to_le_bytes());

    for (off, size, entry_type, name) in dir_records {
        data.extend_from_slice(&off.to_le_bytes());
        data.extend_from_slice(&size.to_le_bytes());
        data.extend_from_slice(&size.to_le_bytes());
        data.push(entry_type);
        data.push(0); // compression = none
        data.extend_from_slice(&0u16.to_le_bytes());
        let mut name_buf = [0u8; 16];
        let nb = name.as_bytes();
        let n = nb.len().min(15);
        name_buf[..n].copy_from_slice(&nb[..n]);
        data.extend_from_slice(&name_buf);
    }
    data
}

/// Encode a 64x64 (or any multiple-of-16) paletted `MipTex` block with 4 mip levels + 256-color RGB palette.
pub fn encode_miptex(
    name: &str,
    width: u32,
    height: u32,
    mip0: &[u8],
    palette: &[[u8; 3]; 256],
) -> Vec<u8> {
    let w0 = width as usize;
    let h0 = height as usize;
    let w1 = (w0 / 2).max(1);
    let h1 = (h0 / 2).max(1);
    let w2 = (w0 / 4).max(1);
    let h2 = (h0 / 4).max(1);
    let w3 = (w0 / 8).max(1);
    let h3 = (h0 / 8).max(1);

    let downsample = |src: &[u8], sw: usize, dw: usize, dh: usize| -> Vec<u8> {
        let mut dst = Vec::with_capacity(dw * dh);
        for y in 0..dh {
            for x in 0..dw {
                dst.push(src[(y * 2) * sw + (x * 2)]);
            }
        }
        dst
    };
    let mip1 = downsample(mip0, w0, w1, h1);
    let mip2 = downsample(&mip1, w1, w2, h2);
    let mip3 = downsample(&mip2, w2, w3, h3);

    let off0 = 40u32;
    let off1 = off0 + (w0 * h0) as u32;
    let off2 = off1 + (w1 * h1) as u32;
    let off3 = off2 + (w2 * h2) as u32;

    let mut out = Vec::with_capacity(off3 as usize + w3 * h3 + 2 + 256 * 3 + 2);
    let mut name_buf = [0u8; 16];
    let nb = name.as_bytes();
    let n = nb.len().min(15);
    name_buf[..n].copy_from_slice(&nb[..n]);
    out.extend_from_slice(&name_buf);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&off0.to_le_bytes());
    out.extend_from_slice(&off1.to_le_bytes());
    out.extend_from_slice(&off2.to_le_bytes());
    out.extend_from_slice(&off3.to_le_bytes());
    out.extend_from_slice(mip0);
    out.extend_from_slice(&mip1);
    out.extend_from_slice(&mip2);
    out.extend_from_slice(&mip3);
    out.extend_from_slice(&256u16.to_le_bytes());
    for rgb in palette {
        out.extend_from_slice(rgb);
    }
    out.extend_from_slice(&[0u8; 2]);
    out
}

/// Deterministic spatial hash in `[0.0, 1.0)` for clean-room procedural surface grain.
#[inline]
fn hash2(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(seed.wrapping_mul(1_442_695_041));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h & 0xFFFF) as f32 / 65536.0
}

fn quantize_rgb_texture(name: &str, size: u32, rgb_pixels: &[[u8; 3]]) -> Vec<u8> {
    // 6x7x6 uniform RGB cube (252 colors) + 4 neutral accents = 256 entries, zero sorting cost.
    let mut palette = [[0u8; 3]; 256];
    let mut idx = 0usize;
    for r in 0..6u32 {
        for g in 0..7u32 {
            for b in 0..6u32 {
                palette[idx] = [
                    ((r * 255) / 5) as u8,
                    ((g * 255) / 6) as u8,
                    ((b * 255) / 5) as u8,
                ];
                idx += 1;
            }
        }
    }
    palette[252] = [32, 32, 32];
    palette[253] = [96, 96, 96];
    palette[254] = [176, 176, 176];
    palette[255] = [240, 240, 240];

    let mut indices = Vec::with_capacity(rgb_pixels.len());
    for (pi, &[r, g, b]) in rgb_pixels.iter().enumerate() {
        let x = (pi as u32) % size;
        let y = (pi as u32) / size;
        // Ordered 2x2 subtle dither so retro 8-bit paletted ramps look silky smooth!
        let d = match ((x & 1) << 1) | (y & 1) {
            0 => -10i32,
            1 => 6,
            2 => 10,
            _ => -6,
        };
        let qr = (((i32::from(r) + d).clamp(0, 255) * 5 + 127) / 255) as usize;
        let qg = (((i32::from(g) + d).clamp(0, 255) * 6 + 127) / 255) as usize;
        let qb = (((i32::from(b) + d).clamp(0, 255) * 5 + 127) / 255) as usize;
        indices.push((qr * 42 + qg * 6 + qb) as u8);
    }
    encode_miptex(name, size, size, &indices, &palette)
}

fn build_floor_miptex() -> Vec<u8> {
    const N: u32 = 64;
    let mut px = Vec::with_capacity((N * N) as usize);
    for y in 0..N {
        for x in 0..N {
            let tile_x = x % 32;
            let tile_y = y % 32;
            let seam = tile_x == 0 || tile_y == 0;
            let bevel_hi = (tile_x == 1 || tile_y == 1) && !seam;
            let bevel_lo = tile_x == 31 || tile_y == 31;
            let grain = (hash2(x, y, 11) - 0.5) * 18.0 + (hash2(x / 2, y / 2, 23) - 0.5) * 12.0;
            let base = if seam {
                [112.0, 108.0, 100.0]
            } else if bevel_hi {
                [186.0, 180.0, 168.0]
            } else if bevel_lo {
                [142.0, 136.0, 126.0]
            } else {
                [168.0, 162.0, 150.0]
            };
            px.push([
                (base[0] + grain).clamp(0.0, 255.0) as u8,
                (base[1] + grain).clamp(0.0, 255.0) as u8,
                (base[2] + grain * 0.9).clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    quantize_rgb_texture("RH_FLOOR", N, &px)
}

fn build_sandwall_miptex() -> Vec<u8> {
    const N: u32 = 64;
    let mut px = Vec::with_capacity((N * N) as usize);
    for y in 0..N {
        let row = y / 16;
        let row_y = y % 16;
        let x_off = if row % 2 == 1 { 16 } else { 0 };
        for x in 0..N {
            let bx = (x + x_off) % 32;
            let brick_id = ((x + x_off) / 32) + row * 7;
            let mortar = row_y == 0 || bx == 0;
            let top_edge = row_y == 1 && !mortar;
            let bot_edge = row_y == 15 && !mortar;
            let brick_tint = (hash2(brick_id, row, 41) - 0.5) * 24.0;
            let grain = (hash2(x, y, 57) - 0.5) * 16.0;
            let base = if mortar {
                [134.0, 120.0, 98.0]
            } else if top_edge {
                [214.0, 188.0, 146.0]
            } else if bot_edge {
                [164.0, 140.0, 104.0]
            } else {
                [
                    194.0 + brick_tint,
                    168.0 + brick_tint * 0.85,
                    126.0 + brick_tint * 0.65,
                ]
            };
            px.push([
                (base[0] + grain).clamp(0.0, 255.0) as u8,
                (base[1] + grain).clamp(0.0, 255.0) as u8,
                (base[2] + grain * 0.85).clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    quantize_rgb_texture("RH_SANDWALL", N, &px)
}

fn build_crate_miptex() -> Vec<u8> {
    const N: u32 = 64;
    let mut px = Vec::with_capacity((N * N) as usize);
    for y in 0..N {
        for x in 0..N {
            let border = !(5..N - 5).contains(&x) || !(5..N - 5).contains(&y);
            let inner_groove = x == 5 || x == N - 6 || y == 5 || y == N - 6;
            let diag1 = (x as i32 - y as i32).abs() <= 3 && !border;
            let diag2 = (x as i32 + y as i32 - (N as i32 - 1)).abs() <= 3 && !border;
            let plank_seam = (y % 12 == 0) && !border && !diag1 && !diag2;
            let rivet = (x == 3 || x == N - 4) && (y == 3 || y == N - 4 || y == N / 2);
            let grain = (hash2(x, y / 2, 79) - 0.5) * 16.0;
            let base = if rivet {
                [162.0, 166.0, 152.0]
            } else if inner_groove || plank_seam {
                [48.0, 56.0, 38.0]
            } else if border {
                [76.0, 88.0, 62.0]
            } else if diag1 || diag2 {
                [88.0, 100.0, 70.0]
            } else {
                // Classic olive-drab military timber crate
                [108.0, 118.0, 82.0]
            };
            px.push([
                (base[0] + grain).clamp(0.0, 255.0) as u8,
                (base[1] + grain).clamp(0.0, 255.0) as u8,
                (base[2] + grain * 0.8).clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    quantize_rgb_texture("RH_CRATE", N, &px)
}

fn build_rusttrim_miptex() -> Vec<u8> {
    const N: u32 = 64;
    let mut px = Vec::with_capacity((N * N) as usize);
    for y in 0..N {
        for x in 0..N {
            let seam = y == 0 || y == 32 || x == 0;
            let rivet = (y == 4 || y == 28 || y == 36 || y == 60) && (x % 16 == 4 || x % 16 == 12);
            let rust_patch = hash2(x / 4, y / 4, 103);
            let grain = (hash2(x, y, 109) - 0.5) * 20.0;
            let base = if seam {
                [46.0, 38.0, 34.0]
            } else if rivet {
                [146.0, 136.0, 124.0]
            } else if rust_patch > 0.58 {
                // Weathered burnt-rust oxide streaks
                [144.0, 82.0, 52.0]
            } else {
                // Industrial gunmetal steel plate
                [98.0, 96.0, 94.0]
            };
            px.push([
                (base[0] + grain).clamp(0.0, 255.0) as u8,
                (base[1] + grain).clamp(0.0, 255.0) as u8,
                (base[2] + grain).clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    quantize_rgb_texture("RH_RUSTTRIM", N, &px)
}

fn build_deck_miptex() -> Vec<u8> {
    const N: u32 = 64;
    let mut px = Vec::with_capacity((N * N) as usize);
    for y in 0..N {
        for x in 0..N {
            let border = x % 32 == 0 || y % 32 == 0;
            // Ribbed industrial steel deck / grip tread
            let tread = ((x + y) % 8 == 0) && ((x % 8) < 4);
            let grain = (hash2(x, y, 131) - 0.5) * 14.0;
            let base = if border {
                [72.0, 74.0, 76.0]
            } else if tread {
                [152.0, 150.0, 144.0]
            } else {
                [118.0, 120.0, 118.0]
            };
            px.push([
                (base[0] + grain).clamp(0.0, 255.0) as u8,
                (base[1] + grain).clamp(0.0, 255.0) as u8,
                (base[2] + grain).clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    quantize_rgb_texture("RH_DECK", N, &px)
}

fn build_spawn_miptex(name: &str, dark: [u8; 3], stripe: [u8; 3]) -> Vec<u8> {
    const N: u32 = 64;
    let mut px = Vec::with_capacity((N * N) as usize);
    for y in 0..N {
        for x in 0..N {
            let seam = x % 32 == 0 || y % 32 == 0;
            let hazard = !(6..N - 6).contains(&y) && ((x / 8) % 2 == 0);
            let grain = (hash2(x, y, 151) - 0.5) * 14.0;
            let base = if seam {
                [
                    dark[0] as f32 * 0.72,
                    dark[1] as f32 * 0.72,
                    dark[2] as f32 * 0.72,
                ]
            } else if hazard {
                [stripe[0] as f32, stripe[1] as f32, stripe[2] as f32]
            } else {
                [
                    138.0 + (dark[0] as f32 - 90.0) * 0.35,
                    136.0 + (dark[1] as f32 - 90.0) * 0.35,
                    132.0 + (dark[2] as f32 - 90.0) * 0.35,
                ]
            };
            px.push([
                (base[0] + grain).clamp(0.0, 255.0) as u8,
                (base[1] + grain).clamp(0.0, 255.0) as u8,
                (base[2] + grain).clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    quantize_rgb_texture(name, N, &px)
}

fn build_sky_miptex() -> Vec<u8> {
    // 16x16 placeholder miptex named `SKY`; the renderer skips `is_sky_texture` faces and renders the skybox.
    let palette = [[135, 175, 220]; 256];
    let mip0 = vec![0u8; 16 * 16];
    encode_miptex("SKY", 16, 16, &mip0, &palette)
}

fn build_radial_decal(name: &str, size: u32, tint: [u8; 3], radius: f32, seed: u32) -> Vec<u8> {
    let mut palette = [[0u8; 3]; 256];
    for (i, slot) in palette.iter_mut().enumerate() {
        let v = i as u8;
        *slot = [v, v, v];
    }
    palette[255] = tint;
    let center = (size as f32 - 1.0) * 0.5;
    let mut mip0 = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let d = (dx * dx + dy * dy).sqrt();
            let jitter = (hash2(x, y, seed) - 0.5) * 1.4;
            let falloff = (1.0 - ((d + jitter) / radius)).clamp(0.0, 1.0);
            let alpha = (falloff * falloff * 240.0) as u8;
            mip0.push(alpha);
        }
    }
    encode_miptex(name, size, size, &mip0, &palette)
}
