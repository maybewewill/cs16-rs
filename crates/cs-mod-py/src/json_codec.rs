//! Zero-dependency RFC 8259 JSON encoder/decoder for the Python `json` module.

use crate::value::{Heap, HeapObj, PyKey, PyVal};

const MAX_JSON_DEPTH: usize = 32;

/// Serializes a [`PyVal`] into a JSON string (`json.dumps`).
pub fn json_dumps(
    heap: &Heap,
    val: &PyVal,
    indent: Option<usize>,
    sort_keys: bool,
) -> Result<String, String> {
    let mut out = String::new();
    encode_val(heap, val, indent, sort_keys, 0, &mut out)?;
    Ok(out)
}

fn encode_val(
    heap: &Heap,
    val: &PyVal,
    indent: Option<usize>,
    sort_keys: bool,
    depth: usize,
    out: &mut String,
) -> Result<(), String> {
    if depth > MAX_JSON_DEPTH {
        return Err("Circular reference or max nesting depth exceeded in JSON".to_string());
    }
    match val {
        PyVal::None => out.push_str("null"),
        PyVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        PyVal::Int(n) => out.push_str(&n.to_string()),
        PyVal::Float(f) => {
            if !f.is_finite() {
                return Err("Out of range float values are not JSON compliant".to_string());
            }
            let s = format!("{f}");
            out.push_str(&s);
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                out.push_str(".0");
            }
        }
        PyVal::Str(s) => encode_str(s, out),
        PyVal::Tuple(items) => {
            encode_seq(heap, items, indent, sort_keys, depth, out)?;
        }
        PyVal::Handle(id) => match heap.get(*id) {
            Some(HeapObj::List(items)) => {
                encode_seq(heap, items, indent, sort_keys, depth, out)?;
            }
            Some(HeapObj::Dict(entries)) => {
                encode_dict(heap, entries, indent, sort_keys, depth, out)?;
            }
            _ => {
                return Err("Object is not JSON serializable".to_string());
            }
        },
    }
    Ok(())
}

fn encode_str(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0C' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn encode_seq(
    heap: &Heap,
    items: &[PyVal],
    indent: Option<usize>,
    sort_keys: bool,
    depth: usize,
    out: &mut String,
) -> Result<(), String> {
    out.push('[');
    if !items.is_empty() {
        for (idx, it) in items.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            if let Some(ind) = indent {
                out.push('\n');
                for _ in 0..((depth + 1) * ind) {
                    out.push(' ');
                }
            } else if idx > 0 {
                out.push(' ');
            }
            encode_val(heap, it, indent, sort_keys, depth + 1, out)?;
        }
        if let Some(ind) = indent {
            out.push('\n');
            for _ in 0..(depth * ind) {
                out.push(' ');
            }
        }
    }
    out.push(']');
    Ok(())
}

fn encode_dict(
    heap: &Heap,
    entries: &[(PyKey, PyVal)],
    indent: Option<usize>,
    sort_keys: bool,
    depth: usize,
    out: &mut String,
) -> Result<(), String> {
    let mut pairs: Vec<(String, &PyVal)> = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let ks = match k {
            PyKey::Str(s) => s.clone(),
            PyKey::Int(n) => n.to_string(),
            PyKey::None => "null".to_string(),
            PyKey::FloatBits(b) => f64::from_bits(*b).to_string(),
            PyKey::Tuple(_) => return Err("keys must be str, int, float, bool or None".to_string()),
        };
        pairs.push((ks, v));
    }
    if sort_keys {
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
    }
    out.push('{');
    if !pairs.is_empty() {
        for (idx, (k, v)) in pairs.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            if let Some(ind) = indent {
                out.push('\n');
                for _ in 0..((depth + 1) * ind) {
                    out.push(' ');
                }
            } else if idx > 0 {
                out.push(' ');
            }
            encode_str(k, out);
            out.push_str(": ");
            encode_val(heap, v, indent, sort_keys, depth + 1, out)?;
        }
        if let Some(ind) = indent {
            out.push('\n');
            for _ in 0..(depth * ind) {
                out.push(' ');
            }
        }
    }
    out.push('}');
    Ok(())
}

