//! Bit-exact GoldSrc Delta Compression engine (`ReHLDS/engine/delta.cpp` + `cstrike/delta.lst`).
//!
//! Supports:
//! - Parsing real Valve `cstrike/delta.lst` / `valve/delta.lst` schemas.
//! - Encoding/decoding `svc_deltadescription` (opcode 14) via `g_MetaDelta`.
//! - Bit-exact `write_delta` / `read_delta` for all 8 `DT_*` field types (`DT_BYTE`,
//!   `DT_SHORT`, `DT_FLOAT`, `DT_INTEGER`, `DT_ANGLE`, `DT_TIMEWINDOW_8`,
//!   `DT_TIMEWINDOW_BIG`, `DT_STRING`, plus `DT_SIGNED`).
//! - Typed struct conversions for `clientdata_t`, `entity_state_t`, `entity_state_player_t`,
//!   `custom_entity_state_t`, `usercmd_t`, `weapon_data_t`, and `event_t`.

use std::collections::HashMap;

use glam::Vec3;

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::delta_offsets::field_layout;

pub const DELTA_MAX_FIELDS: usize = 56;

pub const DT_BYTE: u32 = 1 << 0;
pub const DT_SHORT: u32 = 1 << 1;
pub const DT_FLOAT: u32 = 1 << 2;
pub const DT_INTEGER: u32 = 1 << 3;
pub const DT_ANGLE: u32 = 1 << 4;
pub const DT_TIMEWINDOW_8: u32 = 1 << 5;
pub const DT_TIMEWINDOW_BIG: u32 = 1 << 6;
pub const DT_STRING: u32 = 1 << 7;
pub const DT_SIGNED: u32 = 1 << 31;

/// Single field entry inside a GoldSrc `delta_t` schema (`delta_description_t`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaFieldDesc {
    pub field_type: u32,
    pub name: String,
    pub field_offset: u16,
    pub field_size: u8,
    pub significant_bits: u8,
    pub premultiply: f32,
    pub postmultiply: f32,
}

impl DeltaFieldDesc {
    #[inline]
    pub fn base_type(&self) -> u32 {
        self.field_type & !DT_SIGNED
    }

    #[inline]
    pub fn is_signed(&self) -> bool {
        (self.field_type & DT_SIGNED) != 0
    }
}

/// Complete delta schema for one GoldSrc wire struct (e.g. `entity_state_t`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaSchema {
    pub name: String,
    pub encoder: Option<String>,
    pub fields: Vec<DeltaFieldDesc>,
}

impl DeltaSchema {
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    pub fn zero_record(&self) -> DeltaRecord {
        let values = self
            .fields
            .iter()
            .map(|f| match f.base_type() {
                DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG => DeltaValue::Float(0.0),
                DT_STRING => DeltaValue::Str(String::new()),
                _ if f.is_signed() => DeltaValue::Int(0),
                _ => DeltaValue::Uint(0),
            })
            .collect();
        DeltaRecord { values }
    }
}

/// Dynamic field value stored in a `DeltaRecord`.
#[derive(Debug, Clone, PartialEq)]
pub enum DeltaValue {
    Int(i32),
    Uint(u32),
    Float(f32),
    Str(String),
}

impl DeltaValue {
    pub fn as_f32(&self) -> f32 {
        match *self {
            Self::Float(v) => v,
            Self::Int(v) => v as f32,
            Self::Uint(v) => v as f32,
            Self::Str(_) => 0.0,
        }
    }

    pub fn as_i32(&self) -> i32 {
        match *self {
            Self::Int(v) => v,
            Self::Uint(v) => v as i32,
            Self::Float(v) => v as i32,
            Self::Str(_) => 0,
        }
    }

    pub fn as_u32(&self) -> u32 {
        match *self {
            Self::Uint(v) => v,
            Self::Int(v) => v as u32,
            Self::Float(v) => v as u32,
            Self::Str(_) => 0,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Str(s) => s.as_str(),
            _ => "",
        }
    }

    /// GoldSrc `DELTA_MarkSendFields` equality semantics (`delta.cpp:526-568`).
    fn equals_wire(&self, other: &Self, field: &DeltaFieldDesc) -> bool {
        match field.base_type() {
            DT_BYTE => (self.as_u32() as u8) == (other.as_u32() as u8),
            DT_SHORT => (self.as_u32() as u16) == (other.as_u32() as u16),
            DT_INTEGER => self.as_u32() == other.as_u32(),
            DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG => {
                self.as_f32().to_bits() == other.as_f32().to_bits()
            }
            DT_STRING => self.as_str().eq_ignore_ascii_case(other.as_str()),
            _ => true,
        }
    }
}

/// Ordered field values matching a `DeltaSchema`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DeltaRecord {
    pub values: Vec<DeltaValue>,
}

impl DeltaRecord {
    pub fn set_u32(&mut self, schema: &DeltaSchema, name: &str, v: u32) {
        if let Some(idx) = schema.field_index(name) {
            if schema.fields[idx].is_signed() {
                self.values[idx] = DeltaValue::Int(v as i32);
            } else {
                self.values[idx] = DeltaValue::Uint(v);
            }
        }
    }

    pub fn set_i32(&mut self, schema: &DeltaSchema, name: &str, v: i32) {
        if let Some(idx) = schema.field_index(name) {
            if schema.fields[idx].is_signed() {
                self.values[idx] = DeltaValue::Int(v);
            } else {
                self.values[idx] = DeltaValue::Uint(v as u32);
            }
        }
    }

    pub fn set_f32(&mut self, schema: &DeltaSchema, name: &str, v: f32) {
        if let Some(idx) = schema.field_index(name) {
            self.values[idx] = DeltaValue::Float(v);
        }
    }

    pub fn set_str(&mut self, schema: &DeltaSchema, name: &str, v: impl Into<String>) {
        if let Some(idx) = schema.field_index(name) {
            self.values[idx] = DeltaValue::Str(v.into());
        }
    }

    pub fn get_f32(&self, schema: &DeltaSchema, name: &str) -> f32 {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or(0.0, DeltaValue::as_f32)
    }

    pub fn get_i32(&self, schema: &DeltaSchema, name: &str) -> i32 {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or(0, DeltaValue::as_i32)
    }

    pub fn get_u32(&self, schema: &DeltaSchema, name: &str) -> u32 {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or(0, DeltaValue::as_u32)
    }

    pub fn get_str<'a>(&'a self, schema: &DeltaSchema, name: &str) -> &'a str {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or("", DeltaValue::as_str)
    }
}

/// Built-in `g_MetaDelta` (`delta.cpp:83-92`) used to serialize and deserialize
/// `svc_deltadescription` schemas over the wire.
pub fn meta_delta_schema() -> DeltaSchema {
    DeltaSchema {
        name: "delta_description_t".to_string(),
        encoder: None,
        fields: vec![
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "fieldType".to_string(),
                field_offset: 0,
                field_size: 1,
                significant_bits: 32,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_STRING,
                name: "fieldName".to_string(),
                field_offset: 4,
                field_size: 1,
                significant_bits: 1,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "fieldOffset".to_string(),
                field_offset: 36,
                field_size: 1,
                significant_bits: 16,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "fieldSize".to_string(),
                field_offset: 40,
                field_size: 1,
                significant_bits: 8,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "significant_bits".to_string(),
                field_offset: 44,
                field_size: 1,
                significant_bits: 8,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_FLOAT,
                name: "premultiply".to_string(),
                field_offset: 48,
                field_size: 1,
                significant_bits: 32,
                premultiply: 4000.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_FLOAT,
                name: "postmultiply".to_string(),
                field_offset: 52,
                field_size: 1,
                significant_bits: 32,
                premultiply: 4000.0,
                postmultiply: 1.0,
            },
        ],
    }
}

