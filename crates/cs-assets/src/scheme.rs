//! Valve VGUI2 scheme (`TrackerScheme.res`), CS 1.6 menu background (`BackgroundLayout.txt`),
//! `sprites/hud.txt` definitions, and GoldSrc 6-face skybox loader (`gfx/env/*.tga`).

use std::collections::HashMap;

use cs_vfs::Vfs;
use image::ImageFormat;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum SchemeError {
    #[error("VFS error: {0}")]
    Vfs(#[from] cs_vfs::VfsError),
    #[error("image decode error for {path}: {source}")]
    Image {
        path: String,
        #[source]
        source: image::ImageError,
    },
}

/// Authentic Steam 2003 / VGUI2 color palette parsed from `resource/TrackerScheme.res`.
#[derive(Debug, Clone, Copy)]
pub struct VguiColors {
    pub control_bg: [u8; 4],
    pub control_dark_bg: [u8; 4],
    pub window_bg: [u8; 4],
    pub selection_bg: [u8; 4],
    pub selection_bg2: [u8; 4],
    pub border_bright: [u8; 4],
    pub border_dark: [u8; 4],
    pub base_text: [u8; 4],
    pub bright_text: [u8; 4],
    pub dim_text: [u8; 4],
    pub bright_control_text: [u8; 4],
    pub disabled_text: [u8; 4],
}

impl Default for VguiColors {
    fn default() -> Self {
        // Exact Valve Steam 2003 / CS 1.6 TrackerScheme.res palette
        Self {
            control_bg: [76, 88, 68, 255],
            control_dark_bg: [90, 106, 80, 255],
            window_bg: [62, 70, 55, 255],
            selection_bg: [149, 136, 49, 255],
            selection_bg2: [40, 46, 34, 255],
            border_bright: [136, 145, 128, 255],
            border_dark: [40, 46, 34, 255],
            base_text: [216, 222, 211, 255],
            bright_text: [255, 255, 255, 255],
            dim_text: [160, 170, 149, 255],
            bright_control_text: [196, 181, 80, 255],
            disabled_text: [117, 128, 111, 255],
        }
    }
}

impl VguiColors {
    /// Load colors from `resource/TrackerScheme.res` in VFS, falling back to authentic defaults.
    pub fn load(vfs: &Vfs) -> Self {
        let Ok(text) = vfs.read_string_lossy("resource/TrackerScheme.res") else {
            return Self::default();
        };
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Self {
        let map = parse_scheme_colors(text);
        let mut colors = Self::default();
        if let Some(c) = map.get("controlbg") {
            colors.control_bg = *c;
        }
        if let Some(c) = map.get("controldarkbg") {
            colors.control_dark_bg = *c;
        }
        if let Some(c) = map.get("windowbg") {
            colors.window_bg = *c;
        }
        if let Some(c) = map.get("selectionbg") {
            colors.selection_bg = *c;
        }
        if let Some(c) = map.get("selectionbg2") {
            colors.selection_bg2 = *c;
        }
        if let Some(c) = map.get("borderbright") {
            colors.border_bright = *c;
        }
        if let Some(c) = map.get("borderdark") {
            colors.border_dark = *c;
        }
        if let Some(c) = map.get("basetext") {
            colors.base_text = *c;
        }
        if let Some(c) = map.get("brightbasetext") {
            colors.bright_text = *c;
        }
        if let Some(c) = map.get("dimbasetext") {
            colors.dim_text = *c;
        }
        if let Some(c) = map.get("brightcontroltext") {
            colors.bright_control_text = *c;
        }
        if let Some(c) = map.get("disabledtext1") {
            colors.disabled_text = *c;
        }
        colors
    }
}

fn parse_scheme_colors(text: &str) -> HashMap<String, [u8; 4]> {
    let mut out = HashMap::new();
    let mut in_colors = false;
    let mut brace_depth = 0i32;

    for raw_line in text.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if !in_colors {
            if line.eq_ignore_ascii_case("Colors") {
                in_colors = true;
            }
            continue;
        }
        if line == "{" {
            brace_depth += 1;
            continue;
        }
        if line == "}" {
            brace_depth -= 1;
            if brace_depth <= 0 {
                break;
            }
            continue;
        }
        if brace_depth == 1 {
            let mut quotes = line.split('"');
            let _ = quotes.next();
            let Some(key) = quotes.next() else {
                continue;
            };
            let _ = quotes.next();
            let Some(val) = quotes.next() else {
                continue;
            };
            let nums: Vec<u8> = val
                .split_whitespace()
                .filter_map(|n| n.parse::<u8>().ok())
                .collect();
            if nums.len() >= 3 {
                let a = nums.get(3).copied().unwrap_or(255);
                out.insert(key.to_ascii_lowercase(), [nums[0], nums[1], nums[2], a]);
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct RgbaImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Stitch the iconic 800x600 CS 1.6 menu background from `resource/BackgroundLayout.txt`
/// and the 12 `resource/background/800_*_loading.tga` tiles.
pub fn load_menu_background(vfs: &Vfs) -> Result<RgbaImageData, SchemeError> {
    let layout = vfs.read_string_lossy("resource/BackgroundLayout.txt")?;
    let mut width = 800u32;
    let mut height = 600u32;
    let mut tiles: Vec<(String, u32, u32)> = Vec::new();

    for raw_line in layout.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[0].eq_ignore_ascii_case("resolution") {
            width = parts[1].parse().unwrap_or(800);
            height = parts[2].parse().unwrap_or(600);
        } else if parts.len() >= 4 {
            let x = parts[2].parse().unwrap_or(0);
            let y = parts[3].parse().unwrap_or(0);
            tiles.push((parts[0].to_string(), x, y));
        }
    }

    let mut canvas = vec![0u8; (width * height * 4) as usize];
    for px in canvas.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }

    for (tile_path, ox, oy) in tiles {
        let Ok(bytes) = vfs.read_bytes(&tile_path) else {
            warn!(tile = %tile_path, "Missing menu background tile");
            continue;
        };
        let img = image::load_from_memory_with_format(&bytes, ImageFormat::Tga)
            .map_err(|source| SchemeError::Image {
                path: tile_path.clone(),
                source,
            })?
            .to_rgba8();

        for ty in 0..img.height() {
            let dy = oy + ty;
            if dy >= height {
                continue;
            }
            for tx in 0..img.width() {
                let dx = ox + tx;
                if dx >= width {
                    continue;
                }
                let src = img.get_pixel(tx, ty).0;
                let dst = ((dy * width + dx) * 4) as usize;
                canvas[dst..dst + 4].copy_from_slice(&src);
            }
        }
    }

    Ok(RgbaImageData {
        width,
        height,
        rgba: canvas,
    })
}

/// Entry parsed from `sprites/hud.txt`.
#[derive(Debug, Clone)]
pub struct HudSpriteEntry {
    pub name: String,
    pub resolution: u32,
    pub sprite_name: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub fn load_hud_txt(vfs: &Vfs) -> Result<Vec<HudSpriteEntry>, SchemeError> {
    let text = vfs.read_string_lossy("sprites/hud.txt")?;
    Ok(parse_hud_txt(&text))
}

pub fn parse_hud_txt(text: &str) -> Vec<HudSpriteEntry> {
    let mut entries = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            continue;
        }
        if let (Ok(resolution), Ok(x), Ok(y), Ok(width), Ok(height)) = (
            parts[1].parse(),
            parts[3].parse(),
            parts[4].parse(),
            parts[5].parse(),
            parts[6].parse(),
        ) {
            entries.push(HudSpriteEntry {
                name: parts[0].to_string(),
                resolution,
                sprite_name: parts[2].to_string(),
                x,
                y,
                width,
                height,
            });
        }
    }
    entries
}

/// 6 skybox face textures ordered as `[rt, bk, lf, ft, up, dn]` matching GoldSrc conventions.
pub const SKYBOX_SUFFIXES: [&str; 6] = ["rt", "bk", "lf", "ft", "up", "dn"];

#[derive(Debug, Clone)]
pub struct SkyboxTextures {
    pub skyname: String,
    pub faces: [RgbaImageData; 6],
}

impl SkyboxTextures {
    pub fn load(vfs: &Vfs, skyname: &str) -> Self {
        let mut loaded = Vec::with_capacity(6);
        let effective_name = if vfs.exists(&format!("gfx/env/{skyname}ft.tga")) {
            skyname
        } else {
            "desert"
        };

        for suffix in SKYBOX_SUFFIXES {
            let path = format!("gfx/env/{effective_name}{suffix}.tga");
            let img = vfs
                .read_bytes(&path)
                .ok()
                .and_then(|bytes| {
                    image::load_from_memory_with_format(&bytes, ImageFormat::Tga).ok()
                })
                .map(|dyn_img| {
                    let rgba = dyn_img.to_rgba8();
                    RgbaImageData {
                        width: rgba.width(),
                        height: rgba.height(),
                        rgba: rgba.into_raw(),
                    }
                })
                .unwrap_or_else(fallback_sky_face);
            loaded.push(img);
        }

        let faces: [RgbaImageData; 6] = loaded.try_into().expect("exactly 6 skybox faces");
        Self {
            skyname: effective_name.to_string(),
            faces,
        }
    }
}

fn fallback_sky_face() -> RgbaImageData {
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for _ in 0..16 {
        rgba.extend_from_slice(&[135, 165, 195, 255]);
    }
    RgbaImageData {
        width: 4,
        height: 4,
        rgba,
    }
}

/// Authentic in-game CS 1.6 VGUI2 color scheme parsed from `cstrike/resource/ClientScheme.res`.
/// Used by Buy Menu, Team Menu, Class Menu, MOTD, and Scoreboard (dark translucent black + amber-orange).
#[derive(Debug, Clone, Copy)]
pub struct ClientVguiColors {
    pub base_text: [u8; 4],
    pub title_text: [u8; 4],
    pub label_dim_text: [u8; 4],
    pub disabled_text: [u8; 4],
    pub dim_list_text: [u8; 4],
    pub window_bg: [u8; 4],
    pub list_bg: [u8; 4],
    pub button_bg: [u8; 4],
    pub selection_bg: [u8; 4],
    pub border_bright: [u8; 4],
    pub border_preset: [u8; 4],
    pub team_spec: [u8; 4],
    pub team_t: [u8; 4],
    pub team_ct: [u8; 4],
}

impl Default for ClientVguiColors {
    fn default() -> Self {
        Self {
            base_text: [255, 176, 0, 255],
            title_text: [255, 174, 0, 255],
            label_dim_text: [255, 176, 0, 164],
            disabled_text: [80, 48, 0, 255],
            dim_list_text: [188, 112, 0, 255],
            window_bg: [0, 0, 0, 200],
            list_bg: [0, 0, 0, 128],
            button_bg: [0, 0, 0, 64],
            selection_bg: [255, 176, 0, 100],
            border_bright: [188, 112, 0, 128],
            border_preset: [255, 176, 0, 255],
            team_spec: [204, 204, 204, 255],
            team_t: [255, 64, 64, 255],
            team_ct: [153, 204, 255, 255],
        }
    }
}

impl ClientVguiColors {
    pub fn load(vfs: &Vfs) -> Self {
        let Ok(text) = vfs.read_string_lossy("resource/ClientScheme.res") else {
            return Self::default();
        };
        let map = parse_scheme_colors(&text);
        let mut c = Self::default();
        if let Some(v) = map.get("basetext") {
            c.base_text = *v;
        }
        if let Some(v) = map.get("titletext") {
            c.title_text = *v;
        }
        if let Some(v) = map.get("labeldimtext") {
            c.label_dim_text = *v;
        }
        if let Some(v) = map.get("disabledtext1") {
            c.disabled_text = *v;
        }
        if let Some(v) = map.get("dimlisttext") {
            c.dim_list_text = *v;
        }
        if let Some(v) = map.get("windowbg") {
            c.window_bg = *v;
        }
        if let Some(v) = map.get("listbg") {
            c.list_bg = *v;
        }
        if let Some(v) = map.get("buttonbg") {
            c.button_bg = *v;
        }
        if let Some(v) = map.get("selectionbg") {
            c.selection_bg = *v;
        }
        if let Some(v) = map.get("borderbright") {
            c.border_bright = *v;
        }
        if let Some(v) = map.get("borderbuypreset") {
            c.border_preset = *v;
        }
        if let Some(v) = map.get("team0") {
            c.team_spec = *v;
        }
        if let Some(v) = map.get("team1") {
            c.team_t = *v;
        }
        if let Some(v) = map.get("team2") {
            c.team_ct = *v;
        }
        c
    }
}

/// One 1-pixel edge stroke in a Valve VGUI2 scheme border (`InsetBorder`, `RaisedBorder`, `ButtonBorder`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VguiBorderLine {
    pub color: String,
    pub offset: [i32; 2],
}

/// Parsed VGUI2 border definition from `TrackerScheme.res` / `ClientScheme.res` `Borders` block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VguiBorderSpec {
    pub inset: [i32; 4],
    pub left: Vec<VguiBorderLine>,
    pub right: Vec<VguiBorderLine>,
    pub top: Vec<VguiBorderLine>,
    pub bottom: Vec<VguiBorderLine>,
}

