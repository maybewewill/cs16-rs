//! Bit-exact GoldSrc `sizebuf_t` byte & bit stream reader/writer (`engine/common.cpp`).

use glam::Vec3;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("unexpected end of GoldSrc message buffer")]
    UnexpectedEof,
    #[error("invalid protocol payload: {0}")]
    InvalidPayload(String),
    #[error("unknown or unsupported opcode: {0}")]
    UnknownOpcode(u8),
    #[error("delta schema error: {0}")]
    Delta(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// GoldSrc message writer supporting both byte-aligned (`MSG_Write*`) and
/// LSB-first bit-packed (`MSG_StartBitWriting` / `MSG_WriteBits`) sections.
#[derive(Debug, Default, Clone)]
pub struct MsgWriter {
    buf: Vec<u8>,
    bit_active: bool,
    bit_start: usize,
    bits_written: usize,
}

impl MsgWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            bit_active: false,
            bit_start: 0,
            bits_written: 0,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buf
    }

    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.bit_active = false;
        self.bit_start = 0;
        self.bits_written = 0;
    }

    #[inline]
    pub fn write_u8(&mut self, v: u8) {
        debug_assert!(!self.bit_active, "byte write while bit-writing active");
        self.buf.push(v);
    }

    #[inline]
    pub fn write_i8(&mut self, v: i8) {
        self.write_u8(v as u8);
    }

    #[inline]
    pub fn write_u16(&mut self, v: u16) {
        debug_assert!(!self.bit_active, "byte write while bit-writing active");
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    #[inline]
    pub fn write_i16(&mut self, v: i16) {
        self.write_u16(v as u16);
    }

    #[inline]
    pub fn write_u32(&mut self, v: u32) {
        debug_assert!(!self.bit_active, "byte write while bit-writing active");
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    #[inline]
    pub fn write_i32(&mut self, v: i32) {
        self.write_u32(v as u32);
    }

    #[inline]
    pub fn write_f32(&mut self, v: f32) {
        self.write_u32(v.to_bits());
    }

    #[inline]
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        debug_assert!(!self.bit_active, "byte write while bit-writing active");
        self.buf.extend_from_slice(bytes);
    }

    /// Write a null-terminated C string (`MSG_WriteString`).
    pub fn write_str(&mut self, s: &str) {
        debug_assert!(!self.bit_active, "byte write while bit-writing active");
        for &b in s.as_bytes() {
            if b == 0 {
                break;
            }
            self.buf.push(b);
        }
        self.buf.push(0);
    }

    /// Write a fixed-size null-padded byte field (e.g. `svc_newusermsg` 16-byte name).
    pub fn write_fixed_str(&mut self, s: &str, size: usize) {
        debug_assert!(!self.bit_active, "byte write while bit-writing active");
        let bytes = s.as_bytes();
        let copy_len = bytes.len().min(size.saturating_sub(1));
        self.buf.extend_from_slice(&bytes[..copy_len]);
        self.buf.resize(self.buf.len() + (size - copy_len), 0);
    }

    /// `MSG_WriteCoord`: signed 16-bit fixed point (`f * 8.0`).
    #[inline]
    pub fn write_coord(&mut self, f: f32) {
        self.write_i16((f * 8.0) as i16);
    }

    /// `MSG_WriteAngle`: 8-bit angle (`angle * 256 / 360`).
    #[inline]
    pub fn write_angle(&mut self, angle: f32) {
        let v = ((angle as f64 * 256.0 / 360.0) as i32) & 0xFF;
        self.write_u8(v as u8);
    }

    /// `MSG_WriteHiresAngle`: 16-bit angle (`angle * 65536 / 360`).
    #[inline]
    pub fn write_hires_angle(&mut self, angle: f32) {
        let v = ((angle as f64 * 65536.0 / 360.0) as i32) & 0xFFFF;
        self.write_u16(v as u16);
    }

    /// Begin a GoldSrc bit-packed block (`MSG_StartBitWriting`).
    pub fn start_bit_writing(&mut self) {
        debug_assert!(!self.bit_active, "nested start_bit_writing");
        self.bit_active = true;
        self.bit_start = self.buf.len();
        self.bits_written = 0;
        self.buf.push(0);
    }

    /// Finish a GoldSrc bit-packed block (`MSG_EndBitWriting`).
    pub fn end_bit_writing(&mut self) {
        debug_assert!(self.bit_active, "end_bit_writing without start");
        self.bit_active = false;
        self.bit_start = 0;
        self.bits_written = 0;
    }

    #[inline]
    pub fn is_bit_writing(&self) -> bool {
        self.bit_active
    }

    /// Write a single bit LSB-first (`MSG_WriteOneBit`).
    #[inline]
    pub fn write_one_bit(&mut self, bit: bool) {
        debug_assert!(self.bit_active, "bit write outside start_bit_writing");
        let byte_idx = self.bit_start + (self.bits_written / 8);
        let bit_in_byte = self.bits_written % 8;
        if byte_idx >= self.buf.len() {
            self.buf.push(0);
        }
        if bit {
            self.buf[byte_idx] |= 1u8 << bit_in_byte;
        }
        self.bits_written += 1;
    }

    /// Write `numbits` (`0..=32`) unsigned bits LSB-first (`MSG_WriteBits`).
    /// Clamps overflow to `(1 << numbits) - 1`, matching `ReHLDS`.
    pub fn write_bits(&mut self, mut data: u32, numbits: u32) {
        debug_assert!(self.bit_active, "bit write outside start_bit_writing");
        debug_assert!(numbits <= 32);
        if numbits == 0 {
            return;
        }
        if numbits < 32 {
            let max_val = (1u32 << numbits) - 1;
            if data > max_val {
                data = max_val;
            }
        }
        for i in 0..numbits {
            self.write_one_bit(((data >> i) & 1) != 0);
        }
    }

    /// Write a signed integer using GoldSrc's sign-magnitude format (`MSG_WriteSBits`):
    /// 1 sign bit (`1` if negative) followed by `numbits - 1` magnitude bits.
    pub fn write_sbits(&mut self, data: i32, numbits: u32) {
        debug_assert!(self.bit_active, "bit write outside start_bit_writing");
        debug_assert!((1..=32).contains(&numbits));
        let mut idata = data;
        if numbits < 32 {
            let maxnum = (1i32 << (numbits - 1)) - 1;
            idata = idata.clamp(-maxnum, maxnum);
        }
        let neg = idata < 0;
        self.write_one_bit(neg);
        self.write_bits(idata.unsigned_abs(), numbits - 1);
    }

    /// Write a null-terminated C string inside a bit-packed block (`MSG_WriteBitString`).
    pub fn write_bit_string(&mut self, s: &str) {
        for &b in s.as_bytes() {
            if b == 0 {
                break;
            }
            self.write_bits(b as u32, 8);
        }
        self.write_bits(0, 8);
    }

    /// Write raw bytes inside a bit-packed block (`MSG_WriteBitData`).
    pub fn write_bit_data(&mut self, data: &[u8]) {
        for &b in data {
            self.write_bits(b as u32, 8);
        }
    }

    /// Write an angle in `numbits` (`<= 22`) precision (`MSG_WriteBitAngle`).
    pub fn write_bit_angle(&mut self, angle: f32, numbits: u32) {
        debug_assert!(numbits <= 22);
        let shift = 1u32 << numbits;
        let mask = shift - 1;
        let d = (((shift as f64) * ((angle as f64) % 360.0)) as i32) / 360;
        self.write_bits((d as u32) & mask, numbits);
    }

    /// Write a GoldSrc bit-packed coordinate (`MSG_WriteBitCoord`).
    pub fn write_bit_coord(&mut self, f: f32) {
        let signbit = f <= -0.125;
        let abs_f = f.abs();
        let intval = (abs_f as u32) & 0x0FFF;
        let fractval = ((abs_f * 8.0) as u32) & 7;

        self.write_one_bit(intval != 0);
        self.write_one_bit(fractval != 0);
        if intval != 0 || fractval != 0 {
            self.write_one_bit(signbit);
            if intval != 0 {
                self.write_bits(intval, 12);
            }
            if fractval != 0 {
                self.write_bits(fractval, 3);
            }
        }
    }

    /// Write a GoldSrc bit-packed 3D coordinate vector (`MSG_WriteBitVec3Coord`).
    pub fn write_bit_vec3_coord(&mut self, v: Vec3) {
        let xflag = v.x <= -0.125 || v.x >= 0.125;
        let yflag = v.y <= -0.125 || v.y >= 0.125;
        let zflag = v.z <= -0.125 || v.z >= 0.125;
        self.write_one_bit(xflag);
        self.write_one_bit(yflag);
        self.write_one_bit(zflag);
        if xflag {
            self.write_bit_coord(v.x);
        }
        if yflag {
            self.write_bit_coord(v.y);
        }
        if zflag {
            self.write_bit_coord(v.z);
        }
    }
}