/// Encode a `DeltaRecord` diff (`DELTA_WriteDelta`, `delta.cpp:622-865`) into an active
/// bit-writing `MsgWriter`. Returns `true` if any fields differed (or `force` was set).
pub fn write_delta(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    from: &DeltaRecord,
    to: &DeltaRecord,
    force: bool,
    time_base: f64,
) -> bool {
    let mut mask: u64 = 0;
    let mut last_bit: Option<usize> = None;

    for (i, field) in schema.fields.iter().enumerate() {
        let changed = !from.values[i].equals_wire(&to.values[i], field);
        if changed {
            mask |= 1u64 << i;
            last_bit = Some(i);
        }
    }

    if last_bit.is_none() && !force {
        return false;
    }

    let byte_count = match last_bit {
        Some(lb) => (lb >> 3) + 1,
        None => 0,
    };

    w.write_bits(byte_count as u32, 3);
    let mask_bytes = mask.to_le_bytes();
    for &b in &mask_bytes[..byte_count] {
        w.write_bits(b as u32, 8);
    }

    for (i, field) in schema.fields.iter().enumerate() {
        if (mask & (1u64 << i)) == 0 {
            continue;
        }
        let bits = field.significant_bits as u32;
        let pre = field.premultiply as f64;
        let signed = field.is_signed();

        match field.base_type() {
            DT_BYTE => {
                if signed {
                    let v = ((to.values[i].as_i32() as i8) as f64 * pre) as i8;
                    w.write_sbits(v as i32, bits);
                } else {
                    let v = ((to.values[i].as_u32() as u8) as f64 * pre) as u8;
                    w.write_bits(v as u32, bits);
                }
            }
            DT_SHORT => {
                if signed {
                    let v = ((to.values[i].as_i32() as i16) as f64 * pre) as i16;
                    w.write_sbits(v as i32, bits);
                } else {
                    let v = ((to.values[i].as_u32() as u16) as f64 * pre) as u16;
                    w.write_bits(v as u32, bits);
                }
            }
            DT_FLOAT => {
                let val = (to.values[i].as_f32() as f64) * pre;
                if signed {
                    w.write_sbits(val as i32, bits);
                } else {
                    w.write_bits(val as u32, bits);
                }
            }
            DT_INTEGER => {
                if signed {
                    let mut v = to.values[i].as_i32();
                    if !(0.9999..=1.0001).contains(&pre) {
                        v = (v as f64 * pre) as i32;
                    }
                    w.write_sbits(v, bits);
                } else {
                    let mut v = to.values[i].as_u32();
                    if !(0.9999..=1.0001).contains(&pre) {
                        v = (v as f64 * pre) as u32;
                    }
                    w.write_bits(v, bits);
                }
            }
            DT_ANGLE => {
                w.write_bit_angle(to.values[i].as_f32(), bits);
            }
            DT_TIMEWINDOW_8 => {
                let f2 = to.values[i].as_f32() as f64;
                let tw = (time_base * 100.0) as i32 - (f2 * 100.0) as i32;
                w.write_sbits(tw, 8);
            }
            DT_TIMEWINDOW_BIG => {
                let f2 = to.values[i].as_f32() as f64;
                let tw = (time_base * pre) as i32 - (f2 * pre) as i32;
                w.write_sbits(tw, bits);
            }
            DT_STRING => {
                w.write_bit_string(to.values[i].as_str());
            }
            _ => {}
        }
    }

    last_bit.is_some()
}

/// Check whether two records differ without writing anything (`DELTA_CheckDelta`).
pub fn has_delta_changes(schema: &DeltaSchema, from: &DeltaRecord, to: &DeltaRecord) -> bool {
    schema
        .fields
        .iter()
        .enumerate()
        .any(|(i, field)| !from.values[i].equals_wire(&to.values[i], field))
}

/// Decode a `DeltaRecord` diff (`DELTA_ParseDelta`, `delta.cpp:867-1069`) from an active
/// bit-reading `MsgReader`.
pub fn read_delta(
    r: &mut MsgReader<'_>,
    schema: &DeltaSchema,
    from: &DeltaRecord,
    time_base: f64,
) -> Result<DeltaRecord, ProtoError> {
    let mut to = from.clone();
    if to.values.len() < schema.fields.len() {
        to = schema.zero_record();
    }

    let nbytes = r.read_bits(3) as usize;
    if r.is_bad_read() {
        return Err(ProtoError::UnexpectedEof);
    }
    let mut mask_bytes = [0u8; 8];
    for slot in mask_bytes.iter_mut().take(nbytes.min(8)) {
        *slot = r.read_bits(8) as u8;
    }
    if r.is_bad_read() {
        return Err(ProtoError::UnexpectedEof);
    }
    let mask = u64::from_le_bytes(mask_bytes);

    for (i, field) in schema.fields.iter().enumerate() {
        if i >= 64 || (mask & (1u64 << i)) == 0 {
            continue;
        }
        let bits = field.significant_bits as u32;
        let pre = field.premultiply as f64;
        let post = field.postmultiply as f64;
        let signed = field.is_signed();

        let scale = |mut d: f64| -> f64 {
            if !(0.9999..=1.0001).contains(&pre) && pre != 0.0 {
                d /= pre;
            }
            if !(0.9999..=1.0001).contains(&post) {
                d *= post;
            }
            d
        };

        match field.base_type() {
            DT_BYTE => {
                if signed {
                    let d = scale(r.read_sbits(bits) as f64);
                    to.values[i] = DeltaValue::Int((d as i8) as i32);
                } else {
                    let d = scale(r.read_bits(bits) as f64);
                    to.values[i] = DeltaValue::Uint((d as u8) as u32);
                }
            }
            DT_SHORT => {
                if signed {
                    let d = scale(r.read_sbits(bits) as f64);
                    to.values[i] = DeltaValue::Int((d as i16) as i32);
                } else {
                    let d = scale(r.read_bits(bits) as f64);
                    to.values[i] = DeltaValue::Uint((d as u16) as u32);
                }
            }
            DT_FLOAT => {
                let raw = if signed {
                    r.read_sbits(bits) as f64
                } else {
                    r.read_bits(bits) as f64
                };
                to.values[i] = DeltaValue::Float(scale(raw) as f32);
            }
            DT_INTEGER => {
                if signed {
                    let d = scale(r.read_sbits(bits) as f64);
                    to.values[i] = DeltaValue::Int(d as i32);
                } else {
                    let d = scale(r.read_bits(bits) as f64);
                    to.values[i] = DeltaValue::Uint(d as u32);
                }
            }
            DT_ANGLE => {
                to.values[i] = DeltaValue::Float(r.read_bit_angle(bits));
            }
            DT_TIMEWINDOW_8 => {
                let addt = r.read_sbits(8) as f64;
                let t = (time_base * 100.0 - addt) / 100.0;
                to.values[i] = DeltaValue::Float(t as f32);
            }
            DT_TIMEWINDOW_BIG => {
                let addt = r.read_sbits(bits) as f64;
                let t = if !(0.9999..=1.0001).contains(&pre) && pre != 0.0 {
                    (time_base * pre - addt) / pre
                } else {
                    time_base - addt
                };
                to.values[i] = DeltaValue::Float(t as f32);
            }
            DT_STRING => {
                to.values[i] = DeltaValue::Str(r.read_bit_string());
            }
            _ => {}
        }

        if r.is_bad_read() {
            return Err(ProtoError::UnexpectedEof);
        }
    }

    Ok(to)
}

/// Write a `svc_deltadescription` body (after the opcode byte) using `g_MetaDelta`
/// (`SV_WriteDeltaDescriptionsToClient`, `sv_main.cpp:1054-1067`).
pub fn write_delta_description(w: &mut MsgWriter, schema: &DeltaSchema) {
    let meta = meta_delta_schema();
    let null_rec = meta.zero_record();
    w.write_str(&schema.name);
    w.start_bit_writing();
    w.write_bits(schema.fields.len() as u32, 16);
    for field in &schema.fields {
        let mut rec = meta.zero_record();
        rec.set_u32(&meta, "fieldType", field.field_type);
        rec.set_str(&meta, "fieldName", &field.name);
        rec.set_u32(&meta, "fieldOffset", field.field_offset as u32);
        rec.set_u32(&meta, "fieldSize", field.field_size as u32);
        rec.set_u32(&meta, "significant_bits", field.significant_bits as u32);
        rec.set_f32(&meta, "premultiply", field.premultiply);
        rec.set_f32(&meta, "postmultiply", field.postmultiply);
        write_delta(w, &meta, &null_rec, &rec, true, 0.0);
    }
    w.end_bit_writing();
}

/// Read a `svc_deltadescription` body (after the opcode byte) using `g_MetaDelta`.
pub fn read_delta_description(r: &mut MsgReader<'_>) -> Result<DeltaSchema, ProtoError> {
    let meta = meta_delta_schema();
    let null_rec = meta.zero_record();
    let name = r.read_str()?;
    r.start_bit_reading();
    let count = r.read_bits(16) as usize;
    if count > 256 {
        return Err(ProtoError::Delta(format!(
            "implausible delta field count {count} for {name}"
        )));
    }
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let rec = read_delta(r, &meta, &null_rec, 0.0)?;
        fields.push(DeltaFieldDesc {
            field_type: rec.get_u32(&meta, "fieldType"),
            name: rec.get_str(&meta, "fieldName").to_string(),
            field_offset: rec.get_u32(&meta, "fieldOffset") as u16,
            field_size: rec.get_u32(&meta, "fieldSize") as u8,
            significant_bits: rec.get_u32(&meta, "significant_bits") as u8,
            premultiply: rec.get_f32(&meta, "premultiply"),
            postmultiply: rec.get_f32(&meta, "postmultiply"),
        });
    }
    r.end_bit_reading()?;
    Ok(DeltaSchema {
        name,
        encoder: None,
        fields,
    })
}

