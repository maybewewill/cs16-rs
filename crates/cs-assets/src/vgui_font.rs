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
    pub range: Option<(u32, u32)>,
    pub blur: u32,
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
            range: None,
            blur: 0,
            dropshadow: false,
            outline: false,
            antialias: false,
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
                                            "range" => {
                                                let mut nums = val
                                                    .split_whitespace()
                                                    .filter_map(|n| {
                                                        if let Some(hex) = n.strip_prefix("0x").or_else(|| n.strip_prefix("0X")) {
                                                            u32::from_str_radix(hex, 16).ok()
                                                        } else {
                                                            n.parse::<u32>().ok()
                                                        }
                                                    });
                                                if let (Some(a), Some(b)) = (nums.next(), nums.next()) {
                                                    v.range = Some((a, b));
                                                }
                                            }
                                            "blur" => v.blur = val.parse().unwrap_or(0),
                                            "dropshadow" => v.dropshadow = val == "1",
                                            "outline" => v.outline = val == "1",
                                            "antialias" => v.antialias = val == "1",
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

/// Options mirroring Valve Windows GDI / `vgui_surfaces` font flags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterizeOptions {
    pub tall: f32,
    pub antialias: bool,
    pub outline: bool,
    pub dropshadow: bool,
    pub underline: bool,
    pub additive: bool,
}

impl From<&SchemeFontVariant> for RasterizeOptions {
    fn from(v: &SchemeFontVariant) -> Self {
        Self {
            tall: v.tall,
            antialias: v.antialias,
            outline: v.outline,
            dropshadow: v.dropshadow,
            underline: v.underline,
            additive: v.additive,
        }
    }
}

/// RGBA8 rasterized glyph bitmap with baked outline / dropshadow and integer GDI advance.
#[derive(Debug, Clone)]
pub struct RasterizedGlyphRgba {
    pub ch: char,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub advance_x: i32,
    /// White ink RGB (`255,255,255`), black (`0,0,0`) outline/dropshadow pixels, and coverage alpha.
    pub rgba: Vec<u8>,
}

/// Loaded font source metadata for diagnostics (zero silent fallbacks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeFontLoadEntry {
    pub scheme_font: String,
    pub family: String,
    pub weight: u32,
    pub source: String,
}

