//! `.amxx` container and Pawn 3.2 (`AMX_HEADER`) binary parser & test builder.
//!
//! Ported from AMX Mod X GPL-3 sources:
//! - `amxx_amxxfile.h` (`MAGIC_HEADER`, `MAGIC_HEADER2`, `MAGIC_VERSION`, `PluginEntry`)
//! - `amxx_amxx_loader.cpp` (`CAmxxReader`)
//! - `amxx_amx.h` (`AMX_HEADER`, `AMX_MAGIC`, `AMX_FLAG_*`)
//! - `amxx_amx.cpp` (`amx_Init`, `expand`, `amx_GetPublic`, `amx_GetNative`, `amx_GetPubVar`)

use std::io::Read;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use thiserror::Error;

/// Legacy AMXX v1 container magic (`'AMXB'` in little-endian: `0x414D5842`).
pub const MAGIC_HEADER: u32 = 0x414D_5842;
/// Standard AMXX v2/v3 container magic (`'AMXX'` in little-endian: `0x414D5858`).
pub const MAGIC_HEADER2: u32 = 0x414D_5858;
/// Supported AMXX container format version (`0x0300`).
pub const MAGIC_VERSION: u16 = 0x0300;
/// Pawn AMX header magic (`0xF1E0`).
pub const AMX_MAGIC: u16 = 0xF1E0;

/// Minimum supported AMX file version.
pub const MIN_FILE_VERSION: u8 = 6;
/// Current supported AMX file/engine version.
pub const CUR_FILE_VERSION: u8 = 8;
/// Maximum symbol name length (`sNAMEMAX` in `amx.h`).
pub const MAX_SYMBOL_NAME: usize = 63;
/// Size of `AMX_HEADER` in bytes.
pub const AMX_HEADER_SIZE: usize = 56;
/// Maximum decompressed plugin image size (16 MiB) to guard against zlib bombs.
pub const MAX_DECOMPRESSED_SIZE: usize = 16 * 1024 * 1024;

/// Debug symbolic info available (`AMX_FLAG_DEBUG`).
pub const AMX_FLAG_DEBUG: u16 = 0x02;
/// Compact encoding (`AMX_FLAG_COMPACT`).
pub const AMX_FLAG_COMPACT: u16 = 0x04;
/// Byte opcodes (`AMX_FLAG_BYTEOPC`).
pub const AMX_FLAG_BYTEOPC: u16 = 0x08;
/// No bounds checks (`AMX_FLAG_NOCHECKS`).
pub const AMX_FLAG_NOCHECKS: u16 = 0x10;

/// Errors produced when loading `.amxx` binaries or executing Pawn bytecode.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AmxError {
    #[error("forced exit with code {0}")]
    Exit(i32),
    #[error("assertion failed")]
    Assert,
    #[error("stack/heap collision")]
    StackErr,
    #[error("index out of bounds")]
    Bounds,
    #[error("invalid memory access")]
    MemoryAccess,
    #[error("invalid instruction opcode {0}")]
    InvalidInstruction(u32),
    #[error("stack underflow")]
    StackLow,
    #[error("heap underflow")]
    HeapLow,
    #[error("invalid native callback")]
    Callback,
    #[error("native error: {0}")]
    Native(String),
    #[error("divide by zero")]
    DivideByZero,
    #[error("vm sleep")]
    Sleep,
    #[error("invalid vm state")]
    InvalidState,
    #[error("invalid native: {0}")]
    InvalidNative(String),
    #[error("out of vm memory")]
    OutOfMemory,
    #[error("invalid amx/amxx format: {0}")]
    Format(String),
    #[error("unsupported amx/amxx version: {0}")]
    Version(String),
    #[error("symbol not found: {0}")]
    NotFound(String),
    #[error("invalid entry index")]
    InvalidIndex,
    #[error("invalid parameter: {0}")]
    Params(String),
    #[error("instruction fuel exhausted")]
    FuelExhausted,
    #[error("zlib decompression failed: {0}")]
    Decompress(String),
}

/// Parsed 32-bit `AMX_HEADER` (`amxx_amx.h:263-281`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmxHeader {
    pub size: i32,
    pub magic: u16,
    pub file_version: u8,
    pub amx_version: u8,
    pub flags: u16,
    pub defsize: i16,
    pub cod: i32,
    pub dat: i32,
    pub hea: i32,
    pub stp: i32,
    pub cip: i32,
    pub publics: i32,
    pub natives: i32,
    pub libraries: i32,
    pub pubvars: i32,
    pub tags: i32,
    pub nametable: i32,
}