/// Parse Valve's `cstrike/delta.lst` or `valve/delta.lst` file into `DeltaSchema`s.
pub fn parse_delta_lst(source: &str) -> Result<Vec<DeltaSchema>, ProtoError> {
    // Strip `//` line comments first.
    let mut cleaned = String::with_capacity(source.len());
    for line in source.lines() {
        let stripped = match line.find("//") {
            Some(pos) => &line[..pos],
            None => line,
        };
        cleaned.push_str(stripped);
        cleaned.push('\n');
    }

    let mut schemas = Vec::new();
    let mut rest = cleaned.as_str();

    while let Some(open_brace) = rest.find('{') {
        let header = rest[..open_brace].trim();
        let after_open = &rest[open_brace + 1..];
        let close_brace = after_open
            .find('}')
            .ok_or_else(|| ProtoError::Delta("unclosed '{' in delta.lst".to_string()))?;
        let body = &after_open[..close_brace];
        rest = &after_open[close_brace + 1..];

        let mut header_tokens = header.split_whitespace();
        let Some(struct_name) = header_tokens.next() else {
            continue;
        };
        let encoder = match header_tokens.next() {
            Some("none") | None => None,
            Some("gamedll" | "clientdll") => header_tokens.next().map(str::to_string),
            Some(other) => Some(other.to_string()),
        };

        let mut fields = Vec::new();
        let mut cursor = body;
        while let Some(pos) = cursor.find("DEFINE_DELTA") {
            let after_macro = &cursor[pos..];
            let is_post = after_macro.starts_with("DEFINE_DELTA_POST");
            let Some(lparen) = after_macro.find('(') else {
                break;
            };
            let Some(rparen) = after_macro[lparen + 1..].find(')') else {
                break;
            };
            let args_str = &after_macro[lparen + 1..lparen + 1 + rparen];
            cursor = &after_macro[lparen + 1 + rparen + 1..];

            let args: Vec<&str> = args_str.split(',').map(str::trim).collect();
            if args.len() < 4 {
                continue;
            }
            let field_name = args[0].to_string();
            let field_type = parse_dt_flags(args[1])?;
            let significant_bits: u8 = args[2]
                .parse()
                .map_err(|_| ProtoError::Delta(format!("bad bits '{}' in {field_name}", args[2])))?;
            let premultiply: f32 = args[3].parse().map_err(|_| {
                ProtoError::Delta(format!("bad premultiply '{}' in {field_name}", args[3]))
            })?;
            let postmultiply: f32 = if is_post && args.len() >= 5 {
                args[4].parse().map_err(|_| {
                    ProtoError::Delta(format!("bad postmultiply '{}' in {field_name}", args[4]))
                })?
            } else {
                1.0
            };

            // The real `offsetof` of the member in the original client's C struct: the client
            // stores each decoded value there. An unknown struct or member is a hard error,
            // like ReHLDS `DELTA_FindOffset`.
            let layout = field_layout(struct_name, &field_name).ok_or_else(|| {
                ProtoError::Delta(format!(
                    "no C struct offset for field '{field_name}' of delta struct '{struct_name}'"
                ))
            })?;

            fields.push(DeltaFieldDesc {
                field_type,
                name: field_name,
                field_offset: layout.offset,
                // ReHLDS `DELTA_ParseField` sets `fieldSize = 1` for every parsed field.
                field_size: 1,
                significant_bits,
                premultiply,
                postmultiply,
            });
        }

        schemas.push(DeltaSchema {
            name: struct_name.to_string(),
            encoder,
            fields,
        });
    }

    Ok(schemas)
}

fn parse_dt_flags(expr: &str) -> Result<u32, ProtoError> {
    let mut flags = 0u32;
    for part in expr.split('|').map(str::trim) {
        match part {
            "DT_BYTE" => flags |= DT_BYTE,
            "DT_SHORT" => flags |= DT_SHORT,
            "DT_FLOAT" => flags |= DT_FLOAT,
            "DT_INTEGER" => flags |= DT_INTEGER,
            "DT_ANGLE" => flags |= DT_ANGLE,
            "DT_TIMEWINDOW_8" => flags |= DT_TIMEWINDOW_8,
            "DT_TIMEWINDOW_BIG" => flags |= DT_TIMEWINDOW_BIG,
            "DT_STRING" => flags |= DT_STRING,
            "DT_SIGNED" => flags |= DT_SIGNED,
            "" => {}
            other => {
                return Err(ProtoError::Delta(format!("unknown delta flag '{other}'")));
            }
        }
    }
    Ok(flags)
}

/// Registry holding all active GoldSrc delta schemas for a session (loaded either from
/// `cstrike/delta.lst` on the server or from `svc_deltadescription` packets on the client).
#[derive(Debug, Default, Clone)]
pub struct DeltaRegistry {
    schemas: HashMap<String, DeltaSchema>,
    order: Vec<String>,
}

impl DeltaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry from Valve's `delta.lst` text.
    pub fn from_delta_lst(source: &str) -> Result<Self, ProtoError> {
        let mut reg = Self::new();
        for schema in parse_delta_lst(source)? {
            reg.insert(schema);
        }
        Ok(reg)
    }

    /// Built-in CS 1.6 default delta registry (matches Valve `cstrike/delta.lst`).
    pub fn cs16_default() -> Self {
        Self::from_delta_lst(CS16_DEFAULT_DELTA_LST).expect("valid built-in CS 1.6 delta.lst")
    }

    pub fn insert(&mut self, schema: DeltaSchema) {
        let name = schema.name.clone();
        if !self.schemas.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.schemas.insert(name, schema);
    }

    pub fn get(&self, name: &str) -> Option<&DeltaSchema> {
        self.schemas.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &DeltaSchema> {
        self.order.iter().filter_map(|k| self.schemas.get(k))
    }
}

/// High-level GoldSrc `usercmd_t` (`common/usercmd.h`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GoldSrcUserCmd {
    pub lerp_msec: i16,
    pub msec: u8,
    pub viewangles: Vec3,
    pub forwardmove: f32,
    pub sidemove: f32,
    pub upmove: f32,
    pub lightlevel: u8,
    pub buttons: u16,
    pub impulse: u8,
    pub weaponselect: u8,
    pub impact_index: i32,
    pub impact_position: Vec3,
}

impl GoldSrcUserCmd {
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        let mut rec = schema.zero_record();
        rec.set_i32(schema, "lerp_msec", self.lerp_msec as i32);
        rec.set_u32(schema, "msec", self.msec as u32);
        rec.set_f32(schema, "viewangles[0]", self.viewangles.x);
        rec.set_f32(schema, "viewangles[1]", self.viewangles.y);
        rec.set_f32(schema, "viewangles[2]", self.viewangles.z);
        rec.set_f32(schema, "forwardmove", self.forwardmove);
        rec.set_f32(schema, "sidemove", self.sidemove);
        rec.set_f32(schema, "upmove", self.upmove);
        rec.set_u32(schema, "lightlevel", self.lightlevel as u32);
        rec.set_u32(schema, "buttons", self.buttons as u32);
        rec.set_u32(schema, "impulse", self.impulse as u32);
        rec.set_u32(schema, "weaponselect", self.weaponselect as u32);
        rec.set_i32(schema, "impact_index", self.impact_index);
        rec.set_f32(schema, "impact_position[0]", self.impact_position.x);
        rec.set_f32(schema, "impact_position[1]", self.impact_position.y);
        rec.set_f32(schema, "impact_position[2]", self.impact_position.z);
        rec
    }

    pub fn from_delta(rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        Self {
            lerp_msec: rec.get_i32(schema, "lerp_msec") as i16,
            msec: rec.get_u32(schema, "msec") as u8,
            viewangles: Vec3::new(
                rec.get_f32(schema, "viewangles[0]"),
                rec.get_f32(schema, "viewangles[1]"),
                rec.get_f32(schema, "viewangles[2]"),
            ),
            forwardmove: rec.get_f32(schema, "forwardmove"),
            sidemove: rec.get_f32(schema, "sidemove"),
            upmove: rec.get_f32(schema, "upmove"),
            lightlevel: rec.get_u32(schema, "lightlevel") as u8,
            buttons: rec.get_u32(schema, "buttons") as u16,
            impulse: rec.get_u32(schema, "impulse") as u8,
            weaponselect: rec.get_u32(schema, "weaponselect") as u8,
            impact_index: rec.get_i32(schema, "impact_index"),
            impact_position: Vec3::new(
                rec.get_f32(schema, "impact_position[0]"),
                rec.get_f32(schema, "impact_position[1]"),
                rec.get_f32(schema, "impact_position[2]"),
            ),
        }
    }
}