/// Explicit missing font entry reported when a scheme font file could not be resolved on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSchemeFont {
    pub scheme_font: String,
    pub family: String,
    pub weight: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct SchemeFontLoadReport {
    pub loaded: Vec<SchemeFontLoadEntry>,
    pub missing: Vec<MissingSchemeFont>,
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

    /// Calibrate `PxScale` so `(ascent - descent)` matches Windows GDI `CreateFont(-tall)` cell height.
    fn gdi_px_scale(&self, tall: f32) -> f32 {
        let target = tall.max(6.0);
        let unscaled = self.font.as_scaled(PxScale::from(target));
        let cell = (unscaled.ascent() - unscaled.descent()).max(1.0);
        (target * (target / cell)).clamp(target * 0.85, target * 1.35)
    }

    pub fn metrics(&self, px_height: f32) -> FontLineMetrics {
        let scale = self.gdi_px_scale(px_height);
        let scaled = self.font.as_scaled(PxScale::from(scale));
        let ascent = scaled.ascent().round();
        let descent = scaled.descent().round();
        let line_gap = scaled.line_gap().round();
        FontLineMetrics {
            ascent,
            descent,
            line_gap,
            line_height: (ascent - descent).max(px_height.round()),
        }
    }

    pub fn measure_text(&self, text: &str, px_height: f32) -> (f32, f32) {
        let px = self.gdi_px_scale(px_height);
        let scaled = self.font.as_scaled(PxScale::from(px));
        let mut width = 0i32;
        let mut prev = None;
        for ch in text.chars() {
            let gid = self.font.glyph_id(ch);
            if let Some(p) = prev {
                width += scaled.kern(p, gid).round() as i32;
            }
            width += scaled.h_advance(gid).round() as i32;
            prev = Some(gid);
        }
        (width as f32, (scaled.ascent() - scaled.descent()).round().max(px_height))
    }

    pub fn kern_i32(&self, prev: char, curr: char, px_height: f32) -> i32 {
        let px = self.gdi_px_scale(px_height);
        let scaled = self.font.as_scaled(PxScale::from(px));
        scaled
            .kern(self.font.glyph_id(prev), self.font.glyph_id(curr))
            .round() as i32
    }

    pub fn advance_i32(&self, ch: char, px_height: f32) -> i32 {
        let px = self.gdi_px_scale(px_height);
        let scaled = self.font.as_scaled(PxScale::from(px));
        scaled.h_advance(self.font.glyph_id(ch)).round() as i32
    }

    pub fn rasterize_glyph(&self, ch: char, px_height: f32) -> Option<RasterizedGlyph> {
        let px = self.gdi_px_scale(px_height);
        let scaled = self.font.as_scaled(PxScale::from(px));
        let gid = self.font.glyph_id(ch);
        let advance_x = scaled.h_advance(gid).round();
        let glyph = gid.with_scale_and_position(
            PxScale::from(px),
            ab_glyph::point(0.0, scaled.ascent().round()),
        );
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

    /// Rasterize a glyph using Valve Windows GDI rules:
    /// - 1-bit hard coverage threshold when `!opts.antialias`
    /// - 1px black outline when `opts.outline`
    /// - `(+1, +1)` black drop shadow when `opts.dropshadow`
    pub fn rasterize_glyph_rgba(&self, ch: char, opts: &RasterizeOptions) -> Option<RasterizedGlyphRgba> {
        let px = self.gdi_px_scale(opts.tall);
        let scaled = self.font.as_scaled(PxScale::from(px));
        let gid = self.font.glyph_id(ch);
        let advance_x = scaled.h_advance(gid).round() as i32;
        let baseline = scaled.ascent().round();
        let glyph = gid.with_scale_and_position(PxScale::from(px), ab_glyph::point(0.0, baseline));
        let Some(outlined) = self.font.outline_glyph(glyph) else {
            return Some(RasterizedGlyphRgba {
                ch,
                width: 0,
                height: 0,
                offset_x: 0,
                offset_y: 0,
                advance_x,
                rgba: Vec::new(),
            });
        };
        let bounds = outlined.px_bounds();
        let gw = bounds.width().ceil().max(0.0) as i32;
        let gh = bounds.height().ceil().max(0.0) as i32;
        if gw == 0 || gh == 0 {
            return Some(RasterizedGlyphRgba {
                ch,
                width: 0,
                height: 0,
                offset_x: 0,
                offset_y: 0,
                advance_x,
                rgba: Vec::new(),
            });
        }
        let mut raw_alpha = vec![0u8; (gw * gh) as usize];
        outlined.draw(|x, y, cov| {
            let idx = (y as usize) * (gw as usize) + (x as usize);
            if let Some(dst) = raw_alpha.get_mut(idx) {
                *dst = if opts.antialias {
                    (cov.clamp(0.0, 1.0) * 255.0).round() as u8
                } else if cov >= 0.48 {
                    255
                } else {
                    0
                };
            }
        });

        let pad_l = i32::from(opts.outline);
        let pad_t = i32::from(opts.outline);
        let pad_r = i32::from(opts.outline) + i32::from(opts.dropshadow && !opts.outline);
        let pad_b = i32::from(opts.outline) + i32::from(opts.dropshadow && !opts.outline);
        let out_w = (gw + pad_l + pad_r) as u32;
        let out_h = (gh + pad_t + pad_b) as u32;
        let mut rgba = vec![0u8; (out_w * out_h * 4) as usize];

        let put_shadow = |rgba: &mut [u8], sx: i32, sy: i32, a: u8| {
            if a == 0 || sx < 0 || sy < 0 || sx >= out_w as i32 || sy >= out_h as i32 {
                return;
            }
            let idx = ((sy as u32 * out_w + sx as u32) * 4) as usize;
            if a > rgba[idx + 3] {
                rgba[idx] = 0;
                rgba[idx + 1] = 0;
                rgba[idx + 2] = 0;
                rgba[idx + 3] = a;
            }
        };

        if opts.outline {
            for gy in 0..gh {
                for gx in 0..gw {
                    let a = raw_alpha[(gy * gw + gx) as usize];
                    if a == 0 {
                        continue;
                    }
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if dx != 0 || dy != 0 {
                                put_shadow(&mut rgba, gx + pad_l + dx, gy + pad_t + dy, a);
                            }
                        }
                    }
                }
            }
        } else if opts.dropshadow {
            for gy in 0..gh {
                for gx in 0..gw {
                    let a = raw_alpha[(gy * gw + gx) as usize];
                    if a > 0 {
                        let sa = ((u16::from(a) * 210) / 255) as u8;
                        put_shadow(&mut rgba, gx + pad_l + 1, gy + pad_t + 1, sa);
                    }
                }
            }
        }

        for gy in 0..gh {
            for gx in 0..gw {
                let fg_a = raw_alpha[(gy * gw + gx) as usize];
                if fg_a == 0 {
                    continue;
                }
                let ox = gx + pad_l;
                let oy = gy + pad_t;
                let idx = ((oy as u32 * out_w + ox as u32) * 4) as usize;
                let prev_a = rgba[idx + 3];
                let out_a = fg_a.max(prev_a);
                // White foreground composited over black outline/shadow => gray = fg_a over 0, straight = fg_a*255/out_a
                let rgb = if out_a > 0 {
                    ((u16::from(fg_a) * 255) / u16::from(out_a)).min(255) as u8
                } else {
                    255
                };
                rgba[idx] = rgb;
                rgba[idx + 1] = rgb;
                rgba[idx + 2] = rgb;
                rgba[idx + 3] = out_a;
            }
        }

        Some(RasterizedGlyphRgba {
            ch,
            width: out_w,
            height: out_h,
            offset_x: bounds.min.x.round() as i32 - pad_l,
            offset_y: (bounds.min.y.round() as i32 - baseline as i32) - pad_t,
            advance_x,
            rgba,
        })
    }
}

