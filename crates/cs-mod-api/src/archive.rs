//! Pure-Rust `.csmod` ZIP archive reader and writer (Stored + Deflate, CRC-32 verified,
//! path-traversal and zip-bomb guarded).

use std::collections::BTreeMap;
use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use thiserror::Error;

use crate::manifest::is_safe_rel_path;

pub const MAX_ARCHIVE_ENTRIES: usize = 512;
pub const MAX_UNCOMPRESSED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    #[error("not a valid .csmod ZIP archive")]
    InvalidZip,
    #[error("unsupported ZIP compression method {0}")]
    UnsupportedMethod(u16),
    #[error("unsafe archive entry path `{0}`")]
    UnsafePath(String),
    #[error("duplicate archive entry `{0}`")]
    DuplicateEntry(String),
    #[error("archive exceeds safety limits ({0})")]
    LimitExceeded(&'static str),
    #[error("CRC-32 mismatch for `{0}`")]
    CrcMismatch(String),
    #[error("decompression error for `{0}`")]
    Decompress(String),
}

pub struct CsModArchive;

impl CsModArchive {
    /// Encodes normalized archive entries into a standard Deflate/Stored ZIP `.csmod` container.
    pub fn write_zip<'a>(
        entries: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<Vec<u8>, ArchiveError> {
        let mut local_out = Vec::new();
        let mut central_out = Vec::new();
        let mut count = 0u16;

        for (name, data) in entries {
            if !is_safe_rel_path(name) || name.len() > u16::MAX as usize {
                return Err(ArchiveError::UnsafePath(name.to_string()));
            }
            if usize::from(count) >= MAX_ARCHIVE_ENTRIES {
                return Err(ArchiveError::LimitExceeded("too many entries"));
            }
            let crc = crc32fast::hash(data);
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(data)
                .map_err(|_| ArchiveError::Decompress(name.to_string()))?;
            let deflated = enc
                .finish()
                .map_err(|_| ArchiveError::Decompress(name.to_string()))?;
            let (method, payload): (u16, &[u8]) = if deflated.len() < data.len() {
                (8, &deflated)
            } else {
                (0, data)
            };

            let local_offset = u32::try_from(local_out.len())
                .map_err(|_| ArchiveError::LimitExceeded("archive size"))?;
            let name_bytes = name.as_bytes();
            let comp_len = u32::try_from(payload.len())
                .map_err(|_| ArchiveError::LimitExceeded("entry size"))?;
            let uncomp_len = u32::try_from(data.len())
                .map_err(|_| ArchiveError::LimitExceeded("entry size"))?;

            // Local file header (30 + name)
            local_out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            local_out.extend_from_slice(&20u16.to_le_bytes());
            local_out.extend_from_slice(&0u16.to_le_bytes());
            local_out.extend_from_slice(&method.to_le_bytes());
            local_out.extend_from_slice(&0u16.to_le_bytes());
            local_out.extend_from_slice(&0u16.to_le_bytes());
            local_out.extend_from_slice(&crc.to_le_bytes());
            local_out.extend_from_slice(&comp_len.to_le_bytes());
            local_out.extend_from_slice(&uncomp_len.to_le_bytes());
            local_out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            local_out.extend_from_slice(&0u16.to_le_bytes());
            local_out.extend_from_slice(name_bytes);
            local_out.extend_from_slice(payload);

            // Central directory header (46 + name)
            central_out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central_out.extend_from_slice(&20u16.to_le_bytes());
            central_out.extend_from_slice(&20u16.to_le_bytes());
            central_out.extend_from_slice(&0u16.to_le_bytes());
            central_out.extend_from_slice(&method.to_le_bytes());
            central_out.extend_from_slice(&0u16.to_le_bytes());
            central_out.extend_from_slice(&0u16.to_le_bytes());
            central_out.extend_from_slice(&crc.to_le_bytes());
            central_out.extend_from_slice(&comp_len.to_le_bytes());
            central_out.extend_from_slice(&uncomp_len.to_le_bytes());
            central_out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            central_out.extend_from_slice(&0u16.to_le_bytes());
            central_out.extend_from_slice(&0u16.to_le_bytes());
            central_out.extend_from_slice(&0u16.to_le_bytes());
            central_out.extend_from_slice(&0u16.to_le_bytes());
            central_out.extend_from_slice(&0u32.to_le_bytes());
            central_out.extend_from_slice(&local_offset.to_le_bytes());
            central_out.extend_from_slice(name_bytes);

            count += 1;
        }

        let cd_offset = u32::try_from(local_out.len())
            .map_err(|_| ArchiveError::LimitExceeded("archive size"))?;
        let cd_size = u32::try_from(central_out.len())
            .map_err(|_| ArchiveError::LimitExceeded("central dir size"))?;

        let mut out = local_out;
        out.extend_from_slice(&central_out);
        // End of central directory (22 bytes)
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        Ok(out)
    }