/// Entry from `publics`, `natives`, `libraries`, `pubvars`, or `tags`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    pub name: String,
    pub address: u32,
}

/// Parsed and decompressed 32-bit AMX binary ready for VM instantiation.
#[derive(Debug, Clone)]
pub struct ParsedAmx {
    pub header: AmxHeader,
    /// Full expanded AMX memory image (`0..stp`), containing header, code, and data/heap/stack.
    pub image: Vec<u8>,
    pub publics: Vec<SymbolEntry>,
    pub natives: Vec<SymbolEntry>,
    pub libraries: Vec<SymbolEntry>,
    pub pubvars: Vec<SymbolEntry>,
    pub tags: Vec<SymbolEntry>,
}

impl ParsedAmx {
    /// Parses a `.amxx` container (v1/v2/v3) or raw `.amx` binary into a 32-bit `ParsedAmx`.
    pub fn parse_amxx(bytes: &[u8]) -> Result<Self, AmxError> {
        if bytes.len() < 8 {
            return Err(AmxError::Format("file too short".into()));
        }
        let magic = read_u32_le(bytes, 0)?;

        // Old RLEB format is rejected just like `CAmxxReader`.
        if magic == 0x524C_4542 {
            return Err(AmxError::Version("obsolete RLEB format".into()));
        }

        if magic == MAGIC_HEADER2 {
            return parse_amxx_v2(bytes);
        }
        if magic == MAGIC_HEADER {
            return parse_amxx_v1(bytes);
        }

        // Check for raw `.amx` (`AMX_MAGIC` at byte offset 4).
        let amx_magic = read_u16_le(bytes, 4)?;
        if amx_magic == AMX_MAGIC {
            if bytes.len() > MAX_DECOMPRESSED_SIZE {
                return Err(AmxError::Format("raw amx exceeds size limit".into()));
            }
            return Self::parse_amx(bytes.to_vec());
        }

        Err(AmxError::Format(format!(
            "unrecognized magic 0x{magic:08X}"
        )))
    }

