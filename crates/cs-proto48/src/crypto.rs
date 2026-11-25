//! Bit-exact GoldSrc cryptographic & obfuscation primitives ported from `ReHLDS`:
//! - `CRC32` + `COM_BlockSequenceCRCByte` + `CRC_MapFile` (`engine/crc.cpp`)
//! - `COM_Munge` / `COM_Munge2` / `COM_Munge3` and inverses (`engine/common.cpp`)
//! - Pure-Rust RFC 1321 `MD5` (`engine/md5.cpp`)

/// Standard IEEE 802.3 CRC-32 lookup table (`pulCRCTable` in `engine/crc.cpp`).
pub const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut c = i;
        let mut j = 0;
        while j < 8 {
            if (c & 1) != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            j += 1;
        }
        table[i as usize] = c;
        i += 1;
    }
    table
};

/// Initialize a GoldSrc running CRC-32 accumulator (`CRC32_Init`).
#[inline]
pub const fn crc32_init() -> u32 {
    0xFFFF_FFFF
}

/// Update a GoldSrc running CRC-32 accumulator (`CRC32_ProcessBuffer`).
#[inline]
pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = CRC32_TABLE[((crc as u8) ^ b) as usize] ^ (crc >> 8);
    }
    crc
}

/// Finalize a GoldSrc CRC-32 accumulator (`CRC32_Final`).
#[inline]
pub const fn crc32_final(crc: u32) -> u32 {
    !crc
}

/// Compute finalized IEEE CRC-32 over a byte slice.
#[inline]
pub fn crc32_bytes(data: &[u8]) -> u32 {
    crc32_final(crc32_update(crc32_init(), data))
}

/// GoldSrc `COM_BlockSequenceCRCByte` (`engine/crc.cpp:129`).
///
/// Computes the 1-byte integrity checksum appended to `clc_move` packets by combining
/// up to 60 payload bytes with 4 bytes sliced from the little-endian `CRC32_TABLE`
/// at byte offset `sequence % 0x3FC`.
pub fn com_block_sequence_crc_byte(base: &[u8], sequence: u32) -> u8 {
    let len = base.len().min(60);
    let mut chkb = [0u8; 64];
    chkb[..len].copy_from_slice(&base[..len]);

    let byte_off = (sequence as usize) % 0x3FC;
    for k in 0..4 {
        let idx = byte_off + k;
        let word = CRC32_TABLE[idx / 4].to_le_bytes();
        chkb[len + k] = word[idx % 4];
    }

    (crc32_bytes(&chkb[..len + 4]) & 0xFF) as u8
}

/// GoldSrc `CRC_MapFile` (`engine/crc.cpp:196`) over in-memory BSP v30 bytes.
///
/// Hashes lumps `1..15` (skipping lump 0 = entities, matching `cstrike`/`valve`) starting
/// from `crc32_init()` and returning the **raw un-finalized** accumulator, matching
/// `SV_SpawnServer`'s `g_psv.worldmapCRC` (`sv_main.cpp:6615`).
pub fn crc_map_bsp(bsp_bytes: &[u8]) -> Option<u32> {
    const HEADER_LUMPS: usize = 15;
    const HEADER_SIZE: usize = 4 + HEADER_LUMPS * 8;
    if bsp_bytes.len() < HEADER_SIZE {
        return None;
    }
    let version = i32::from_le_bytes(bsp_bytes[0..4].try_into().ok()?);
    if version != 30 && version != 29 {
        return None;
    }
    let mut crc = crc32_init();
    for lump in 1..HEADER_LUMPS {
        let base = 4 + lump * 8;
        let ofs = i32::from_le_bytes(bsp_bytes[base..base + 4].try_into().ok()?) as usize;
        let len = i32::from_le_bytes(bsp_bytes[base + 4..base + 8].try_into().ok()?) as usize;
        let end = ofs.checked_add(len)?;
        if end > bsp_bytes.len() {
            return None;
        }
        crc = crc32_update(crc, &bsp_bytes[ofs..end]);
    }
    Some(crc)
}

pub const MUNGIFY_TABLE: [u8; 16] = [
    0x7A, 0x64, 0x05, 0xF1, 0x1B, 0x9B, 0xA0, 0xB5, 0xCA, 0xED, 0x61, 0x0D, 0x4A, 0xDF, 0x8E, 0xC7,
];

pub const MUNGIFY_TABLE2: [u8; 16] = [
    0x05, 0x61, 0x7A, 0xED, 0x1B, 0xCA, 0x0D, 0x9B, 0x4A, 0xF1, 0x64, 0xC7, 0xB5, 0x8E, 0xDF, 0xA0,
];

pub const MUNGIFY_TABLE3: [u8; 16] = [
    0x20, 0x07, 0x13, 0x61, 0x03, 0x45, 0x17, 0x72, 0x0A, 0x2D, 0x48, 0x0C, 0x4A, 0x12, 0xA9, 0xB5,
];

fn munge_with_table(data: &mut [u8], seq: i32, table: &[u8; 16]) {
    let seq_u = seq as u32;
    let chunks = data.len() / 4;
    for i in 0..chunks {
        let slice = &mut data[i * 4..i * 4 + 4];
        let mut c = u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
        c ^= !seq_u;
        c = c.swap_bytes();
        let mut b = c.to_le_bytes();
        for (j, byte) in b.iter_mut().enumerate() {
            let jb = j as u8;
            *byte ^= 0xA5 | (jb << jb) | jb | table[(i + j) & 0x0F];
        }
        c = u32::from_le_bytes(b) ^ seq_u;
        slice.copy_from_slice(&c.to_le_bytes());
    }
}

