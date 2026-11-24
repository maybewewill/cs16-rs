//! GPU texture-atlas renderer backed by `cs_assets::vgui_font`'s Windows-GDI-matched
//! TrueType rasterizer (`NEAREST` filtering, integer-snapped pen coordinates, 1-bit non-AA
//! below threshold, baked 1px outline & `(+1,+1)` dropshadow, explicit missing-font report).

use std::collections::{HashMap, HashSet};

use cs_assets::vgui_font::{
    MissingSchemeFont, RasterizeOptions, SchemeFontLoadEntry, SchemeFontLoadReport,
    SchemeFontSet, SchemeFontVariant, VguiTrueTypeFont, resolve_font_bytes,
};
use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{Color32, FontId, Pos2, Rect, Stroke, TextureHandle, Vec2};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TtfCacheKey {
    family: String,
    bold: bool,
    italic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AtlasTierKey {
    scheme_tag: String,
    font_name: String,
    screen_h_bucket: u32,
    /// The pixel height the tier was built at: proportional and fixed-size requests of the same
    /// font must not share one tier.
    tall_px: u32,
}

#[derive(Debug, Clone, Copy)]
struct PackedGlyph {
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    width: f32,
    height: f32,
    offset_x: f32,
    offset_y: f32,
    advance_x: f32,
}

struct FontTierAtlas {
    ttf: Option<VguiTrueTypeFont>,
    tall: f32,
    ascent: f32,
    line_height: f32,
    opts: RasterizeOptions,
    atlas_w: u32,
    atlas_h: u32,
    pixels: Vec<u8>,
    pen_x: u32,
    pen_y: u32,
    row_h: u32,
    glyphs: HashMap<char, PackedGlyph>,
    texture: Option<TextureHandle>,
    dirty: bool,
    tex_id: String,
}

impl FontTierAtlas {
    fn new(tex_id: String, ttf: Option<VguiTrueTypeFont>, variant: &SchemeFontVariant, tall: f32) -> Self {
        let mut opts = RasterizeOptions::from(variant);
        opts.tall = tall;
        let (ascent, line_height) = ttf.as_ref().map_or((tall * 0.8, tall), |f| {
            let m = f.metrics(tall);
            (m.ascent, m.line_height)
        });
        let atlas_w = 512u32;
        let atlas_h = 256u32;
        Self {
            ttf,
            tall,
            ascent,
            line_height,
            opts,
            atlas_w,
            atlas_h,
            pixels: vec![0u8; (atlas_w * atlas_h * 4) as usize],
            pen_x: 1,
            pen_y: 1,
            row_h: 0,
            glyphs: HashMap::new(),
            texture: None,
            dirty: false,
            tex_id,
        }
    }

    fn ensure_glyph(&mut self, ch: char) {
        if self.glyphs.contains_key(&ch) {
            return;
        }
        let Some(ref ttf) = self.ttf else {
            return;
        };
        let Some(bmp) = ttf.rasterize_glyph_rgba(ch, &self.opts) else {
            return;
        };
        if bmp.width == 0 || bmp.height == 0 {
            self.glyphs.insert(
                ch,
                PackedGlyph {
                    uv_min: [0.0, 0.0],
                    uv_max: [0.0, 0.0],
                    width: 0.0,
                    height: 0.0,
                    offset_x: bmp.offset_x as f32,
                    offset_y: bmp.offset_y as f32,
                    advance_x: bmp.advance_x as f32,
                },
            );
            return;
        }

        if self.pen_x + bmp.width + 1 >= self.atlas_w {
            self.pen_x = 1;
            self.pen_y += self.row_h + 1;
            self.row_h = 0;
        }
        if self.pen_y + bmp.height + 1 >= self.atlas_h {
            let new_h = (self.atlas_h * 2).max(self.pen_y + bmp.height + 2);
            self.pixels.resize((self.atlas_w * new_h * 4) as usize, 0);
            let old_h = self.atlas_h;
            self.atlas_h = new_h;
            let scale_y = old_h as f32 / new_h as f32;
            for g in self.glyphs.values_mut() {
                g.uv_min[1] *= scale_y;
                g.uv_max[1] *= scale_y;
            }
        }

        let gx = self.pen_x;
        let gy = self.pen_y;
        for row in 0..bmp.height {
            let src_off = (row * bmp.width * 4) as usize;
            let dst_off = (((gy + row) * self.atlas_w + gx) * 4) as usize;
            let len = (bmp.width * 4) as usize;
            if src_off + len <= bmp.rgba.len() && dst_off + len <= self.pixels.len() {
                self.pixels[dst_off..dst_off + len]
                    .copy_from_slice(&bmp.rgba[src_off..src_off + len]);
            }
        }

        let packed = PackedGlyph {
            uv_min: [
                gx as f32 / self.atlas_w as f32,
                gy as f32 / self.atlas_h as f32,
            ],
            uv_max: [
                (gx + bmp.width) as f32 / self.atlas_w as f32,
                (gy + bmp.height) as f32 / self.atlas_h as f32,
            ],
            width: bmp.width as f32,
            height: bmp.height as f32,
            offset_x: bmp.offset_x as f32,
            offset_y: bmp.offset_y as f32,
            advance_x: bmp.advance_x as f32,
        };
        self.glyphs.insert(ch, packed);
        self.pen_x += bmp.width + 1;
        self.row_h = self.row_h.max(bmp.height);
        self.dirty = true;
    }

    fn ensure_text_and_upload(&mut self, ctx: &egui::Context, text: &str) -> Option<&TextureHandle> {
        self.ttf.as_ref()?;
        if self.glyphs.is_empty() {
            for code in 32u32..=126 {
                if let Some(c) = char::from_u32(code) {
                    self.ensure_glyph(c);
                }
            }
        }
        for c in text.chars() {
            if c != '\n' && c != '\r' {
                self.ensure_glyph(c);
            }
        }
        if self.dirty || self.texture.is_none() {
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [self.atlas_w as usize, self.atlas_h as usize],
                &self.pixels,
            );
            let tex = ctx.load_texture(&self.tex_id, img, egui::TextureOptions::NEAREST);
            self.texture = Some(tex);
            self.dirty = false;
        }
        self.texture.as_ref()
    }

    fn measure_line(&self, text: &str) -> f32 {
        let Some(ref ttf) = self.ttf else {
            return text.chars().count() as f32 * self.tall * 0.52;
        };
        let mut w = 0.0f32;
        let mut prev: Option<char> = None;
        for c in text.chars() {
            if let Some(p) = prev {
                w += ttf.kern_i32(p, c, self.tall) as f32;
            }
            if let Some(g) = self.glyphs.get(&c) {
                w += g.advance_x;
            } else {
                w += ttf.advance_i32(c, self.tall) as f32;
            }
            prev = Some(c);
        }
        w
    }
}

