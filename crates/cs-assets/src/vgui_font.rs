//! TrueType font rasterizer (`ab_glyph`), Marlett symbol font loader (`resource/marlett.ttf`),
//! and `TrackerScheme.res` / `ClientScheme.res` `Fonts` block resolver (`yres` resolution tiers).

use std::collections::HashMap;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use cs_vfs::Vfs;

/// Classic GoldSrc `marlett.ttf` window/checkbox/arrow symbol codepoints.
pub struct MarlettGlyph;

impl MarlettGlyph {
    pub const MINIMIZE: char = '0';
    pub const MAXIMIZE: char = '1';
    pub const RESTORE: char = '2';
    pub const ARROW_LEFT: char = '3';
    pub const ARROW_RIGHT: char = '4';
    pub const ARROW_UP: char = '5';
    pub const ARROW_DOWN: char = '6';
    pub const CHECK: char = 'a';
    pub const BULLET: char = 'h';
    pub const CLOSE: char = 'r';
    pub const SMALL_ARROW_DOWN: char = 'u';
    pub const SMALL_ARROW_UP: char = 't';
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemeFontVariant {
    pub name: String,
    pub tall: f32,
    pub weight: u32,
    pub yres_min: Option<u32>,
    pub yres_max: Option<u32>,
    pub dropshadow: bool,
    pub outline: bool,
    pub antialias: bool,
    pub additive: bool,
    pub symbol: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for SchemeFontVariant {
    fn default() -> Self {
        Self {
            name: "Tahoma".to_string(),
            tall: 14.0,
            weight: 400,
            yres_min: None,
            yres_max: None,
            dropshadow: false,
            outline: false,
            antialias: true,
            additive: false,
            symbol: false,
            italic: false,
            underline: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchemeFontSet {
    pub custom_font_files: Vec<String>,
    pub fonts: HashMap<String, Vec<SchemeFontVariant>>,
}

impl SchemeFontSet {
    pub fn load(vfs: &Vfs, path: &str) -> Self {
        let Ok(text) = vfs.read_string_lossy(path) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Self {
        let mut set = Self::default();
        let tokens = tokenize_font_kv(text);
        let mut i = 0usize;
        while i < tokens.len() {
            if tokens[i].eq_ignore_ascii_case("CustomFontFiles")
                && tokens.get(i + 1).map(String::as_str) == Some("{")
            {
                i += 2;
                while i < tokens.len() && tokens[i] != "}" {
                    if i + 1 < tokens.len() && tokens[i + 1] != "}" && tokens[i + 1] != "{" {
                        set.custom_font_files.push(tokens[i + 1].replace('\\', "/"));
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                i += 1;
                continue;
            }
            if tokens[i].eq_ignore_ascii_case("Fonts")
                && tokens.get(i + 1).map(String::as_str) == Some("{")
            {
                i += 2;
                while i < tokens.len() && tokens[i] != "}" {
                    let font_name = tokens[i].to_ascii_lowercase();
                    i += 1;
                    if tokens.get(i).map(String::as_str) == Some("{") {
                        i += 1;
                        let mut variants = Vec::new();
                        while i < tokens.len() && tokens[i] != "}" {
                            let _tier_name = &tokens[i];
                            i += 1;
                            if tokens.get(i).map(String::as_str) == Some("{") {
                                i += 1;
                                let mut v = SchemeFontVariant::default();
                                while i < tokens.len() && tokens[i] != "}" {
                                    let k = tokens[i].to_ascii_lowercase();
                                    i += 1;
                                    if i < tokens.len() && tokens[i] != "}" && tokens[i] != "{" {
                                        let val = &tokens[i];
                                        match k.as_str() {
                                            "name" => v.name = val.clone(),
                                            "tall" => {
                                                if let Ok(t) = val.parse::<f32>() {
                                                    v.tall = t.max(6.0);
                                                }
                                            }
                                            "weight" => v.weight = val.parse().unwrap_or(400),
                                            "yres" => {
                                                let mut nums = val
                                                    .split_whitespace()
                                                    .filter_map(|n| n.parse::<u32>().ok());
                                                v.yres_min = nums.next();
                                                v.yres_max = nums.next();
                                            }
                                            "dropshadow" => v.dropshadow = val == "1",
                                            "outline" => v.outline = val == "1",
                                            "antialias" => v.antialias = val != "0",
                                            "additive" => v.additive = val == "1",
                                            "symbol" => v.symbol = val == "1",
                                            "italic" => v.italic = val == "1",
                                            "underline" => v.underline = val == "1",
                                            _ => {}
                                        }
                                        i += 1;
                                    }
                                }
                                if i < tokens.len() && tokens[i] == "}" {
                                    i += 1;
                                }
                                variants.push(v);
                            }
                        }
                        if i < tokens.len() && tokens[i] == "}" {
                            i += 1;
                        }
                        set.fonts.insert(font_name, variants);
                    }
                }
                i += 1;
                continue;
            }
            i += 1;
        }
        set
    }

    /// Resolve a scheme font name (`"Default"`, `"EngineFont"`, `"Title"`, `"Marlett"`, etc.)
    /// matching the vertical resolution tier `screen_h`.
    pub fn resolve(&self, font_name: &str, screen_h: u32) -> Option<&SchemeFontVariant> {
        let variants = self.fonts.get(&font_name.trim().to_ascii_lowercase())?;
        if let Some(exact) = variants.iter().find(|v| match (v.yres_min, v.yres_max) {
            (Some(lo), Some(hi)) => screen_h >= lo && screen_h <= hi,
            (Some(lo), None) => screen_h >= lo,
            (None, Some(hi)) => screen_h <= hi,
            (None, None) => false,
        }) {
            return Some(exact);
        }
        variants
            .iter()
            .find(|v| v.yres_min.is_none() && v.yres_max.is_none())
            .or_else(|| variants.first())
    }
}

#[derive(Debug, Clone)]
pub struct FontLineMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub line_height: f32,
}

#[derive(Debug, Clone)]
pub struct RasterizedGlyph {
    pub ch: char,
    pub width: u32,
    pub height: u32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub advance_x: f32,
    pub alpha: Vec<u8>,
}

/// Pure-Rust TrueType/OpenType font rasterizer (`ab_glyph`).
#[derive(Clone)]
pub struct VguiTrueTypeFont {
    font: FontArc,
    raw: Vec<u8>,
}

impl VguiTrueTypeFont {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        let font = FontArc::try_from_vec(bytes.clone()).map_err(|e| format!("invalid font: {e}"))?;
        Ok(Self { font, raw: bytes })
    }

    pub fn raw_bytes(&self) -> &[u8] {
        &self.raw
    }

    pub fn metrics(&self, px_height: f32) -> FontLineMetrics {
        let scaled = self.font.as_scaled(PxScale::from(px_height.max(6.0)));
        let ascent = scaled.ascent();
        let descent = scaled.descent();
        let line_gap = scaled.line_gap();
        FontLineMetrics {
            ascent,
            descent,
            line_gap,
            line_height: ascent - descent + line_gap,
        }
    }

    pub fn measure_text(&self, text: &str, px_height: f32) -> (f32, f32) {
        let px = px_height.max(6.0);
        let scaled = self.font.as_scaled(PxScale::from(px));
        let mut width = 0.0f32;
        let mut prev = None;
        for ch in text.chars() {
            let gid = self.font.glyph_id(ch);
            if let Some(p) = prev {
                width += scaled.kern(p, gid);
            }
            width += scaled.h_advance(gid);
            prev = Some(gid);
        }
        (width, scaled.height())
    }

    pub fn rasterize_glyph(&self, ch: char, px_height: f32) -> Option<RasterizedGlyph> {
        let px = px_height.max(6.0);
        let scaled = self.font.as_scaled(PxScale::from(px));
        let gid = self.font.glyph_id(ch);
        let advance_x = scaled.h_advance(gid);
        let glyph = gid.with_scale_and_position(PxScale::from(px), ab_glyph::point(0.0, scaled.ascent()));
        let outlined = self.font.outline_glyph(glyph)?;
        let bounds = outlined.px_bounds();
        let width = bounds.width().ceil().max(0.0) as u32;
        let height = bounds.height().ceil().max(0.0) as u32;
        let mut alpha = vec![0u8; (width * height) as usize];
        outlined.draw(|x, y, cov| {
            let idx = (y as usize) * (width as usize) + (x as usize);
            if let Some(dst) = alpha.get_mut(idx) {
                *dst = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        Some(RasterizedGlyph {
            ch,
            width,
            height,
            offset_x: bounds.min.x,
            offset_y: bounds.min.y,
            advance_x,
            alpha,
        })
    }
}

/// Load Valve's `resource/marlett.ttf` symbol font from VFS (`TrackerScheme.res` "Marlett").
pub fn load_marlett_font(vfs: &Vfs) -> Option<VguiTrueTypeFont> {
    let bytes = vfs.read_bytes("resource/marlett.ttf").ok()?;
    VguiTrueTypeFont::from_bytes(bytes).ok()
}

fn tokenize_font_kv(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '/' {
            chars.next();
            if chars.peek() == Some(&'/') {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            out.push("/".to_string());
            continue;
        }
        if c == '{' || c == '}' {
            out.push(c.to_string());
            chars.next();
            continue;
        }
        if c == '"' {
            chars.next();
            let mut s = String::new();
            for ch in chars.by_ref() {
                if ch == '"' {
                    break;
                }
                s.push(ch);
            }
            out.push(s);
            continue;
        }
        let mut bare = String::new();
        while let Some(&nc) = chars.peek() {
            if nc.is_whitespace() || nc == '{' || nc == '}' || nc == '"' {
                break;
            }
            bare.push(nc);
            chars.next();
        }
        if !bare.is_empty() {
            out.push(bare);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_yres_font_tiers() {
        let scheme = r#"
        Scheme {
            Fonts {
                "EngineFont" {
                    "1" { "name" "Verdana" "tall" "13" "yres" "480 599" "dropshadow" "1" }
                    "2" { "name" "Verdana" "tall" "15" "yres" "600 767" "dropshadow" "1" }
                    "3" { "name" "Verdana" "tall" "16" "yres" "768 1023" "dropshadow" "1" }
                }
                "Default" {
                    "1" { "name" "Tahoma" "tall" "16" "weight" "500" }
                }
            }
        }"#;
        let parsed = SchemeFontSet::parse(scheme);
        assert_eq!(parsed.resolve("EngineFont", 480).unwrap().tall, 13.0);
        assert_eq!(parsed.resolve("EngineFont", 720).unwrap().tall, 15.0);
        assert_eq!(parsed.resolve("EngineFont", 768).unwrap().tall, 16.0);
        let def = parsed.resolve("Default", 1080).unwrap();
        assert_eq!(def.name, "Tahoma");
        assert_eq!(def.weight, 500);
    }
}