fn unmunge_with_table(data: &mut [u8], seq: i32, table: &[u8; 16]) {
    let seq_u = seq as u32;
    let chunks = data.len() / 4;
    for i in 0..chunks {
        let slice = &mut data[i * 4..i * 4 + 4];
        let mut c = u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
        c ^= seq_u;
        let mut b = c.to_le_bytes();
        for (j, byte) in b.iter_mut().enumerate() {
            let jb = j as u8;
            *byte ^= 0xA5 | (jb << jb) | jb | table[(i + j) & 0x0F];
        }
        c = u32::from_le_bytes(b).swap_bytes() ^ !seq_u;
        slice.copy_from_slice(&c.to_le_bytes());
    }
}

/// GoldSrc `COM_Munge` (used on `clc_move` payloads).
pub fn com_munge(data: &mut [u8], seq: i32) {
    munge_with_table(data, seq, &MUNGIFY_TABLE);
}

/// GoldSrc `COM_UnMunge` (inverse of `COM_Munge`).
pub fn com_unmunge(data: &mut [u8], seq: i32) {
    unmunge_with_table(data, seq, &MUNGIFY_TABLE);
}

/// GoldSrc `COM_Munge2` (used on `NetChan` payloads after byte 8 and `spawn` CRC).
pub fn com_munge2(data: &mut [u8], seq: i32) {
    munge_with_table(data, seq, &MUNGIFY_TABLE2);
}

/// GoldSrc `COM_UnMunge2` (inverse of `COM_Munge2`).
pub fn com_unmunge2(data: &mut [u8], seq: i32) {
    unmunge_with_table(data, seq, &MUNGIFY_TABLE2);
}

/// GoldSrc `COM_Munge3` (used on `worldmapCRC` in `svc_serverinfo`).
pub fn com_munge3(data: &mut [u8], seq: i32) {
    munge_with_table(data, seq, &MUNGIFY_TABLE3);
}

/// GoldSrc `COM_UnMunge3` (inverse of `COM_Munge3`).
pub fn com_unmunge3(data: &mut [u8], seq: i32) {
    unmunge_with_table(data, seq, &MUNGIFY_TABLE3);
}

/// Pure-Rust RFC 1321 MD5 digest (used by GoldSrc for challenges, CD-key hashes,
/// and resource customization hashes).
pub fn md5_digest(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(input.len() + 72);
    msg.extend_from_slice(input);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xEFCD_AB89;
    let mut c0: u32 = 0x98BA_DCFE;
    let mut d0: u32 = 0x1032_5476;

    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let off = i * 4;
            *word = u32::from_le_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Format an already-computed 16-byte MD5 digest as a lowercase 32-character hex string.
pub fn md5_digest_hex(digest: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

/// Compute MD5 of `input` and format it as a lowercase 32-character hex string.
pub fn md5_hex(input: &[u8]) -> String {
    md5_digest_hex(&md5_digest(input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_and_md5_match_standard_vectors() {
        assert_eq!(crc32_bytes(b"123456789"), 0xCBF4_3926);
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
    }

    #[test]
    fn munge2_matches_rehlds_unrolled_constants_bit_for_bit() {
        // Exact unrolled constants from ReHLDS `engine/common.cpp:2605-2620`.
        const REHLDS_MUNGE2_MASKS: [u32; 16] = [
            0xFFFF_E7A5,
            0xBFEF_FFE5,
            0xFFBF_EFFF,
            0xBFEF_BFED,
            0xBFAF_EFBF,
            0xFFBF_AFEF,
            0xFFEF_BFAD,
            0xFFFF_EFBF,
            0xFFEF_F7EF,
            0xBFEF_E7F5,
            0xBFBF_E7E5,
            0xFFAF_B7E7,
            0xBFFF_AFB5,
            0xBFAF_FFAF,
            0xFFAF_A7FF,
            0xFFEF_A7A5,
        ];
        let seq: i32 = 42;
        let m_seq = (!(seq as u32)).swap_bytes() ^ (seq as u32);
        let mut buf = [0u8; 67];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(17).wrapping_add(3);
        }
        let orig = buf;
        com_munge2(&mut buf, seq);

        for (i, &expected_mask) in REHLDS_MUNGE2_MASKS.iter().enumerate() {
            let orig_word = u32::from_le_bytes(orig[i * 4..i * 4 + 4].try_into().unwrap());
            let expected = orig_word.swap_bytes() ^ m_seq ^ expected_mask;
            let actual = u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
            assert_eq!(actual, expected, "mismatch at dword {i}");
        }
        // Trailing 3 bytes must be untouched.
        assert_eq!(&buf[64..67], &orig[64..67]);

        com_unmunge2(&mut buf, seq);
        assert_eq!(buf, orig);
    }

    #[test]
    fn munge1_and_munge3_roundtrip_and_block_crc_is_deterministic() {
        let mut data = *b"GoldSrc Protocol 48 test payload!!";
        let orig = data;
        com_munge(&mut data, 1337);
        assert_ne!(data, orig);
        com_unmunge(&mut data, 1337);
        assert_eq!(data, orig);

        let player_slot = 0i32;
        com_munge3(&mut data, (-1 - player_slot) & 0xFF);
        assert_ne!(data, orig);
        com_unmunge3(&mut data, (-1 - player_slot) & 0xFF);
        assert_eq!(data, orig);

        let c1 = com_block_sequence_crc_byte(&orig, 17);
        let c2 = com_block_sequence_crc_byte(&orig, 17);
        let c3 = com_block_sequence_crc_byte(&orig, 18);
        assert_eq!(c1, c2);
        assert_ne!(c1, c3);
    }
}
