//! Pure-Rust 32-bit Pawn 3.2 bytecode virtual machine.
//!
//! Ported from AMX Mod X GPL-3 `amxx_amx.h` and `amxx_amx.cpp`:
//! - Exact opcode IDs `0..=144` (`OP_NONE` through `OP_FLOAT_CMP`)
//! - Bounds-checked data/stack/heap memory (zero `unsafe`, zero panics)
//! - Deterministic instruction fuel limit (`FuelExhausted`)
//! - Packed & unpacked AMX string helpers (`get_amx_string`, `set_amx_string`)

pub use crate::amxxfile::AmxError;
use crate::amxxfile::{AmxHeader, ParsedAmx, SymbolEntry};

/// Safety margin kept between heap top (`hea`) and stack pointer (`stk`), in bytes
/// (`STKMARGIN = 16 * sizeof(cell)` in `amxx_amx.cpp:1561`).
pub const STKMARGIN: i32 = 16 * 4;

/// Default maximum VM instructions executed per `exec()` call.
pub const DEFAULT_MAX_STEPS: u64 = 1_000_000;

/// Maximum string bytes read by `get_amx_string` to avoid unbounded scans.
const MAX_AMX_STRING_BYTES: usize = 65_536;

/// Pawn 3.2 + AMX Mod X opcode constants (`0..=144`, `amxx_amx.cpp:131-279`).
pub mod op {
    pub const OP_NONE: u32 = 0;
    pub const OP_LOAD_PRI: u32 = 1;
    pub const OP_LOAD_ALT: u32 = 2;
    pub const OP_LOAD_S_PRI: u32 = 3;
    pub const OP_LOAD_S_ALT: u32 = 4;
    pub const OP_LREF_PRI: u32 = 5;
    pub const OP_LREF_ALT: u32 = 6;
    pub const OP_LREF_S_PRI: u32 = 7;
    pub const OP_LREF_S_ALT: u32 = 8;
    pub const OP_LOAD_I: u32 = 9;
    pub const OP_LODB_I: u32 = 10;
    pub const OP_CONST_PRI: u32 = 11;
    pub const OP_CONST_ALT: u32 = 12;
    pub const OP_ADDR_PRI: u32 = 13;
    pub const OP_ADDR_ALT: u32 = 14;
    pub const OP_STOR_PRI: u32 = 15;
    pub const OP_STOR_ALT: u32 = 16;
    pub const OP_STOR_S_PRI: u32 = 17;
    pub const OP_STOR_S_ALT: u32 = 18;
    pub const OP_SREF_PRI: u32 = 19;
    pub const OP_SREF_ALT: u32 = 20;
    pub const OP_SREF_S_PRI: u32 = 21;
    pub const OP_SREF_S_ALT: u32 = 22;
    pub const OP_STOR_I: u32 = 23;
    pub const OP_STRB_I: u32 = 24;
    pub const OP_LIDX: u32 = 25;
    pub const OP_LIDX_B: u32 = 26;
    pub const OP_IDXADDR: u32 = 27;
    pub const OP_IDXADDR_B: u32 = 28;
    pub const OP_ALIGN_PRI: u32 = 29;
    pub const OP_ALIGN_ALT: u32 = 30;
    pub const OP_LCTRL: u32 = 31;
    pub const OP_SCTRL: u32 = 32;
    pub const OP_MOVE_PRI: u32 = 33;
    pub const OP_MOVE_ALT: u32 = 34;
    pub const OP_XCHG: u32 = 35;
    pub const OP_PUSH_PRI: u32 = 36;
    pub const OP_PUSH_ALT: u32 = 37;
    pub const OP_PUSH_R: u32 = 38;
    pub const OP_PUSH_C: u32 = 39;
    pub const OP_PUSH: u32 = 40;
    pub const OP_PUSH_S: u32 = 41;
    pub const OP_POP_PRI: u32 = 42;
    pub const OP_POP_ALT: u32 = 43;
    pub const OP_STACK: u32 = 44;
    pub const OP_HEAP: u32 = 45;
    pub const OP_PROC: u32 = 46;
    pub const OP_RET: u32 = 47;
    pub const OP_RETN: u32 = 48;
    pub const OP_CALL: u32 = 49;
    pub const OP_CALL_PRI: u32 = 50;
    pub const OP_JUMP: u32 = 51;
    pub const OP_JREL: u32 = 52;
    pub const OP_JZER: u32 = 53;
    pub const OP_JNZ: u32 = 54;
    pub const OP_JEQ: u32 = 55;
    pub const OP_JNEQ: u32 = 56;
    pub const OP_JLESS: u32 = 57;
    pub const OP_JLEQ: u32 = 58;
    pub const OP_JGRTR: u32 = 59;
    pub const OP_JGEQ: u32 = 60;
    pub const OP_JSLESS: u32 = 61;
    pub const OP_JSLEQ: u32 = 62;
    pub const OP_JSGRTR: u32 = 63;
    pub const OP_JSGEQ: u32 = 64;
    pub const OP_SHL: u32 = 65;
    pub const OP_SHR: u32 = 66;
    pub const OP_SSHR: u32 = 67;
    pub const OP_SHL_C_PRI: u32 = 68;
    pub const OP_SHL_C_ALT: u32 = 69;
    pub const OP_SHR_C_PRI: u32 = 70;
    pub const OP_SHR_C_ALT: u32 = 71;
    pub const OP_SMUL: u32 = 72;
    pub const OP_SDIV: u32 = 73;
    pub const OP_SDIV_ALT: u32 = 74;
    pub const OP_UMUL: u32 = 75;
    pub const OP_UDIV: u32 = 76;
    pub const OP_UDIV_ALT: u32 = 77;
    pub const OP_ADD: u32 = 78;
    pub const OP_SUB: u32 = 79;
    pub const OP_SUB_ALT: u32 = 80;
    pub const OP_AND: u32 = 81;
    pub const OP_OR: u32 = 82;
    pub const OP_XOR: u32 = 83;
    pub const OP_NOT: u32 = 84;
    pub const OP_NEG: u32 = 85;
    pub const OP_INVERT: u32 = 86;
    pub const OP_ADD_C: u32 = 87;
    pub const OP_SMUL_C: u32 = 88;
    pub const OP_ZERO_PRI: u32 = 89;
    pub const OP_ZERO_ALT: u32 = 90;
    pub const OP_ZERO: u32 = 91;
    pub const OP_ZERO_S: u32 = 92;
    pub const OP_SIGN_PRI: u32 = 93;
    pub const OP_SIGN_ALT: u32 = 94;
    pub const OP_EQ: u32 = 95;
    pub const OP_NEQ: u32 = 96;
    pub const OP_LESS: u32 = 97;
    pub const OP_LEQ: u32 = 98;
    pub const OP_GRTR: u32 = 99;
    pub const OP_GEQ: u32 = 100;
    pub const OP_SLESS: u32 = 101;
    pub const OP_SLEQ: u32 = 102;
    pub const OP_SGRTR: u32 = 103;
    pub const OP_SGEQ: u32 = 104;
    pub const OP_EQ_C_PRI: u32 = 105;
    pub const OP_EQ_C_ALT: u32 = 106;
    pub const OP_INC_PRI: u32 = 107;
    pub const OP_INC_ALT: u32 = 108;
    pub const OP_INC: u32 = 109;
    pub const OP_INC_S: u32 = 110;
    pub const OP_INC_I: u32 = 111;
    pub const OP_DEC_PRI: u32 = 112;
    pub const OP_DEC_ALT: u32 = 113;
    pub const OP_DEC: u32 = 114;
    pub const OP_DEC_S: u32 = 115;
    pub const OP_DEC_I: u32 = 116;
    pub const OP_MOVS: u32 = 117;
    pub const OP_CMPS: u32 = 118;
    pub const OP_FILL: u32 = 119;
    pub const OP_HALT: u32 = 120;
    pub const OP_BOUNDS: u32 = 121;
    pub const OP_SYSREQ_PRI: u32 = 122;
    pub const OP_SYSREQ_C: u32 = 123;
    pub const OP_FILE: u32 = 124;
    pub const OP_LINE: u32 = 125;
    pub const OP_SYMBOL: u32 = 126;
    pub const OP_SRANGE: u32 = 127;
    pub const OP_JUMP_PRI: u32 = 128;
    pub const OP_SWITCH: u32 = 129;
    pub const OP_CASETBL: u32 = 130;
    pub const OP_SWAP_PRI: u32 = 131;
    pub const OP_SWAP_ALT: u32 = 132;
    pub const OP_PUSH_ADR: u32 = 133;
    pub const OP_NOP: u32 = 134;
    pub const OP_SYSREQ_D: u32 = 135;
    pub const OP_SYMTAG: u32 = 136;
    pub const OP_BREAK: u32 = 137;
    pub const OP_FLOAT_MUL: u32 = 138;
    pub const OP_FLOAT_DIV: u32 = 139;
    pub const OP_FLOAT_ADD: u32 = 140;
    pub const OP_FLOAT_SUB: u32 = 141;
    pub const OP_FLOAT_TO: u32 = 142;
    pub const OP_FLOAT_ROUND: u32 = 143;
    pub const OP_FLOAT_CMP: u32 = 144;
    pub const OP_NUM_OPCODES: u32 = 145;
}

