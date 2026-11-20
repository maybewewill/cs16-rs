//! GoldSrc WAD3 archive and MipTex texture decoder.

use std::collections::HashMap;

use thiserror::Error;

pub const WAD3_MAGIC: &[u8; 4] = b"WAD3";
pub const MIPTEX_TYPE: u8 = 0x43;
pub const DECAL_MIPTEX_TYPE: u8 = 0x40;

#[derive(Debug, Error)]
pub enum WadError {
    #[error("invalid WAD3 header or magic")]
    InvalidHeader,
    #[error("truncated WAD3 data: {0}")]
    Truncated(&'static str),
    #[error("invalid MipTex dimensions ({width}x{height}) for texture {name}")]
    InvalidDimensions {
        name: String,
        width: u32,
        height: u32,
    },
}

/// Decoded RGBA8 texture with metadata derived from GoldSrc naming conventions.
#[derive(Debug, Clone)]
pub struct DecodedTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub alpha_test: bool,
}

#[derive(Debug, Clone)]
pub struct WadEntry {
    pub name: String,
    pub offset: usize,
    pub size: usize,
    pub entry_type: u8,
}

/// Parsed WAD3 archive keeping raw bytes in memory for fast on-demand texture decoding.
#[derive(Debug, Clone)]
pub struct WadArchive {
    data: Vec<u8>,
    entries: HashMap<String, WadEntry>,
}

impl WadArchive {
    pub fn parse(data: Vec<u8>) -> Result<Self, WadError> {
        if data.len() < 12 || &data[0..4] != WAD3_MAGIC {
            return Err(WadError::InvalidHeader);
        }
        let num_entries = read_u32_le(&data, 4)? as usize;
        let dir_offset = read_u32_le(&data, 8)? as usize;

        let dir_bytes = num_entries
            .checked_mul(32)
            .ok_or(WadError::Truncated("directory size overflow"))?;
        if dir_offset.saturating_add(dir_bytes) > data.len() {
            return Err(WadError::Truncated("directory table"));
        }

        let mut entries = HashMap::with_capacity(num_entries);
        for i in 0..num_entries {
            let base = dir_offset + i * 32;
            let offset = read_u32_le(&data, base)? as usize;
            let disk_size = read_u32_le(&data, base + 4)? as usize;
            let _size = read_u32_le(&data, base + 8)? as usize;
            let entry_type = data[base + 12];
            let name = read_cstr(&data[base + 16..base + 32]);
            let key = name.to_ascii_lowercase();
            entries.insert(
                key,
                WadEntry {
                    name,
                    offset,
                    size: disk_size,
                    entry_type,
                },
            );
        }

        Ok(Self { data, entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(&name.to_ascii_lowercase())
    }

    pub fn decode_texture(&self, name: &str) -> Result<Option<DecodedTexture>, WadError> {
        let Some(entry) = self.entries.get(&name.to_ascii_lowercase()) else {
            return Ok(None);
        };
        if entry.entry_type != MIPTEX_TYPE && entry.entry_type != DECAL_MIPTEX_TYPE {
            return Ok(None);
        }
        let end = entry.offset.saturating_add(entry.size);
        if end > self.data.len() {
            return Err(WadError::Truncated("miptex entry slice"));
        }
        let tex = decode_miptex(&self.data[entry.offset..end])?;
        Ok(Some(tex))
    }
}

/// Decode a GoldSrc `MipTex` block (either from a WAD3 entry or embedded inside BSP `LUMP_TEXTURES`).
pub fn decode_miptex(slice: &[u8]) -> Result<DecodedTexture, WadError> {
    if slice.len() < 40 {
        return Err(WadError::Truncated("miptex header"));
    }
    let name = read_cstr(&slice[0..16]);
    let width = read_u32_le(slice, 16)?;
    let height = read_u32_le(slice, 20)?;
    let off0 = read_u32_le(slice, 24)? as usize;
    let off3 = read_u32_le(slice, 36)? as usize;

    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(WadError::InvalidDimensions {
            name,
            width,
            height,
        });
    }
    if off0 == 0 {
        return Err(WadError::Truncated("external miptex has offset 0"));
    }

    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(WadError::Truncated("pixel count overflow"))?;
    if off0.saturating_add(pixel_count) > slice.len() {
        return Err(WadError::Truncated("mip0 pixel indices"));
    }

    let mip3_w = (width as usize / 8).max(1);
    let mip3_h = (height as usize / 8).max(1);
    let mip3_size = mip3_w * mip3_h;
    let pal_header_off = off3.saturating_add(mip3_size);
    if pal_header_off + 2 > slice.len() {
        return Err(WadError::Truncated("palette size"));
    }
    let palette_colors = u16::from_le_bytes([slice[pal_header_off], slice[pal_header_off + 1]]) as usize;
    let pal_off = pal_header_off + 2;
    let pal_bytes = palette_colors * 3;
    if pal_off.saturating_add(pal_bytes) > slice.len() {
        return Err(WadError::Truncated("palette rgb table"));
    }
    let palette = &slice[pal_off..pal_off + pal_bytes];

    let alpha_test = name.starts_with('{');
    let indices = &slice[off0..off0 + pixel_count];
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for &idx in indices {
        if alpha_test && idx == 255 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let ci = (idx as usize).min(palette_colors.saturating_sub(1));
            let base = ci * 3;
            rgba.extend_from_slice(&[palette[base], palette[base + 1], palette[base + 2], 255]);
        }
    }

    Ok(DecodedTexture {
        name,
        width,
        height,
        rgba,
        alpha_test,
    })
}

/// Generate a magenta/black checkerboard fallback texture when an external WAD is missing.
pub fn fallback_checkerboard(name: &str) -> DecodedTexture {
    let size = 64u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dark = ((x / 8) + (y / 8)) % 2 == 0;
            if dark {
                rgba.extend_from_slice(&[32, 32, 36, 255]);
            } else {
                rgba.extend_from_slice(&[196, 181, 80, 255]);
            }
        }
    }
    DecodedTexture {
        name: name.to_string(),
        width: size,
        height: size,
        rgba,
        alpha_test: false,
    }
}

pub(crate) fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, WadError> {
    let arr: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(WadError::Truncated("u32"))?
        .try_into()
        .map_err(|_| WadError::Truncated("u32"))?;
    Ok(u32::from_le_bytes(arr))
}

pub(crate) fn read_cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
