//! GoldSrc `HPAK` (`.hpk` custom spray archive) and custom `WAD3` decal/spray validation.
//!
//! Ported from Xash3D FWGS `engine/common/hpak.c` (GPL-3.0-or-later, read 2026-11-25).

use thiserror::Error;

use crate::wad::{WAD3_MAGIC, WadArchive};

pub const HPAK_MAGIC: [u8; 4] = *b"HPAK";
pub const HPAK_VERSION: i32 = 1;
pub const HPAK_HEADER_BYTES: usize = 12;
pub const HPAK_RESOURCE_BYTES: usize = 136;
pub const HPAK_LUMP_BYTES: usize = 144;
pub const HPAK_ENTRY_MIN_SIZE: usize = 128;
pub const HPAK_ENTRY_MAX_SIZE: usize = 1024 * 1024;
pub const HPAK_MAX_ENTRIES: usize = 0x8000;
/// Default GoldSrc spray WAD upload cap (`sv_uploadmax` = `0.5` MB / `tempdecal.wad` <= 65536 B).
pub const MAX_SPRAY_WAD_BYTES: usize = 65_536;

#[derive(Debug, Error)]
pub enum HpakError {
    #[error("invalid HPAK magic or version")]
    InvalidHeader,
    #[error("truncated or out-of-bounds HPAK data: {0}")]
    Truncated(&'static str),
    #[error("invalid HPAK lump count {0}")]
    InvalidEntryCount(i32),
    #[error("invalid HPAK lump size {0}")]
    InvalidLumpSize(usize),
    #[error("invalid spray WAD: {0}")]
    InvalidSpray(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSpray {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub byte_len: usize,
}

/// Validates a client-uploaded custom spray WAD (`tempdecal.wad` / `!MD5<hex>`):
/// must be valid WAD3 `<= max_bytes`, contain exactly 1 miptex lump, have dimensions
/// divisible by 16 (`16..=256`), and decode cleanly via [`WadArchive::decode_decal`].
pub fn validate_spray_wad(data: &[u8], max_bytes: usize) -> Result<ValidatedSpray, HpakError> {
    let limit = max_bytes.min(HPAK_ENTRY_MAX_SIZE);
    if data.len() < HPAK_ENTRY_MIN_SIZE || data.len() > limit {
        return Err(HpakError::InvalidSpray(format!(
            "spray size {} outside valid range {}..={limit}",
            data.len(),
            HPAK_ENTRY_MIN_SIZE
        )));
    }
    if &data[0..4] != WAD3_MAGIC {
        return Err(HpakError::InvalidSpray("missing WAD3 header".into()));
    }
    let wad = WadArchive::parse(data.to_vec())
        .map_err(|e| HpakError::InvalidSpray(format!("bad WAD3: {e}")))?;
    if wad.len() != 1 {
        return Err(HpakError::InvalidSpray(format!(
            "spray WAD must contain exactly 1 lump, got {}",
            wad.len()
        )));
    }
    let name = wad
        .entry_names()
        .into_iter()
        .next()
        .ok_or_else(|| HpakError::InvalidSpray("missing spray lump".into()))?
        .to_string();
    let decoded = wad
        .decode_decal(&name)
        .map_err(|e| HpakError::InvalidSpray(format!("decal decode failed: {e}")))?
        .ok_or_else(|| HpakError::InvalidSpray("lump is not a decal miptex".into()))?;
    if !(16..=256).contains(&decoded.width)
        || !(16..=256).contains(&decoded.height)
        || decoded.width % 16 != 0
        || decoded.height % 16 != 0
    {
        return Err(HpakError::InvalidSpray(format!(
            "spray dimensions {}x{} must be multiples of 16 in 16..=256",
            decoded.width, decoded.height
        )));
    }
    Ok(ValidatedSpray {
        name: decoded.name,
        width: decoded.width,
        height: decoded.height,
        byte_len: data.len(),
    })
}

/// Resource metadata stored inside each 144-byte `hpak_lump_t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HpakResource {
    pub filename: String,
    pub res_type: u8,
    pub index: i32,
    pub download_size: u32,
    pub flags: u8,
    pub md5: [u8; 16],
    pub reserved: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HpakLump {
    pub resource: HpakResource,
    pub data: Vec<u8>,
}

/// Parsed GoldSrc / Xash3D `HPAK` v1 `.hpk` archive (`custom.hpk`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HpakArchive {
    pub lumps: Vec<HpakLump>,
}

impl HpakArchive {
    pub fn parse(data: &[u8]) -> Result<Self, HpakError> {
        if data.len() < HPAK_HEADER_BYTES || data[0..4] != HPAK_MAGIC {
            return Err(HpakError::InvalidHeader);
        }
        let version = i32::from_le_bytes(data[4..8].try_into().unwrap());
        if version != HPAK_VERSION {
            return Err(HpakError::InvalidHeader);
        }
        let info_ofs = usize::try_from(i32::from_le_bytes(data[8..12].try_into().unwrap()))
            .map_err(|_| HpakError::Truncated("negative infotableofs"))?;
        if info_ofs < HPAK_HEADER_BYTES || info_ofs.saturating_add(4) > data.len() {
            return Err(HpakError::Truncated("infotableofs out of bounds"));
        }
        let count_i32 = i32::from_le_bytes(data[info_ofs..info_ofs + 4].try_into().unwrap());
        let count = usize::try_from(count_i32)
            .ok()
            .filter(|&c| (1..=HPAK_MAX_ENTRIES).contains(&c))
            .ok_or(HpakError::InvalidEntryCount(count_i32))?;
        let dir_end = info_ofs
            .checked_add(4)
            .and_then(|o| o.checked_add(count.checked_mul(HPAK_LUMP_BYTES)?))
            .ok_or(HpakError::Truncated("directory size overflow"))?;
        if dir_end > data.len() {
            return Err(HpakError::Truncated("directory table"));
        }

        let mut lumps = Vec::with_capacity(count);
        for i in 0..count {
            let off = info_ofs + 4 + i * HPAK_LUMP_BYTES;
            let res_slice = &data[off..off + HPAK_RESOURCE_BYTES];
            let nul = res_slice[0..64].iter().position(|&b| b == 0).unwrap_or(64);
            let filename = String::from_utf8_lossy(&res_slice[..nul]).into_owned();
            let res_type = i32::from_le_bytes(res_slice[64..68].try_into().unwrap()) as u8;
            let index = i32::from_le_bytes(res_slice[68..72].try_into().unwrap());
            let download_size = u32::from_le_bytes(res_slice[72..76].try_into().unwrap());
            let flags = res_slice[76];
            let mut md5 = [0u8; 16];
            md5.copy_from_slice(&res_slice[77..93]);
            // res_slice[93..104] holds cFlags/rguc_reserved alignment + playernum + pNext/pPrev compat
            // fields: `dresource_t` keeps `rguc_reserved[32]` at offset 104..136.
            let mut reserved = [0u8; 32];
            reserved.copy_from_slice(&res_slice[104..136]);

            let filepos = usize::try_from(i32::from_le_bytes(
                data[off + 136..off + 140].try_into().unwrap(),
            ))
            .map_err(|_| HpakError::Truncated("negative lump filepos"))?;
            let disksize = usize::try_from(i32::from_le_bytes(
                data[off + 140..off + 144].try_into().unwrap(),
            ))
            .map_err(|_| HpakError::Truncated("negative lump disksize"))?;
            if !(HPAK_ENTRY_MIN_SIZE..=HPAK_ENTRY_MAX_SIZE).contains(&disksize) {
                return Err(HpakError::InvalidLumpSize(disksize));
            }
            let lump_end = filepos
                .checked_add(disksize)
                .ok_or(HpakError::Truncated("lump end overflow"))?;
            if filepos < HPAK_HEADER_BYTES || lump_end > data.len() {
                return Err(HpakError::Truncated("lump payload"));
            }
            lumps.push(HpakLump {
                resource: HpakResource {
                    filename,
                    res_type,
                    index,
                    download_size,
                    flags,
                    md5,
                    reserved,
                },
                data: data[filepos..lump_end].to_vec(),
            });
        }
        Ok(Self { lumps })
    }

    pub fn insert(&mut self, mut resource: HpakResource, payload: Vec<u8>) -> Result<(), HpakError> {
        if !(HPAK_ENTRY_MIN_SIZE..=HPAK_ENTRY_MAX_SIZE).contains(&payload.len()) {
            return Err(HpakError::InvalidLumpSize(payload.len()));
        }
        resource.download_size = payload.len() as u32;
        if let Some(existing) = self
            .lumps
            .iter_mut()
            .find(|l| l.resource.md5 == resource.md5)
        {
            existing.resource = resource;
            existing.data = payload;
        } else {
            self.lumps.push(HpakLump {
                resource,
                data: payload,
            });
        }
        Ok(())
    }

    pub fn find_by_md5(&self, md5: &[u8; 16]) -> Option<&HpakLump> {
        self.lumps.iter().find(|l| &l.resource.md5 == md5)
    }

    /// Verifies each lump's MD5 using a caller-supplied digest function.
    pub fn verify_checksums(&self, md5_fn: impl Fn(&[u8]) -> [u8; 16]) -> bool {
        self.lumps
            .iter()
            .all(|l| md5_fn(&l.data) == l.resource.md5)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, HpakError> {
        if self.lumps.is_empty() || self.lumps.len() > HPAK_MAX_ENTRIES {
            return Err(HpakError::InvalidEntryCount(self.lumps.len() as i32));
        }
        let payload_len: usize = self.lumps.iter().map(|l| l.data.len()).sum();
        let dir_offset = HPAK_HEADER_BYTES + payload_len;
        let total = dir_offset + 4 + self.lumps.len() * HPAK_LUMP_BYTES;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&HPAK_MAGIC);
        out.extend_from_slice(&HPAK_VERSION.to_le_bytes());
        out.extend_from_slice(&(dir_offset as i32).to_le_bytes());

        let mut offsets = Vec::with_capacity(self.lumps.len());
        for lump in &self.lumps {
            if !(HPAK_ENTRY_MIN_SIZE..=HPAK_ENTRY_MAX_SIZE).contains(&lump.data.len()) {
                return Err(HpakError::InvalidLumpSize(lump.data.len()));
            }
            offsets.push(out.len());
            out.extend_from_slice(&lump.data);
        }

        out.extend_from_slice(&(self.lumps.len() as i32).to_le_bytes());
        for (idx, lump) in self.lumps.iter().enumerate() {
            let mut rec = [0u8; HPAK_LUMP_BYTES];
            let name_bytes = lump.resource.filename.as_bytes();
            let n = name_bytes.len().min(63);
            rec[..n].copy_from_slice(&name_bytes[..n]);
            rec[64..68].copy_from_slice(&i32::from(lump.resource.res_type).to_le_bytes());
            rec[68..72].copy_from_slice(&lump.resource.index.to_le_bytes());
            rec[72..76].copy_from_slice(&lump.resource.download_size.to_le_bytes());
            rec[76] = lump.resource.flags;
            rec[77..93].copy_from_slice(&lump.resource.md5);
            rec[104..136].copy_from_slice(&lump.resource.reserved);
            rec[136..140].copy_from_slice(&(offsets[idx] as i32).to_le_bytes());
            rec[140..144].copy_from_slice(&(lump.data.len() as i32).to_le_bytes());
            out.extend_from_slice(&rec);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wad::DECAL_MIPTEX_TYPE;

    fn build_synthetic_spray_wad(width: u32, height: u32) -> Vec<u8> {
        let mip0 = (width * height) as usize;
        let mip1 = ((width / 2) * (height / 2)) as usize;
        let mip2 = ((width / 4) * (height / 4)) as usize;
        let mip3 = ((width / 8) * (height / 8)) as usize;
        let miptex_len = 40 + mip0 + mip1 + mip2 + mip3 + 2 + 256 * 3;
        let mut miptex = vec![0u8; miptex_len];
        miptex[0..6].copy_from_slice(b"{logo\0");
        miptex[16..20].copy_from_slice(&width.to_le_bytes());
        miptex[20..24].copy_from_slice(&height.to_le_bytes());
        let off0 = 40u32;
        let off1 = off0 + mip0 as u32;
        let off2 = off1 + mip1 as u32;
        let off3 = off2 + mip2 as u32;
        miptex[24..28].copy_from_slice(&off0.to_le_bytes());
        miptex[28..32].copy_from_slice(&off1.to_le_bytes());
        miptex[32..36].copy_from_slice(&off2.to_le_bytes());
        miptex[36..40].copy_from_slice(&off3.to_le_bytes());
        let pal_hdr = off3 as usize + mip3;
        miptex[pal_hdr..pal_hdr + 2].copy_from_slice(&256u16.to_le_bytes());
        // Blue-keyed last palette entry [0, 0, 255].
        let last_rgb = pal_hdr + 2 + 255 * 3;
        miptex[last_rgb..last_rgb + 3].copy_from_slice(&[0, 0, 255]);

        let dir_off = 12 + miptex.len();
        let mut wad = Vec::with_capacity(dir_off + 32);
        wad.extend_from_slice(WAD3_MAGIC);
        wad.extend_from_slice(&1u32.to_le_bytes());
        wad.extend_from_slice(&(dir_off as u32).to_le_bytes());
        wad.extend_from_slice(&miptex);
        let mut dir = [0u8; 32];
        dir[0..4].copy_from_slice(&12u32.to_le_bytes());
        dir[4..8].copy_from_slice(&(miptex.len() as u32).to_le_bytes());
        dir[8..12].copy_from_slice(&(miptex.len() as u32).to_le_bytes());
        dir[12] = DECAL_MIPTEX_TYPE;
        dir[16..22].copy_from_slice(b"{logo\0");
        wad.extend_from_slice(&dir);
        wad
    }

    #[test]
    fn spray_wad_validation_accepts_valid_and_rejects_bad() {
        let good = build_synthetic_spray_wad(64, 64);
        let info = validate_spray_wad(&good, MAX_SPRAY_WAD_BYTES).unwrap();
        assert_eq!(info.name, "{logo");
        assert_eq!((info.width, info.height), (64, 64));

        let bad_dim = build_synthetic_spray_wad(24, 24);
        assert!(validate_spray_wad(&bad_dim, MAX_SPRAY_WAD_BYTES).is_err());
        assert!(validate_spray_wad(b"not a wad", MAX_SPRAY_WAD_BYTES).is_err());
        assert!(validate_spray_wad(&good, 500).is_err());
    }

    #[test]
    fn hpak_roundtrips_and_rejects_corrupt_headers() {
        let spray = build_synthetic_spray_wad(32, 32);
        let md5 = [0x42u8; 16];
        let mut pak = HpakArchive::default();
        pak.insert(
            HpakResource {
                filename: "!MD5424242".into(),
                res_type: 3,
                index: 0,
                download_size: spray.len() as u32,
                flags: 4,
                md5,
                reserved: [0; 32],
            },
            spray.clone(),
        )
        .unwrap();

        let encoded = pak.to_bytes().unwrap();
        let decoded = HpakArchive::parse(&encoded).unwrap();
        assert_eq!(decoded.lumps.len(), 1);
        assert_eq!(decoded.find_by_md5(&md5).unwrap().data, spray);
        assert!(decoded.verify_checksums(|_| md5));
        assert!(!decoded.verify_checksums(|_| [0; 16]));

        assert!(HpakArchive::parse(b"HPAK").is_err());
    }
}