use op::*;

/// Entry point passed to [`PawnVm::exec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecEntry {
    /// `AMX_EXEC_MAIN (-1)`
    Main,
    /// `AMX_EXEC_CONT (-2)`
    Continue,
    /// Index into the plugin's `publics` table.
    Public(usize),
    /// Invalid negative index.
    Invalid,
}

impl From<i32> for ExecEntry {
    fn from(value: i32) -> Self {
        match value {
            -1 => Self::Main,
            -2 => Self::Continue,
            n if n >= 0 => Self::Public(n as usize),
            _ => Self::Invalid,
        }
    }
}

/// Host callback interface invoked on `SYSREQ.C` / `SYSREQ.PRI`.
pub trait NativeDispatch {
    /// Dispatches a native function call. `params_addr` points to `params[0]` (argument byte
    /// count) on the Pawn stack, followed by `params[1..=argc]`.
    fn call_native(
        &mut self,
        vm: &mut PawnVm,
        native_idx: usize,
        native_name: &str,
        params_addr: i32,
    ) -> Result<i32, AmxError>;
}

/// No-op native dispatcher for standalone VM tests without host natives.
pub struct NoopNatives;

impl NativeDispatch for NoopNatives {
    fn call_native(
        &mut self,
        _vm: &mut PawnVm,
        _native_idx: usize,
        native_name: &str,
        _params_addr: i32,
    ) -> Result<i32, AmxError> {
        Err(AmxError::InvalidNative(native_name.to_string()))
    }
}

/// Pure-Rust 32-bit Pawn 3.2 abstract machine.
#[derive(Debug, Clone)]
pub struct PawnVm {
    header: AmxHeader,
    code: Vec<u8>,
    data: Vec<u8>,
    publics: Vec<SymbolEntry>,
    natives: Vec<SymbolEntry>,
    pubvars: Vec<SymbolEntry>,
    /// Instruction pointer (byte offset into `code`).
    cip: usize,
    /// Primary register.
    pri: i32,
    /// Alternate register.
    alt: i32,
    /// Frame pointer (byte offset into `data`).
    frm: i32,
    /// Heap top (byte offset into `data`).
    hea: i32,
    /// Heap low-water mark (`header.hea - header.dat`).
    hlw: i32,
    /// Stack pointer (byte offset into `data`).
    stk: i32,
    /// Stack top (`header.stp - header.dat - 4`).
    stp: i32,
    reset_stk: i32,
    reset_hea: i32,
    max_steps: u64,
    last_steps: u64,
}

impl PawnVm {
    /// Instantiates a VM from a parsed 32-bit [`ParsedAmx`].
    pub fn from_parsed(parsed: &ParsedAmx) -> Result<Self, AmxError> {
        let code = parsed.code_slice().to_vec();
        let mut data = parsed.data_slice().to_vec();
        let data_len = data.len();
        if data_len < 8 || !data_len.is_multiple_of(4) {
            return Err(AmxError::Format("data/stack segment misaligned".into()));
        }
        // Write zero sentinel cell at the top of stack (`amxx_amx.cpp:893`).
        let stp = (data_len - 4) as i32;
        data[data_len - 4..data_len].copy_from_slice(&0i32.to_le_bytes());

        let hlw = parsed.header.hea - parsed.header.dat;
        if hlw < 0 || hlw > stp {
            return Err(AmxError::Format("invalid heap/stack bounds".into()));
        }

        Ok(Self {
            header: parsed.header,
            code,
            data,
            publics: parsed.publics.clone(),
            natives: parsed.natives.clone(),
            pubvars: parsed.pubvars.clone(),
            cip: 0,
            pri: 0,
            alt: 0,
            frm: 0,
            hea: hlw,
            hlw,
            stk: stp,
            stp,
            reset_stk: stp,
            reset_hea: hlw,
            max_steps: DEFAULT_MAX_STEPS,
            last_steps: 0,
        })
    }

    /// Sets the per-invocation instruction fuel limit.
    pub fn set_max_steps(&mut self, max_steps: u64) {
        self.max_steps = max_steps;
    }

    /// Returns the configured per-invocation instruction fuel limit.
    pub fn max_steps(&self) -> u64 {
        self.max_steps
    }

    /// Returns the number of instructions executed in the most recent [`exec`](Self::exec).
    pub fn last_steps(&self) -> u64 {
        self.last_steps
    }

    /// Public functions table.
    pub fn publics(&self) -> &[SymbolEntry] {
        &self.publics
    }

    /// Native functions table.
    pub fn natives(&self) -> &[SymbolEntry] {
        &self.natives
    }

    /// Public variables table.
    pub fn pubvars(&self) -> &[SymbolEntry] {
        &self.pubvars
    }

    /// Finds a public function index by exact name.
    pub fn find_public(&self, name: &str) -> Option<usize> {
        self.publics.iter().position(|s| s.name == name)
    }