/// GoldSrc message reader supporting both byte-aligned (`MSG_Read*`) and
/// LSB-first bit-packed (`MSG_StartBitReading` / `MSG_ReadBits`) sections.
#[derive(Debug, Clone)]
pub struct MsgReader<'a> {
    data: &'a [u8],
    pos: usize,
    bad_read: bool,
    bit_active: bool,
    bit_start: usize,
    bits_read: usize,
}

impl<'a> MsgReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bad_read: false,
            bit_active: false,
            bit_start: 0,
            bits_read: 0,
        }
    }

    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    #[inline]
    pub fn remaining_slice(&self) -> &'a [u8] {
        &self.data[self.pos.min(self.data.len())..]
    }

    #[inline]
    pub fn is_bad_read(&self) -> bool {
        self.bad_read
    }

    #[inline]
    pub fn is_bit_reading(&self) -> bool {
        self.bit_active
    }

    #[inline]
    pub fn current_bit(&self) -> usize {
        if self.bit_active {
            self.bit_start * 8 + self.bits_read
        } else {
            self.pos * 8
        }
    }

    pub fn read_u8(&mut self) -> Result<u8, ProtoError> {
        debug_assert!(!self.bit_active, "byte read while bit-reading active");
        if self.pos >= self.data.len() {
            self.bad_read = true;
            return Err(ProtoError::UnexpectedEof);
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    #[inline]
    pub fn read_i8(&mut self) -> Result<i8, ProtoError> {
        Ok(self.read_u8()? as i8)
    }

    pub fn read_u16(&mut self) -> Result<u16, ProtoError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    #[inline]
    pub fn read_i16(&mut self) -> Result<i16, ProtoError> {
        Ok(self.read_u16()? as i16)
    }

    pub fn read_u32(&mut self) -> Result<u32, ProtoError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    #[inline]
    pub fn read_i32(&mut self) -> Result<i32, ProtoError> {
        Ok(self.read_u32()? as i32)
    }

    #[inline]
    pub fn read_f32(&mut self) -> Result<f32, ProtoError> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    pub fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], ProtoError> {
        debug_assert!(!self.bit_active, "byte read while bit-reading active");
        let end = self.pos.checked_add(len).ok_or(ProtoError::UnexpectedEof)?;
        if end > self.data.len() {
            self.bad_read = true;
            return Err(ProtoError::UnexpectedEof);
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read a null-terminated string (`MSG_ReadString`). Stops at NUL or end-of-buffer.
    pub fn read_str(&mut self) -> Result<String, ProtoError> {
        debug_assert!(!self.bit_active, "byte read while bit-reading active");
        let mut out = Vec::new();
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            if b == 0 {
                break;
            }
            out.push(b);
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Read a line terminated by `\n`, `\0`, or EOF (`MSG_ReadStringLine`).
    pub fn read_str_line(&mut self) -> Result<String, ProtoError> {
        debug_assert!(!self.bit_active, "byte read while bit-reading active");
        let mut out = Vec::new();
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            if b == 0 || b == b'\n' {
                break;
            }
            out.push(b);
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    #[inline]
    pub fn read_coord(&mut self) -> Result<f32, ProtoError> {
        Ok(self.read_i16()? as f32 * 0.125)
    }

    #[inline]
    pub fn read_angle(&mut self) -> Result<f32, ProtoError> {
        Ok(self.read_u8()? as f32 * (360.0 / 256.0))
    }

    #[inline]
    pub fn read_hires_angle(&mut self) -> Result<f32, ProtoError> {
        Ok(self.read_i16()? as f32 * (360.0 / 65536.0))
    }

    /// Begin a GoldSrc bit-packed block (`MSG_StartBitReading`).
    pub fn start_bit_reading(&mut self) {
        debug_assert!(!self.bit_active, "nested start_bit_reading");
        self.bit_active = true;
        self.bit_start = self.pos;
        self.bits_read = 0;
        if self.pos >= self.data.len() {
            self.bad_read = true;
        }
    }

    /// Finish a GoldSrc bit-packed block (`MSG_EndBitReading`).
    /// Advances `pos` by `((bits_read.max(1) + 7) / 8)` bytes, matching `ReHLDS`.
    pub fn end_bit_reading(&mut self) -> Result<(), ProtoError> {
        debug_assert!(self.bit_active, "end_bit_reading without start");
        let bytes_consumed = self.bits_read.max(1).div_ceil(8);
        let new_pos = self.bit_start.saturating_add(bytes_consumed);
        self.bit_active = false;
        self.bit_start = 0;
        self.bits_read = 0;
        if new_pos > self.data.len() {
            self.pos = self.data.len();
            self.bad_read = true;
            return Err(ProtoError::UnexpectedEof);
        }
        self.pos = new_pos;
        if self.bad_read {
            return Err(ProtoError::UnexpectedEof);
        }
        Ok(())
    }

    /// Read a single bit LSB-first (`MSG_ReadOneBit`).
    #[inline]
    pub fn read_one_bit(&mut self) -> bool {
        debug_assert!(self.bit_active, "bit read outside start_bit_reading");
        if self.bad_read {
            return true;
        }
        let byte_idx = self.bit_start + (self.bits_read / 8);
        let bit_in_byte = self.bits_read % 8;
        if byte_idx >= self.data.len() {
            self.bad_read = true;
            return true;
        }
        let v = (self.data[byte_idx] & (1u8 << bit_in_byte)) != 0;
        self.bits_read += 1;
        v
    }

    /// Read `numbits` (`0..=32`) unsigned bits LSB-first (`MSG_ReadBits`).
    pub fn read_bits(&mut self, numbits: u32) -> u32 {
        debug_assert!(self.bit_active, "bit read outside start_bit_reading");
        debug_assert!(numbits <= 32);
        if self.bad_read {
            return 1;
        }
        let mut out = 0u32;
        for i in 0..numbits {
            if self.read_one_bit() {
                if self.bad_read {
                    return 1;
                }
                out |= 1u32 << i;
            }
        }
        out
    }

    /// Peek `numbits` without advancing the bit cursor (`MSG_PeekBits`).
    pub fn peek_bits(&mut self, numbits: u32) -> u32 {
        let saved_bits = self.bits_read;
        let saved_bad = self.bad_read;
        let v = self.read_bits(numbits);
        self.bits_read = saved_bits;
        self.bad_read = saved_bad;
        v
    }

    /// Read a signed integer using GoldSrc's sign-magnitude format (`MSG_ReadSBits`).
    pub fn read_sbits(&mut self, numbits: u32) -> i32 {
        debug_assert!((1..=32).contains(&numbits));
        let sign = self.read_one_bit();
        let mag = self.read_bits(numbits - 1) as i32;
        if sign { -mag } else { mag }
    }

    /// Read a null-terminated string from a bit-packed block (`MSG_ReadBitString`).
    pub fn read_bit_string(&mut self) -> String {
        let mut out = Vec::new();
        loop {
            let c = self.read_bits(8) as u8;
            if self.bad_read || c == 0 {
                break;
            }
            out.push(c);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Read `len` raw bytes from a bit-packed block (`MSG_ReadBitData`).
    pub fn read_bit_data(&mut self, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            let b = self.read_bits(8) as u8;
            if self.bad_read {
                break;
            }
            out.push(b);
        }
        out
    }

    /// Read an angle with `numbits` precision (`MSG_ReadBitAngle`).
    pub fn read_bit_angle(&mut self, numbits: u32) -> f32 {
        let raw = self.read_bits(numbits) as f64;
        (raw * (360.0 / (1u64 << numbits) as f64)) as f32
    }

    /// Read a bit-packed coordinate (`MSG_ReadBitCoord`).
    pub fn read_bit_coord(&mut self) -> f32 {
        let has_int = self.read_one_bit();
        let has_frac = self.read_one_bit();
        if !has_int && !has_frac {
            return 0.0;
        }
        let neg = self.read_one_bit();
        let intval = if has_int { self.read_bits(12) } else { 0 };
        let fractval = if has_frac { self.read_bits(3) } else { 0 };
        let v = intval as f32 + (fractval as f32) * 0.125;
        if neg { -v } else { v }
    }

    /// Read a bit-packed 3D coordinate vector (`MSG_ReadBitVec3Coord`).
    pub fn read_bit_vec3_coord(&mut self) -> Vec3 {
        let xflag = self.read_one_bit();
        let yflag = self.read_one_bit();
        let zflag = self.read_one_bit();
        Vec3::new(
            if xflag { self.read_bit_coord() } else { 0.0 },
            if yflag { self.read_bit_coord() } else { 0.0 },
            if zflag { self.read_bit_coord() } else { 0.0 },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_writer_and_reader_match_goldsrc_byte_boundaries() {
        for nbits in [0u32, 1, 7, 8, 9, 15, 16, 17, 24, 31, 32] {
            let mut w = MsgWriter::new();
            w.write_u8(0xAA);
            let mask = if nbits == 32 {
                u32::MAX
            } else {
                (1u32 << nbits) - 1
            };
            let input = 0xDEAD_BEEF & mask;
            w.start_bit_writing();
            if nbits > 0 {
                w.write_bits(input, nbits);
            }
            w.end_bit_writing();
            w.write_u8(0x55);

            let expected_bit_bytes = (nbits as usize).max(1).div_ceil(8);
            assert_eq!(w.len(), 1 + expected_bit_bytes + 1, "nbits={nbits}");

            let mut r = MsgReader::new(w.as_slice());
            assert_eq!(r.read_u8().unwrap(), 0xAA);
            r.start_bit_reading();
            if nbits > 0 {
                assert_eq!(r.read_bits(nbits), input);
            }
            r.end_bit_reading().unwrap();
            assert_eq!(r.read_u8().unwrap(), 0x55);
            assert_eq!(r.remaining(), 0);
        }

        // Verify ReHLDS overflow clamping (`common.cpp:489-490`).
        let mut w = MsgWriter::new();
        w.start_bit_writing();
        w.write_bits(0xDEAD_BEEF, 7);
        w.end_bit_writing();
        let mut r = MsgReader::new(w.as_slice());
        r.start_bit_reading();
        assert_eq!(r.read_bits(7), 127);
        r.end_bit_reading().unwrap();
    }

    #[test]
    fn sbits_sign_magnitude_and_bit_coords_roundtrip() {
        let mut w = MsgWriter::new();
        w.start_bit_writing();
        w.write_sbits(-42, 10);
        w.write_sbits(42, 10);
        w.write_sbits(0, 8);
        w.write_bit_angle(180.0, 16);
        w.write_bit_string("de_dust2");
        w.write_bit_vec3_coord(Vec3::new(-512.25, 0.0, 1024.875));
        w.end_bit_writing();

        let mut r = MsgReader::new(w.as_slice());
        r.start_bit_reading();
        assert_eq!(r.read_sbits(10), -42);
        assert_eq!(r.read_sbits(10), 42);
        assert_eq!(r.read_sbits(8), 0);
        assert!((r.read_bit_angle(16) - 180.0).abs() < 0.02);
        assert_eq!(r.read_bit_string(), "de_dust2");
        let v = r.read_bit_vec3_coord();
        assert!((v - Vec3::new(-512.25, 0.0, 1024.875)).length() < 0.001);
        r.end_bit_reading().unwrap();
    }
}