/// Parses a JSON string into a [`PyVal`] (`json.loads`).
pub fn json_loads(heap: &mut Heap, src: &str) -> Result<PyVal, String> {
    let mut parser = JsonParser {
        chars: src.chars().collect(),
        pos: 0,
    };
    parser.skip_ws();
    let val = parser.parse_value(heap, 0)?;
    parser.skip_ws();
    if parser.pos < parser.chars.len() {
        return Err("Extra data after JSON value".to_string());
    }
    Ok(val)
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self, heap: &mut Heap, depth: usize) -> Result<PyVal, String> {
        if depth > MAX_JSON_DEPTH {
            return Err("JSON nesting depth exceeded".to_string());
        }
        self.skip_ws();
        match self.peek() {
            Some('n') => self.expect_lit("null").map(|()| PyVal::None),
            Some('t') => self.expect_lit("true").map(|()| PyVal::Bool(true)),
            Some('f') => self.expect_lit("false").map(|()| PyVal::Bool(false)),
            Some('"') => self.parse_string().map(PyVal::Str),
            Some('[') => self.parse_array(heap, depth + 1),
            Some('{') => self.parse_object(heap, depth + 1),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some(other) => Err(format!("Unexpected character '{other}' in JSON")),
            None => Err("Unexpected end of JSON input".to_string()),
        }
    }

    fn expect_lit(&mut self, lit: &str) -> Result<(), String> {
        for expected in lit.chars() {
            if self.bump() != Some(expected) {
                return Err(format!("Expected '{lit}' in JSON"));
            }
        }
        Ok(())
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.bump() != Some('"') {
            return Err("Expected '\"' in JSON string".to_string());
        }
        let mut out = String::new();
        while let Some(ch) = self.bump() {
            match ch {
                '"' => return Ok(out),
                '\\' => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('b') => out.push('\x08'),
                    Some('f') => out.push('\x0C'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('u') => {
                        let mut hex = String::with_capacity(4);
                        for _ in 0..4 {
                            let c = self.bump().ok_or("Incomplete \\u escape in JSON")?;
                            hex.push(c);
                        }
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| "Invalid \\u hex escape in JSON".to_string())?;
                        if let Some(u) = char::from_u32(code) {
                            out.push(u);
                        }
                    }
                    _ => return Err("Invalid escape in JSON string".to_string()),
                },
                c if (c as u32) < 0x20 => {
                    return Err("Control character in JSON string".to_string());
                }
                c => out.push(c),
            }
        }
        Err("Unterminated JSON string".to_string())
    }

    fn parse_number(&mut self) -> Result<PyVal, String> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.bump();
        }
        let mut is_float = false;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        if self.peek() == Some('.') {
            is_float = true;
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        if is_float {
            let f = s.parse::<f64>().map_err(|_| format!("Invalid JSON float `{s}`"))?;
            Ok(PyVal::Float(f))
        } else {
            let n = s.parse::<i64>().map_err(|_| format!("Invalid JSON int `{s}`"))?;
            Ok(PyVal::Int(n))
        }
    }

    fn parse_array(&mut self, heap: &mut Heap, depth: usize) -> Result<PyVal, String> {
        self.bump(); // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            let id = heap
                .alloc(HeapObj::List(items))
                .map_err(|l| format!("memory limit {l} exceeded"))?;
            return Ok(PyVal::Handle(id));
        }
        loop {
            items.push(self.parse_value(heap, depth)?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    break;
                }
                _ => return Err("Expected ',' or ']' in JSON array".to_string()),
            }
        }
        let id = heap
            .alloc(HeapObj::List(items))
            .map_err(|l| format!("memory limit {l} exceeded"))?;
        Ok(PyVal::Handle(id))
    }

    fn parse_object(&mut self, heap: &mut Heap, depth: usize) -> Result<PyVal, String> {
        self.bump(); // '{'
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            let id = heap
                .alloc(HeapObj::Dict(entries))
                .map_err(|l| format!("memory limit {l} exceeded"))?;
            return Ok(PyVal::Handle(id));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.bump() != Some(':') {
                return Err("Expected ':' in JSON object".to_string());
            }
            let val = self.parse_value(heap, depth)?;
            let pk = PyKey::Str(key);
            if let Some(pos) = entries.iter().position(|(k, _)| k == &pk) {
                entries[pos].1 = val;
            } else {
                entries.push((pk, val));
            }
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some('}') => {
                    self.bump();
                    break;
                }
                _ => return Err("Expected ',' or '}' in JSON object".to_string()),
            }
        }
        let id = heap
            .alloc(HeapObj::Dict(entries))
            .map_err(|l| format!("memory limit {l} exceeded"))?;
        Ok(PyVal::Handle(id))
    }
}
