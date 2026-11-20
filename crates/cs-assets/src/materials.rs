//! Parser for GoldSrc `sound/materials.txt` mapping texture prefixes to footstep & wallbang materials.

use std::collections::HashMap;

use cs_vfs::Vfs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceMaterial {
    #[default]
    Concrete,
    Metal,
    Dirt,
    Vent,
    Grate,
    Tile,
    Slosh,
    Wood,
    Computer,
    Glass,
    Snow,
    Grass,
}

impl SurfaceMaterial {
    pub fn from_code(code: char) -> Self {
        match code.to_ascii_uppercase() {
            'M' => Self::Metal,
            'D' => Self::Dirt,
            'V' => Self::Vent,
            'G' => Self::Grate,
            'T' => Self::Tile,
            'S' => Self::Slosh,
            'W' => Self::Wood,
            'P' => Self::Computer,
            'Y' => Self::Glass,
            'N' => Self::Snow,
            'X' => Self::Grass,
            _ => Self::Concrete,
        }
    }

    /// Wall-penetration resistance (lower = easier wallbangs, matching ReGameDLL_CS concepts).
    pub fn penetration_modifier(self) -> f32 {
        match self {
            Self::Wood | Self::Glass | Self::Vent => 0.85,
            Self::Grate | Self::Dirt | Self::Grass | Self::Snow => 0.72,
            Self::Concrete | Self::Tile | Self::Computer => 0.55,
            Self::Metal => 0.42,
            Self::Slosh => 0.90,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MaterialTable {
    entries: HashMap<String, SurfaceMaterial>,
}

impl MaterialTable {
    pub fn load(vfs: &Vfs) -> Self {
        let mut table = Self::default();
        // Load valve defaults first, then cstrike overrides
        if let Ok(text) = vfs.read_string_lossy("sound/materials.txt") {
            table.parse_into(&text);
        }
        table
    }

    pub fn parse_into(&mut self, text: &str) {
        for raw in text.lines() {
            let line = raw.split("//").next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(code_str) = parts.next() else {
                continue;
            };
            let Some(tex_name) = parts.next() else {
                continue;
            };
            if let Some(ch) = code_str.chars().next() {
                let key = normalize_mat_key(tex_name);
                self.entries.insert(key, SurfaceMaterial::from_code(ch));
            }
        }
    }

    pub fn lookup(&self, texture_name: &str) -> SurfaceMaterial {
        let key = normalize_mat_key(texture_name);
        if let Some(&mat) = self.entries.get(&key) {
            return mat;
        }
        // GoldSrc heuristic fallback for untagged textures
        let lower = texture_name.to_ascii_lowercase();
        if lower.contains("sand") || lower.contains("dirt") || lower.contains("ground") || lower.contains("gravel") {
            SurfaceMaterial::Dirt
        } else if lower.contains("crate") || lower.contains("wood") || lower.contains("door") || lower.contains("plank") {
            SurfaceMaterial::Wood
        } else if lower.contains("metal") || lower.contains("steel") || lower.contains("iron") || lower.contains("duct") {
            SurfaceMaterial::Metal
        } else if lower.contains("grate") || lower.starts_with('{') {
            SurfaceMaterial::Grate
        } else if lower.contains("tile") || lower.contains("marble") {
            SurfaceMaterial::Tile
        } else if lower.starts_with('!') || lower.starts_with('*') || lower.contains("water") {
            SurfaceMaterial::Slosh
        } else {
            SurfaceMaterial::Concrete
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn normalize_mat_key(name: &str) -> String {
    // Strip GoldSrc special prefixes like `+0`, `-0`, `{`, `!` and truncate to 12 chars
    let mut s = name.trim();
    if let Some(rest) = s.strip_prefix(['{', '!', '~', ' ']) {
        s = rest;
    }
    if (s.starts_with('+') || s.starts_with('-')) && s.len() >= 2 {
        s = &s[2..];
    }
    s.chars()
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase()
}