/// Full parsed VGUI2 scheme (`Colors`, `BaseSettings`, `Fonts`, `Borders`).
#[derive(Debug, Clone, Default)]
pub struct VguiScheme {
    pub source_path: String,
    pub colors: HashMap<String, [u8; 4]>,
    pub base_settings: HashMap<String, String>,
    pub fonts: crate::vgui_font::SchemeFontSet,
    pub borders: HashMap<String, VguiBorderSpec>,
    pub border_aliases: HashMap<String, String>,
}

impl VguiScheme {
    pub fn load(vfs: &Vfs, path: &str) -> Option<Self> {
        let text = vfs.read_string_lossy(path).ok()?;
        let mut s = Self::parse(&text);
        s.source_path = path.to_string();
        Some(s)
    }

    pub fn load_tracker(vfs: &Vfs) -> Self {
        Self::load(vfs, "resource/TrackerScheme.res").unwrap_or_default()
    }

    pub fn load_client(vfs: &Vfs) -> Self {
        Self::load(vfs, "resource/ClientScheme.res").unwrap_or_default()
    }

    pub fn load_source_or_tracker(vfs: &Vfs) -> Self {
        Self::load(vfs, "resource/SourceScheme.res").unwrap_or_else(|| Self::load_tracker(vfs))
    }