    /// Parses an uncompressed (optionally compact-encoded) raw 32-bit AMX image.
    pub fn parse_amx(mut image: Vec<u8>) -> Result<Self, AmxError> {
        if image.len() < AMX_HEADER_SIZE {
            return Err(AmxError::Format("amx header truncated".into()));
        }

        let header = AmxHeader {
            size: read_i32_le(&image, 0)?,
            magic: read_u16_le(&image, 4)?,
            file_version: image[6],
            amx_version: image[7],
            flags: read_u16_le(&image, 8)?,
            defsize: read_i16_le(&image, 10)?,
            cod: read_i32_le(&image, 12)?,
            dat: read_i32_le(&image, 16)?,
            hea: read_i32_le(&image, 20)?,
            stp: read_i32_le(&image, 24)?,
            cip: read_i32_le(&image, 28)?,
            publics: read_i32_le(&image, 32)?,
            natives: read_i32_le(&image, 36)?,
            libraries: read_i32_le(&image, 40)?,
            pubvars: read_i32_le(&image, 44)?,
            tags: read_i32_le(&image, 48)?,
            nametable: read_i32_le(&image, 52)?,
        };

        if header.magic != AMX_MAGIC {
            return Err(AmxError::Format(format!(
                "invalid AMX magic 0x{:04X}",
                header.magic
            )));
        }
        if header.file_version < MIN_FILE_VERSION || header.amx_version > CUR_FILE_VERSION {
            return Err(AmxError::Version(format!(
                "file_version={} amx_version={}",
                header.file_version, header.amx_version
            )));
        }
        if header.defsize != 20 && header.defsize != 8 {
            return Err(AmxError::Format(format!(
                "unsupported defsize {}",
                header.defsize
            )));
        }
        if header.stp <= 0
            || header.cod < AMX_HEADER_SIZE as i32
            || header.dat < header.cod
            || header.hea < header.dat
            || header.stp < header.hea
        {
            return Err(AmxError::Format("invalid AMX section layout".into()));
        }
        if header.publics < AMX_HEADER_SIZE as i32
            || header.natives < header.publics
            || header.libraries < header.natives
            || header.pubvars < header.libraries
            || header.tags < header.pubvars
        {
            return Err(AmxError::Format("invalid AMX table offsets".into()));
        }
        if (header.stp as usize) > MAX_DECOMPRESSED_SIZE {
            return Err(AmxError::Format("AMX stp exceeds maximum size".into()));
        }

        if header.defsize == 8 {
            if header.nametable < header.tags || header.nametable > header.cod {
                return Err(AmxError::Format("invalid AMX nametable offset".into()));
            }
            let max_len = read_u16_le(&image, header.nametable as usize)? as usize;
            if max_len > MAX_SYMBOL_NAME {
                return Err(AmxError::Format(format!(
                    "nametable max length {max_len} > {MAX_SYMBOL_NAME}"
                )));
            }
        } else if header.tags > header.cod {
            return Err(AmxError::Format("invalid AMX tags offset".into()));
        }

        let target_len = (header.stp as usize).max(header.hea as usize);
        if (header.flags & AMX_FLAG_COMPACT) != 0 {
            if header.size < header.cod || (header.size as usize) > image.len() {
                return Err(AmxError::Format("compact image truncated".into()));
            }
            if image.len() < target_len {
                image.resize(target_len, 0);
            }
            let cod = header.cod as usize;
            let hea = header.hea as usize;
            let codesize = (header.size - header.cod) as usize;
            let memsize = hea - cod;
            expand_compact(&mut image[cod..hea], codesize, memsize)?;
        } else {
            if (header.hea as usize) > image.len() {
                return Err(AmxError::Format("uncompressed image truncated".into()));
            }
            if image.len() < target_len {
                image.resize(target_len, 0);
            }
        }

        let publics = parse_table(&image, &header, header.publics, header.natives)?;
        let natives = parse_table(&image, &header, header.natives, header.libraries)?;
        let libraries = parse_table(&image, &header, header.libraries, header.pubvars)?;
        let pubvars = parse_table(&image, &header, header.pubvars, header.tags)?;
        let tags_end = if header.file_version < 7 {
            header.cod
        } else {
            header.nametable
        };
        let tags = parse_table(&image, &header, header.tags, tags_end)?;

        Ok(Self {
            header,
            image,
            publics,
            natives,
            libraries,
            pubvars,
            tags,
        })
    }

    /// Looks up a public function by exact name, returning `(public_index, code_offset)`.
    pub fn find_public(&self, name: &str) -> Option<(usize, u32)> {
        self.publics
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
            .map(|(i, s)| (i, s.address))
    }

    /// Looks up a native function by exact name, returning `(native_index, address)`.
    pub fn find_native(&self, name: &str) -> Option<(usize, u32)> {
        self.natives
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
            .map(|(i, s)| (i, s.address))
    }

    /// Looks up a public variable by exact name, returning its data-relative AMX address.
    pub fn find_pubvar(&self, name: &str) -> Option<u32> {
        self.pubvars
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.address)
    }

    /// Byte slice of the code segment (`cod..dat`).
    pub fn code_slice(&self) -> &[u8] {
        let cod = self.header.cod as usize;
        let dat = self.header.dat as usize;
        &self.image[cod..dat]
    }

    /// Byte slice of the initial data/stack/heap segment (`dat..stp`).
    pub fn data_slice(&self) -> &[u8] {
        let dat = self.header.dat as usize;
        let stp = self.header.stp as usize;
        &self.image[dat..stp]
    }
}