/// Optional text draw options for VGUI2 controls and HUD strings.
#[derive(Debug, Clone, Copy)]
pub struct VguiTextDrawOpts {
    pub align: TextAlignment,
    pub process_hotkey: bool,
    pub proportional_scale: bool,
}

impl Default for VguiTextDrawOpts {
    fn default() -> Self {
        Self {
            align: TextAlignment::West,
            process_hotkey: false,
            proportional_scale: true,
        }
    }
}

/// Cached TrueType font + NEAREST GDI-matched glyph atlas registry for VGUI2 schemes.
#[derive(Default)]
pub struct VguiFontAtlas {
    ttfs: HashMap<TtfCacheKey, Option<VguiTrueTypeFont>>,
    tiers: HashMap<AtlasTierKey, FontTierAtlas>,
    report: SchemeFontLoadReport,
    recorded_keys: HashSet<String>,
}

impl VguiFontAtlas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load_report(&self) -> &SchemeFontLoadReport {
        &self.report
    }

    pub fn clear(&mut self) {
        self.ttfs.clear();
        self.tiers.clear();
        self.report = SchemeFontLoadReport::default();
        self.recorded_keys.clear();
    }

    fn effective_tall(font_name: &str, variant: &SchemeFontVariant, screen_h: u32, proportional_scale: bool) -> f32 {
        if proportional_scale
            && variant.yres_min.is_none()
            && variant.yres_max.is_none()
            && !variant.symbol
            && (font_name.eq_ignore_ascii_case("title") || variant.tall >= 16.0)
        {
            (variant.tall * (screen_h.max(480) as f32 / 480.0)).round()
        } else {
            variant.tall
        }
    }

    fn resolve_or_load_ttf(
        &mut self,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        variant: &SchemeFontVariant,
    ) -> Option<VguiTrueTypeFont> {
        let lower = variant.name.trim().to_ascii_lowercase();
        let bold = variant.weight >= 600 || lower.contains("bold");
        let italic = variant.italic || lower.contains("italic");
        let cache_key = TtfCacheKey {
            family: lower,
            bold,
            italic,
        };
        if let Some(existing) = self.ttfs.get(&cache_key) {
            return existing.clone();
        }
        let rec_id = format!("{scheme_tag}:{}:{}:{}", font_name.to_ascii_lowercase(), variant.name, variant.weight);
        match resolve_font_bytes(vfs, variant, &scheme_fonts.custom_font_files)
            .and_then(|(bytes, src)| VguiTrueTypeFont::from_bytes(bytes).map(|f| (f, src)))
        {
            Ok((ttf, source)) => {
                if self.recorded_keys.insert(rec_id) {
                    self.report.loaded.push(SchemeFontLoadEntry {
                        scheme_font: format!("{scheme_tag}/{font_name}"),
                        family: variant.name.clone(),
                        weight: variant.weight,
                        source,
                    });
                }
                self.ttfs.insert(cache_key, Some(ttf.clone()));
                Some(ttf)
            }
            Err(reason) => {
                if self.recorded_keys.insert(rec_id) {
                    tracing::warn!(scheme = scheme_tag, font = font_name, family = %variant.name, %reason, "VGUI2 scheme font unavailable");
                    self.report.missing.push(MissingSchemeFont {
                        scheme_font: format!("{scheme_tag}/{font_name}"),
                        family: variant.name.clone(),
                        weight: variant.weight,
                        reason,
                    });
                }
                self.ttfs.insert(cache_key, None);
                None
            }
        }
    }

    fn ensure_tier_with_tall<'a>(
        &'a mut self,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        tall: f32,
    ) -> Option<&'a mut FontTierAtlas> {
        let effective_name = if font_name.trim().is_empty() {
            "Default"
        } else {
            font_name
        };
        let variant = scheme_fonts
            .resolve(effective_name, screen_h)
            .or_else(|| scheme_fonts.resolve("Default", screen_h))
            .cloned()
            .unwrap_or_default();
        let rounded_tall = tall.round().max(1.0);
        let key = AtlasTierKey {
            scheme_tag: scheme_tag.to_string(),
            font_name: effective_name.to_ascii_lowercase(),
            screen_h_bucket: screen_h.max(240),
            tall_px: rounded_tall as u32,
        };
        if !self.tiers.contains_key(&key) {
            let ttf = self.resolve_or_load_ttf(vfs, scheme_fonts, scheme_tag, effective_name, &variant);
            let tex_id = format!(
                "vgui_font_{}_{}_{}_{}",
                scheme_tag,
                key.font_name,
                key.screen_h_bucket,
                key.tall_px
            );
            let tier = FontTierAtlas::new(tex_id, ttf, &variant, rounded_tall);
            self.tiers.insert(key.clone(), tier);
        }
        self.tiers.get_mut(&key)
    }

    fn ensure_tier<'a>(
        &'a mut self,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        proportional_scale: bool,
    ) -> Option<&'a mut FontTierAtlas> {
        let effective_name = if font_name.trim().is_empty() {
            "Default"
        } else {
            font_name
        };
        let variant = scheme_fonts
            .resolve(effective_name, screen_h)
            .or_else(|| scheme_fonts.resolve("Default", screen_h))
            .cloned()
            .unwrap_or_default();
        let tall = Self::effective_tall(effective_name, &variant, screen_h, proportional_scale);
        self.ensure_tier_with_tall(vfs, scheme_fonts, scheme_tag, effective_name, screen_h, tall)
    }

    /// Scale Windows GDI cell height (`TrackerScheme.res` `tall`) to `ab_glyph` em-height (`~0.82`),
    /// matching GDI Tahoma cap/cell metrics (`Options` 11px / `Player name` 12px at `1280x720`).
    fn resolved_tall_for_scale(
        scheme_fonts: &SchemeFontSet,
        font_name: &str,
        screen_h: u32,
        tall_scale: f32,
    ) -> f32 {
        const GDI_CELL_TO_EM: f32 = 0.82;
        let effective_name = if font_name.trim().is_empty() {
            "Default"
        } else {
            font_name
        };
        let base = scheme_fonts
            .resolve(effective_name, screen_h)
            .or_else(|| scheme_fonts.resolve("Default", screen_h))
            .map_or(16.0, |v| v.tall);
        (base * GDI_CELL_TO_EM * tall_scale).round().max(1.0)
    }

    /// Measure `(width, height)` with an explicit `tall_scale` multiplier on the scheme's base `tall`.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_text_with_scale(
        &mut self,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        text: &str,
        tall_scale: f32,
    ) -> Vec2 {
        let tall = Self::resolved_tall_for_scale(scheme_fonts, font_name, screen_h, tall_scale);
        let Some(tier) =
            self.ensure_tier_with_tall(vfs, scheme_fonts, scheme_tag, font_name, screen_h, tall)
        else {
            return Vec2::ZERO;
        };
        tier.ensure_text_and_upload(ctx, text);
        Vec2::new(tier.measure_line(text), tier.line_height)
    }

    /// Measure `(width, height)` in pixels using GDI-calibrated scheme font metrics.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_text(
        &mut self,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        text: &str,
    ) -> Vec2 {
        self.measure_text_scaled(ctx, vfs, scheme_fonts, scheme_tag, font_name, screen_h, text, true)
    }

    /// [`measure_text`](Self::measure_text) with the same `proportional_scale` choice as the
    /// matching [`paint_text`](Self::paint_text) call, so the measured box fits the drawn text.
    #[allow(clippy::too_many_arguments)]
    pub fn measure_text_scaled(
        &mut self,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        text: &str,
        proportional_scale: bool,
    ) -> Vec2 {
        let Some(tier) =
            self.ensure_tier(vfs, scheme_fonts, scheme_tag, font_name, screen_h, proportional_scale)
        else {
            return Vec2::ZERO;
        };
        tier.ensure_text_and_upload(ctx, text);
        let w = tier.measure_line(text);
        Vec2::new(w, tier.line_height)
    }

    /// Paint integer-snapped scheme text inside `rect`, returning rendered `(width, height)`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_text(
        &mut self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        rect: Rect,
        text: &str,
        color: Color32,
        opts: VguiTextDrawOpts,
    ) -> Vec2 {
        let effective_name = if font_name.trim().is_empty() {
            "Default"
        } else {
            font_name
        };
        let variant = scheme_fonts
            .resolve(effective_name, screen_h)
            .or_else(|| scheme_fonts.resolve("Default", screen_h))
            .cloned()
            .unwrap_or_default();
        let tall = Self::effective_tall(effective_name, &variant, screen_h, opts.proportional_scale);
        self.paint_text_at_tall(
            painter, ctx, vfs, scheme_fonts, scheme_tag, effective_name, screen_h, rect, text,
            color, opts.align, opts.process_hotkey, tall,
        )
    }

    /// Paint scheme text scaling its base `tall` by `tall_scale` (e.g. `max(1, H/720)` for GameUI).
    #[allow(clippy::too_many_arguments)]
    pub fn paint_text_with_scale(
        &mut self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        rect: Rect,
        text: &str,
        color: Color32,
        align: TextAlignment,
        process_hotkey: bool,
        tall_scale: f32,
    ) -> Vec2 {
        let tall = Self::resolved_tall_for_scale(scheme_fonts, font_name, screen_h, tall_scale);
        self.paint_text_at_tall(
            painter, ctx, vfs, scheme_fonts, scheme_tag, font_name, screen_h, rect, text, color,
            align, process_hotkey, tall,
        )
    }

    /// Paint single- or multi-line wrapped scheme text inside `rect`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_wrapped_text_with_scale(
        &mut self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        rect: Rect,
        text: &str,
        color: Color32,
        align: TextAlignment,
        wrap: bool,
        tall_scale: f32,
    ) {
        if text.is_empty() {
            return;
        }
        let max_w = rect.width().max(8.0);
        let mut lines: Vec<String> = Vec::new();
        for para in text.split('\n') {
            if !wrap {
                lines.push(para.to_string());
                continue;
            }
            let mut cur = String::new();
            for word in para.split_whitespace() {
                let candidate = if cur.is_empty() {
                    word.to_string()
                } else {
                    format!("{cur} {word}")
                };
                let w = self
                    .measure_text_with_scale(
                        ctx, vfs, scheme_fonts, scheme_tag, font_name, screen_h, &candidate, tall_scale,
                    )
                    .x;
                if w <= max_w || cur.is_empty() {
                    cur = candidate;
                } else {
                    lines.push(cur);
                    cur = word.to_string();
                }
            }
            if !cur.is_empty() || para.is_empty() {
                lines.push(cur);
            }
        }
        let line_h = self
            .measure_text_with_scale(
                ctx, vfs, scheme_fonts, scheme_tag, font_name, screen_h, "Ay", tall_scale,
            )
            .y
            .max(12.0 * tall_scale);
        let total_h = lines.len() as f32 * line_h;
        let start_y = match align {
            TextAlignment::NorthWest | TextAlignment::North | TextAlignment::NorthEast => rect.top(),
            TextAlignment::SouthWest | TextAlignment::South | TextAlignment::SouthEast => {
                (rect.bottom() - total_h).max(rect.top())
            }
            _ => {
                if lines.len() == 1 {
                    rect.center().y - line_h * 0.5
                } else {
                    rect.top()
                }
            }
        };
        let row_align = match align {
            TextAlignment::Center | TextAlignment::North | TextAlignment::South => {
                TextAlignment::North
            }
            TextAlignment::East | TextAlignment::NorthEast | TextAlignment::SouthEast => {
                TextAlignment::NorthEast
            }
            _ => TextAlignment::NorthWest,
        };
        let tall = Self::resolved_tall_for_scale(scheme_fonts, font_name, screen_h, tall_scale);
        for (idx, line) in lines.iter().enumerate() {
            let y = (start_y + idx as f32 * line_h).round();
            let row_rect = Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(rect.width(), line_h + 2.0));
            self.paint_text_at_tall(
                painter, ctx, vfs, scheme_fonts, scheme_tag, font_name, screen_h, row_rect, line,
                color, row_align, false, tall,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_text_at_tall(
        &mut self,
        painter: &egui::Painter,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        scheme_fonts: &SchemeFontSet,
        scheme_tag: &str,
        font_name: &str,
        screen_h: u32,
        rect: Rect,
        text: &str,
        color: Color32,
        align: TextAlignment,
        process_hotkey: bool,
        tall: f32,
    ) -> Vec2 {
        let (clean, hotkey_idx) = if process_hotkey {
            strip_vgui_hotkey(text)
        } else {
            (text.to_string(), None)
        };
        if clean.is_empty() {
            return Vec2::ZERO;
        }
        let Some(tier) =
            self.ensure_tier_with_tall(vfs, scheme_fonts, scheme_tag, font_name, screen_h, tall)
        else {
            return Vec2::ZERO;
        };
        let tex_id = tier.ensure_text_and_upload(ctx, &clean).map(TextureHandle::id);
        let line_w = tier.measure_line(&clean);
        let line_h = tier.line_height;
        let ascent = tier.ascent;
        let tall = tier.tall;

        let Some(texture_id) = tex_id else {
            // Honest fallback draw ONLY when resolution failed (already recorded in report.missing).
            let (anchor, pos) = egui_align_pos(rect, align);
            painter.text(pos, anchor, &clean, FontId::proportional(tall), color);
            return Vec2::new(line_w, line_h);
        };

        let start_x = match align {
            TextAlignment::West | TextAlignment::NorthWest | TextAlignment::SouthWest => rect.left(),
            TextAlignment::Center | TextAlignment::North | TextAlignment::South => rect.center().x - line_w * 0.5,
            TextAlignment::East | TextAlignment::NorthEast | TextAlignment::SouthEast => rect.right() - line_w,
        }
        .round();
        let top_y = match align {
            TextAlignment::NorthWest | TextAlignment::North | TextAlignment::NorthEast => rect.top(),
            TextAlignment::West | TextAlignment::Center | TextAlignment::East => rect.center().y - line_h * 0.5,
            TextAlignment::SouthWest | TextAlignment::South | TextAlignment::SouthEast => rect.bottom() - line_h,
        }
        .round();
        let baseline_y = (top_y + ascent).round();

        let mut mesh = egui::Mesh::with_texture(texture_id);
        let mut pen_x = start_x;
        let mut prev_ch: Option<char> = None;
        let mut underline_span: Option<(f32, f32)> = None;

        for (idx, ch) in clean.chars().enumerate() {
            if let (Some(p), Some(ttf)) = (prev_ch, tier.ttf.as_ref()) {
                pen_x += ttf.kern_i32(p, ch, tall) as f32;
            }
            if let Some(g) = tier.glyphs.get(&ch).copied() {
                if hotkey_idx == Some(idx) {
                    underline_span = Some((pen_x, pen_x + g.advance_x.max(4.0)));
                }
                if g.width > 0.0 && g.height > 0.0 {
                    let gx = (pen_x + g.offset_x).round();
                    let gy = (baseline_y + g.offset_y).round();
                    let q = Rect::from_min_size(Pos2::new(gx, gy), Vec2::new(g.width, g.height));
                    let uv = Rect::from_min_max(
                        Pos2::new(g.uv_min[0], g.uv_min[1]),
                        Pos2::new(g.uv_max[0], g.uv_max[1]),
                    );
                    mesh.add_rect_with_uv(q, uv, color);
                }
                pen_x += g.advance_x;
            }
            prev_ch = Some(ch);
        }

        let clipped = painter.with_clip_rect(rect);
        clipped.add(egui::Shape::mesh(mesh));
        if let Some((u0, u1)) = underline_span {
            let uy = (baseline_y + 2.0).round();
            clipped.line_segment([Pos2::new(u0, uy), Pos2::new(u1, uy)], Stroke::new(1.0, color));
        }
        Vec2::new(line_w, line_h)
    }
}

pub fn strip_vgui_hotkey(text: &str) -> (String, Option<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut hotkey = None;
    let mut idx = 0usize;
    while let Some(c) = chars.next() {
        if c == '&' {
            if let Some(&next) = chars.peek() {
                if next == '&' {
                    chars.next();
                    out.push('&');
                    idx += 1;
                } else {
                    hotkey = Some(idx);
                }
            }
        } else {
            out.push(c);
            idx += 1;
        }
    }
    (out, hotkey)
}

fn egui_align_pos(rect: Rect, align: TextAlignment) -> (egui::Align2, Pos2) {
    match align {
        TextAlignment::West => (egui::Align2::LEFT_CENTER, Pos2::new(rect.left() + 4.0, rect.center().y)),
        TextAlignment::Center => (egui::Align2::CENTER_CENTER, rect.center()),
        TextAlignment::East => (egui::Align2::RIGHT_CENTER, Pos2::new(rect.right() - 4.0, rect.center().y)),
        TextAlignment::NorthWest => (egui::Align2::LEFT_TOP, Pos2::new(rect.left() + 4.0, rect.top() + 2.0)),
        TextAlignment::North => (egui::Align2::CENTER_TOP, Pos2::new(rect.center().x, rect.top() + 2.0)),
        TextAlignment::NorthEast => (egui::Align2::RIGHT_TOP, Pos2::new(rect.right() - 4.0, rect.top() + 2.0)),
        TextAlignment::SouthWest => (egui::Align2::LEFT_BOTTOM, Pos2::new(rect.left() + 4.0, rect.bottom() - 2.0)),
        TextAlignment::South => (egui::Align2::CENTER_BOTTOM, Pos2::new(rect.center().x, rect.bottom() - 2.0)),
        TextAlignment::SouthEast => (egui::Align2::RIGHT_BOTTOM, Pos2::new(rect.right() - 4.0, rect.bottom() - 2.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_assets::scheme::VguiScheme;

    #[test]
    fn strip_hotkey_extracts_first_ampersand_position() {
        assert_eq!(strip_vgui_hotkey("&1 TERRORIST FORCE"), ("1 TERRORIST FORCE".to_string(), Some(0)));
        assert_eq!(strip_vgui_hotkey("Save && &Exit"), ("Save & Exit".to_string(), Some(7)));
    }

    #[test]
    fn client_and_tracker_scheme_atlases_pack_real_glyphs_if_installed() {
        let Some(root) = std::env::var_os("CS16_GAMEDIR") else {
            return;
        };
        let vfs = match cs_vfs::Vfs::open(root, cs_vfs::VfsConfig::default()) {
            Ok(v) => v,
            Err(_) => return,
        };
        let client_scheme = VguiScheme::load_client(&vfs);
        let tracker_scheme = VguiScheme::load_tracker(&vfs);
        let ctx = egui::Context::default();
        let mut atlas = VguiFontAtlas::new();

        let title_sz = atlas.measure_text(&ctx, Some(&vfs), &client_scheme.fonts, "client", "Title", 720, "Counter-Strike");
        assert!(title_sz.x > 40.0 && title_sz.y >= 18.0, "title_sz={title_sz:?}");

        let def_sz = atlas.measure_text(&ctx, Some(&vfs), &client_scheme.fonts, "client", "Default", 720, "1. Terrorist Force");
        assert!(def_sz.x > 40.0 && def_sz.y >= 12.0, "def_sz={def_sz:?}");

        let eng_sz = atlas.measure_text(&ctx, Some(&vfs), &tracker_scheme.fonts, "tracker", "EngineFont", 720, "Player connected");
        assert!(eng_sz.x > 40.0 && eng_sz.y >= 12.0, "eng_sz={eng_sz:?}");

        assert!(
            atlas.load_report().missing.is_empty(),
            "unexpected missing fonts: {:?}",
            atlas.load_report().missing
        );
    }
}