    pub fn parse(text: &str) -> Self {
        let mut scheme = Self {
            fonts: crate::vgui_font::SchemeFontSet::parse(text),
            colors: parse_scheme_colors(text),
            ..Default::default()
        };
        let tokens = tokenize_scheme_kv(text);
        let mut i = 0usize;
        while i < tokens.len() {
            if tokens[i].eq_ignore_ascii_case("BaseSettings")
                && tokens.get(i + 1).map(String::as_str) == Some("{")
            {
                i += 2;
                parse_base_settings(&tokens, &mut i, "", &mut scheme.base_settings);
                continue;
            }
            if tokens[i].eq_ignore_ascii_case("Borders")
                && tokens.get(i + 1).map(String::as_str) == Some("{")
            {
                i += 2;
                parse_borders_block(
                    &tokens,
                    &mut i,
                    &mut scheme.borders,
                    &mut scheme.border_aliases,
                );
                continue;
            }
            i += 1;
        }
        scheme
    }

    pub fn base_setting(&self, key: &str) -> Option<&str> {
        self.base_settings
            .get(&key.trim().to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Resolve an RGBA value from a literal `"R G B A"`, a `BaseSettings` key, or a `Colors` key.
    pub fn resolve_color(&self, token_or_rgba: &str) -> Option<[u8; 4]> {
        let trimmed = token_or_rgba.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(rgba) = parse_rgba_str(trimmed) {
            return Some(rgba);
        }
        let mut cur = trimmed;
        for _ in 0..4 {
            let key = cur.to_ascii_lowercase();
            if let Some(c) = self.colors.get(&key) {
                return Some(*c);
            }
            if let Some(next) = self.base_settings.get(&key) {
                if let Some(rgba) = parse_rgba_str(next) {
                    return Some(rgba);
                }
                cur = next.as_str();
            } else {
                break;
            }
        }
        None
    }

    /// Resolve a border definition by name, following `Borders` aliases (`ButtonBorder -> RaisedBorder`).
    pub fn resolve_border(&self, name: &str) -> Option<&VguiBorderSpec> {
        let mut cur = name.trim().to_ascii_lowercase();
        for _ in 0..4 {
            if let Some(spec) = self.borders.get(&cur) {
                return Some(spec);
            }
            if let Some(next) = self.border_aliases.get(&cur) {
                cur = next.to_ascii_lowercase();
            } else {
                break;
            }
        }
        None
    }
}

fn parse_rgba_str(raw: &str) -> Option<[u8; 4]> {
    let nums: Vec<u8> = raw
        .split_whitespace()
        .filter_map(|p| p.parse::<u8>().ok())
        .collect();
    if nums.len() >= 3 {
        Some([
            nums[0],
            nums[1],
            nums[2],
            nums.get(3).copied().unwrap_or(255),
        ])
    } else {
        None
    }
}

fn parse_base_settings(
    tokens: &[String],
    cursor: &mut usize,
    prefix: &str,
    out: &mut HashMap<String, String>,
) {
    while *cursor < tokens.len() {
        let tok = &tokens[*cursor];
        if tok == "}" {
            *cursor += 1;
            break;
        }
        let key = tok.clone();
        *cursor += 1;
        if *cursor < tokens.len() && tokens[*cursor] == "{" {
            *cursor += 1;
            let next_prefix = if prefix.is_empty() {
                key.to_ascii_lowercase()
            } else {
                format!("{}/{}", prefix, key.to_ascii_lowercase())
            };
            parse_base_settings(tokens, cursor, &next_prefix, out);
        } else if *cursor < tokens.len() && tokens[*cursor] != "}" {
            let val = tokens[*cursor].clone();
            *cursor += 1;
            let full_key = if prefix.is_empty() {
                key.to_ascii_lowercase()
            } else {
                format!("{}/{}", prefix, key.to_ascii_lowercase())
            };
            out.insert(full_key, val);
        }
    }
}

fn parse_borders_block(
    tokens: &[String],
    cursor: &mut usize,
    borders: &mut HashMap<String, VguiBorderSpec>,
    aliases: &mut HashMap<String, String>,
) {
    while *cursor < tokens.len() {
        let tok = &tokens[*cursor];
        if tok == "}" {
            *cursor += 1;
            break;
        }
        let name = tok.to_ascii_lowercase();
        *cursor += 1;
        if *cursor < tokens.len() && tokens[*cursor] == "{" {
            *cursor += 1;
            borders.insert(name, parse_one_border(tokens, cursor));
        } else if *cursor < tokens.len() && tokens[*cursor] != "}" {
            let target = tokens[*cursor].to_ascii_lowercase();
            *cursor += 1;
            aliases.insert(name, target);
        }
    }
}

fn parse_one_border(tokens: &[String], cursor: &mut usize) -> VguiBorderSpec {
    let mut spec = VguiBorderSpec::default();
    while *cursor < tokens.len() {
        let tok = &tokens[*cursor];
        if tok == "}" {
            *cursor += 1;
            break;
        }
        let key = tok.to_ascii_lowercase();
        *cursor += 1;
        if *cursor < tokens.len() && tokens[*cursor] == "{" {
            *cursor += 1;
            let lines = parse_border_side(tokens, cursor);
            match key.as_str() {
                "left" => spec.left = lines,
                "right" => spec.right = lines,
                "top" => spec.top = lines,
                "bottom" => spec.bottom = lines,
                _ => {}
            }
        } else if *cursor < tokens.len() && tokens[*cursor] != "}" {
            let val = &tokens[*cursor];
            *cursor += 1;
            if key == "inset" {
                let nums: Vec<i32> = val
                    .split_whitespace()
                    .filter_map(|n| n.parse::<i32>().ok())
                    .collect();
                if nums.len() >= 4 {
                    spec.inset = [nums[0], nums[1], nums[2], nums[3]];
                }
            }
        }
    }
    spec
}

fn parse_border_side(tokens: &[String], cursor: &mut usize) -> Vec<VguiBorderLine> {
    let mut out = Vec::new();
    while *cursor < tokens.len() {
        let tok = &tokens[*cursor];
        if tok == "}" {
            *cursor += 1;
            break;
        }
        *cursor += 1;
        if *cursor < tokens.len() && tokens[*cursor] == "{" {
            *cursor += 1;
            let mut color = String::new();
            let mut offset = [0i32; 2];
            while *cursor < tokens.len() {
                let k = &tokens[*cursor];
                if k == "}" {
                    *cursor += 1;
                    break;
                }
                let kl = k.to_ascii_lowercase();
                *cursor += 1;
                if *cursor < tokens.len() && tokens[*cursor] != "}" && tokens[*cursor] != "{" {
                    let v = &tokens[*cursor];
                    *cursor += 1;
                    match kl.as_str() {
                        "color" => color = v.clone(),
                        "offset" => {
                            let mut nums =
                                v.split_whitespace().filter_map(|p| p.parse::<i32>().ok());
                            offset[0] = nums.next().unwrap_or(0);
                            offset[1] = nums.next().unwrap_or(0);
                        }
                        _ => {}
                    }
                }
            }
            if !color.is_empty() {
                out.push(VguiBorderLine { color, offset });
            }
        }
    }
    out
}

fn tokenize_scheme_kv(text: &str) -> Vec<String> {
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

/// Load any TGA image (weapon previews, class portraits, CS_logo, Steam icons) from VFS.
pub fn load_tga_image(vfs: &Vfs, path: &str) -> Result<RgbaImageData, SchemeError> {
    load_image_rgba(vfs, path)
}

/// Load TGA, BMP, PNG (auto-detected by extension or header) from VFS as RGBA8.
pub fn load_image_rgba(vfs: &Vfs, path: &str) -> Result<RgbaImageData, SchemeError> {
    let bytes = vfs.read_bytes(path)?;
    let lower = path.to_ascii_lowercase();
    let fmt = if lower.ends_with(".bmp") {
        Some(ImageFormat::Bmp)
    } else if lower.ends_with(".png") {
        Some(ImageFormat::Png)
    } else if lower.ends_with(".tga") {
        Some(ImageFormat::Tga)
    } else {
        None
    };
    let loaded = match fmt {
        Some(f) => image::load_from_memory_with_format(&bytes, f)
            .or_else(|_| image::load_from_memory(&bytes)),
        None => image::load_from_memory(&bytes),
    };
    let rgba = loaded
        .map_err(|source| SchemeError::Image {
            path: path.to_string(),
            source,
        })?
        .to_rgba8();
    Ok(RgbaImageData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Load a map overview BMP/TGA and key pure GoldSrc overview green `(0, 255, 0)` to transparent.
pub fn load_overview_image_rgba(vfs: &Vfs, path: &str) -> Result<RgbaImageData, SchemeError> {
    let mut img = load_image_rgba(vfs, path)?;
    for px in img.rgba.as_chunks_mut::<4>().0 {
        if px[0] <= 12 && px[1] >= 242 && px[2] <= 12 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 0;
        }
    }
    Ok(img)
}

const ALL_LANG_PREFIXES: &[&str] = &[
    "english",
    "russian",
    "german",
    "french",
    "spanish",
    "italian",
    "portuguese",
    "polish",
    "koreana",
    "korean",
    "schinese",
    "tchinese",
    "japanese",
    "thai",
    "turkish",
];

/// Valve VGUI2 `#Token` localization dictionary loaded from `resource/platform_*.txt`,
/// `resource/valve_*.txt`, `resource/gameui_*.txt`, `servers/serverbrowser_*.txt`,
/// `resource/cstrike_*.txt`, and `titles.txt` fallback (supports UTF-16LE/BE BOM and UTF-8).
#[derive(Debug, Default, Clone)]
pub struct LocalizationTable {
    tokens: HashMap<String, String>,
    titles_tokens: HashMap<String, String>,
    language: String,
}

impl LocalizationTable {
    pub fn load(vfs: &Vfs) -> Self {
        Self::load_language(vfs, "english")
    }

    pub fn load_language(vfs: &Vfs, lang: &str) -> Self {
        let mut table = Self {
            tokens: HashMap::new(),
            titles_tokens: HashMap::new(),
            language: lang.trim().to_ascii_lowercase(),
        };
        let load_set = |tbl: &mut Self, l: &str| {
            let paths = [
                format!("resource/platform_{l}.txt"),
                format!("resource/vgui_{l}.txt"),
                format!("resource/valve_{l}.txt"),
                format!("resource/gameui_{l}.txt"),
                format!("servers/serverbrowser_{l}.txt"),
                format!("resource/cstrike_{l}.txt"),
            ];
            for path in paths {
                if let Ok(bytes) = vfs.read_bytes(&path) {
                    let text = decode_valve_text(&bytes);
                    tbl.parse_into(&text);
                }
            }
        };
        load_set(&mut table, "english");
        let clean_lang = table.language.clone();
        if !clean_lang.is_empty() && clean_lang != "english" {
            load_set(&mut table, &clean_lang);
        }
        // Populate lowest-priority fallback from `titles.txt` so HUD/ShowMenu tokens like
        // `#Team_Select_Spect` resolve even when callers do not pass a separate `TitlesTable`.
        let titles = crate::vgui_scripts::TitlesTable::load(vfs);
        for (k, entry) in titles.iter() {
            table.titles_tokens.insert(k.clone(), entry.text.clone());
        }
        table
    }

    pub fn discover_languages(vfs: &Vfs) -> Vec<String> {
        let mut out = Vec::new();
        for &lang in ALL_LANG_PREFIXES {
            if vfs.exists(&format!("resource/cstrike_{lang}.txt"))
                || vfs.exists(&format!("resource/gameui_{lang}.txt"))
                || vfs.exists(&format!("servers/serverbrowser_{lang}.txt"))
            {
                out.push(lang.to_string());
            }
        }
        if out.is_empty() {
            out.push("english".to_string());
        }
        out
    }

    pub fn language(&self) -> &str {
        if self.language.is_empty() {
            "english"
        } else {
            &self.language
        }
    }

    /// Merge raw localization text (for tests or server/mod overrides).
    pub fn insert_text(&mut self, text: &str) {
        self.parse_into(text);
    }

    fn parse_into(&mut self, text: &str) {
        let mut in_tokens = false;
        let mut depth = 0i32;

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            if !in_tokens {
                if line.eq_ignore_ascii_case("\"Tokens\"") || line.eq_ignore_ascii_case("Tokens") {
                    in_tokens = true;
                }
                continue;
            }
            if line.starts_with('{') {
                depth += 1;
                continue;
            }
            if line.starts_with('}') {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
                continue;
            }
            if depth >= 1
                && let Some((k, v)) = parse_kv_quoted_line(line)
            {
                let unescaped = v.replace("\\n", "\n").replace("\\t", "    ");
                self.tokens.insert(k.to_ascii_lowercase(), unescaped);
            }
        }
    }

    /// Resolve a `#Token` string (or return the input untouched if not starting with `#` or not found).
    pub fn resolve<'a>(&'a self, raw: &'a str) -> &'a str {
        self.resolve_with_titles(raw, None)
    }

    /// Resolve a `#Token` via `LocalizationTable`, `cstrike_titlestxt_<token>`, or `titles.txt`.
    pub fn resolve_with_titles<'a>(
        &'a self,
        raw: &'a str,
        titles: Option<&'a crate::vgui_scripts::TitlesTable>,
    ) -> &'a str {
        let mut cur = raw;
        for _ in 0..2 {
            let Some(key) = cur.strip_prefix('#') else {
                break;
            };
            let lower = key.to_ascii_lowercase();
            let next = if let Some(val) = self.tokens.get(&lower) {
                val.as_str()
            } else {
                let titles_key = format!("cstrike_titlestxt_{lower}");
                if let Some(val) = self.tokens.get(&titles_key) {
                    val.as_str()
                } else if let Some(t) = titles
                    && let Some(entry) = t.get(key)
                {
                    entry.text.as_str()
                } else if let Some(val) = self.titles_tokens.get(&lower) {
                    val.as_str()
                } else {
                    break;
                }
            };
            if next == cur {
                break;
            }
            cur = next;
            if !cur.starts_with('#') {
                break;
            }
        }
        cur
    }

    /// Resolve `#Token` substrings anywhere inside a line (e.g. `\x01#Game_connected` or `1. #Cstrike_CT_Forces`),
    /// consulting both `LocalizationTable` and optional `TitlesTable`.
    pub fn resolve_inline(
        &self,
        text: &str,
        titles: Option<&crate::vgui_scripts::TitlesTable>,
    ) -> String {
        let mut current = text.to_string();
        for _ in 0..2 {
            if !current.contains('#') {
                return current;
            }
            let mut out = String::with_capacity(current.len());
            let bytes = current.as_bytes();
            let mut i = 0usize;
            let mut changed = false;
            while i < current.len() {
                if bytes[i] == b'#'
                    && let Some(ch_next) = current[i + 1..].chars().next()
                    && (ch_next.is_ascii_alphanumeric() || ch_next == '_')
                {
                    let mut end = i + 1;
                    while end < current.len() {
                        let b = bytes[end];
                        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                            end += 1;
                        } else {
                            break;
                        }
                    }
                    let token = &current[i..end];
                    let resolved = self.resolve_with_titles(token, titles);
                    if resolved != token {
                        changed = true;
                    }
                    out.push_str(resolved);
                    i = end;
                    continue;
                }
                if let Some(ch) = current[i..].chars().next() {
                    out.push(ch);
                    i += ch.len_utf8();
                } else {
                    break;
                }
            }
            current = out;
            if !changed {
                break;
            }
        }
        current
    }

    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

/// Positioned label inside a `cstrike/classes/<name>.res` preview panel.
#[derive(Debug, Clone)]
pub struct ClassResLabel {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub small_font: bool,
    pub bright: bool,
}

/// Parsed `cstrike/classes/<name>.res` preview panel (weapon specs, equipment info, or player class bio).
#[derive(Debug, Clone, Default)]
pub struct ClassResPanel {
    pub image_path: Option<String>,
    pub image_box: [f32; 4],  // [x, y, w, h] of imageBG / imageBorder
    pub image_rect: [f32; 4], // [x, y, w, h] of classimage
    pub labels: Vec<ClassResLabel>,
}

impl ClassResPanel {
    /// Load and resolve `classes/<name>.res` directly from VFS.
    pub fn load(vfs: &Vfs, name: &str, loc: &LocalizationTable) -> Option<Self> {
        let path = format!("classes/{name}.res");
        let text = vfs.read_string_lossy(&path).ok()?;
        Some(Self::parse(&text, loc))
    }