/// High-level GoldSrc `clientdata_t` (`common/entity_state.h`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GoldSrcClientData {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub viewmodel: i32,
    pub punchangle: Vec3,
    pub flags: u32,
    pub waterlevel: i32,
    pub view_ofs: Vec3,
    pub health: f32,
    pub b_in_duck: i32,
    pub weapons: u32,
    /// `ammo_nails`, `ammo_shells`, `ammo_cells`, `ammo_rockets`: 10 signed bits each on the
    /// wire (the HL ammo counters the stock client reads; CS sends its ammo in `AmmoX`).
    pub ammo_nails: i32,
    pub ammo_shells: i32,
    pub ammo_cells: i32,
    pub ammo_rockets: i32,
    pub fl_time_step_sound: i32,
    pub fl_duck_time: i32,
    pub maxspeed: f32,
    pub m_i_id: i32,
    pub m_fl_next_attack: f32,
    pub fov: f32,
    pub weaponanim: i32,
    pub deadflag: i32,
    pub iuser1: i32,
    pub iuser2: i32,
    pub iuser3: i32,
    pub iuser4: i32,
    pub physinfo: String,
    /// `flSwimTime` (10 bits on the wire).
    pub fl_swim_time: i32,
    /// `waterjumptime` (15 bits).
    pub waterjumptime: i32,
    /// `pushmsec` (11 bits).
    pub pushmsec: i32,
    pub fuser1: f32,
    pub fuser2: f32,
    pub fuser3: f32,
    pub vuser2: Vec3,
    pub vuser3: Vec3,
    /// Only axes 0 and 1 are in the schema; `from_delta` gives axis 2 as 0.
    pub vuser4: Vec3,
}

impl GoldSrcClientData {
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        let mut rec = schema.zero_record();
        rec.set_f32(schema, "origin[0]", self.origin.x);
        rec.set_f32(schema, "origin[1]", self.origin.y);
        rec.set_f32(schema, "origin[2]", self.origin.z);
        rec.set_f32(schema, "velocity[0]", self.velocity.x);
        rec.set_f32(schema, "velocity[1]", self.velocity.y);
        rec.set_f32(schema, "velocity[2]", self.velocity.z);
        rec.set_i32(schema, "viewmodel", self.viewmodel);
        rec.set_f32(schema, "punchangle[0]", self.punchangle.x);
        rec.set_f32(schema, "punchangle[1]", self.punchangle.y);
        rec.set_f32(schema, "punchangle[2]", self.punchangle.z);
        rec.set_u32(schema, "flags", self.flags);
        rec.set_i32(schema, "waterlevel", self.waterlevel);
        rec.set_f32(schema, "view_ofs[2]", self.view_ofs.z);
        rec.set_f32(schema, "health", self.health);
        rec.set_i32(schema, "bInDuck", self.b_in_duck);
        rec.set_u32(schema, "weapons", self.weapons);
        rec.set_i32(schema, "ammo_nails", self.ammo_nails);
        rec.set_i32(schema, "ammo_shells", self.ammo_shells);
        rec.set_i32(schema, "ammo_cells", self.ammo_cells);
        rec.set_i32(schema, "ammo_rockets", self.ammo_rockets);
        rec.set_i32(schema, "flTimeStepSound", self.fl_time_step_sound);
        rec.set_i32(schema, "flDuckTime", self.fl_duck_time);
        rec.set_f32(schema, "maxspeed", self.maxspeed);
        rec.set_i32(schema, "m_iId", self.m_i_id);
        rec.set_f32(schema, "m_flNextAttack", self.m_fl_next_attack);
        rec.set_f32(schema, "fov", self.fov);
        rec.set_i32(schema, "weaponanim", self.weaponanim);
        rec.set_i32(schema, "deadflag", self.deadflag);
        rec.set_i32(schema, "iuser1", self.iuser1);
        rec.set_i32(schema, "iuser2", self.iuser2);
        rec.set_i32(schema, "iuser3", self.iuser3);
        rec.set_i32(schema, "iuser4", self.iuser4);
        rec.set_str(schema, "physinfo", &self.physinfo);
        rec.set_i32(schema, "flSwimTime", self.fl_swim_time);
        rec.set_i32(schema, "waterjumptime", self.waterjumptime);
        rec.set_i32(schema, "pushmsec", self.pushmsec);
        rec.set_f32(schema, "fuser1", self.fuser1);
        rec.set_f32(schema, "fuser2", self.fuser2);
        rec.set_f32(schema, "fuser3", self.fuser3);
        for (name, v) in [("vuser2", self.vuser2), ("vuser3", self.vuser3)] {
            rec.set_f32(schema, &format!("{name}[0]"), v.x);
            rec.set_f32(schema, &format!("{name}[1]"), v.y);
            rec.set_f32(schema, &format!("{name}[2]"), v.z);
        }
        rec.set_f32(schema, "vuser4[0]", self.vuser4.x);
        rec.set_f32(schema, "vuser4[1]", self.vuser4.y);
        rec
    }

    pub fn from_delta(rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        let vec3 = |name: &str| {
            Vec3::new(
                rec.get_f32(schema, &format!("{name}[0]")),
                rec.get_f32(schema, &format!("{name}[1]")),
                rec.get_f32(schema, &format!("{name}[2]")),
            )
        };
        Self {
            origin: Vec3::new(
                rec.get_f32(schema, "origin[0]"),
                rec.get_f32(schema, "origin[1]"),
                rec.get_f32(schema, "origin[2]"),
            ),
            velocity: Vec3::new(
                rec.get_f32(schema, "velocity[0]"),
                rec.get_f32(schema, "velocity[1]"),
                rec.get_f32(schema, "velocity[2]"),
            ),
            viewmodel: rec.get_i32(schema, "viewmodel"),
            punchangle: Vec3::new(
                rec.get_f32(schema, "punchangle[0]"),
                rec.get_f32(schema, "punchangle[1]"),
                rec.get_f32(schema, "punchangle[2]"),
            ),
            flags: rec.get_u32(schema, "flags"),
            waterlevel: rec.get_i32(schema, "waterlevel"),
            view_ofs: Vec3::new(0.0, 0.0, rec.get_f32(schema, "view_ofs[2]")),
            health: rec.get_f32(schema, "health"),
            b_in_duck: rec.get_i32(schema, "bInDuck"),
            weapons: rec.get_u32(schema, "weapons"),
            ammo_nails: rec.get_i32(schema, "ammo_nails"),
            ammo_shells: rec.get_i32(schema, "ammo_shells"),
            ammo_cells: rec.get_i32(schema, "ammo_cells"),
            ammo_rockets: rec.get_i32(schema, "ammo_rockets"),
            fl_time_step_sound: rec.get_i32(schema, "flTimeStepSound"),
            fl_duck_time: rec.get_i32(schema, "flDuckTime"),
            maxspeed: rec.get_f32(schema, "maxspeed"),
            m_i_id: rec.get_i32(schema, "m_iId"),
            m_fl_next_attack: rec.get_f32(schema, "m_flNextAttack"),
            fov: rec.get_f32(schema, "fov"),
            weaponanim: rec.get_i32(schema, "weaponanim"),
            deadflag: rec.get_i32(schema, "deadflag"),
            iuser1: rec.get_i32(schema, "iuser1"),
            iuser2: rec.get_i32(schema, "iuser2"),
            iuser3: rec.get_i32(schema, "iuser3"),
            iuser4: rec.get_i32(schema, "iuser4"),
            physinfo: rec.get_str(schema, "physinfo").to_string(),
            fl_swim_time: rec.get_i32(schema, "flSwimTime"),
            waterjumptime: rec.get_i32(schema, "waterjumptime"),
            pushmsec: rec.get_i32(schema, "pushmsec"),
            fuser1: rec.get_f32(schema, "fuser1"),
            fuser2: rec.get_f32(schema, "fuser2"),
            fuser3: rec.get_f32(schema, "fuser3"),
            vuser2: vec3("vuser2"),
            vuser3: vec3("vuser3"),
            vuser4: Vec3::new(
                rec.get_f32(schema, "vuser4[0]"),
                rec.get_f32(schema, "vuser4[1]"),
                0.0,
            ),
        }
    }
}