/// Resolve the exact TrueType font bytes named by a `SchemeFontVariant` without silent substitution.
pub fn resolve_font_bytes(
    vfs: Option<&Vfs>,
    variant: &SchemeFontVariant,
    custom_font_files: &[String],
) -> Result<(Vec<u8>, String), String> {
    let lower = variant.name.trim().to_ascii_lowercase();
    let bold = variant.weight >= 600 || lower.contains("bold");
    let italic = variant.italic || lower.contains("italic");

    if (lower == "marlett" || variant.symbol)
        && let Some(v) = vfs
        && let Ok(bytes) = v.read_bytes("resource/marlett.ttf")
    {
        return Ok((bytes, "vfs:resource/marlett.ttf".to_string()));
    }

    if let Some(v) = vfs {
        for raw in custom_font_files {
            let norm = raw.replace('\\', "/");
            if let Ok(bytes) = v.read_bytes(&norm).or_else(|_| v.read_bytes(&format!("resource/{norm}"))) {
                let stem = norm.rsplit('/').next().unwrap_or(&norm).to_ascii_lowercase();
                if stem.contains(&lower.replace(' ', "")) {
                    return Ok((bytes, format!("vfs:{norm}")));
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let candidates: Vec<&'static str> = match lower.as_str() {
            "tahoma" => {
                if bold {
                    vec!["tahomabd.ttf", "tahoma.ttf"]
                } else {
                    vec!["tahoma.ttf"]
                }
            }
            "verdana" | "verdana bold" => {
                if bold && italic {
                    vec!["verdanaz.ttf", "verdanab.ttf"]
                } else if bold {
                    vec!["verdanab.ttf"]
                } else if italic {
                    vec!["verdanai.ttf"]
                } else {
                    vec!["verdana.ttf"]
                }
            }
            "trebuchet ms" => {
                if bold && italic {
                    vec!["trebucbi.ttf", "trebucbd.ttf"]
                } else if bold {
                    vec!["trebucbd.ttf"]
                } else if italic {
                    vec!["trebucit.ttf"]
                } else {
                    vec!["trebuc.ttf"]
                }
            }
            "courier" | "courier new" => {
                if bold && italic {
                    vec!["courbi.ttf", "courbd.ttf"]
                } else if bold {
                    vec!["courbd.ttf"]
                } else if italic {
                    vec!["couri.ttf"]
                } else {
                    vec!["cour.ttf"]
                }
            }
            "arial" => {
                if bold && italic {
                    vec!["arialbi.ttf", "arialbd.ttf"]
                } else if bold {
                    vec!["arialbd.ttf"]
                } else if italic {
                    vec!["ariali.ttf"]
                } else {
                    vec!["arial.ttf"]
                }
            }
            "marlett" => vec!["marlett.ttf"],
            _ => Vec::new(),
        };

        let dirs = [
            r"C:\Windows\Fonts",
            "/usr/share/fonts/truetype",
            "/usr/share/fonts/truetype/msttcorefonts",
            "/System/Library/Fonts/Supplemental",
            "/Library/Fonts",
        ];
        for file in candidates {
            for dir in dirs {
                let path = std::path::Path::new(dir).join(file);
                if let Ok(bytes) = std::fs::read(&path) {
                    return Ok((bytes, path.display().to_string()));
                }
            }
        }
    }

    Err(format!(
        "no exact font file found for family '{}' (weight={}, italic={})",
        variant.name, variant.weight, italic
    ))
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
        assert!(!def.antialias, "Valve scheme omits antialias -> false");
    }

    #[test]
    fn gdi_rasterizer_enforces_crisp_non_aa_and_outline_shadow() {
        let Ok(vfs) = cs_vfs::Vfs::discover(None, cs_vfs::VfsConfig::default()) else {
            eprintln!("SKIPPED gdi_rasterizer test: CS16_GAMEDIR / Half-Life not found");
            return;
        };
        let scheme_font = SchemeFontVariant {
            name: "Tahoma".to_string(),
            tall: 16.0,
            weight: 400,
            antialias: false,
            ..Default::default()
        };
        let Ok((bytes, _src)) = resolve_font_bytes(Some(&vfs), &scheme_font, &[]) else {
            eprintln!("SKIPPED gdi_rasterizer test: Tahoma OS font unavailable");
            return;
        };
        let font = VguiTrueTypeFont::from_bytes(bytes).expect("parse Tahoma");
        let crisp = font
            .rasterize_glyph_rgba(
                'A',
                &RasterizeOptions {
                    tall: 16.0,
                    antialias: false,
                    outline: false,
                    dropshadow: false,
                    underline: false,
                    additive: false,
                },
            )
            .expect("rasterize crisp");
        assert!(crisp.width > 0 && crisp.height > 0);
        assert!(
            crisp
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| px[3] == 0 || px[3] == 255),
            "antialias=false must produce binary 0/255 alpha coverage"
        );

        let outlined = font
            .rasterize_glyph_rgba(
                'A',
                &RasterizeOptions {
                    tall: 16.0,
                    antialias: false,
                    outline: true,
                    dropshadow: false,
                    underline: false,
                    additive: false,
                },
            )
            .expect("rasterize outlined");
        assert!(outlined.width >= crisp.width + 2);
        assert!(
            outlined
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[3] == 255 && px[0] == 0 && px[1] == 0 && px[2] == 0),
            "outline must bake opaque black border pixels"
        );

        let missing = SchemeFontVariant {
            name: "NonExistentValveTestFont_XYZ".to_string(),
            ..Default::default()
        };
        assert!(resolve_font_bytes(Some(&vfs), &missing, &[]).is_err());
    }
}
