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
                .and_then(|bytes| image::load_from_memory_with_format(&bytes, ImageFormat::Tga).ok())
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

/// Load any TGA image (weapon previews, class portraits, CS_logo, Steam icons) from VFS.
pub fn load_tga_image(vfs: &Vfs, path: &str) -> Result<RgbaImageData, SchemeError> {
    let bytes = vfs.read_bytes(path)?;
    let rgba = image::load_from_memory_with_format(&bytes, ImageFormat::Tga)
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

/// Valve VGUI2 `#Token` localization dictionary loaded from `resource/cstrike_english.txt`
/// and `resource/gameui_english.txt` (supports UTF-16LE BOM and UTF-8).
#[derive(Debug, Default, Clone)]
pub struct LocalizationTable {
    tokens: HashMap<String, String>,
}

impl LocalizationTable {
    pub fn load(vfs: &Vfs) -> Self {
        let mut table = Self::default();
        for path in [
            "resource/gameui_english.txt",
            "resource/cstrike_english.txt",
        ] {
            if let Ok(bytes) = vfs.read_bytes(path) {
                let text = decode_valve_text(&bytes);
                table.parse_into(&text);
            }
        }
        table
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
        if let Some(key) = raw.strip_prefix('#')
            && let Some(val) = self.tokens.get(&key.to_ascii_lowercase())
        {
            return val.as_str();
        }
        raw
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
    pub image_box: [f32; 4],   // [x, y, w, h] of imageBG / imageBorder
    pub image_rect: [f32; 4],  // [x, y, w, h] of classimage
    pub labels: Vec<ClassResLabel>,
}

impl ClassResPanel {
    /// Load and resolve `classes/<name>.res` directly from VFS.
    pub fn load(vfs: &Vfs, name: &str, loc: &LocalizationTable) -> Option<Self> {
        let path = format!("classes/{name}.res");
        let text = vfs.read_string_lossy(&path).ok()?;
        let blocks = parse_res_blocks(&text);
        let mut panel = Self {
            image_path: None,
            image_box: [0.0, 0.0, 300.0, 80.0],
            image_rect: [0.0, 8.0, 256.0, 64.0],
            labels: Vec::new(),
        };

        for (block_name, props) in blocks {
            let ctrl = props
                .get("controlname")
                .map(String::as_str)
                .unwrap_or("");
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
                panel.labels.push(ClassResLabel {
                    x,
                    y,
                    w,
                    h,
                    text: resolved,
                    small_font,
                    bright,
                });
            }
        }

        Some(panel)
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

fn decode_valve_text(bytes: &[u8]) -> String {
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