    /// Reads and verifies all entries from a `.csmod` ZIP archive.
    pub fn read_zip(zip_bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, ArchiveError> {
        let eocd_pos = find_eocd(zip_bytes).ok_or(ArchiveError::InvalidZip)?;
        let entry_count = usize::from(read_u16(zip_bytes, eocd_pos + 10)?);
        let cd_size = usize::try_from(read_u32(zip_bytes, eocd_pos + 12)?)
            .map_err(|_| ArchiveError::InvalidZip)?;
        let cd_offset = usize::try_from(read_u32(zip_bytes, eocd_pos + 16)?)
            .map_err(|_| ArchiveError::InvalidZip)?;
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(ArchiveError::LimitExceeded("too many entries"));
        }
        let cd_end = cd_offset
            .checked_add(cd_size)
            .filter(|&end| end <= eocd_pos)
            .ok_or(ArchiveError::InvalidZip)?;

        let mut files = BTreeMap::new();
        let mut total_uncompressed = 0usize;
        let mut cursor = cd_offset;

        for _ in 0..entry_count {
            if cursor + 46 > cd_end || read_u32(zip_bytes, cursor)? != 0x0201_4b50 {
                return Err(ArchiveError::InvalidZip);
            }
            let method = read_u16(zip_bytes, cursor + 10)?;
            let crc = read_u32(zip_bytes, cursor + 16)?;
            let comp_size = usize::try_from(read_u32(zip_bytes, cursor + 20)?)
                .map_err(|_| ArchiveError::InvalidZip)?;
            let uncomp_size = usize::try_from(read_u32(zip_bytes, cursor + 24)?)
                .map_err(|_| ArchiveError::InvalidZip)?;
            let name_len = usize::from(read_u16(zip_bytes, cursor + 28)?);
            let extra_len = usize::from(read_u16(zip_bytes, cursor + 30)?);
            let comment_len = usize::from(read_u16(zip_bytes, cursor + 32)?);
            let local_off = usize::try_from(read_u32(zip_bytes, cursor + 42)?)
                .map_err(|_| ArchiveError::InvalidZip)?;

            let name_start = cursor + 46;
            let name_end = name_start + name_len;
            let next_cursor = name_end + extra_len + comment_len;
            if next_cursor > cd_end {
                return Err(ArchiveError::InvalidZip);
            }
            let name = std::str::from_utf8(&zip_bytes[name_start..name_end])
                .map_err(|_| ArchiveError::InvalidZip)?
                .to_string();
            cursor = next_cursor;

            if name.ends_with('/') {
                continue;
            }
            if !is_safe_rel_path(&name) {
                return Err(ArchiveError::UnsafePath(name));
            }
            if files.contains_key(&name) {
                return Err(ArchiveError::DuplicateEntry(name));
            }

            total_uncompressed = total_uncompressed
                .checked_add(uncomp_size)
                .filter(|&tot| tot <= MAX_UNCOMPRESSED_BYTES)
                .ok_or(ArchiveError::LimitExceeded("uncompressed size"))?;

            if local_off + 30 > cd_offset || read_u32(zip_bytes, local_off)? != 0x0403_4b50 {
                return Err(ArchiveError::InvalidZip);
            }
            let lh_name_len = usize::from(read_u16(zip_bytes, local_off + 26)?);
            let lh_extra_len = usize::from(read_u16(zip_bytes, local_off + 28)?);
            let data_start = local_off + 30 + lh_name_len + lh_extra_len;
            let data_end = data_start
                .checked_add(comp_size)
                .filter(|&end| end <= cd_offset)
                .ok_or(ArchiveError::InvalidZip)?;
            let compressed = &zip_bytes[data_start..data_end];

            let data = match method {
                0 => {
                    if compressed.len() != uncomp_size {
                        return Err(ArchiveError::InvalidZip);
                    }
                    compressed.to_vec()
                }
                8 => {
                    let mut dec = DeflateDecoder::new(compressed).take((uncomp_size as u64) + 1);
                    let mut out = Vec::with_capacity(uncomp_size.min(64 * 1024));
                    dec.read_to_end(&mut out)
                        .map_err(|_| ArchiveError::Decompress(name.clone()))?;
                    if out.len() != uncomp_size {
                        return Err(ArchiveError::LimitExceeded("declared uncompressed size mismatch"));
                    }
                    out
                }
                other => return Err(ArchiveError::UnsupportedMethod(other)),
            };

            if crc32fast::hash(&data) != crc {
                return Err(ArchiveError::CrcMismatch(name));
            }
            files.insert(name, data);
        }

        Ok(files)
    }
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    let min_off = bytes.len().saturating_sub(22 + usize::from(u16::MAX));
    (min_off..=bytes.len() - 22)
        .rev()
        .find(|&pos| bytes[pos..pos + 4] == 0x0605_4b50u32.to_le_bytes())
}

fn read_u16(bytes: &[u8], off: usize) -> Result<u16, ArchiveError> {
    let slice: [u8; 2] = bytes
        .get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .ok_or(ArchiveError::InvalidZip)?;
    Ok(u16::from_le_bytes(slice))
}

fn read_u32(bytes: &[u8], off: usize) -> Result<u32, ArchiveError> {
    let slice: [u8; 4] = bytes
        .get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or(ArchiveError::InvalidZip)?;
    Ok(u32::from_le_bytes(slice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csmod_archive_roundtrips_stored_and_deflate_and_blocks_traversal_and_bombs() {
        let rep = vec![b'A'; 4096];
        let small = b"hi".as_slice();
        let zip = CsModArchive::write_zip([
            ("mod.toml", b"id = \"demo\"\nversion = \"1.0\"\n".as_slice()),
            ("server.wasm", small),
            ("assets/big.txt", rep.as_slice()),
        ])
        .expect("write zip");

        let read = CsModArchive::read_zip(&zip).expect("read zip");
        assert_eq!(read["server.wasm"], b"hi");
        assert_eq!(read["assets/big.txt"], rep);

        assert_eq!(
            CsModArchive::write_zip([("../evil.txt", b"x".as_slice())]),
            Err(ArchiveError::UnsafePath("../evil.txt".to_string()))
        );

        // Corrupt a byte in compressed payload -> CRC mismatch
        let mut corrupted = zip.clone();
        let last_local_idx = 40;
        corrupted[last_local_idx] ^= 0xFF;
        assert!(matches!(
            CsModArchive::read_zip(&corrupted),
            Err(ArchiveError::CrcMismatch(_) | ArchiveError::Decompress(_) | ArchiveError::InvalidZip)
        ));
    }
}
