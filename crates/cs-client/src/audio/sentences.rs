//! GoldSrc `sound/sentences.txt` & VOX speech concatenation parser.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct VoxWord {
    /// VFS-normalized relative sound path (without `sound/` prefix, e.g. `"hgrunt/take.wav"`).
    pub path: String,
    pub volume: f32,
    pub pitch: u8,
    pub start_pct: u8,
    pub end_pct: u8,
    pub time_compress: u8,
    pub pause_sec: f32,
}

#[derive(Debug, Clone, Copy)]
struct VoxMod {
    volume: f32,
    pitch: u8,
    start_pct: u8,
    end_pct: u8,
    time_compress: u8,
}

impl Default for VoxMod {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pitch: 100,
            start_pct: 0,
            end_pct: 100,
            time_compress: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SentenceTable {
    entries: Vec<(String, String)>,
    by_name: HashMap<String, usize>,
}

impl SentenceTable {
    pub fn parse(text: &str) -> Self {
        let mut table = Self::default();
        table.parse_append(text);
        table
    }

    pub fn parse_append(&mut self, text: &str) {
        for raw_line in text.lines() {
            let line = raw_line.split("//").next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some(split_at) = line.find(char::is_whitespace) else {
                continue;
            };
            let name = line[..split_at].trim().to_ascii_uppercase();
            let body = line[split_at..].trim().to_string();
            if name.is_empty() || body.is_empty() {
                continue;
            }
            if let Some(&idx) = self.by_name.get(&name) {
                self.entries[idx].1 = body;
            } else {
                let idx = self.entries.len();
                self.entries.push((name.clone(), body));
                self.by_name.insert(name, idx);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get_body(&self, name_or_ref: &str) -> Option<&str> {
        let trimmed = name_or_ref.trim().trim_start_matches('%');
        if let Some(rest) = trimmed.strip_prefix('!') {
            if let Ok(idx) = rest.parse::<usize>() {
                return self.entries.get(idx).map(|(_, b)| b.as_str());
            }
            let key = rest.to_ascii_uppercase();
            return self.by_name.get(&key).map(|&idx| self.entries[idx].1.as_str());
        }
        let key = trimmed.trim_start_matches('#').to_ascii_uppercase();
        self.by_name.get(&key).map(|&idx| self.entries[idx].1.as_str())
    }

    /// Resolve `!SENTENCE`, `!42`, `%!MRAD_*`, or raw VOX words (such as `spk "vox/hello world"`).
    pub fn resolve(&self, spec: &str) -> Option<Vec<VoxWord>> {
        let trimmed = spec.trim();
        if trimmed.is_empty() {
            return None;
        }
        let body = if trimmed.starts_with(['!', '#', '%']) {
            self.get_body(trimmed)?
        } else if let Some(found) = self.get_body(trimmed) {
            found
        } else {
            trimmed
        };
        let words = parse_vox_body(body);
        (!words.is_empty()).then_some(words)
    }
}

fn apply_mod_block(block: &str, base: VoxMod) -> VoxMod {
    let mut out = base;
    let bytes = block.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = (bytes[i] as char).to_ascii_lowercase();
        if ch.is_ascii_alphabetic() {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i > start
                && let Ok(val) = block[start..i].parse::<u32>()
            {
                match ch {
                    'p' => out.pitch = val.clamp(50, 255) as u8,
                    'v' => out.volume = (val.min(200) as f32) / 100.0,
                    's' => out.start_pct = val.min(99) as u8,
                    'e' => out.end_pct = val.clamp(1, 100) as u8,
                    't' => out.time_compress = val.min(100) as u8,
                    _ => {}
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

pub fn parse_vox_body(body: &str) -> Vec<VoxWord> {
    let mut dir = "vox/".to_string();
    let mut defaults = VoxMod::default();
    let mut words = Vec::new();

    // Tokenize keeping `word(params)` or standalone `(params)` together.
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut paren_depth = 0usize;
    for ch in body.chars() {
        match ch {
            '(' => {
                paren_depth += 1;
                cur.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                cur.push(ch);
            }
            c if c.is_whitespace() && paren_depth == 0 => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }

    for mut tok in tokens {
        // Optional directory prefix switch, e.g. `hgrunt/(t30)` or `hgrunt/clik`.
        if let Some(slash) = tok.rfind('/') {
            let prefix = &tok[..=slash];
            dir = prefix.trim_start_matches('/').to_ascii_lowercase();
            tok = tok[slash + 1..].to_string();
            if tok.is_empty() {
                continue;
            }
        }

        // Standalone parameter block `(p110 t40)` modifies sticky defaults.
        if tok.starts_with('(') {
            let inner = tok.trim_start_matches('(').trim_end_matches(')');
            defaults = apply_mod_block(inner, defaults);
            continue;
        }

        let mut word_mod = defaults;
        let mut pause_sec = 0.0f32;
        if let Some(open) = tok.find('(') {
            let tail = &tok[open + 1..];
            if let Some(close) = tail.find(')') {
                word_mod = apply_mod_block(&tail[..close], word_mod);
                let after = &tail[close + 1..];
                if after.contains(',') {
                    pause_sec += 0.15;
                }
                if after.contains('.') {
                    pause_sec += 0.30;
                }
            }
            tok = tok[..open].to_string();
        }

        while let Some(last) = tok.chars().last() {
            match last {
                ',' => {
                    pause_sec += 0.15;
                    tok.pop();
                }
                '.' => {
                    pause_sec += 0.30;
                    tok.pop();
                }
                '!' => {
                    tok.pop();
                }
                _ => break,
            }
        }
        if tok.is_empty() {
            if let Some(prev) = words.last_mut()
                && pause_sec > 0.0
            {
                let prev_w: &mut VoxWord = prev;
                prev_w.pause_sec += pause_sec;
            }
            continue;
        }

        let lower = tok.to_ascii_lowercase();
        let file = if lower.ends_with(".wav") {
            lower
        } else {
            format!("{lower}.wav")
        };
        let path = format!("{dir}{file}");
        words.push(VoxWord {
            path,
            volume: word_mod.volume,
            pitch: word_mod.pitch,
            start_pct: word_mod.start_pct,
            end_pct: word_mod.end_pct,
            time_compress: word_mod.time_compress,
            pause_sec,
        });
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sentences_txt_vox_syntax_and_indices() {
        let src = r#"
// comment
HG_GREN1 hgrunt/(t30) clik take!(e75) cover!(s5), clik(p120)
RADIO_GO radio/go.
"#;
        let table = SentenceTable::parse(src);
        assert_eq!(table.len(), 2);

        let w = table.resolve("!HG_GREN1").expect("resolve by name");
        assert_eq!(w.len(), 4);
        assert_eq!(w[0].path, "hgrunt/clik.wav");
        assert_eq!(w[1].path, "hgrunt/take.wav");
        assert_eq!(w[1].end_pct, 75);
        assert_eq!(w[2].path, "hgrunt/cover.wav");
        assert_eq!(w[2].start_pct, 5);
        assert!((w[2].pause_sec - 0.15).abs() < 1e-4);
        assert_eq!(w[3].pitch, 120);

        let by_idx = table.resolve("!1").expect("resolve by index");
        assert_eq!(by_idx[0].path, "radio/go.wav");
        assert!((by_idx[0].pause_sec - 0.30).abs() < 1e-4);

        let inline = table.resolve("vox/hello world").expect("inline");
        assert_eq!(inline[0].path, "vox/hello.wav");
        assert_eq!(inline[1].path, "vox/world.wav");
    }
}