    /// Finds a public variable's data-relative address by exact name.
    pub fn find_pubvar(&self, name: &str) -> Option<i32> {
        self.pubvars
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.address as i32)
    }

    /// Allocates `cells` 32-bit cells on the VM heap and returns the AMX address.
    /// Heap allocations made before `exec` are automatically reclaimed when `exec` finishes,
    /// or can be reclaimed explicitly with `release_heap`.
    pub fn allot(&mut self, cells: usize) -> Result<i32, AmxError> {
        let bytes = i32::try_from(cells.saturating_mul(4)).map_err(|_| AmxError::OutOfMemory)?;
        if self.stk - self.hea - bytes < STKMARGIN {
            return Err(AmxError::OutOfMemory);
        }
        let addr = self.hea;
        self.hea += bytes;
        Ok(addr)
    }

    /// Reclaims heap memory back to `addr` (`amx_Release`).
    pub fn release_heap(&mut self, addr: i32) {
        if addr >= self.hlw && self.hea > addr {
            self.hea = addr;
        }
    }

    /// Allocates and writes a NUL-terminated unpacked string on the heap, returning its AMX
    /// address (convenient for passing strings into `exec` arguments).
    pub fn allot_string(&mut self, s: &str) -> Result<i32, AmxError> {
        let bytes = s.as_bytes();
        let cells = bytes.len() + 1;
        let addr = self.allot(cells)?;
        self.set_amx_string(addr, s, bytes.len())?;
        Ok(addr)
    }

    /// Allocates and writes an array of cells on the heap, returning its AMX address.
    pub fn allot_array(&mut self, values: &[i32]) -> Result<i32, AmxError> {
        let addr = self.allot(values.len())?;
        for (i, &val) in values.iter().enumerate() {
            self.write_cell(addr + (i as i32) * 4, val)?;
        }
        Ok(addr)
    }

    /// Reads a 32-bit cell from `amx_addr` in data/heap/stack memory.
    pub fn read_cell(&self, amx_addr: i32) -> Result<i32, AmxError> {
        if amx_addr < 0 {
            return Err(AmxError::MemoryAccess);
        }
        let off = amx_addr as usize;
        let end = off.checked_add(4).ok_or(AmxError::MemoryAccess)?;
        let slice = self.data.get(off..end).ok_or(AmxError::MemoryAccess)?;
        Ok(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    /// Writes a 32-bit cell to `amx_addr` in data/heap/stack memory.
    pub fn write_cell(&mut self, amx_addr: i32, val: i32) -> Result<(), AmxError> {
        if amx_addr < 0 {
            return Err(AmxError::MemoryAccess);
        }
        let off = amx_addr as usize;
        let end = off.checked_add(4).ok_or(AmxError::MemoryAccess)?;
        let slice = self.data.get_mut(off..end).ok_or(AmxError::MemoryAccess)?;
        slice.copy_from_slice(&val.to_le_bytes());
        Ok(())
    }

    /// Returns the number of arguments passed to a native (`params[0] / 4`).
    pub fn param_count(&self, params_addr: i32) -> Result<usize, AmxError> {
        let byte_count = self.read_cell(params_addr)?;
        if byte_count < 0 || byte_count % 4 != 0 {
            return Err(AmxError::Params("unaligned native param byte count".into()));
        }
        Ok((byte_count / 4) as usize)
    }

    /// Reads 1-based native parameter `index` (`params[index]`).
    pub fn param(&self, params_addr: i32, index: usize) -> Result<i32, AmxError> {
        let off = i32::try_from(index)
            .ok()
            .and_then(|i| i.checked_mul(4))
            .and_then(|o| params_addr.checked_add(o))
            .ok_or(AmxError::MemoryAccess)?;
        self.read_cell(off)
    }

    /// Reads 1-based native parameter `index` as IEEE-754 `f32` cell bits.
    pub fn param_f32(&self, params_addr: i32, index: usize) -> Result<f32, AmxError> {
        Ok(f32::from_bits(self.param(params_addr, index)? as u32))
    }

    /// Reads a string pointed to by 1-based native parameter `index`.
    pub fn param_string(&self, params_addr: i32, index: usize) -> Result<String, AmxError> {
        let str_addr = self.param(params_addr, index)?;
        self.get_amx_string(str_addr)
    }

    /// Reads a packed or unpacked Pawn string starting at `amx_addr` (`amxx_amx.cpp:3848`).
    pub fn get_amx_string(&self, amx_addr: i32) -> Result<String, AmxError> {
        self.verify_data_addr(amx_addr, 4)?;
        let first = self.read_cell(amx_addr)? as u32;
        const UNPACKEDMAX: u32 = 0x00FF_FFFF;

        let mut bytes = Vec::new();
        if first > UNPACKEDMAX {
            // Packed string: 4 bytes per cell, big-endian byte order inside each cell.
            let mut cursor = amx_addr;
            'outer: while bytes.len() < MAX_AMX_STRING_BYTES {
                let cell = self.read_cell(cursor)? as u32;
                for shift in [24, 16, 8, 0] {
                    let b = ((cell >> shift) & 0xFF) as u8;
                    if b == 0 {
                        break 'outer;
                    }
                    bytes.push(b);
                    if bytes.len() >= MAX_AMX_STRING_BYTES {
                        break 'outer;
                    }
                }
                cursor = cursor.checked_add(4).ok_or(AmxError::MemoryAccess)?;
            }
        } else {
            // Unpacked string: 1 byte per cell.
            let mut cursor = amx_addr;
            while bytes.len() < MAX_AMX_STRING_BYTES {
                let cell = self.read_cell(cursor)?;
                if cell == 0 {
                    break;
                }
                bytes.push((cell & 0xFF) as u8);
                cursor = cursor.checked_add(4).ok_or(AmxError::MemoryAccess)?;
            }
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Writes an unpacked NUL-terminated string into AMX memory at `amx_addr`, writing at most
    /// `max_chars` characters plus the NUL terminator. Returns the number of characters written.
    pub fn set_amx_string(
        &mut self,
        amx_addr: i32,
        s: &str,
        max_chars: usize,
    ) -> Result<usize, AmxError> {
        let src = s.as_bytes();
        let count = src.len().min(max_chars);
        for (i, &b) in src.iter().take(count).enumerate() {
            let cell_addr = amx_addr
                .checked_add((i as i32) * 4)
                .ok_or(AmxError::MemoryAccess)?;
            self.write_cell(cell_addr, i32::from(b))?;
        }
        let nul_addr = amx_addr
            .checked_add((count as i32) * 4)
            .ok_or(AmxError::MemoryAccess)?;
        self.write_cell(nul_addr, 0)?;
        Ok(count)
    }

    /// Formats an AMX vararg string (`amxx_format`, supporting `%s`, `%d`, `%i`, `%u`, `%f`,
    /// `%x`, `%X`, `%c`, `%%`, `%L`, width & precision).
    pub fn format_amx_string(
        &self,
        params_addr: i32,
        fmt_param_idx: usize,
        first_arg_idx: usize,
    ) -> Result<String, AmxError> {
        let fmt = self.param_string(params_addr, fmt_param_idx)?;
        let argc = self.param_count(params_addr)?;
        let mut arg_idx = first_arg_idx;
        let mut out = String::with_capacity(fmt.len());
        let bytes = fmt.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i] != b'%' {
                // Push UTF-8 chunk cleanly.
                let ch = fmt[i..].chars().next().unwrap_or('\0');
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
            i += 1;
            if i >= bytes.len() {
                out.push('%');
                break;
            }
            if bytes[i] == b'%' {
                out.push('%');
                i += 1;
                continue;
            }

            // Parse optional flags/width/precision.
            let mut left_align = false;
            let mut zero_pad = false;
            while i < bytes.len() && matches!(bytes[i], b'-' | b'0' | b'+' | b' ') {
                if bytes[i] == b'-' {
                    left_align = true;
                } else if bytes[i] == b'0' {
                    zero_pad = true;
                }
                i += 1;
            }
            let mut width: Option<usize> = None;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                let d = (bytes[i] - b'0') as usize;
                width = Some(width.unwrap_or(0).saturating_mul(10).saturating_add(d));
                i += 1;
            }
            let mut precision: Option<usize> = None;
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                let mut prec = 0usize;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    prec = prec
                        .saturating_mul(10)
                        .saturating_add((bytes[i] - b'0') as usize);
                    i += 1;
                }
                precision = Some(prec);
            }
            if i >= bytes.len() {
                break;
            }
            let spec = bytes[i];
            i += 1;

            if arg_idx > argc {
                return Err(AmxError::Params("format string has too few arguments".into()));
            }
            let cell = self.param(params_addr, arg_idx)?;
            arg_idx += 1;

            let formatted = match spec {
                b's' => {
                    let s = self.get_amx_string(cell)?;
                    if let Some(p) = precision {
                        s.chars().take(p).collect()
                    } else {
                        s
                    }
                }
                b'd' | b'i' => {
                    let v = self.read_cell_or_val(cell, params_addr, arg_idx - 1)?;
                    format!("{v}")
                }
                b'u' => {
                    let v = self.read_cell_or_val(cell, params_addr, arg_idx - 1)? as u32;
                    format!("{v}")
                }
                b'x' => {
                    let v = self.read_cell_or_val(cell, params_addr, arg_idx - 1)? as u32;
                    format!("{v:x}")
                }
                b'X' => {
                    let v = self.read_cell_or_val(cell, params_addr, arg_idx - 1)? as u32;
                    format!("{v:X}")
                }
                b'f' => {
                    let raw = self.read_cell_or_val(cell, params_addr, arg_idx - 1)? as u32;
                    let f = f32::from_bits(raw);
                    let prec = precision.unwrap_or(6).min(16);
                    format!("{f:.prec$}")
                }
                b'c' => {
                    let v = self.read_cell_or_val(cell, params_addr, arg_idx - 1)? as u8;
                    String::from(v as char)
                }
                b'L' => {
                    // Lang key: `%L` consumes target player id + lang key string.
                    if arg_idx <= argc {
                        let key_addr = self.param(params_addr, arg_idx)?;
                        arg_idx += 1;
                        self.get_amx_string(key_addr).unwrap_or_default()
                    } else {
                        String::new()
                    }
                }
                other => {
                    let mut tmp = String::from('%');
                    tmp.push(other as char);
                    tmp
                }
            };

            if let Some(w) = width
                && formatted.len() < w
            {
                let pad = w - formatted.len();
                let pad_char = if zero_pad && !left_align { '0' } else { ' ' };
                if left_align {
                    out.push_str(&formatted);
                    for _ in 0..pad {
                        out.push(' ');
                    }
                } else {
                    for _ in 0..pad {
                        out.push(pad_char);
                    }
                    out.push_str(&formatted);
                }
            } else {
                out.push_str(&formatted);
            }
        }

        Ok(out)
    }

    fn read_cell_or_val(
        &self,
        cell: i32,
        _params_addr: i32,
        _arg_idx: usize,
    ) -> Result<i32, AmxError> {
        // In Pawn varargs (`format`, `client_print`, etc.), scalar arguments like `%d`/`%f`
        // are passed by reference (their address in AMX data/stack) by the Pawn compiler!
        // However, synthetic callers may also pass direct values or stack/data addresses.
        // Wait: in Pawn `native format(output[], len, const format[], any:...)`:
        // Pawn ALWAYS passes varargs (`...`) by reference! Let's check if `cell` is a valid
        // data/stack cell address when reading from `amxxpc`-compiled plugins vs direct tests:
        // Actually, for `%d` in real Pawn `format("%d", x)`, `x` is passed by reference (`addr`
        // into `data`), while `format_amx_string` in AMXX reads `*get_amxaddr(amx, params[i])`.
        // To support BOTH real Pawn by-ref varargs AND direct integer/float values if someone
        // passes an out-of-range scalar, wait: what if scalar `5` happens to look like data
        // address `4`? Address `0` is possible in data!
        // In Pawn compiler (`sc3.c`), `...` ALWAYS passes by reference (address of variable/const
        // on heap/stack/data). Let's read by reference if `verify_data_addr(cell, 4)` succeeds
        // ONLY? Wait: if `x = 12` and `data.len() == 1024`, `12` is a valid data address!
        // So reading by-reference MUST be the rule for Pawn varargs — wait: what if in our tests
        // we construct Pawn bytecode with `PUSH_C` pointing to data/stack? Yes! When assembling
        // Pawn calls to `format` / `client_print`, varargs are placed in data or stack and their
        // address is pushed, OR let's check `amxx_amx.cpp`!
        // In `amxx_amx.cpp` (`amxmodx` `format_amxstring`), `*get_amxaddr(amx, params[parm++])`
        // ALWAYS dereferences the parameter as an address!
        // Wait: what if a test pushes a small immediate `PUSH_C 42` for `client_print`? No, a real
        // Pawn compiler creates a stack/data slot and pushes its address for `any:...`!
        // So dereferencing `cell` via `self.read_cell(cell)` matches real AMX Mod X 100%.
        self.read_cell(cell)
    }

    /// Executes `entry` (`ExecEntry::Main`, `ExecEntry::Public(idx)`, or `ExecEntry::Continue`)
    /// with `args` pushed onto the Pawn stack and returns `pri` on normal exit/halt.
    pub fn exec(
        &mut self,
        entry: impl Into<ExecEntry>,
        args: &[i32],
        host: &mut dyn NativeDispatch,
    ) -> Result<i32, AmxError> {
        let entry = entry.into();
        let saved_reset_stk = self.reset_stk;
        let saved_reset_hea = self.reset_hea;

        // `reset_hea` tracks `hlw` or whatever was allotted before `exec`?
        // Wait: if caller called `allot_string` before `exec`, `self.hea` is already advanced,
        // and we should restore `hea` back to `hlw`? No: if caller did NOT call `allot_string`
        // before `exec`, `reset_hea == self.hea == self.hlw`. If caller DID call `allot_string`
        // for `exec` args, restoring `self.hea = self.hlw` (if not nested) or keeping caller's
        // `release_heap` handles it. Even cleaner: `exec` saves `prev_hea = self.hea`, `prev_stk = self.stk`,
        // and restores them on return (unless `Sleep`), while `allot_string` callers can either
        // `release_heap` or let `exec_with_strings` clean up! Let's match `amxx_amx.cpp:1724-1725`:
        // `reset_stk = stk; reset_hea = hea;` (before pushing args, `amx_Push` already pushed args,
        // so `reset_stk` becomes `stk + args.len() * 4`).
        let mut reset_stk = self.stk;
        let mut reset_hea = self.hea;

        let mut pri = 0i32;
        let mut alt = 0i32;
        let mut frm = 0i32;
        let mut hea = self.hea;
        let mut stk = self.stk;

        let start_cip = match entry {
            ExecEntry::Main => {
                if self.header.cip < 0 {
                    return Err(AmxError::InvalidIndex);
                }
                self.header.cip as usize
            }
            ExecEntry::Continue => {
                frm = self.frm;
                stk = self.stk;
                hea = self.hea;
                pri = self.pri;
                alt = self.alt;
                reset_stk = saved_reset_stk;
                reset_hea = saved_reset_hea;
                self.cip
            }
            ExecEntry::Public(idx) => {
                let sym = self.publics.get(idx).ok_or(AmxError::InvalidIndex)?;
                sym.address as usize
            }
            ExecEntry::Invalid => return Err(AmxError::InvalidIndex),
        };

        if start_cip >= self.code.len() || !start_cip.is_multiple_of(4) {
            return Err(AmxError::InvalidIndex);
        }
        if stk > self.stp {
            return Err(AmxError::StackLow);
        }
        if hea < self.hlw {
            return Err(AmxError::HeapLow);
        }

        if entry != ExecEntry::Continue {
            // Push arguments right-to-left (`amx_Push` order).
            for &arg in args.iter().rev() {
                stk = self.push_on_stack(stk, hea, arg)?;
            }
            // Note `reset_stk` already equals the pre-push `stk`!
            let param_bytes = (args.len() as i32).wrapping_mul(4);
            stk = self.push_on_stack(stk, hea, param_bytes)?;
            stk = self.push_on_stack(stk, hea, 0)?; // zero return address
        }

        if hea + STKMARGIN > stk {
            self.stk = reset_stk;
            self.hea = reset_hea;
            return Err(AmxError::StackErr);
        }

        let mut cip = start_cip;
        let mut remaining = self.max_steps;
        let mut steps = 0u64;

        let res = self.run_loop(
            &mut cip,
            &mut pri,
            &mut alt,
            &mut frm,
            &mut hea,
            &mut stk,
            reset_stk,
            reset_hea,
            &mut remaining,
            &mut steps,
            host,
        );

        self.last_steps = steps;

        match res {
            Ok(val) => {
                self.stk = reset_stk;
                self.hea = reset_hea;
                self.cip = cip;
                self.pri = val;
                self.alt = alt;
                self.frm = frm;
                Ok(val)
            }
            Err(AmxError::Sleep) => {
                self.cip = cip;
                self.pri = pri;
                self.alt = alt;
                self.frm = frm;
                self.hea = hea;
                self.stk = stk;
                self.reset_stk = reset_stk;
                self.reset_hea = reset_hea;
                Err(AmxError::Sleep)
            }
            Err(e) => {
                self.stk = reset_stk;
                self.hea = reset_hea;
                self.cip = cip;
                self.pri = pri;
                self.alt = alt;
                self.frm = frm;
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_loop(
        &mut self,
        cip: &mut usize,
        pri: &mut i32,
        alt: &mut i32,
        frm: &mut i32,
        hea: &mut i32,
        stk: &mut i32,
        reset_stk: i32,
        reset_hea: i32,
        remaining: &mut u64,
        steps: &mut u64,
        host: &mut dyn NativeDispatch,
    ) -> Result<i32, AmxError> {
        loop {
            if *remaining == 0 {
                return Err(AmxError::FuelExhausted);
            }
            *remaining -= 1;
            *steps += 1;

            let opcode = self.fetch_code_u32(cip)?;
            match opcode {
                OP_NONE => return Err(AmxError::InvalidInstruction(opcode)),

                OP_LOAD_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    *pri = self.read_cell(offs)?;
                }
                OP_LOAD_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    *alt = self.read_cell(offs)?;
                }
                OP_LOAD_S_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    *pri = self.read_cell(frm.wrapping_add(offs))?;
                }
                OP_LOAD_S_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    *alt = self.read_cell(frm.wrapping_add(offs))?;
                }
                OP_LREF_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(offs)?;
                    *pri = self.read_cell(ptr)?;
                }
                OP_LREF_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(offs)?;
                    *alt = self.read_cell(ptr)?;
                }
                OP_LREF_S_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(frm.wrapping_add(offs))?;
                    *pri = self.read_cell(ptr)?;
                }
                OP_LREF_S_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(frm.wrapping_add(offs))?;
                    *alt = self.read_cell(ptr)?;
                }
                OP_LOAD_I => {
                    self.verify_active_addr(*pri, 4, *hea, *stk)?;
                    *pri = self.read_cell(*pri)?;
                }
                OP_LODB_I => {
                    let width = self.fetch_code_i32(cip)?;
                    self.verify_active_addr(*pri, width, *hea, *stk)?;
                    *pri = self.read_width(*pri, width)?;
                }
                OP_CONST_PRI => {
                    *pri = self.fetch_code_i32(cip)?;
                }
                OP_CONST_ALT => {
                    *alt = self.fetch_code_i32(cip)?;
                }
                OP_ADDR_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    *pri = frm.wrapping_add(offs);
                }
                OP_ADDR_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    *alt = frm.wrapping_add(offs);
                }
                OP_STOR_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    self.write_cell(offs, *pri)?;
                }
                OP_STOR_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    self.write_cell(offs, *alt)?;
                }
                OP_STOR_S_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    self.write_cell(frm.wrapping_add(offs), *pri)?;
                }
                OP_STOR_S_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    self.write_cell(frm.wrapping_add(offs), *alt)?;
                }
                OP_SREF_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(offs)?;
                    self.write_cell(ptr, *pri)?;
                }
                OP_SREF_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(offs)?;
                    self.write_cell(ptr, *alt)?;
                }
                OP_SREF_S_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(frm.wrapping_add(offs))?;
                    self.write_cell(ptr, *pri)?;
                }
                OP_SREF_S_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    let ptr = self.read_cell(frm.wrapping_add(offs))?;
                    self.write_cell(ptr, *alt)?;
                }
                OP_STOR_I => {
                    self.verify_active_addr(*alt, 4, *hea, *stk)?;
                    self.write_cell(*alt, *pri)?;
                }
                OP_STRB_I => {
                    let width = self.fetch_code_i32(cip)?;
                    self.verify_active_addr(*alt, width, *hea, *stk)?;
                    self.write_width(*alt, width, *pri)?;
                }
                OP_LIDX => {
                    let offs = pri.wrapping_mul(4).wrapping_add(*alt);
                    self.verify_active_addr(offs, 4, *hea, *stk)?;
                    *pri = self.read_cell(offs)?;
                }
                OP_LIDX_B => {
                    let shift = self.fetch_code_i32(cip)?;
                    let offs = pri.wrapping_shl(shift as u32).wrapping_add(*alt);
                    self.verify_active_addr(offs, 4, *hea, *stk)?;
                    *pri = self.read_cell(offs)?;
                }
                OP_IDXADDR => {
                    *pri = pri.wrapping_mul(4).wrapping_add(*alt);
                }
                OP_IDXADDR_B => {
                    let shift = self.fetch_code_i32(cip)?;
                    *pri = pri.wrapping_shl(shift as u32).wrapping_add(*alt);
                }
                OP_ALIGN_PRI => {
                    let offs = self.fetch_code_i32(cip)?;
                    if cfg!(target_endian = "little") && (offs as u32) < 4 {
                        *pri ^= 4 - offs;
                    }
                }
                OP_ALIGN_ALT => {
                    let offs = self.fetch_code_i32(cip)?;
                    if cfg!(target_endian = "little") && (offs as u32) < 4 {
                        *alt ^= 4 - offs;
                    }
                }
                OP_LCTRL => {
                    let reg = self.fetch_code_i32(cip)?;
                    match reg {
                        0 => *pri = self.header.cod,
                        1 => *pri = self.header.dat,
                        2 => *pri = *hea,
                        3 => *pri = self.stp,
                        4 => *pri = *stk,
                        5 => *pri = *frm,
                        6 => *pri = *cip as i32,
                        _ => {}
                    }
                }
                OP_SCTRL => {
                    let reg = self.fetch_code_i32(cip)?;
                    match reg {
                        2 => *hea = *pri,
                        4 => *stk = *pri,
                        5 => *frm = *pri,
                        6 => {
                            let target = *pri as usize;
                            self.verify_code_target(target)?;
                            *cip = target;
                        }
                        _ => {}
                    }
                }
                OP_MOVE_PRI => *pri = *alt,
                OP_MOVE_ALT => *alt = *pri,
                OP_XCHG => std::mem::swap(pri, alt),
                OP_PUSH_PRI => {
                    *stk = self.push_on_stack(*stk, *hea, *pri)?;
                }
                OP_PUSH_ALT => {
                    *stk = self.push_on_stack(*stk, *hea, *alt)?;
                }
                OP_PUSH_R => {
                    let count = self.fetch_code_i32(cip)?;
                    if count < 0 {
                        return Err(AmxError::Params("negative PUSH.R count".into()));
                    }
                    for _ in 0..count {
                        *stk = self.push_on_stack(*stk, *hea, *pri)?;
                    }
                }
                OP_PUSH_C => {
                    let val = self.fetch_code_i32(cip)?;
                    *stk = self.push_on_stack(*stk, *hea, val)?;
                }
                OP_PUSH => {
                    let offs = self.fetch_code_i32(cip)?;
                    let val = self.read_cell(offs)?;
                    *stk = self.push_on_stack(*stk, *hea, val)?;
                }
                OP_PUSH_S => {
                    let offs = self.fetch_code_i32(cip)?;
                    let val = self.read_cell(frm.wrapping_add(offs))?;
                    *stk = self.push_on_stack(*stk, *hea, val)?;
                }
                OP_POP_PRI => {
                    let (new_stk, val) = self.pop_from_stack(*stk)?;
                    *stk = new_stk;
                    *pri = val;
                }
                OP_POP_ALT => {
                    let (new_stk, val) = self.pop_from_stack(*stk)?;
                    *stk = new_stk;
                    *alt = val;
                }
                OP_STACK => {
                    let offs = self.fetch_code_i32(cip)?;
                    *alt = *stk;
                    *stk = stk.wrapping_add(offs);
                    if *hea + STKMARGIN > *stk {
                        return Err(AmxError::StackErr);
                    }
                    if *stk > self.stp {
                        return Err(AmxError::StackLow);
                    }
                }
                OP_HEAP => {
                    let offs = self.fetch_code_i32(cip)?;
                    *alt = *hea;
                    *hea = hea.wrapping_add(offs);
                    if *hea + STKMARGIN > *stk {
                        return Err(AmxError::StackErr);
                    }
                    if *hea < self.hlw {
                        return Err(AmxError::HeapLow);
                    }
                }
                OP_PROC => {
                    *stk = self.push_on_stack(*stk, *hea, *frm)?;
                    *frm = *stk;
                    if *hea + STKMARGIN > *stk {
                        return Err(AmxError::StackErr);
                    }
                }
                OP_RET => {
                    let (s1, new_frm) = self.pop_from_stack(*stk)?;
                    let (s2, ret_addr) = self.pop_from_stack(s1)?;
                    *frm = new_frm;
                    *stk = s2;
                    let target = ret_addr as usize;
                    self.verify_code_target(target)?;
                    *cip = target;
                }
                OP_RETN => {
                    let (s1, new_frm) = self.pop_from_stack(*stk)?;
                    let (s2, ret_addr) = self.pop_from_stack(s1)?;
                    *frm = new_frm;
                    let target = ret_addr as usize;
                    self.verify_code_target(target)?;
                    let param_bytes = self.read_cell(s2)?;
                    let new_stk = s2.wrapping_add(param_bytes).wrapping_add(4);
                    if new_stk < *hea || new_stk > self.stp {
                        return Err(AmxError::StackLow);
                    }
                    *stk = new_stk;
                    *cip = target;
                }
                OP_CALL => {
                    let target = self.fetch_code_i32(cip)? as usize;
                    let ret_addr = *cip as i32;
                    *stk = self.push_on_stack(*stk, *hea, ret_addr)?;
                    self.verify_code_target(target)?;
                    *cip = target;
                }
                OP_CALL_PRI => {
                    let ret_addr = *cip as i32;
                    *stk = self.push_on_stack(*stk, *hea, ret_addr)?;
                    let target = *pri as usize;
                    self.verify_code_target(target)?;
                    *cip = target;
                }
                OP_JUMP => {
                    let target = self.fetch_code_i32(cip)? as usize;
                    self.verify_code_target(target)?;
                    *cip = target;
                }
                OP_JREL => {
                    let rel = self.fetch_code_i32(cip)?;
                    let target = (*cip as isize).wrapping_add(rel as isize) as usize;
                    self.verify_code_target(target)?;
                    *cip = target;
                }
                OP_JZER => self.cond_jump(cip, *pri == 0)?,
                OP_JNZ => self.cond_jump(cip, *pri != 0)?,
                OP_JEQ => self.cond_jump(cip, *pri == *alt)?,
                OP_JNEQ => self.cond_jump(cip, *pri != *alt)?,
                OP_JLESS => self.cond_jump(cip, (*pri as u32) < (*alt as u32))?,
                OP_JLEQ => self.cond_jump(cip, (*pri as u32) <= (*alt as u32))?,
                OP_JGRTR => self.cond_jump(cip, (*pri as u32) > (*alt as u32))?,
                OP_JGEQ => self.cond_jump(cip, (*pri as u32) >= (*alt as u32))?,
                OP_JSLESS => self.cond_jump(cip, *pri < *alt)?,
                OP_JSLEQ => self.cond_jump(cip, *pri <= *alt)?,
                OP_JSGRTR => self.cond_jump(cip, *pri > *alt)?,
                OP_JSGEQ => self.cond_jump(cip, *pri >= *alt)?,

                OP_SHL => *pri = pri.wrapping_shl(*alt as u32),
                OP_SHR => *pri = ((*pri as u32).wrapping_shr(*alt as u32)) as i32,
                OP_SSHR => *pri = pri.wrapping_shr(*alt as u32),
                OP_SHL_C_PRI => {
                    let c = self.fetch_code_i32(cip)?;
                    *pri = pri.wrapping_shl(c as u32);
                }
                OP_SHL_C_ALT => {
                    let c = self.fetch_code_i32(cip)?;
                    *alt = alt.wrapping_shl(c as u32);
                }
                OP_SHR_C_PRI => {
                    let c = self.fetch_code_i32(cip)?;
                    *pri = ((*pri as u32).wrapping_shr(c as u32)) as i32;
                }
                OP_SHR_C_ALT => {
                    let c = self.fetch_code_i32(cip)?;
                    *alt = ((*alt as u32).wrapping_shr(c as u32)) as i32;
                }
                OP_SMUL => *pri = pri.wrapping_mul(*alt),
                OP_SDIV => {
                    if *alt == 0 {
                        return Err(AmxError::DivideByZero);
                    }
                    let (q, r) = floor_div_i32(*pri, *alt);
                    *pri = q;
                    *alt = r;
                }
                OP_SDIV_ALT => {
                    if *pri == 0 {
                        return Err(AmxError::DivideByZero);
                    }
                    let (q, r) = floor_div_i32(*alt, *pri);
                    *pri = q;
                    *alt = r;
                }
                OP_UMUL => *pri = ((*pri as u32).wrapping_mul(*alt as u32)) as i32,
                OP_UDIV => {
                    if *alt == 0 {
                        return Err(AmxError::DivideByZero);
                    }
                    let p = *pri as u32;
                    let a = *alt as u32;
                    *pri = (p / a) as i32;
                    *alt = (p % a) as i32;
                }
                OP_UDIV_ALT => {
                    if *pri == 0 {
                        return Err(AmxError::DivideByZero);
                    }
                    let a = *alt as u32;
                    let p = *pri as u32;
                    *pri = (a / p) as i32;
                    *alt = (a % p) as i32;
                }
                OP_ADD => *pri = pri.wrapping_add(*alt),
                OP_SUB => *pri = pri.wrapping_sub(*alt),
                OP_SUB_ALT => *pri = alt.wrapping_sub(*pri),
                OP_AND => *pri &= *alt,
                OP_OR => *pri |= *alt,
                OP_XOR => *pri ^= *alt,
                OP_NOT => *pri = i32::from(*pri == 0),
                OP_NEG => *pri = pri.wrapping_neg(),
                OP_INVERT => *pri = !*pri,
                OP_ADD_C => {
                    let c = self.fetch_code_i32(cip)?;
                    *pri = pri.wrapping_add(c);
                }
                OP_SMUL_C => {
                    let c = self.fetch_code_i32(cip)?;
                    *pri = pri.wrapping_mul(c);
                }
                OP_ZERO_PRI => *pri = 0,
                OP_ZERO_ALT => *alt = 0,
                OP_ZERO => {
                    let offs = self.fetch_code_i32(cip)?;
                    self.write_cell(offs, 0)?;
                }
                OP_ZERO_S => {
                    let offs = self.fetch_code_i32(cip)?;
                    self.write_cell(frm.wrapping_add(offs), 0)?;
                }
                OP_SIGN_PRI => {
                    if (*pri & 0xff) >= 0x80 {
                        *pri = ((*pri as u32) | !0xffu32) as i32;
                    }
                }
                OP_SIGN_ALT => {
                    if (*alt & 0xff) >= 0x80 {
                        *alt = ((*alt as u32) | !0xffu32) as i32;
                    }
                }
                OP_EQ => *pri = i32::from(*pri == *alt),
                OP_NEQ => *pri = i32::from(*pri != *alt),
                OP_LESS => *pri = i32::from((*pri as u32) < (*alt as u32)),
                OP_LEQ => *pri = i32::from((*pri as u32) <= (*alt as u32)),
                OP_GRTR => *pri = i32::from((*pri as u32) > (*alt as u32)),
                OP_GEQ => *pri = i32::from((*pri as u32) >= (*alt as u32)),
                OP_SLESS => *pri = i32::from(*pri < *alt),
                OP_SLEQ => *pri = i32::from(*pri <= *alt),
                OP_SGRTR => *pri = i32::from(*pri > *alt),
                OP_SGEQ => *pri = i32::from(*pri >= *alt),
                OP_EQ_C_PRI => {
                    let c = self.fetch_code_i32(cip)?;
                    *pri = i32::from(*pri == c);
                }
                OP_EQ_C_ALT => {
                    let c = self.fetch_code_i32(cip)?;
                    *pri = i32::from(*alt == c);
                }
                OP_INC_PRI => *pri = pri.wrapping_add(1),
                OP_INC_ALT => *alt = alt.wrapping_add(1),
                OP_INC => {
                    let offs = self.fetch_code_i32(cip)?;
                    let v = self.read_cell(offs)?;
                    self.write_cell(offs, v.wrapping_add(1))?;
                }
                OP_INC_S => {
                    let offs = self.fetch_code_i32(cip)?;
                    let addr = frm.wrapping_add(offs);
                    let v = self.read_cell(addr)?;
                    self.write_cell(addr, v.wrapping_add(1))?;
                }
                OP_INC_I => {
                    self.verify_active_addr(*pri, 4, *hea, *stk)?;
                    let v = self.read_cell(*pri)?;
                    self.write_cell(*pri, v.wrapping_add(1))?;
                }
                OP_DEC_PRI => *pri = pri.wrapping_sub(1),
                OP_DEC_ALT => *alt = alt.wrapping_sub(1),
                OP_DEC => {
                    let offs = self.fetch_code_i32(cip)?;
                    let v = self.read_cell(offs)?;
                    self.write_cell(offs, v.wrapping_sub(1))?;
                }
                OP_DEC_S => {
                    let offs = self.fetch_code_i32(cip)?;
                    let addr = frm.wrapping_add(offs);
                    let v = self.read_cell(addr)?;
                    self.write_cell(addr, v.wrapping_sub(1))?;
                }
                OP_DEC_I => {
                    self.verify_active_addr(*pri, 4, *hea, *stk)?;
                    let v = self.read_cell(*pri)?;
                    self.write_cell(*pri, v.wrapping_sub(1))?;
                }
                OP_MOVS => {
                    let len = self.fetch_code_i32(cip)?;
                    self.verify_active_range(*pri, len, *hea, *stk)?;
                    self.verify_active_range(*alt, len, *hea, *stk)?;
                    if len > 0 {
                        let src = *pri as usize;
                        let dst = *alt as usize;
                        let n = len as usize;
                        // Copy via temp vec when regions might overlap across slices.
                        let tmp = self.data[src..src + n].to_vec();
                        self.data[dst..dst + n].copy_from_slice(&tmp);
                    }
                }
                OP_CMPS => {
                    let len = self.fetch_code_i32(cip)?;
                    self.verify_active_range(*pri, len, *hea, *stk)?;
                    self.verify_active_range(*alt, len, *hea, *stk)?;
                    let src = *pri as usize;
                    let dst = *alt as usize;
                    let n = len as usize;
                    *pri = match self.data[dst..dst + n].cmp(&self.data[src..src + n]) {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    };
                }
                OP_FILL => {
                    let mut len = self.fetch_code_i32(cip)?;
                    self.verify_active_range(*alt, len, *hea, *stk)?;
                    let mut addr = *alt;
                    while len >= 4 {
                        self.write_cell(addr, *pri)?;
                        addr = addr.wrapping_add(4);
                        len -= 4;
                    }
                }
                OP_HALT => {
                    let code_val = self.fetch_code_i32(cip)?;
                    return match code_val {
                        0 => Ok(*pri),
                        12 => Err(AmxError::Sleep),
                        2 => Err(AmxError::Assert),
                        other => Err(AmxError::Exit(other)),
                    };
                }
                OP_BOUNDS => {
                    let bound = self.fetch_code_i32(cip)?;
                    if (*pri as u32) > (bound as u32) {
                        return Err(AmxError::Bounds);
                    }
                }
                OP_SYSREQ_PRI => {
                    let native_idx = *pri;
                    *pri = self.invoke_native(native_idx, *stk, *hea, *frm, *cip, host)?;
                }
                OP_SYSREQ_C => {
                    let native_idx = self.fetch_code_i32(cip)?;
                    *pri = self.invoke_native(native_idx, *stk, *hea, *frm, *cip, host)?;
                }
                OP_FILE | OP_SYMBOL => {
                    let skip = self.fetch_code_i32(cip)?;
                    if skip < 0 {
                        return Err(AmxError::InvalidInstruction(opcode));
                    }
                    *cip = cip.saturating_add(skip as usize);
                }
                OP_LINE | OP_SRANGE => {
                    let _a = self.fetch_code_i32(cip)?;
                    let _b = self.fetch_code_i32(cip)?;
                }
                OP_SYMTAG => {
                    let _tag = self.fetch_code_i32(cip)?;
                }
                OP_JUMP_PRI => {
                    let target = *pri as usize;
                    self.verify_code_target(target)?;
                    *cip = target;
                }
                OP_SWITCH => {
                    let casetbl_off = self.fetch_code_i32(cip)? as usize;
                    let mut tbl_cip = casetbl_off;
                    let tbl_op = self.fetch_code_u32(&mut tbl_cip)?;
                    if tbl_op != OP_CASETBL {
                        return Err(AmxError::InvalidInstruction(tbl_op));
                    }
                    let count = self.fetch_code_i32(&mut tbl_cip)?;
                    if !(0..=65_536).contains(&count) {
                        return Err(AmxError::Format("invalid casetbl count".into()));
                    }
                    let mut chosen = self.fetch_code_i32(&mut tbl_cip)? as usize;
                    for _ in 0..count {
                        let case_val = self.fetch_code_i32(&mut tbl_cip)?;
                        let case_target = self.fetch_code_i32(&mut tbl_cip)? as usize;
                        if case_val == *pri {
                            chosen = case_target;
                            break;
                        }
                    }
                    self.verify_code_target(chosen)?;
                    *cip = chosen;
                }
                OP_CASETBL => {
                    return Err(AmxError::InvalidInstruction(opcode));
                }
                OP_SWAP_PRI => {
                    let top = self.read_cell(*stk)?;
                    self.write_cell(*stk, *pri)?;
                    *pri = top;
                }
                OP_SWAP_ALT => {
                    let top = self.read_cell(*stk)?;
                    self.write_cell(*stk, *alt)?;
                    *alt = top;
                }
                OP_PUSH_ADR => {
                    let offs = self.fetch_code_i32(cip)?;
                    *stk = self.push_on_stack(*stk, *hea, frm.wrapping_add(offs))?;
                }
                OP_NOP | OP_BREAK => {}
                OP_SYSREQ_D => {
                    return Err(AmxError::InvalidInstruction(opcode));
                }
                OP_FLOAT_MUL => {
                    let a = f32::from_bits(self.read_cell(stk.wrapping_add(4))? as u32);
                    let b = f32::from_bits(self.read_cell(stk.wrapping_add(8))? as u32);
                    *pri = (a * b).to_bits() as i32;
                }
                OP_FLOAT_DIV => {
                    let a = f32::from_bits(self.read_cell(stk.wrapping_add(4))? as u32);
                    let b = f32::from_bits(self.read_cell(stk.wrapping_add(8))? as u32);
                    *pri = (a / b).to_bits() as i32;
                }
                OP_FLOAT_ADD => {
                    let a = f32::from_bits(self.read_cell(stk.wrapping_add(4))? as u32);
                    let b = f32::from_bits(self.read_cell(stk.wrapping_add(8))? as u32);
                    *pri = (a + b).to_bits() as i32;
                }
                OP_FLOAT_SUB => {
                    let a = f32::from_bits(self.read_cell(stk.wrapping_add(4))? as u32);
                    let b = f32::from_bits(self.read_cell(stk.wrapping_add(8))? as u32);
                    *pri = (a - b).to_bits() as i32;
                }
                OP_FLOAT_TO => {
                    let a = self.read_cell(stk.wrapping_add(4))?;
                    *pri = (a as f32).to_bits() as i32;
                }
                OP_FLOAT_ROUND => {
                    let a = f32::from_bits(self.read_cell(stk.wrapping_add(4))? as u32);
                    let mode = self.read_cell(stk.wrapping_add(8))?;
                    let rounded = match mode {
                        0 => (a + 0.5).floor(),
                        1 => a.floor(),
                        _ => a.ceil(),
                    };
                    *pri = rounded as i32;
                }
                OP_FLOAT_CMP => {
                    let a = f32::from_bits(self.read_cell(stk.wrapping_add(4))? as u32);
                    let b = f32::from_bits(self.read_cell(stk.wrapping_add(8))? as u32);
                    *pri = if a == b {
                        0
                    } else if a > b {
                        1
                    } else {
                        -1
                    };
                }
                _ => {
                    let _ = (reset_stk, reset_hea);
                    return Err(AmxError::InvalidInstruction(opcode));
                }
            }
        }
    }

    fn invoke_native(
        &mut self,
        native_idx: i32,
        stk: i32,
        hea: i32,
        frm: i32,
        cip: usize,
        host: &mut dyn NativeDispatch,
    ) -> Result<i32, AmxError> {
        let idx = usize::try_from(native_idx).map_err(|_| AmxError::InvalidIndex)?;
        let name = self
            .natives
            .get(idx)
            .ok_or(AmxError::InvalidIndex)?
            .name
            .clone();
        self.cip = cip;
        self.hea = hea;
        self.frm = frm;
        self.stk = stk;
        host.call_native(self, idx, &name, stk)
    }

    fn cond_jump(&self, cip: &mut usize, cond: bool) -> Result<(), AmxError> {
        let target = self.fetch_code_i32(cip)? as usize;
        if cond {
            self.verify_code_target(target)?;
            *cip = target;
        }
        Ok(())
    }

    fn fetch_code_u32(&self, cip: &mut usize) -> Result<u32, AmxError> {
        let end = cip.checked_add(4).ok_or(AmxError::MemoryAccess)?;
        let slice = self.code.get(*cip..end).ok_or(AmxError::MemoryAccess)?;
        *cip = end;
        Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn fetch_code_i32(&self, cip: &mut usize) -> Result<i32, AmxError> {
        Ok(self.fetch_code_u32(cip)? as i32)
    }

    fn verify_code_target(&self, target: usize) -> Result<(), AmxError> {
        if target >= self.code.len() || !target.is_multiple_of(4) {
            return Err(AmxError::MemoryAccess);
        }
        Ok(())
    }

    fn verify_data_addr(&self, addr: i32, size: i32) -> Result<(), AmxError> {
        if addr < 0 || size < 0 {
            return Err(AmxError::MemoryAccess);
        }
        let end = (addr as usize)
            .checked_add(size as usize)
            .ok_or(AmxError::MemoryAccess)?;
        if end > self.data.len() {
            return Err(AmxError::MemoryAccess);
        }
        Ok(())
    }

    fn verify_active_addr(&self, addr: i32, size: i32, hea: i32, stk: i32) -> Result<(), AmxError> {
        if addr < 0 || size <= 0 || (addr as u32) >= (self.stp as u32) {
            return Err(AmxError::MemoryAccess);
        }
        let end = addr.checked_add(size).ok_or(AmxError::MemoryAccess)?;
        if (end as u32) > (self.stp as u32).saturating_add(4) {
            return Err(AmxError::MemoryAccess);
        }
        if (addr >= hea && addr < stk) || (addr < hea && end > hea) {
            return Err(AmxError::MemoryAccess);
        }
        Ok(())
    }

    fn verify_active_range(&self, addr: i32, len: i32, hea: i32, stk: i32) -> Result<(), AmxError> {
        if addr < 0 || len < 0 || (addr as u32) >= (self.stp as u32) {
            return Err(AmxError::MemoryAccess);
        }
        let end = addr.checked_add(len).ok_or(AmxError::MemoryAccess)?;
        if (end as u32) > (self.stp as u32) {
            return Err(AmxError::MemoryAccess);
        }
        if (addr >= hea && addr < stk) || (end > hea && end < stk) || (addr < hea && end > hea) {
            return Err(AmxError::MemoryAccess);
        }
        Ok(())
    }

    fn push_on_stack(&mut self, stk: i32, hea: i32, val: i32) -> Result<i32, AmxError> {
        let new_stk = stk.checked_sub(4).ok_or(AmxError::StackErr)?;
        if new_stk < hea || new_stk < 0 {
            return Err(AmxError::StackErr);
        }
        self.write_cell(new_stk, val)?;
        Ok(new_stk)
    }

    fn pop_from_stack(&self, stk: i32) -> Result<(i32, i32), AmxError> {
        if stk >= self.stp {
            return Err(AmxError::StackLow);
        }
        let val = self.read_cell(stk)?;
        Ok((stk + 4, val))
    }

    fn read_width(&self, addr: i32, width: i32) -> Result<i32, AmxError> {
        let off = addr as usize;
        match width {
            1 => self
                .data
                .get(off)
                .map(|&b| i32::from(b))
                .ok_or(AmxError::MemoryAccess),
            2 => {
                let s = self
                    .data
                    .get(off..off + 2)
                    .ok_or(AmxError::MemoryAccess)?;
                Ok(i32::from(u16::from_le_bytes([s[0], s[1]])))
            }
            4 => self.read_cell(addr),
            _ => Err(AmxError::Params(format!("invalid LODB.I width {width}"))),
        }
    }

    fn write_width(&mut self, addr: i32, width: i32, val: i32) -> Result<(), AmxError> {
        let off = addr as usize;
        match width {
            1 => {
                let b = self.data.get_mut(off).ok_or(AmxError::MemoryAccess)?;
                *b = val as u8;
                Ok(())
            }
            2 => {
                let s = self
                    .data
                    .get_mut(off..off + 2)
                    .ok_or(AmxError::MemoryAccess)?;
                s.copy_from_slice(&(val as u16).to_le_bytes());
                Ok(())
            }
            4 => self.write_cell(addr, val),
            _ => Err(AmxError::Params(format!("invalid STRB.I width {width}"))),
        }
    }
}