/// `weapon_data_t` (`common/weaponinfo.h`): the per-weapon state the server sends in
/// `svc_clientdata` and the client predicts with. Field names are the C names in snake_case.
/// The `weapon_data_t` delta schema carries 18 of the 22 fields (checked against the installed
/// `cstrike/delta.lst`, 2026-11-22): `iuser1` is the only `iuser`, and `iuser2..4` and `fuser4`
/// are not on the wire (`to_delta` leaves them out, `from_delta` gives them as 0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GoldSrcWeaponData {
    pub m_i_id: i32,
    pub m_i_clip: i32,
    pub m_fl_next_primary_attack: f32,
    pub m_fl_next_secondary_attack: f32,
    pub m_fl_time_weapon_idle: f32,
    pub m_f_in_reload: i32,
    pub m_f_in_special_reload: i32,
    pub m_fl_next_reload: f32,
    pub m_fl_pump_time: f32,
    pub m_f_reload_time: f32,
    pub m_f_aimed_damage: f32,
    pub m_f_next_aim_bonus: f32,
    pub m_f_in_zoom: i32,
    pub m_i_weapon_state: i32,
    pub iuser1: i32,
    pub iuser2: i32,
    pub iuser3: i32,
    pub iuser4: i32,
    pub fuser1: f32,
    pub fuser2: f32,
    pub fuser3: f32,
    pub fuser4: f32,
}

impl GoldSrcWeaponData {
    /// The record of the fields the schema has.
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        let mut rec = schema.zero_record();
        rec.set_i32(schema, "m_iId", self.m_i_id);
        rec.set_i32(schema, "m_iClip", self.m_i_clip);
        rec.set_f32(schema, "m_flNextPrimaryAttack", self.m_fl_next_primary_attack);
        rec.set_f32(schema, "m_flNextSecondaryAttack", self.m_fl_next_secondary_attack);
        rec.set_f32(schema, "m_flTimeWeaponIdle", self.m_fl_time_weapon_idle);
        rec.set_i32(schema, "m_fInReload", self.m_f_in_reload);
        rec.set_i32(schema, "m_fInSpecialReload", self.m_f_in_special_reload);
        rec.set_f32(schema, "m_flNextReload", self.m_fl_next_reload);
        rec.set_f32(schema, "m_flPumpTime", self.m_fl_pump_time);
        rec.set_f32(schema, "m_fReloadTime", self.m_f_reload_time);
        rec.set_f32(schema, "m_fAimedDamage", self.m_f_aimed_damage);
        rec.set_f32(schema, "m_fNextAimBonus", self.m_f_next_aim_bonus);
        rec.set_i32(schema, "m_fInZoom", self.m_f_in_zoom);
        rec.set_i32(schema, "m_iWeaponState", self.m_i_weapon_state);
        rec.set_i32(schema, "iuser1", self.iuser1);
        rec.set_i32(schema, "iuser2", self.iuser2);
        rec.set_i32(schema, "iuser3", self.iuser3);
        rec.set_i32(schema, "iuser4", self.iuser4);
        rec.set_f32(schema, "fuser1", self.fuser1);
        rec.set_f32(schema, "fuser2", self.fuser2);
        rec.set_f32(schema, "fuser3", self.fuser3);
        rec.set_f32(schema, "fuser4", self.fuser4);
        rec
    }

    pub fn from_delta(rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        Self {
            m_i_id: rec.get_i32(schema, "m_iId"),
            m_i_clip: rec.get_i32(schema, "m_iClip"),
            m_fl_next_primary_attack: rec.get_f32(schema, "m_flNextPrimaryAttack"),
            m_fl_next_secondary_attack: rec.get_f32(schema, "m_flNextSecondaryAttack"),
            m_fl_time_weapon_idle: rec.get_f32(schema, "m_flTimeWeaponIdle"),
            m_f_in_reload: rec.get_i32(schema, "m_fInReload"),
            m_f_in_special_reload: rec.get_i32(schema, "m_fInSpecialReload"),
            m_fl_next_reload: rec.get_f32(schema, "m_flNextReload"),
            m_fl_pump_time: rec.get_f32(schema, "m_flPumpTime"),
            m_f_reload_time: rec.get_f32(schema, "m_fReloadTime"),
            m_f_aimed_damage: rec.get_f32(schema, "m_fAimedDamage"),
            m_f_next_aim_bonus: rec.get_f32(schema, "m_fNextAimBonus"),
            m_f_in_zoom: rec.get_i32(schema, "m_fInZoom"),
            m_i_weapon_state: rec.get_i32(schema, "m_iWeaponState"),
            iuser1: rec.get_i32(schema, "iuser1"),
            iuser2: rec.get_i32(schema, "iuser2"),
            iuser3: rec.get_i32(schema, "iuser3"),
            iuser4: rec.get_i32(schema, "iuser4"),
            fuser1: rec.get_f32(schema, "fuser1"),
            fuser2: rec.get_f32(schema, "fuser2"),
            fuser3: rec.get_f32(schema, "fuser3"),
            fuser4: rec.get_f32(schema, "fuser4"),
        }
    }
}

/// High-level GoldSrc `entity_state_t` (`common/entity_state.h`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GoldSrcEntityState {
    pub number: u16,
    pub entity_type: u8,
    pub origin: Vec3,
    pub angles: Vec3,
    pub modelindex: u16,
    pub sequence: u16,
    pub gaitsequence: u16,
    pub frame: f32,
    pub animtime: f32,
    pub framerate: f32,
    pub weaponmodel: u16,
    pub movetype: u8,
    pub solid: u8,
    pub effects: u32,
    pub rendermode: u8,
    pub renderamt: u8,
    pub renderfx: u8,
    pub skin: i16,
    pub body: u8,
    pub scale: f32,
    pub owner: u16,
    pub aiment: u16,
    pub usehull: u8,
    pub spectator: bool,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub colormap: u16,
    /// `eflags` (`EFLAG_SLERP`: studio interpolation); 2 bits on the wire.
    pub eflags: u8,
    /// `rendercolor` (`color24`): red, green, blue.
    pub rendercolor: [u8; 3],
    /// The four bone controller positions a studio model is posed with.
    pub controller: [u8; 4],
    /// The two sequence blend weights; the client pitches a player model with `blending[0]`.
    /// (`entity_state_t::blending` has four bytes, the delta description names two.)
    pub blending: [u8; 2],
    /// Friction for prediction (`entity_state_player_t` only).
    pub friction: f32,
    /// Gravity multiplier for prediction (`entity_state_player_t` only).
    pub gravity: f32,
    /// Conveyor velocity the entity stands on.
    pub basevelocity: Vec3,
    /// Parametric movement overrides (`startpos`, `endpos`, `starttime`, `impacttime`).
    pub startpos: Vec3,
    pub endpos: Vec3,
    pub impacttime: f32,
    pub starttime: f32,
    pub iuser4: i32,
}