fn parse_amxx_v2(bytes: &[u8]) -> Result<ParsedAmx, AmxError> {
    let version = read_u16_le(bytes, 4)?;
    if version > MAGIC_VERSION {
        return Err(AmxError::Version(format!(
            "amxx version 0x{version:04X} > 0x{MAGIC_VERSION:04X}"
        )));
    }
    let num_plugins = bytes[6] as usize;
    const ENTRY_SIZE: usize = 17;
    let table_end = 7usize
        .checked_add(num_plugins.saturating_mul(ENTRY_SIZE))
        .ok_or_else(|| AmxError::Format("plugin table overflow".into()))?;
    if table_end > bytes.len() {
        return Err(AmxError::Format("plugin entry table truncated".into()));
    }

    let mut chosen = None;
    for i in 0..num_plugins {
        let base = 7 + i * ENTRY_SIZE;
        let cellsize = bytes[base];
        let disksize = read_i32_le(bytes, base + 1)?;
        let imagesize = read_i32_le(bytes, base + 5)?;
        let memsize = read_i32_le(bytes, base + 9)?;
        let offs = read_i32_le(bytes, base + 13)?;
        if cellsize == 4 {
            chosen = Some((disksize, imagesize, memsize, offs));
            break;
        }
    }

    let (disksize, imagesize, memsize, offs) =
        chosen.ok_or_else(|| AmxError::Format("no 32-bit cellsize=4 section in .amxx".into()))?;
    if disksize < 0 || imagesize <= 0 || memsize < 0 || offs < 0 {
        return Err(AmxError::Format("negative sizes/offset in .amxx entry".into()));
    }
    let offs_u = offs as usize;
    let disk_u = disksize as usize;
    let end = offs_u
        .checked_add(disk_u)
        .ok_or_else(|| AmxError::Format("section offset overflow".into()))?;
    if end > bytes.len() {
        return Err(AmxError::Format("compressed section truncated".into()));
    }

    let expected_buf = (imagesize as usize).max(memsize as usize);
    if expected_buf > MAX_DECOMPRESSED_SIZE {
        return Err(AmxError::Format(
            "declared image/mem size exceeds 16 MiB limit".into(),
        ));
    }

    let mut image = decompress_zlib_bounded(&bytes[offs_u..end], MAX_DECOMPRESSED_SIZE)?;
    if image.len() < expected_buf {
        image.resize(expected_buf, 0);
    }
    ParsedAmx::parse_amx(image)
}

fn parse_amxx_v1(bytes: &[u8]) -> Result<ParsedAmx, AmxError> {
    if bytes.len() < 5 {
        return Err(AmxError::Format("amxx v1 header truncated".into()));
    }
    let num_plugins = bytes[4] as usize;
    const ENTRY_SIZE: usize = 9;
    let table_end = 5 + num_plugins * ENTRY_SIZE;
    if table_end > bytes.len() {
        return Err(AmxError::Format("amxx v1 table truncated".into()));
    }

    let mut chosen = None;
    for i in 0..num_plugins {
        let base = 5 + i * ENTRY_SIZE;
        let cellsize = bytes[base];
        let orig_size = read_i32_le(bytes, base + 1)?;
        let offset = read_i32_le(bytes, base + 5)?;
        if cellsize == 4 {
            let next_offset = if i + 1 < num_plugins {
                read_i32_le(bytes, base + ENTRY_SIZE + 5)? as usize
            } else {
                bytes.len()
            };
            chosen = Some((orig_size, offset, next_offset));
            break;
        }
    }

    let (orig_size, offset, end) =
        chosen.ok_or_else(|| AmxError::Format("no 32-bit section in .amxx v1".into()))?;
    if orig_size <= 0 || offset < 0 || (offset as usize) > end || end > bytes.len() {
        return Err(AmxError::Format("invalid .amxx v1 section bounds".into()));
    }
    if (orig_size as usize) > MAX_DECOMPRESSED_SIZE {
        return Err(AmxError::Format("declared size exceeds 16 MiB limit".into()));
    }
    let mut image = decompress_zlib_bounded(&bytes[offset as usize..end], MAX_DECOMPRESSED_SIZE)?;
    if image.len() < orig_size as usize {
        image.resize(orig_size as usize, 0);
    }
    ParsedAmx::parse_amx(image)
}

fn decompress_zlib_bounded(compressed: &[u8], max_bytes: usize) -> Result<Vec<u8>, AmxError> {
    let decoder = ZlibDecoder::new(compressed);
    let mut limited = decoder.take((max_bytes + 1) as u64);
    let mut out = Vec::new();
    limited
        .read_to_end(&mut out)
        .map_err(|e| AmxError::Decompress(e.to_string()))?;
    if out.len() > max_bytes {
        return Err(AmxError::Decompress(format!(
            "decompressed payload exceeds {max_bytes} byte limit"
        )));
    }
    Ok(out)
}

