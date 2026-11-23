//! GoldSrc Sprite v2 (`IDSP`) decoder for HUD sprites (`640hud*.spr`, `radar640.spr`)
//! and world sprites (`muzzleflash*.spr`, `smokepuff.spr`).

use cs_vfs::Vfs;
use thiserror::Error;

pub const SPR_MAGIC: &[u8; 4] = b"IDSP";
pub const SPR_VERSION: i32 = 2;

/// `IDSP` header `type` field, values 0..=4 as stored on disk.
///
/// Xash3D FWGS names the same values `SPR_FWD_PARALLEL_UPRIGHT`, `SPR_FACING_UPRIGHT`,
/// `SPR_FWD_PARALLEL`, `SPR_ORIENTED`, `SPR_FWD_PARALLEL_ORIENTED`
/// (`ref/gl/gl_sprite.c`, `R_DrawSpriteModel`, read 2026-11-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteType {
    ParallelUpright = 0,
    FacingUpright = 1,
    Parallel = 2,
    Oriented = 3,
    ParallelOriented = 4,
}

impl SpriteType {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            0 => Self::ParallelUpright,
            1 => Self::FacingUpright,
            3 => Self::Oriented,
            4 => Self::ParallelOriented,
            _ => Self::Parallel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteTexFormat {
    Normal = 0,
    Additive = 1,
    IndexAlpha = 2,
    AlphaTest = 3,
}

#[derive(Debug, Error)]
pub enum SprError {
    #[error("invalid SPR header or version (expected IDSP v2)")]
    InvalidHeader,
    #[error("truncated SPR data: {0}")]
    Truncated(&'static str),
    #[error("VFS error: {0}")]
    Vfs(#[from] cs_vfs::VfsError),
}

#[derive(Debug, Clone)]
pub struct SpriteFrame {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct GoldSrcSprite {
    pub sprite_type: SpriteType,
    pub tex_format: SpriteTexFormat,
    pub max_width: u32,
    pub max_height: u32,
    pub frames: Vec<SpriteFrame>,
}

impl GoldSrcSprite {
    pub fn load(vfs: &Vfs, rel_path: &str) -> Result<Self, SprError> {
        let bytes = vfs.read_bytes(rel_path)?;
        Self::parse(&bytes)
    }

    pub fn parse(data: &[u8]) -> Result<Self, SprError> {
        if data.len() < 42 || &data[0..4] != SPR_MAGIC {
            return Err(SprError::InvalidHeader);
        }
        let version = read_i32(data, 4)?;
        if version != SPR_VERSION {
            return Err(SprError::InvalidHeader);
        }
        let sprite_type = SpriteType::from_raw(read_i32(data, 8)?);
        let tex_fmt_raw = read_i32(data, 12)?;
        let tex_format = match tex_fmt_raw {
            1 => SpriteTexFormat::Additive,
            2 => SpriteTexFormat::IndexAlpha,
            3 => SpriteTexFormat::AlphaTest,
            _ => SpriteTexFormat::Normal,
        };
        let max_width = read_i32(data, 20)?.max(1) as u32;
        let max_height = read_i32(data, 24)?.max(1) as u32;
        let num_frames = read_i32(data, 28)?.max(0) as usize;

        let pal_colors = u16::from_le_bytes([data[40], data[41]]) as usize;
        let pal_start = 42;
        let pal_end = pal_start + pal_colors * 3;
        if pal_end > data.len() {
            return Err(SprError::Truncated("palette"));
        }
        let palette = &data[pal_start..pal_end];

        let mut offset = pal_end;
        let mut frames = Vec::with_capacity(num_frames);

        for _ in 0..num_frames {
            if offset + 4 > data.len() {
                break;
            }
            let group = read_i32(data, offset)?;
            offset += 4;
            if group == 0 {
                let (frame, next) = read_single_frame(data, offset, palette, pal_colors, tex_format)?;
                frames.push(frame);
                offset = next;
            } else {
                let count = read_i32(data, offset)?.max(0) as usize;
                offset += 4 + count * 4; // skip interval floats
                for _ in 0..count {
                    let (frame, next) =
                        read_single_frame(data, offset, palette, pal_colors, tex_format)?;
                    frames.push(frame);
                    offset = next;
                }
            }
        }

        Ok(Self {
            sprite_type,
            tex_format,
            max_width,
            max_height,
            frames,
        })
    }
}

fn read_single_frame(
    data: &[u8],
    offset: usize,
    palette: &[u8],
    pal_colors: usize,
    tex_format: SpriteTexFormat,
) -> Result<(SpriteFrame, usize), SprError> {
    if offset + 16 > data.len() {
        return Err(SprError::Truncated("frame header"));
    }
    let origin_x = read_i32(data, offset)?;
    let origin_y = read_i32(data, offset + 4)?;
    let width = read_i32(data, offset + 8)?.max(1) as u32;
    let height = read_i32(data, offset + 12)?.max(1) as u32;
    let pix_start = offset + 16;
    let pix_count = (width as usize) * (height as usize);
    let pix_end = pix_start + pix_count;
    if pix_end > data.len() {
        return Err(SprError::Truncated("frame pixels"));
    }

    let mut rgba = Vec::with_capacity(pix_count * 4);
    let last_color_off = pal_colors.saturating_sub(1) * 3;
    for &idx in &data[pix_start..pix_end] {
        let ci = (idx as usize).min(pal_colors.saturating_sub(1));
        let p = ci * 3;
        let (r, g, b) = (palette[p], palette[p + 1], palette[p + 2]);
        match tex_format {
            SpriteTexFormat::Additive => {
                // Luminance becomes alpha so additive sprites compose cleanly in 2D UI & 3D
                let a = r.max(g).max(b);
                rgba.extend_from_slice(&[r, g, b, a]);
            }
            SpriteTexFormat::IndexAlpha => {
                let br = palette[last_color_off];
                let bg = palette[last_color_off + 1];
                let bb = palette[last_color_off + 2];
                rgba.extend_from_slice(&[br, bg, bb, idx]);
            }
            SpriteTexFormat::AlphaTest => {
                if idx == 255 {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    rgba.extend_from_slice(&[r, g, b, 255]);
                }
            }
            SpriteTexFormat::Normal => {
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }

    Ok((
        SpriteFrame {
            origin_x,
            origin_y,
            width,
            height,
            rgba,
        },
        pix_end,
    ))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, SprError> {
    let arr: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(SprError::Truncated("i32"))?
        .try_into()
        .map_err(|_| SprError::Truncated("i32"))?;
    Ok(i32::from_le_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 1x1 frame, 2-colour palette, with the given header `type` and `texFormat`.
    fn tiny_sprite(sprite_type: i32, tex_format: i32) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(SPR_MAGIC);
        for v in [SPR_VERSION, sprite_type, tex_format, 0, 1, 1, 1, 0, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&[10, 20, 30, 40, 50, 60]);
        d.extend_from_slice(&0i32.to_le_bytes()); // single frame
        for v in [-1i32, 1, 1, 1] {
            d.extend_from_slice(&v.to_le_bytes()); // origin x/y, width, height
        }
        d.push(0);
        d
    }

    #[test]
    fn header_type_is_parsed() {
        for (raw, want) in [
            (0, SpriteType::ParallelUpright),
            (1, SpriteType::FacingUpright),
            (2, SpriteType::Parallel),
            (3, SpriteType::Oriented),
            (4, SpriteType::ParallelOriented),
            (9, SpriteType::Parallel),
        ] {
            let spr = GoldSrcSprite::parse(&tiny_sprite(raw, 0)).expect("parse");
            assert_eq!(spr.sprite_type, want, "raw type {raw}");
            assert_eq!(spr.frames.len(), 1);
            assert_eq!((spr.frames[0].origin_x, spr.frames[0].origin_y), (-1, 1));
        }
    }
}