impl GoldSrcEntityState {
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        let mut rec = schema.zero_record();
        rec.set_f32(schema, "animtime", self.animtime);
        rec.set_f32(schema, "frame", self.frame);
        rec.set_f32(schema, "origin[0]", self.origin.x);
        rec.set_f32(schema, "origin[1]", self.origin.y);
        rec.set_f32(schema, "origin[2]", self.origin.z);
        rec.set_f32(schema, "angles[0]", self.angles.x);
        rec.set_f32(schema, "angles[1]", self.angles.y);
        rec.set_f32(schema, "angles[2]", self.angles.z);
        rec.set_u32(schema, "sequence", self.sequence as u32);
        rec.set_u32(schema, "gaitsequence", self.gaitsequence as u32);
        rec.set_u32(schema, "modelindex", self.modelindex as u32);
        rec.set_u32(schema, "movetype", self.movetype as u32);
        rec.set_u32(schema, "solid", self.solid as u32);
        rec.set_f32(schema, "mins[0]", self.mins.x);
        rec.set_f32(schema, "mins[1]", self.mins.y);
        rec.set_f32(schema, "mins[2]", self.mins.z);
        rec.set_f32(schema, "maxs[0]", self.maxs.x);
        rec.set_f32(schema, "maxs[1]", self.maxs.y);
        rec.set_f32(schema, "maxs[2]", self.maxs.z);
        rec.set_u32(schema, "weaponmodel", self.weaponmodel as u32);
        rec.set_u32(schema, "owner", self.owner as u32);
        rec.set_u32(schema, "effects", self.effects);
        rec.set_u32(schema, "colormap", self.colormap as u32);
        rec.set_f32(schema, "framerate", self.framerate);
        rec.set_i32(schema, "skin", self.skin as i32);
        rec.set_u32(schema, "body", self.body as u32);
        rec.set_u32(schema, "rendermode", self.rendermode as u32);
        rec.set_u32(schema, "renderamt", self.renderamt as u32);
        rec.set_u32(schema, "renderfx", self.renderfx as u32);
        rec.set_f32(schema, "scale", self.scale);
        rec.set_u32(schema, "aiment", self.aiment as u32);
        rec.set_u32(schema, "usehull", self.usehull as u32);
        rec.set_u32(schema, "spectator", u32::from(self.spectator));
        rec.set_u32(schema, "eflags", u32::from(self.eflags));
        rec.set_u32(schema, "rendercolor.r", u32::from(self.rendercolor[0]));
        rec.set_u32(schema, "rendercolor.g", u32::from(self.rendercolor[1]));
        rec.set_u32(schema, "rendercolor.b", u32::from(self.rendercolor[2]));
        for (i, c) in self.controller.iter().enumerate() {
            rec.set_u32(schema, &format!("controller[{i}]"), u32::from(*c));
        }
        for (i, b) in self.blending.iter().enumerate() {
            rec.set_u32(schema, &format!("blending[{i}]"), u32::from(*b));
        }
        rec.set_f32(schema, "friction", self.friction);
        rec.set_f32(schema, "gravity", self.gravity);
        for (i, axis) in ["0", "1", "2"].iter().enumerate() {
            rec.set_f32(schema, &format!("basevelocity[{axis}]"), self.basevelocity[i]);
            rec.set_f32(schema, &format!("startpos[{axis}]"), self.startpos[i]);
            rec.set_f32(schema, &format!("endpos[{axis}]"), self.endpos[i]);
        }
        rec.set_f32(schema, "impacttime", self.impacttime);
        rec.set_f32(schema, "starttime", self.starttime);
        rec.set_i32(schema, "iuser4", self.iuser4);
        rec
    }

    pub fn from_delta(number: u16, entity_type: u8, rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        Self {
            number,
            entity_type,
            origin: Vec3::new(
                rec.get_f32(schema, "origin[0]"),
                rec.get_f32(schema, "origin[1]"),
                rec.get_f32(schema, "origin[2]"),
            ),
            angles: Vec3::new(
                rec.get_f32(schema, "angles[0]"),
                rec.get_f32(schema, "angles[1]"),
                rec.get_f32(schema, "angles[2]"),
            ),
            modelindex: rec.get_u32(schema, "modelindex") as u16,
            sequence: rec.get_u32(schema, "sequence") as u16,
            gaitsequence: rec.get_u32(schema, "gaitsequence") as u16,
            frame: rec.get_f32(schema, "frame"),
            animtime: rec.get_f32(schema, "animtime"),
            framerate: rec.get_f32(schema, "framerate"),
            weaponmodel: rec.get_u32(schema, "weaponmodel") as u16,
            movetype: rec.get_u32(schema, "movetype") as u8,
            solid: rec.get_u32(schema, "solid") as u8,
            effects: rec.get_u32(schema, "effects"),
            rendermode: rec.get_u32(schema, "rendermode") as u8,
            renderamt: rec.get_u32(schema, "renderamt") as u8,
            renderfx: rec.get_u32(schema, "renderfx") as u8,
            skin: rec.get_i32(schema, "skin") as i16,
            body: rec.get_u32(schema, "body") as u8,
            scale: rec.get_f32(schema, "scale"),
            owner: rec.get_u32(schema, "owner") as u16,
            aiment: rec.get_u32(schema, "aiment") as u16,
            usehull: rec.get_u32(schema, "usehull") as u8,
            spectator: rec.get_u32(schema, "spectator") != 0,
            mins: Vec3::new(
                rec.get_f32(schema, "mins[0]"),
                rec.get_f32(schema, "mins[1]"),
                rec.get_f32(schema, "mins[2]"),
            ),
            maxs: Vec3::new(
                rec.get_f32(schema, "maxs[0]"),
                rec.get_f32(schema, "maxs[1]"),
                rec.get_f32(schema, "maxs[2]"),
            ),
            colormap: rec.get_u32(schema, "colormap") as u16,
            eflags: rec.get_u32(schema, "eflags") as u8,
            rendercolor: [
                rec.get_u32(schema, "rendercolor.r") as u8,
                rec.get_u32(schema, "rendercolor.g") as u8,
                rec.get_u32(schema, "rendercolor.b") as u8,
            ],
            controller: std::array::from_fn(|i| {
                rec.get_u32(schema, &format!("controller[{i}]")) as u8
            }),
            blending: std::array::from_fn(|i| {
                rec.get_u32(schema, &format!("blending[{i}]")) as u8
            }),
            friction: rec.get_f32(schema, "friction"),
            gravity: rec.get_f32(schema, "gravity"),
            basevelocity: Vec3::new(
                rec.get_f32(schema, "basevelocity[0]"),
                rec.get_f32(schema, "basevelocity[1]"),
                rec.get_f32(schema, "basevelocity[2]"),
            ),
            startpos: Vec3::new(
                rec.get_f32(schema, "startpos[0]"),
                rec.get_f32(schema, "startpos[1]"),
                rec.get_f32(schema, "startpos[2]"),
            ),
            endpos: Vec3::new(
                rec.get_f32(schema, "endpos[0]"),
                rec.get_f32(schema, "endpos[1]"),
                rec.get_f32(schema, "endpos[2]"),
            ),
            impacttime: rec.get_f32(schema, "impacttime"),
            starttime: rec.get_f32(schema, "starttime"),
            iuser4: rec.get_i32(schema, "iuser4"),
        }
    }
}