/// Ported from `amxx_amx.cpp` `expand()` (lines 789-841): in-place decompression of Pawn
/// compact-encoded code+data segment.
fn expand_compact(code: &mut [u8], codesize: usize, mut memsize: usize) -> Result<(), AmxError> {
    if !memsize.is_multiple_of(4) || codesize > memsize || memsize > code.len() {
        return Err(AmxError::Format("invalid compact encoding sizes".into()));
    }
    const MARGIN: usize = 64;
    let mut spare = [(0usize, 0u32); MARGIN];
    let mut sh = 0usize;
    let mut st = 0usize;
    let mut sc = 0usize;
    let mut cs = codesize;

    while cs > 0 {
        let mut c: u32 = 0;
        let mut shift: u32 = 0;
        loop {
            cs -= 1;
            if shift >= 32 {
                return Err(AmxError::Format("compact encoding shift overflow".into()));
            }
            if shift == 0 && (code[cs] & 0x80) != 0 {
                return Err(AmxError::Format(
                    "compact encoding byte missing terminator".into(),
                ));
            }
            c |= u32::from(code[cs] & 0x7f) << shift;
            shift += 7;
            if cs == 0 || (code[cs - 1] & 0x80) == 0 {
                break;
            }
        }
        // Sign-extend based on bit 6 of the MSB byte (`code[cs]`).
        if (code[cs] & 0x40) != 0 {
            while shift < 32 {
                c |= 0xffu32.wrapping_shl(shift);
                shift += 8;
            }
        }
        while sc > 0 && spare[sh].0 > cs {
            let loc = spare[sh].0;
            code[loc..loc + 4].copy_from_slice(&spare[sh].1.to_le_bytes());
            sh = (sh + 1) % MARGIN;
            sc -= 1;
        }
        if memsize < 4 {
            return Err(AmxError::Format("compact memsize underflow".into()));
        }
        memsize -= 4;
        if memsize > cs || (memsize == cs && memsize == 0) {
            code[memsize..memsize + 4].copy_from_slice(&c.to_le_bytes());
        } else {
            if sc >= MARGIN {
                return Err(AmxError::Format("compact spare buffer overflow".into()));
            }
            spare[st] = (memsize, c);
            st = (st + 1) % MARGIN;
            sc += 1;
        }
    }
    if memsize != 0 || sc != 0 {
        return Err(AmxError::Format("compact expansion did not finish cleanly".into()));
    }
    Ok(())
}

fn parse_table(
    image: &[u8],
    hdr: &AmxHeader,
    start: i32,
    end: i32,
) -> Result<Vec<SymbolEntry>, AmxError> {
    let defsize = hdr.defsize as usize;
    let span = (end - start) as usize;
    if !span.is_multiple_of(defsize) {
        return Err(AmxError::Format("symbol table unaligned to defsize".into()));
    }
    let count = span / defsize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = start as usize + i * defsize;
        let address = read_u32_le(image, off)?;
        let name = if defsize == 8 {
            let nameofs = read_u32_le(image, off + 4)? as usize;
            read_cstr_bounded(image, nameofs, MAX_SYMBOL_NAME)?
        } else {
            read_cstr_slice(&image[off + 4..off + 24])
        };
        out.push(SymbolEntry { name, address });
    }
    Ok(out)
}

fn read_cstr_bounded(buf: &[u8], offset: usize, max_len: usize) -> Result<String, AmxError> {
    if offset >= buf.len() {
        return Err(AmxError::Format("symbol name offset out of bounds".into()));
    }
    let end = (offset + max_len + 1).min(buf.len());
    let slice = &buf[offset..end];
    let nul = slice
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| AmxError::Format("unterminated symbol name".into()))?;
    Ok(String::from_utf8_lossy(&slice[..nul]).into_owned())
}

fn read_cstr_slice(slice: &[u8]) -> String {
    let nul = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..nul]).into_owned()
}

fn read_u16_le(buf: &[u8], offset: usize) -> Result<u16, AmxError> {
    let bytes: [u8; 2] = buf
        .get(offset..offset + 2)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| AmxError::Format("unexpected EOF reading u16".into()))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_i16_le(buf: &[u8], offset: usize) -> Result<i16, AmxError> {
    let bytes: [u8; 2] = buf
        .get(offset..offset + 2)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| AmxError::Format("unexpected EOF reading i16".into()))?;
    Ok(i16::from_le_bytes(bytes))
}