    /// Parse and resolve a `classes/<name>.res` body.
    pub fn parse(text: &str, loc: &LocalizationTable) -> Self {
        let blocks = parse_res_blocks(text);
        let mut panel = Self {
            image_path: None,
            image_box: [0.0, 0.0, 300.0, 80.0],
            image_rect: [0.0, 8.0, 256.0, 64.0],
            labels: Vec::new(),
        };

        let mut label_keys: Vec<(String, i32)> = Vec::new();
        for (block_name, props) in blocks {
            let ctrl = props.get("controlname").map(String::as_str).unwrap_or("");
            let x = parse_f32_prop(&props, "xpos", 0.0);
            let y = parse_f32_prop(&props, "ypos", 0.0);
            let w = parse_f32_prop(&props, "wide", 150.0);
            let h = parse_f32_prop(&props, "tall", 20.0);

            if block_name.eq_ignore_ascii_case("imagebg")
                || block_name.eq_ignore_ascii_case("imageborder")
            {
                panel.image_box = [x, y, w, h];
            } else if ctrl.eq_ignore_ascii_case("ImagePanel")
                && let Some(img) = props.get("image")
                && !img.is_empty()
            {
                let mut img_path = img.replace('\\', "/");
                if !img_path.to_ascii_lowercase().ends_with(".tga") {
                    img_path.push_str(".tga");
                }
                panel.image_path = Some(img_path);
                panel.image_rect = [x, y, w, h];
            } else if ctrl.eq_ignore_ascii_case("Label")
                && let Some(raw_text) = props.get("labeltext")
                && !raw_text.is_empty()
            {
                let resolved = loc.resolve(raw_text).to_string();
                let font = props
                    .get("font")
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();
                let small_font = font.contains("small");
                let bright = props.get("brighttext").map(String::as_str) == Some("1");
                let lbl = ClassResLabel {
                    x,
                    y,
                    w,
                    h,
                    text: resolved,
                    small_font,
                    bright,
                };
                // Valve EditablePanel::LoadControlSettings keys controls by `fieldName`, so a
                // later entry sharing `fieldName` at the same column (e.g. `classes/urban.res`
                // `className` + `infolabel` at `x=0`) overwrites the earlier entry, while paired
                // label/value columns in equipment `.res` files (`classes/kevlar.res` `pricelabel`
                // at `x=0` and `price` at `x=140`, both named `infolabel`) remain side by side.
                let key = (
                    props
                        .get("fieldname")
                        .map(String::as_str)
                        .unwrap_or(block_name.as_str())
                        .to_ascii_lowercase(),
                    x.round() as i32,
                );
                if let Some(idx) = label_keys.iter().position(|k| k == &key) {
                    panel.labels[idx] = lbl;
                } else {
                    label_keys.push(key);
                    panel.labels.push(lbl);
                }
            }
        }

        panel
    }
}

/// Load classic CS 1.6 map mission briefing from `maps/<map_name>.txt`.
pub fn load_map_briefing(vfs: &Vfs, map_name: &str) -> Option<String> {
    let clean = map_name.strip_suffix(".bsp").unwrap_or(map_name);
    let path = format!("maps/{clean}.txt");
    vfs.read_string_lossy(&path)
        .ok()
        .map(|s| s.replace("\r\n", "\n"))
}

pub fn decode_valve_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u16s: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let u16s: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn parse_kv_quoted_line(line: &str) -> Option<(String, String)> {
    let mut chars = line.chars().peekable();
    let k = read_quoted_token(&mut chars)?;
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
    let v = read_quoted_token(&mut chars)?;
    Some((k, v))
}

fn read_quoted_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    if chars.next()? != '"' {
        return None;
    }
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => out.push('\n'),
                    't' => out.push_str("    "),
                    '"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            }
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn parse_res_blocks(text: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut pending_name: Option<String> = None;
    let mut current_name: Option<String> = None;
    let mut current_props: HashMap<String, String> = HashMap::new();

    for raw_line in text.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "{" {
            depth += 1;
            if depth == 2 {
                current_name = pending_name.take();
                current_props.clear();
            }
            continue;
        }
        if line == "}" {
            if depth == 2
                && let Some(name) = current_name.take()
            {
                out.push((name, std::mem::take(&mut current_props)));
            }
            depth = (depth - 1).max(0);
            continue;
        }
        if depth == 1 {
            let trimmed = line.trim_matches('"');
            if !trimmed.is_empty() {
                pending_name = Some(trimmed.to_string());
            }
        } else if depth == 2
            && let Some((k, v)) = parse_kv_quoted_line(line)
        {
            current_props.insert(k.to_ascii_lowercase(), v);
        }
    }
    out
}