/// Floored signed 32-bit division matching Pawn `SDIV` (`amxx_amx.cpp:2205-2207`).
fn floor_div_i32(dividend: i32, divisor: i32) -> (i32, i32) {
    let p = i64::from(dividend);
    let a = i64::from(divisor);
    let rem = ((p % a) + a) % a;
    let quot = (p - rem) / a;
    (quot as i32, rem as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amxxfile::build_test_amxx_plugin;

    fn vm_from_code(publics: &[(&str, u32)], code: &[u32], data: &[u8]) -> PawnVm {
        let amxx = build_test_amxx_plugin(publics, &[], &[], code, data, 2048, 0);
        let parsed = ParsedAmx::parse_amxx(&amxx).expect("parse amxx");
        PawnVm::from_parsed(&parsed).expect("vm init")
    }

    #[test]
    fn arithmetic_and_floored_sdiv_and_float_opcodes() {
        // Code offset 0: HALT 0
        // Code offset 8: public test_arith(a, b) -> returns (a + b) * 2 - 3
        // Code offset 76: public test_sdiv(a, b) -> floored div, returns pri + alt*100
        // Code offset 124: public test_float() -> float(3) * 2.5 = 7.5 -> rounded = 8
        let code = [
            OP_HALT, 0, // [0..8]
            // test_arith at byte 8:
            OP_PROC,
            OP_LOAD_S_PRI, 12,
            OP_LOAD_S_ALT, 16,
            OP_ADD,
            OP_SMUL_C, 2,
            OP_ADD_C, (-3i32) as u32,
            OP_RETN,
            // test_sdiv at byte 48:
            OP_PROC,
            OP_LOAD_S_PRI, 12,
            OP_LOAD_S_ALT, 16,
            OP_SDIV,
            OP_PUSH_PRI,
            OP_MOVE_PRI,
            OP_SMUL_C, 100,
            OP_POP_ALT,
            OP_ADD,
            OP_RETN,
            // test_float at byte 96:
            OP_PROC,
            OP_PUSH_C, 3,
            OP_PUSH_C, 4,
            OP_FLOAT_TO,
            OP_STACK, 8,
            OP_PUSH_C, 2.5f32.to_bits(),
            OP_PUSH_PRI,
            OP_PUSH_C, 8,
            OP_FLOAT_MUL,
            OP_STACK, 12,
            OP_PUSH_C, 0, // round nearest
            OP_PUSH_PRI,
            OP_PUSH_C, 8,
            OP_FLOAT_ROUND,
            OP_STACK, 12,
            OP_RETN,
        ];
        let mut vm = vm_from_code(
            &[("test_arith", 8), ("test_sdiv", 52), ("test_float", 104)],
            &code,
            &[],
        );
        let mut noop = NoopNatives;

        let p0 = vm.find_public("test_arith").unwrap();
        assert_eq!(vm.exec(p0 as i32, &[10, 5], &mut noop), Ok(27));

        let p1 = vm.find_public("test_sdiv").unwrap();
        // Pawn floored division: -7 / 3 => quotient = -3, remainder = 2 -> -3 + 200 = 197
        assert_eq!(vm.exec(p1 as i32, &[-7, 3], &mut noop), Ok(197));

        let p2 = vm.find_public("test_float").unwrap();
        assert_eq!(vm.exec(p2 as i32, &[], &mut noop), Ok(8));
    }

    #[test]
    fn movs_fill_cmps_and_memory_bounds() {
        // Data layout:
        // [0..16]: src buffer (4 cells)
        // [16..32]: dst buffer (4 cells)
        let data = vec![0u8; 32];
        let code = [
            OP_HALT, 0, // [0..8]
            // public run() at byte 8:
            OP_PROC,
            // FILL dst (addr 16) with 0x11223344 for 16 bytes
            OP_CONST_PRI, 0x1122_3344,
            OP_CONST_ALT, 16,
            OP_FILL, 16,
            // FILL src (addr 0) with 0x11223344 for 16 bytes
            OP_CONST_ALT, 0,
            OP_FILL, 16,
            // CMPS 16 bytes -> should be 0 (equal)
            OP_CONST_PRI, 0,
            OP_CMPS, 16,
            OP_JZER, 88, // jump if equal to byte 88 (index 22)
            OP_CONST_PRI, 999,
            OP_RETN,
            // byte 88 (index 22): modify src[0] = 0x55, MOVS src -> dst (16 bytes), return dst[0]
            OP_CONST_PRI, 0x55,
            OP_STOR_PRI, 0,
            OP_CONST_PRI, 0,
            OP_CONST_ALT, 16,
            OP_MOVS, 16,
            OP_LOAD_PRI, 16,
            OP_RETN,
        ];

        let mut vm = vm_from_code(&[("run", 8)], &code, &data);
        let mut noop = NoopNatives;
        assert_eq!(vm.exec(0, &[], &mut noop), Ok(0x55));
    }

    #[test]
    fn switch_and_casetbl_dispatch() {
        // byte 0: HALT 0
        // byte 8: PROC, LOAD_S_PRI 12, SWITCH -> casetbl
        // byte 28: casetbl (3 cases: 1->72, 5->80, 9->88; default->96)
        let code = [
            OP_HALT, 0, // [0..8]
            OP_PROC,            // [8]
            OP_LOAD_S_PRI, 12,  // [12..20]
            OP_SWITCH, 28,      // [20..28]
            // casetbl at byte 28 (index 7):
            OP_CASETBL,
            3,   // count
            96,  // default -> byte 96 (index 24)
            1, 72, // case 1 -> byte 72 (index 18)
            5, 80, // case 5 -> byte 80 (index 20)
            9, 88, // case 9 -> byte 88 (index 22)
            // index 18 (byte 72):
            OP_CONST_PRI, 100, // [18..20] -> Wait: 72/4 = 18!
            OP_RETN,           // wait: RETN is at 80! Let's recount carefully!
        ];
        let _ = code;
        // Build with exact byte indices:
        let mut prog = vec![OP_HALT, 0, OP_PROC, OP_LOAD_S_PRI, 12, OP_SWITCH, 28];
        let tbl_idx = prog.len();
        assert_eq!(tbl_idx * 4, 28);
        prog.extend_from_slice(&[OP_CASETBL, 3, 0, 1, 0, 5, 0, 9, 0]);
        let case1 = (prog.len() * 4) as u32;
        prog.extend_from_slice(&[OP_CONST_PRI, 100, OP_RETN]);
        let case5 = (prog.len() * 4) as u32;
        prog.extend_from_slice(&[OP_CONST_PRI, 500, OP_RETN]);
        let case9 = (prog.len() * 4) as u32;
        prog.extend_from_slice(&[OP_CONST_PRI, 900, OP_RETN]);
        let def_case = (prog.len() * 4) as u32;
        prog.extend_from_slice(&[OP_CONST_PRI, (-1i32) as u32, OP_RETN]);

        prog[tbl_idx + 2] = def_case;
        prog[tbl_idx + 4] = case1;
        prog[tbl_idx + 6] = case5;
        prog[tbl_idx + 8] = case9;

        let mut vm = vm_from_code(&[("sw", 8)], &prog, &[]);
        let mut noop = NoopNatives;
        assert_eq!(vm.exec(0, &[1], &mut noop), Ok(100));
        assert_eq!(vm.exec(0, &[5], &mut noop), Ok(500));
        assert_eq!(vm.exec(0, &[9], &mut noop), Ok(900));
        assert_eq!(vm.exec(0, &[42], &mut noop), Ok(-1));
    }

    #[test]
    fn bounds_divide_by_zero_memory_access_and_fuel_exhaustion() {
        let mut noop = NoopNatives;

        // BOUNDS failure
        let bounds_code = [OP_HALT, 0, OP_PROC, OP_LOAD_S_PRI, 12, OP_BOUNDS, 4, OP_RETN];
        let mut vm = vm_from_code(&[("b", 8)], &bounds_code, &[]);
        assert_eq!(vm.exec(0, &[4], &mut noop), Ok(4));
        assert_eq!(vm.exec(0, &[5], &mut noop), Err(AmxError::Bounds));
        assert_eq!(vm.exec(0, &[-1], &mut noop), Err(AmxError::Bounds));

        // Divide by zero
        let div_code = [OP_HALT, 0, OP_PROC, OP_CONST_PRI, 10, OP_ZERO_ALT, OP_SDIV, OP_RETN];
        let mut vm = vm_from_code(&[("d", 8)], &div_code, &[]);
        assert_eq!(vm.exec(0, &[], &mut noop), Err(AmxError::DivideByZero));

        // Out-of-bounds memory access
        let mem_code = [OP_HALT, 0, OP_PROC, OP_CONST_PRI, 999_999, OP_LOAD_I, OP_RETN];
        let mut vm = vm_from_code(&[("m", 8)], &mem_code, &[]);
        assert_eq!(vm.exec(0, &[], &mut noop), Err(AmxError::MemoryAccess));

        // Fuel exhaustion on infinite loop (JUMP to byte 8)
        let loop_code = [OP_HALT, 0, OP_JUMP, 8];
        let mut vm = vm_from_code(&[("inf", 8)], &loop_code, &[]);
        vm.set_max_steps(50);
        assert_eq!(vm.exec(0, &[], &mut noop), Err(AmxError::FuelExhausted));
        assert_eq!(vm.last_steps(), 50);
    }

    #[test]
    fn packed_and_unpacked_strings_roundtrip() {
        let mut data = vec![0u8; 64];
        // Packed string "Hi!" at offset 0: 'H'(0x48), 'i'(0x69), '!'(0x21), '\0'(0x00) -> 0x48692100
        data[0..4].copy_from_slice(&0x4869_2100u32.to_le_bytes());

        let vm_amxx = build_test_amxx_plugin(&[], &[], &[], &[OP_HALT, 0], &data, 1024, 0);
        let parsed = ParsedAmx::parse_amxx(&vm_amxx).unwrap();
        let mut vm = PawnVm::from_parsed(&parsed).unwrap();

        assert_eq!(vm.get_amx_string(0).unwrap(), "Hi!");
        vm.set_amx_string(16, "Counter-Strike", 7).unwrap();
        assert_eq!(vm.get_amx_string(16).unwrap(), "Counter");
    }

    #[test]
    fn call_ret_lidx_idxaddr_and_lctrl() {
        // byte 0: HALT 0
        // byte 8: helper(x) -> returns x + 10 (uses PROC + RETN)
        // byte 28: main_pub() -> calls helper(5), tests LIDX on data[0..12]
        let mut data = Vec::new();
        for v in [100i32, 200, 300] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut code = vec![OP_HALT, 0];
        let helper_off = (code.len() * 4) as u32;
        code.extend_from_slice(&[OP_PROC, OP_LOAD_S_PRI, 12, OP_ADD_C, 10, OP_RETN]);
        let entry_off = (code.len() * 4) as u32;
        code.extend_from_slice(&[
            OP_PROC,
            OP_PUSH_C, 5,
            OP_PUSH_C, 4,
            OP_CALL, helper_off, // returns 15 in pri
            OP_PUSH_PRI,         // save 15
            OP_CONST_PRI, 2,     // index 2
            OP_ZERO_ALT,         // base 0
            OP_LIDX,             // pri = data[2*4 + 0] = 300
            OP_POP_ALT,          // alt = 15
            OP_ADD,              // 315
            OP_RETN,
        ]);

        let mut vm = vm_from_code(&[("entry", entry_off)], &code, &data);
        let mut noop = NoopNatives;
        assert_eq!(vm.exec(0, &[], &mut noop), Ok(315));
    }
}