fn read_u32_le(buf: &[u8], offset: usize) -> Result<u32, AmxError> {
    let bytes: [u8; 4] = buf
        .get(offset..offset + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| AmxError::Format("unexpected EOF reading u32".into()))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_i32_le(buf: &[u8], offset: usize) -> Result<i32, AmxError> {
    let bytes: [u8; 4] = buf
        .get(offset..offset + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| AmxError::Format("unexpected EOF reading i32".into()))?;
    Ok(i32::from_le_bytes(bytes))
}

/// Constructs a valid zlib-compressed `.amxx` (`MAGIC_HEADER2`, `0x0300`, 32-bit cell size)
/// binary from Pawn code cells, initial data bytes, publics, natives, and pubvars.
///
/// Publics and pubvars are automatically sorted by name (matching the Pawn compiler's binary
/// search invariant), while `natives` keeps its exact input order so `OP_SYSREQ_C` indices match.
pub fn build_test_amxx_plugin(
    publics: &[(&str, u32)],
    natives: &[&str],
    pubvars: &[(&str, u32)],
    code: &[u32],
    data: &[u8],
    stack_heap_bytes: u32,
    entry_cip: i32,
) -> Vec<u8> {
    let amx_image = build_raw_amx_image(
        publics,
        natives,
        pubvars,
        code,
        data,
        stack_heap_bytes,
        entry_cip,
        0,
    );
    wrap_amxx_container(&amx_image, amx_image.len() as i32)
}

/// Wraps a raw AMX binary image in a compressed `MAGIC_HEADER2` `.amxx` container.
pub fn wrap_amxx_container(amx_image: &[u8], memsize: i32) -> Vec<u8> {
    use std::io::Write;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(amx_image).expect("zlib write to vec");
    let compressed = encoder.finish().expect("zlib finish");

    // Header: magic (4) + version (2) + num_plugins (1) + 1 PluginEntry (17) = 24 bytes
    let offs: i32 = 24;
    let mut out = Vec::with_capacity(24 + compressed.len());
    out.extend_from_slice(&MAGIC_HEADER2.to_le_bytes());
    out.extend_from_slice(&MAGIC_VERSION.to_le_bytes());
    out.push(1); // num_plugins
    out.push(4); // cellsize = 4
    out.extend_from_slice(&(compressed.len() as i32).to_le_bytes());
    out.extend_from_slice(&(amx_image.len() as i32).to_le_bytes());
    out.extend_from_slice(&memsize.to_le_bytes());
    out.extend_from_slice(&offs.to_le_bytes());
    out.extend_from_slice(&compressed);
    out
}

/// Builds a raw uncompressed 32-bit AMX v8 image (exposed for compact-encoding & VM tests).
#[allow(clippy::too_many_arguments)]
pub fn build_raw_amx_image(
    publics: &[(&str, u32)],
    natives: &[&str],
    pubvars: &[(&str, u32)],
    code: &[u32],
    data: &[u8],
    stack_heap_bytes: u32,
    entry_cip: i32,
    flags: u16,
) -> Vec<u8> {
    let mut sorted_publics: Vec<(&str, u32)> = publics.to_vec();
    sorted_publics.sort_by(|a, b| a.0.cmp(b.0));

    let mut sorted_pubvars: Vec<(&str, u32)> = pubvars.to_vec();
    sorted_pubvars.sort_by(|a, b| a.0.cmp(b.0));

    let publics_off = AMX_HEADER_SIZE;
    let natives_off = publics_off + sorted_publics.len() * 8;
    let libraries_off = natives_off + natives.len() * 8;
    let pubvars_off = libraries_off;
    let tags_off = pubvars_off + sorted_pubvars.len() * 8;
    let nametable_off = tags_off;

    let mut nametable = Vec::new();
    nametable.extend_from_slice(&(MAX_SYMBOL_NAME as u16).to_le_bytes());

    let mut public_entries = Vec::with_capacity(sorted_publics.len());
    for (name, addr) in &sorted_publics {
        let name_off = (nametable_off + nametable.len()) as u32;
        nametable.extend_from_slice(name.as_bytes());
        nametable.push(0);
        public_entries.push((*addr, name_off));
    }

    let mut native_entries = Vec::with_capacity(natives.len());
    for name in natives {
        let name_off = (nametable_off + nametable.len()) as u32;
        nametable.extend_from_slice(name.as_bytes());
        nametable.push(0);
        native_entries.push((0u32, name_off));
    }

    let mut pubvar_entries = Vec::with_capacity(sorted_pubvars.len());
    for (name, addr) in &sorted_pubvars {
        let name_off = (nametable_off + nametable.len()) as u32;
        nametable.extend_from_slice(name.as_bytes());
        nametable.push(0);
        pubvar_entries.push((*addr, name_off));
    }

    // Pad nametable to 4-byte alignment before code segment.
    while !(nametable_off + nametable.len()).is_multiple_of(4) {
        nametable.push(0);
    }

    let cod_off = nametable_off + nametable.len();
    let code_bytes_len = code.len() * 4;
    let dat_off = cod_off + code_bytes_len;

    // Pad data segment to 4-byte alignment.
    let mut data_padded = data.to_vec();
    while !data_padded.len().is_multiple_of(4) {
        data_padded.push(0);
    }

    let hea_off = dat_off + data_padded.len();
    let stack_heap_aligned = ((stack_heap_bytes.max(1024) + 3) & !3) as usize;
    let stp_off = hea_off + stack_heap_aligned;

    let mut img = Vec::with_capacity(hea_off);
    // AMX_HEADER (56 bytes)
    img.extend_from_slice(&(hea_off as i32).to_le_bytes()); // size
    img.extend_from_slice(&AMX_MAGIC.to_le_bytes()); // magic
    img.push(CUR_FILE_VERSION); // file_version
    img.push(CUR_FILE_VERSION); // amx_version
    img.extend_from_slice(&flags.to_le_bytes()); // flags
    img.extend_from_slice(&8i16.to_le_bytes()); // defsize = AMX_FUNCSTUBNT
    img.extend_from_slice(&(cod_off as i32).to_le_bytes());
    img.extend_from_slice(&(dat_off as i32).to_le_bytes());
    img.extend_from_slice(&(hea_off as i32).to_le_bytes());
    img.extend_from_slice(&(stp_off as i32).to_le_bytes());
    img.extend_from_slice(&entry_cip.to_le_bytes());
    img.extend_from_slice(&(publics_off as i32).to_le_bytes());
    img.extend_from_slice(&(natives_off as i32).to_le_bytes());
    img.extend_from_slice(&(libraries_off as i32).to_le_bytes());
    img.extend_from_slice(&(pubvars_off as i32).to_le_bytes());
    img.extend_from_slice(&(tags_off as i32).to_le_bytes());
    img.extend_from_slice(&(nametable_off as i32).to_le_bytes());

    for (addr, name_off) in public_entries {
        img.extend_from_slice(&addr.to_le_bytes());
        img.extend_from_slice(&name_off.to_le_bytes());
    }
    for (addr, name_off) in native_entries {
        img.extend_from_slice(&addr.to_le_bytes());
        img.extend_from_slice(&name_off.to_le_bytes());
    }
    for (addr, name_off) in pubvar_entries {
        img.extend_from_slice(&addr.to_le_bytes());
        img.extend_from_slice(&name_off.to_le_bytes());
    }
    img.extend_from_slice(&nametable);
    for &cell in code {
        img.extend_from_slice(&cell.to_le_bytes());
    }
    img.extend_from_slice(&data_padded);
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn encode_compact_cells(cells: &[u32]) -> Vec<u8> {
        let mut encoded = Vec::new();
        for &cell in cells {
            let mut s = cell as i32;
            let mut tmp = Vec::new();
            loop {
                let byte = (s & 0x7f) as u8;
                s >>= 7;
                let done = (s == 0 && (byte & 0x40) == 0) || (s == -1 && (byte & 0x40) != 0);
                tmp.push(byte);
                if done {
                    break;
                }
            }
            // Stored MSB-first with bit 7 set on all bytes except the final LSB byte.
            for i in (0..tmp.len()).rev() {
                let raw7 = tmp[i] & 0x7f;
                if i == 0 {
                    encoded.push(raw7);
                } else {
                    encoded.push(raw7 | 0x80);
                }
            }
        }
        encoded
    }

    #[test]
    fn parse_compressed_amxx_container_and_lookup_symbols() {
        let code = [120u32, 0, 46, 11, 42, 48];
        let amxx = build_test_amxx_plugin(
            &[("plugin_init", 8), ("alpha_fn", 8)],
            &["register_plugin", "server_print"],
            &[("MaxClients", 0)],
            &code,
            &[0u8; 4],
            1024,
            0,
        );
        let parsed = ParsedAmx::parse_amxx(&amxx).expect("valid amxx");
        assert_eq!(parsed.header.magic, AMX_MAGIC);
        assert_eq!(parsed.find_public("alpha_fn"), Some((0, 8)));
        assert_eq!(parsed.find_public("plugin_init"), Some((1, 8)));
        assert_eq!(parsed.find_native("register_plugin"), Some((0, 0)));
        assert_eq!(parsed.find_native("server_print"), Some((1, 0)));
        assert_eq!(parsed.find_pubvar("MaxClients"), Some(0));
    }

    #[test]
    fn skips_64bit_entry_and_selects_32bit_section() {
        let raw32 = build_raw_amx_image(&[("main_pub", 8)], &[], &[], &[120, 0, 46, 11, 7, 48], &[], 1024, 0, 0);
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw32).unwrap();
        let comp32 = enc.finish().unwrap();

        let header_size: i32 = 7 + 2 * 17;
        let mut container = Vec::new();
        container.extend_from_slice(&MAGIC_HEADER2.to_le_bytes());
        container.extend_from_slice(&MAGIC_VERSION.to_le_bytes());
        container.push(2); // 2 entries
        // Entry 0: 64-bit (`cellsize = 8`) dummy
        container.push(8);
        container.extend_from_slice(&4i32.to_le_bytes());
        container.extend_from_slice(&4i32.to_le_bytes());
        container.extend_from_slice(&4i32.to_le_bytes());
        container.extend_from_slice(&header_size.to_le_bytes());
        // Entry 1: 32-bit (`cellsize = 4`)
        container.push(4);
        container.extend_from_slice(&(comp32.len() as i32).to_le_bytes());
        container.extend_from_slice(&(raw32.len() as i32).to_le_bytes());
        container.extend_from_slice(&(raw32.len() as i32).to_le_bytes());
        container.extend_from_slice(&(header_size + 4).to_le_bytes());
        container.extend_from_slice(&[0u8; 4]);
        container.extend_from_slice(&comp32);

        let parsed = ParsedAmx::parse_amxx(&container).expect("selects 32-bit section");
        assert_eq!(parsed.find_public("main_pub"), Some((0, 8)));
    }

    #[test]
    fn compact_flag_expands_positive_and_negative_cells() {
        let cells = [120u32, 0, 46, 11, (-123456i32) as u32, 48, 0x1234_5678, (-1i32) as u32];
        let mut raw = build_raw_amx_image(&[], &[], &[], &cells, &[], 1024, 0, AMX_FLAG_COMPACT);
        let cod = read_i32_le(&raw, 12).unwrap() as usize;
        let hea = read_i32_le(&raw, 20).unwrap() as usize;
        let compact_payload = encode_compact_cells(&cells);
        let new_size = cod + compact_payload.len();
        raw[0..4].copy_from_slice(&(new_size as i32).to_le_bytes());
        raw.truncate(cod);
        raw.extend_from_slice(&compact_payload);
        // Keep `hea` original expanded offset; `stp` >= `hea`
        assert!(new_size <= hea);

        let parsed = ParsedAmx::parse_amx(raw).expect("compact expand");
        for (i, &expected) in cells.iter().enumerate() {
            let got = read_u32_le(parsed.code_slice(), i * 4).unwrap();
            assert_eq!(got, expected, "cell {i}");
        }
    }

    #[test]
    fn rejects_invalid_magic_and_zlib_bomb_sizes() {
        assert!(matches!(
            ParsedAmx::parse_amxx(b"BAD!0000"),
            Err(AmxError::Format(_))
        ));
        let mut oversized = Vec::new();
        oversized.extend_from_slice(&MAGIC_HEADER2.to_le_bytes());
        oversized.extend_from_slice(&MAGIC_VERSION.to_le_bytes());
        oversized.push(1);
        oversized.push(4);
        oversized.extend_from_slice(&4i32.to_le_bytes());
        oversized.extend_from_slice(&((MAX_DECOMPRESSED_SIZE as i32) + 1).to_le_bytes());
        oversized.extend_from_slice(&1024i32.to_le_bytes());
        oversized.extend_from_slice(&24i32.to_le_bytes());
        oversized.extend_from_slice(&[0u8; 4]);
        assert!(matches!(
            ParsedAmx::parse_amxx(&oversized),
            Err(AmxError::Format(_))
        ));
    }
}