fn parse_f32_prop(props: &HashMap<String, String>, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vgui_scripts::TitlesTable;

    #[test]
    fn parses_scheme_colors_borders_and_base_settings() {
        let txt = r##"
        Scheme
        {
            Colors
            {
                "BorderBright" "136 145 128 255"
                "BorderDark"   "40 46 34 255"
                "ControlBG"    "76 88 68 255"
            }
            BaseSettings
            {
                "BgColor" "ControlBG"
                InGameDesktop
                {
                    "MenuColor" "238 174 0 255"
                }
            }
            Borders
            {
                ButtonBorder "RaisedBorder"
                RaisedBorder
                {
                    "inset" "0 0 1 1"
                    Left { "1" { "color" "BorderBright" "offset" "0 1" } }
                    Right { "1" { "color" "BorderDark" "offset" "0 0" } }
                }
            }
        }
        "##;
        let scheme = VguiScheme::parse(txt);
        assert_eq!(scheme.resolve_color("BgColor"), Some([76, 88, 68, 255]));
        assert_eq!(
            scheme.resolve_color("InGameDesktop/MenuColor"),
            Some([238, 174, 0, 255])
        );
        let b = scheme
            .resolve_border("ButtonBorder")
            .expect("ButtonBorder alias");
        assert_eq!(b.inset, [0, 0, 1, 1]);
        assert_eq!(b.left[0].color, "BorderBright");
        assert_eq!(b.left[0].offset, [0, 1]);
    }

    #[test]
    fn resolves_tokens_via_cstrike_titlestxt_and_titles_table_inline() {
        let mut loc = LocalizationTable::default();
        loc.insert_text(
            "\"Tokens\"\n{\n\"Cstrike_TitlesTXT_CTs_Win\" \"Counter-Terrorists Win!\"\n}",
        );
        let titles = TitlesTable::parse("Game_connected\n{\n%s connected\n}\n");
        assert_eq!(
            loc.resolve_with_titles("#CTs_Win", Some(&titles)),
            "Counter-Terrorists Win!"
        );
        assert_eq!(
            loc.resolve_with_titles("#Game_connected", Some(&titles)),
            "%s connected"
        );
        assert_eq!(
            loc.resolve_inline("\x01#Game_connected", Some(&titles)),
            "\x01%s connected"
        );

        let nested = TitlesTable::parse(
            "Game_Commencing\n{\n#Cstrike_TitlesTXT_Game_Commencing\n}\nCstrike_TitlesTXT_Game_Commencing\n{\nGame Commencing!\n}\n",
        );
        loc.insert_text(
            "\"Tokens\"\n{\n\"Cstrike_TitlesTXT_Game_Commencing\" \"Round Starting!\"\n}",
        );
        assert_eq!(
            loc.resolve_with_titles("#Game_Commencing", Some(&nested)),
            "Round Starting!"
        );

        let Ok(vfs) = Vfs::discover(None, cs_vfs::VfsConfig::default()) else {
            eprintln!("SKIPPED retail LocalizationTable titles.txt test: CS16_GAMEDIR not found");
            return;
        };
        let retail_loc = LocalizationTable::load(&vfs);
        for tok in [
            "#Game_connected",
            "#Game_join_ct",
            "#Game_join_terrorist",
            "#Team_Select_Spect",
        ] {
            let resolved = retail_loc.resolve(tok);
            assert!(
                !resolved.starts_with('#'),
                "expected {tok} to resolve cleanly, got {resolved:?}"
            );
        }
    }

    #[test]
    fn class_res_panel_overwrites_duplicate_field_name() {
        let res = r#"
        "classes/urban.res"
        {
            "className"
            {
                "ControlName" "Label"
                "fieldName" "infolabel"
                "xpos" "0"
                "ypos" "204"
                "wide" "300"
                "tall" "20"
                "labelText" "SEAL TEAM 6"
            }
            "infolabel"
            {
                "ControlName" "Label"
                "fieldName" "infolabel"
                "xpos" "0"
                "ypos" "228"
                "wide" "300"
                "tall" "80"
                "labelText" "ST-6 description"
                "font" "DefaultSmall"
            }
        }
        "#;
        let loc = LocalizationTable::default();
        let panel = ClassResPanel::parse(res, &loc);
        assert_eq!(panel.labels.len(), 1);
        assert_eq!(panel.labels[0].y, 228.0);
        assert_eq!(panel.labels[0].text, "ST-6 description");
        assert!(panel.labels[0].small_font);
    }
}