/// Built-in CS 1.6 `delta.lst` schema matching Valve's `cstrike/delta.lst`.
const CS16_DEFAULT_DELTA_LST: &str = r#"
clientdata_t none
{
	DEFINE_DELTA( flTimeStepSound, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 21, 128.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 21, 128.0 ),
	DEFINE_DELTA( velocity[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( velocity[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( m_flNextAttack, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 21, 128.0 ),
	DEFINE_DELTA( velocity[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( ammo_nails, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( ammo_shells, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( ammo_cells, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( ammo_rockets, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( m_iId, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( punchangle[2], DT_SIGNED | DT_FLOAT, 21, 8.0 ),
	DEFINE_DELTA( flags, DT_INTEGER, 32, 1.0 ),
	DEFINE_DELTA( weaponanim, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( health, DT_SIGNED | DT_FLOAT, 10, 1.0 ),
	DEFINE_DELTA( maxspeed, DT_FLOAT, 16, 10.0 ),
	DEFINE_DELTA( flDuckTime, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( view_ofs[2], DT_SIGNED | DT_FLOAT, 10, 4.0 ),
	DEFINE_DELTA( punchangle[0], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( punchangle[1], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( viewmodel, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( weapons, DT_INTEGER, 32, 1.0 ),
	DEFINE_DELTA( pushmsec, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( deadflag, DT_INTEGER, 3, 1.0 ),
	DEFINE_DELTA( fov, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA( physinfo, DT_STRING, 1, 1.0 ),
	DEFINE_DELTA( bInDuck, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( flSwimTime, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( waterjumptime, DT_INTEGER, 15, 1.0 ),
	DEFINE_DELTA( waterlevel, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( iuser1, DT_INTEGER, 3, 1.0 ),
	DEFINE_DELTA( iuser2, DT_INTEGER, 6, 1.0 ),
	DEFINE_DELTA( iuser3, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( iuser4, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( vuser2[0], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser2[1], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser2[2], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser3[0], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser3[1], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser3[2], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser4[0], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser4[1], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( fuser1, DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( fuser2, DT_FLOAT, 14, 1.0 ),
	DEFINE_DELTA( fuser3, DT_FLOAT, 10, 1.0 )
}

entity_state_t gamedll Entity_Encode
{
	DEFINE_DELTA( animtime, DT_TIMEWINDOW_8, 8, 1.0 ),
	DEFINE_DELTA( frame, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( angles[0], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( angles[1], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( sequence, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( modelindex, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( movetype, DT_INTEGER, 4, 1.0 ),
	DEFINE_DELTA( solid, DT_SHORT, 3, 1.0 ),
	DEFINE_DELTA( mins[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( endpos[0], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( endpos[1], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( endpos[2], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( startpos[0], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( startpos[1], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( startpos[2], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( impacttime, DT_TIMEWINDOW_BIG, 13, 100.0 ),
	DEFINE_DELTA( starttime, DT_TIMEWINDOW_BIG, 13, 100.0 ),
	DEFINE_DELTA( weaponmodel, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( owner, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( effects, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( eflags, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( angles[2], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( colormap, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( framerate, DT_SIGNED | DT_FLOAT, 8, 16.0 ),
	DEFINE_DELTA( skin, DT_SHORT | DT_SIGNED, 9, 1.0 ),
	DEFINE_DELTA( controller[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[2], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[3], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( body, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( rendermode, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderamt, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderfx, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( scale, DT_FLOAT, 16, 256.0 ),
	DEFINE_DELTA( rendercolor.r, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.g, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.b, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( aiment, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( basevelocity[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( iuser4, DT_INTEGER, 2, 1.0 )
}

entity_state_player_t gamedll Player_Encode
{
	DEFINE_DELTA( animtime, DT_TIMEWINDOW_8, 8, 1.0 ),
	DEFINE_DELTA( frame, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 18, 32.0 ),
	DEFINE_DELTA( angles[0], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( angles[1], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 18, 32.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 18, 32.0 ),
	DEFINE_DELTA( gaitsequence, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( sequence, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( modelindex, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( movetype, DT_INTEGER, 4, 1.0 ),
	DEFINE_DELTA( solid, DT_SHORT, 3, 1.0 ),
	DEFINE_DELTA( mins[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( weaponmodel, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( owner, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( effects, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( angles[2], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( colormap, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( framerate, DT_SIGNED | DT_FLOAT, 8, 16.0 ),
	DEFINE_DELTA( skin, DT_SHORT | DT_SIGNED, 9, 1.0 ),
	DEFINE_DELTA( controller[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[2], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[3], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( body, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( rendermode, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderamt, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderfx, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( scale, DT_FLOAT, 16, 256.0 ),
	DEFINE_DELTA( rendercolor.r, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.g, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.b, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( friction, DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( usehull, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( gravity, DT_SIGNED | DT_FLOAT, 16, 32.0 ),
	DEFINE_DELTA( aiment, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( basevelocity[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( spectator, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( iuser4, DT_INTEGER, 2, 1.0 )
}

custom_entity_state_t gamedll Custom_Encode
{
	DEFINE_DELTA( rendermode, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( angles[0], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( angles[1], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( angles[2], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( sequence, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( skin, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( modelindex, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA_POST( scale, DT_FLOAT, 8, 1.0, 0.1 ),
	DEFINE_DELTA( body, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.r, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.g, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.b, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( renderfx, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderamt, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( frame, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA_POST( animtime, DT_FLOAT, 8, 1.0, 0.1 )
}

usercmd_t none
{
	DEFINE_DELTA( lerp_msec, DT_SHORT, 9, 1.0 ),
	DEFINE_DELTA( msec, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( viewangles[1], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( viewangles[0], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( buttons, DT_SHORT, 16, 1.0 ),
	DEFINE_DELTA( forwardmove, DT_SIGNED | DT_FLOAT, 12, 1.0 ),
	DEFINE_DELTA( lightlevel, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( sidemove, DT_SIGNED | DT_FLOAT, 12, 1.0 ),
	DEFINE_DELTA( upmove, DT_SIGNED | DT_FLOAT, 12, 1.0 ),
	DEFINE_DELTA( impulse, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( viewangles[2], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( impact_index, DT_INTEGER, 6, 1.0 ),
	DEFINE_DELTA( impact_position[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( impact_position[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( impact_position[2], DT_SIGNED | DT_FLOAT, 16, 8.0 )
}

weapon_data_t none
{
	DEFINE_DELTA( m_flTimeWeaponIdle, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_flNextPrimaryAttack, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_flNextReload, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_fNextAimBonus, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_flNextSecondaryAttack, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_iClip, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( m_flPumpTime, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_fInSpecialReload, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( m_fReloadTime, DT_FLOAT, 16, 100.0 ),
	DEFINE_DELTA( m_fInReload, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( m_fAimedDamage, DT_FLOAT, 22, 1000.0 ),
	DEFINE_DELTA( m_fInZoom, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( m_iWeaponState, DT_INTEGER, 7, 1.0 ),
	DEFINE_DELTA( m_iId, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( fuser1, DT_SIGNED | DT_FLOAT, 22, 1000.0 ),
	DEFINE_DELTA( fuser2, DT_SIGNED | DT_FLOAT, 22, 128.0 ),
	DEFINE_DELTA( fuser3, DT_SIGNED | DT_FLOAT, 22, 128.0 ),
	DEFINE_DELTA( iuser1, DT_SIGNED | DT_INTEGER, 16, 128.0 )
}

event_t none
{
	DEFINE_DELTA( entindex, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( bparam1, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( bparam2, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( fparam1, DT_FLOAT | DT_SIGNED, 20, 100.0 ),
	DEFINE_DELTA( fparam2, DT_FLOAT | DT_SIGNED, 20, 100.0 ),
	DEFINE_DELTA( iparam1, DT_INTEGER | DT_SIGNED, 18, 1.0 ),
	DEFINE_DELTA( iparam2, DT_INTEGER | DT_SIGNED, 18, 1.0 ),
	DEFINE_DELTA( angles[0], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( angles[1], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( angles[2], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( ducking, DT_INTEGER, 1, 1.0 )
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clientdata_fields_round_trip() {
        use crate::messages::{
            SvcMessage, SvcParseContext, parse_svc_stream, write_svc_clientdata,
            write_svc_deltadescriptions,
        };

        let reg = DeltaRegistry::cs16_default();
        let schema = reg.get("clientdata_t").unwrap();
        // Values inside the widths the schema gives (flSwimTime 10 bits, waterjumptime 15,
        // pushmsec 11, fuser1 9, fuser2 14, fuser3 10, the vuser axes 9); all of them unsigned.
        let data = GoldSrcClientData {
            fl_swim_time: 321,
            waterjumptime: 12_345,
            pushmsec: 1_500,
            fuser1: 77.0,
            fuser2: 9_000.0,
            fuser3: 500.0,
            vuser2: Vec3::new(1.0, 2.0, 3.0),
            vuser3: Vec3::new(4.0, 5.0, 255.0),
            // The schema has no third axis for `vuser4`.
            vuser4: Vec3::new(6.0, 7.0, 0.0),
            ..Default::default()
        };
        // In memory: `to_delta` then `from_delta`.
        assert_eq!(GoldSrcClientData::from_delta(&data.to_delta(schema), schema), data);
        // A zero record leaves them all zero.
        let zero = GoldSrcClientData::from_delta(&schema.zero_record(), schema);
        assert_eq!(zero, GoldSrcClientData::default());

        // Over the wire: `svc_clientdata` as the server writes it and the client parses it.
        let mut w = MsgWriter::new();
        write_svc_deltadescriptions(&mut w, &reg);
        write_svc_clientdata(&mut w, schema, None, &data, 1.0);
        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        let got = parsed
            .iter()
            .find_map(|m| match m {
                SvcMessage::ClientData { parsed, .. } => Some(parsed.clone()),
                _ => None,
            })
            .expect("the clientdata message");
        assert_eq!(got, data);
    }

    #[test]
    fn parses_real_cs16_delta_lst_and_roundtrips_meta_wire_and_records() {
        let reg = DeltaRegistry::cs16_default();
        assert_eq!(reg.iter().count(), 7);
        let player_schema = reg.get("entity_state_player_t").unwrap();
        assert_eq!(player_schema.fields.len(), 48);

        // If Steam CS 1.6 is installed locally, also verify parsing Valve's real cstrike/delta.lst.
        let steam_delta = r"D:\SteamLibrary\steamapps\common\Half-Life\cstrike\delta.lst";
        if let Ok(raw) = std::fs::read_to_string(steam_delta) {
            let parsed = DeltaRegistry::from_delta_lst(&raw).unwrap();
            assert_eq!(parsed.iter().count(), 7);
            assert_eq!(
                parsed.get("entity_state_player_t").unwrap().fields.len(),
                48
            );
            assert_eq!(parsed.get("usercmd_t").unwrap().fields.len(), 15);
            assert_eq!(parsed.get("weapon_data_t").unwrap().fields.len(), 18);
        }

        // Round-trip a full schema through `svc_deltadescription` (`g_MetaDelta`).
        let mut w = MsgWriter::new();
        write_delta_description(&mut w, player_schema);
        let mut r = MsgReader::new(w.as_slice());
        let decoded_schema = read_delta_description(&mut r).unwrap();
        assert_eq!(decoded_schema.name, player_schema.name);
        assert_eq!(decoded_schema.fields.len(), player_schema.fields.len());
        for (a, b) in player_schema.fields.iter().zip(decoded_schema.fields.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.field_type, b.field_type);
            assert_eq!(a.significant_bits, b.significant_bits);
            assert!((a.premultiply - b.premultiply).abs() < 0.001);
            assert!((a.postmultiply - b.postmultiply).abs() < 0.001);
        }

        // Round-trip a `GoldSrcEntityState` delta diff.
        let from_ent = GoldSrcEntityState::default();
        let to_ent = GoldSrcEntityState {
            number: 1,
            entity_type: 1,
            origin: Vec3::new(-456.25, 1024.5, 64.0),
            angles: Vec3::new(12.5, 180.0, 0.0),
            modelindex: 42,
            sequence: 7,
            gaitsequence: 4,
            weaponmodel: 19,
            animtime: 10.25,
            ..Default::default()
        };
        let from_rec = from_ent.to_delta(&decoded_schema);
        let to_rec = to_ent.to_delta(&decoded_schema);

        let mut dw = MsgWriter::new();
        dw.start_bit_writing();
        assert!(write_delta(
            &mut dw,
            &decoded_schema,
            &from_rec,
            &to_rec,
            false,
            10.30
        ));
        dw.end_bit_writing();

        let mut dr = MsgReader::new(dw.as_slice());
        dr.start_bit_reading();
        let out_rec = read_delta(&mut dr, &decoded_schema, &from_rec, 10.30).unwrap();
        dr.end_bit_reading().unwrap();

        let decoded_ent = GoldSrcEntityState::from_delta(1, 1, &out_rec, &decoded_schema);
        assert!((decoded_ent.origin - to_ent.origin).length() < 0.05);
        assert!((decoded_ent.angles.y - 180.0).abs() < 0.02);
        assert_eq!(decoded_ent.modelindex, 42);
        assert_eq!(decoded_ent.sequence, 7);
        assert_eq!(decoded_ent.weaponmodel, 19);
        assert!((decoded_ent.animtime - 10.25).abs() < 0.02);
    }

    /// Every field the `entity_state_t` and `entity_state_player_t` delta descriptions name goes
    /// through the wire and back: the blending, controllers, colormap, friction, gravity,
    /// render colour, base velocity, the parametric movement and `iuser4`.
    #[test]
    fn entity_state_wire_round_trips_every_field() {
        let reg = DeltaRegistry::cs16_default();
        let full = GoldSrcEntityState {
            number: 1,
            entity_type: 1,
            colormap: 1,
            eflags: 1,
            rendercolor: [11, 22, 33],
            controller: [127, 128, 129, 130],
            blending: [128, 127],
            friction: 1.0,
            gravity: 1.0,
            basevelocity: Vec3::new(8.0, -16.0, 24.5),
            startpos: Vec3::new(100.0, -200.0, 300.0),
            endpos: Vec3::new(-100.0, 200.0, -300.0),
            impacttime: 12.5,
            starttime: 11.25,
            iuser4: 3,
            ..Default::default()
        };
        for (name, player) in [("entity_state_player_t", true), ("entity_state_t", false)] {
            let schema = reg.get(name).unwrap();
            let zero = GoldSrcEntityState::default().to_delta(schema);
            let to = full.to_delta(schema);
            let mut w = MsgWriter::new();
            w.start_bit_writing();
            assert!(write_delta(&mut w, schema, &zero, &to, true, 20.0));
            w.end_bit_writing();
            let mut r = MsgReader::new(w.as_slice());
            r.start_bit_reading();
            let out = read_delta(&mut r, schema, &zero, 20.0).unwrap();
            r.end_bit_reading().unwrap();
            let got = GoldSrcEntityState::from_delta(1, 1, &out, schema);
            assert_eq!(got.colormap, 1, "{name}");
            assert_eq!(got.rendercolor, [11, 22, 33], "{name}");
            assert_eq!(got.controller, [127, 128, 129, 130], "{name}");
            assert_eq!(got.blending, [128, 127], "{name}");
            assert_eq!(got.basevelocity, full.basevelocity, "{name}");
            assert_eq!(got.iuser4, 3, "{name}");
            if player {
                assert_eq!((got.friction, got.gravity), (1.0, 1.0));
                // `eflags` and the parametric movement are not in `entity_state_player_t`.
                assert_eq!(got.eflags, 0);
            } else {
                // Only `entity_state_player_t` names them.
                assert_eq!((got.friction, got.gravity), (0.0, 0.0));
                assert_eq!(got.eflags, 1);
                assert_eq!(got.startpos, full.startpos);
                assert_eq!(got.endpos, full.endpos);
                assert!((got.impacttime - 12.5).abs() < 0.02 && (got.starttime - 11.25).abs() < 0.02);
            }
        }
    }

    /// `svc_deltadescription` for `clientdata_t` carries the real C offsets of the 32-bit
    /// struct (`fieldOffset` of the meta delta), and the ReHLDS `fieldSize` of 1.
    #[test]
    fn clientdata_description_wire_carries_real_c_offsets() {
        let reg = DeltaRegistry::cs16_default();
        let schema = reg.get("clientdata_t").unwrap();
        let mut w = MsgWriter::new();
        write_delta_description(&mut w, schema);
        let mut r = MsgReader::new(w.as_slice());
        let on_wire = read_delta_description(&mut r).unwrap();
        assert_eq!(on_wire.name, "clientdata_t");
        let offset = |name: &str| {
            on_wire
                .fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("{name} not on the wire"))
                .field_offset
        };
        // The values IDA shows for the original client's clientdata_t.
        assert_eq!(offset("viewmodel"), 24);
        assert_eq!(offset("origin[0]"), 0);
        assert_eq!(offset("origin[2]"), 8);
        assert_eq!(offset("velocity[0]"), 12);
        assert_eq!(offset("punchangle[2]"), 36);
        assert_eq!(offset("view_ofs[2]"), 60);
        assert_eq!(offset("m_flNextAttack"), 124);
        assert_eq!(offset("physinfo"), 140);
        assert_eq!(offset("fuser3"), 420);
        assert_eq!(offset("vuser4[1]"), 468);
        assert!(on_wire.fields.iter().all(|f| f.field_size == 1));
        let entity = reg.get("entity_state_player_t").unwrap();
        assert_eq!(
            entity.fields.iter().find(|f| f.name == "modelindex").unwrap().field_offset,
            40
        );
    }

    /// Every described field of every built-in struct has a real, distinct-per-member offset.
    #[test]
    fn built_in_schemas_have_no_fake_offsets() {
        let reg = DeltaRegistry::cs16_default();
        for schema in reg.iter() {
            for f in &schema.fields {
                let layout = field_layout(&schema.name, &f.name)
                    .unwrap_or_else(|| panic!("{}.{} has no layout", schema.name, f.name));
                assert_eq!(f.field_offset, layout.offset, "{}.{}", schema.name, f.name);
            }
            let mut offsets: Vec<u16> = schema.fields.iter().map(|f| f.field_offset).collect();
            offsets.sort_unstable();
            offsets.dedup();
            assert_eq!(offsets.len(), schema.fields.len(), "{} reuses an offset", schema.name);
        }
    }

    /// The hard-coded meta delta agrees with the `delta_description_s` layout.
    #[test]
    fn meta_delta_offsets_match_the_c_struct() {
        for f in &meta_delta_schema().fields {
            assert_eq!(
                f.field_offset,
                field_layout("delta_description_t", &f.name).unwrap().offset,
                "{}",
                f.name
            );
        }
    }

    #[test]
    fn unknown_field_or_struct_is_a_parse_error() {
        let bad_field = "clientdata_t none
{
	DEFINE_DELTA( not_a_member, DT_INTEGER, 8, 1.0 )
}
";
        let err = parse_delta_lst(bad_field).unwrap_err().to_string();
        assert!(err.contains("not_a_member"), "{err}");
        let bad_struct = "mystery_t none
{
	DEFINE_DELTA( flags, DT_INTEGER, 8, 1.0 )
}
";
        assert!(parse_delta_lst(bad_struct).is_err());
    }
}
