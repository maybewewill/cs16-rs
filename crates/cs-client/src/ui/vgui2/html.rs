//! Structured GoldSrc VGUI2 HTML subset parser & renderer (`MOTD.res` / `TeamMenu.res` `HTML`).
//! Handles `<body>`, `<h1>..<h6>`, `<p>`, `<br>`, `<b>`/`<strong>`, `<i>`, `<font color/size>`,
//! `<center>`, `<ul>`/`<li>`, `<hr>`, `<a href>`, HTML entities, `<style>` CSS (`body`, `a`,
//! `a:hover`), 3- and 6-digit hex colors, and drops `<script>`.

use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{Color32, CursorIcon, FontId, RichText, Sense, Stroke, Vec2};

use crate::ui::vgui2::runtime::{VguiRuntime, VguiSchemeKind};

#[derive(Debug, Clone, PartialEq)]
pub struct HtmlSpan {
    pub text: String,
    pub color: Option<[u8; 4]>,
    pub bold: bool,
    pub italic: bool,
    pub size: f32,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HtmlBlock {
    Heading { level: u8, spans: Vec<HtmlSpan>, centered: bool },
    Paragraph { spans: Vec<HtmlSpan>, centered: bool },
    BulletItem(Vec<HtmlSpan>),
    HorizontalRule,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HtmlDoc {
    pub bg_color: Option<[u8; 4]>,
    pub text_color: [u8; 4],
    pub link_color: [u8; 4],
    pub link_hover_color: [u8; 4],
    pub link_underline: bool,
    pub link_hover_underline: bool,
    pub body_font_size: f32,
    pub line_height: Option<f32>,
    pub margin: f32,
    pub blocks: Vec<HtmlBlock>,
}

impl Default for HtmlDoc {
    fn default() -> Self {
        Self {
            bg_color: None,
            text_color: [255, 176, 0, 255],
            link_color: [255, 255, 255, 255],
            link_hover_color: [238, 238, 238, 255],
            link_underline: true,
            link_hover_underline: false,
            body_font_size: 14.0,
            line_height: None,
            margin: 8.0,
            blocks: Vec::new(),
        }
    }
}

pub fn parse_html_doc(html: &str) -> HtmlDoc {
    let mut doc = HtmlDoc::default();
    let mut current_spans: Vec<HtmlSpan> = Vec::new();
    let mut color_stack: Vec<[u8; 4]> = Vec::new();
    let mut size_stack: Vec<f32> = vec![doc.body_font_size];
    let mut bold_depth = 0u32;
    let mut italic_depth = 0u32;
    let mut center_depth = 0u32;
    let mut active_link: Option<String> = None;
    let mut heading_level: Option<u8> = None;
    let mut in_li = false;
    let mut skip_depth = 0u32;
    let mut in_style = false;
    let mut style_buf = String::new();

    let flush = |blocks: &mut Vec<HtmlBlock>,
                 spans: &mut Vec<HtmlSpan>,
                 heading: Option<u8>,
                 li: bool,
                 centered: bool| {
        if spans.is_empty() {
            return;
        }
        let taken = std::mem::take(spans);
        if let Some(level) = heading {
            blocks.push(HtmlBlock::Heading {
                level,
                spans: taken,
                centered,
            });
        } else if li {
            blocks.push(HtmlBlock::BulletItem(taken));
        } else {
            blocks.push(HtmlBlock::Paragraph {
                spans: taken,
                centered,
            });
        }
    };

    let mut chars = html.chars().peekable();
    let mut text_buf = String::new();
    while let Some(c) = chars.next() {
        if c == '<' {
            if in_style {
                style_buf.push_str(&text_buf);
            } else if skip_depth == 0 && !text_buf.is_empty() {
                let decoded = decode_entities(&text_buf);
                let clean = collapse_ws(&decoded);
                if !clean.is_empty() {
                    current_spans.push(HtmlSpan {
                        text: clean,
                        color: color_stack.last().copied(),
                        bold: bold_depth > 0 || heading_level.is_some(),
                        italic: italic_depth > 0,
                        size: *size_stack.last().unwrap_or(&doc.body_font_size),
                        link: active_link.clone(),
                    });
                }
            }
            text_buf.clear();

            let mut tag = String::new();
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
                tag.push(tc);
            }
            let trimmed = tag.trim();
            if trimmed.starts_with('!') || trimmed.starts_with('?') {
                continue;
            }
            let is_close = trimmed.starts_with('/');
            let body = trimmed.trim_start_matches('/').trim();
            let mut parts = body.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or("").to_ascii_lowercase();
            let attrs = parts.next().unwrap_or("");

            if name == "style" {
                if is_close {
                    in_style = false;
                    apply_css_rules(&style_buf, &mut doc);
                    size_stack[0] = doc.body_font_size;
                    style_buf.clear();
                } else {
                    in_style = true;
                }
                continue;
            }
            if in_style {
                continue;
            }
            if matches!(name.as_str(), "script" | "title" | "head" | "meta") {
                if name == "meta" {
                    continue;
                }
                if is_close {
                    skip_depth = skip_depth.saturating_sub(1);
                } else {
                    skip_depth += 1;
                }
                continue;
            }
            if skip_depth > 0 {
                continue;
            }

            match (name.as_str(), is_close) {
                ("body", false) => {
                    if let Some(bg) = extract_attr(attrs, "bgcolor").and_then(|v| parse_hex_color(&v)) {
                        doc.bg_color = Some(bg);
                    }
                    if let Some(fg) = extract_attr(attrs, "text").and_then(|v| parse_hex_color(&v)) {
                        doc.text_color = fg;
                    }
                    if let Some(lc) = extract_attr(attrs, "link").and_then(|v| parse_hex_color(&v)) {
                        doc.link_color = lc;
                    }
                }
                ("br", _) => {
                    if current_spans.is_empty() {
                        doc.blocks.push(HtmlBlock::Paragraph {
                            spans: Vec::new(),
                            centered: center_depth > 0,
                        });
                    } else {
                        flush(
                            &mut doc.blocks,
                            &mut current_spans,
                            heading_level,
                            in_li,
                            center_depth > 0,
                        );
                    }
                }
                ("hr", false) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                    doc.blocks.push(HtmlBlock::HorizontalRule);
                }
                ("p" | "div" | "tr", true) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                }
                ("p" | "div", false) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                }
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                    let lvl = name[1..].parse::<u8>().unwrap_or(1);
                    heading_level = Some(lvl);
                    size_stack.push(match lvl {
                        1 => 22.0,
                        2 => 18.0,
                        3 => 16.0,
                        _ => 14.5,
                    });
                }
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                    heading_level = None;
                    if size_stack.len() > 1 {
                        size_stack.pop();
                    }
                }
                ("li", false) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                    in_li = true;
                }
                ("li", true) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                    in_li = false;
                }
                ("b" | "strong", false) => bold_depth += 1,
                ("b" | "strong", true) => bold_depth = bold_depth.saturating_sub(1),
                ("i" | "em", false) => italic_depth += 1,
                ("i" | "em", true) => italic_depth = italic_depth.saturating_sub(1),
                ("center", false) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                    center_depth += 1;
                }
                ("center", true) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
                    center_depth = center_depth.saturating_sub(1);
                }
                ("font", false) => {
                    if let Some(c) = extract_attr(attrs, "color").and_then(|v| parse_hex_color(&v)) {
                        color_stack.push(c);
                    }
                    if let Some(sz) = extract_attr(attrs, "size").and_then(|v| v.parse::<f32>().ok()) {
                        let px = if sz <= 7.0 { 10.0 + sz * 2.0 } else { sz };
                        size_stack.push(px.clamp(10.0, 28.0));
                    }
                }
                ("font", true) => {
                    color_stack.pop();
                    if size_stack.len() > 1 {
                        size_stack.pop();
                    }
                }
                ("a", false) => active_link = extract_attr(attrs, "href"),
                ("a", true) => active_link = None,
                _ => {}
            }
        } else {
            text_buf.push(c);
        }
    }
    if !in_style && skip_depth == 0 && !text_buf.is_empty() {
        let clean = collapse_ws(&decode_entities(&text_buf));
        if !clean.is_empty() {
            current_spans.push(HtmlSpan {
                text: clean,
                color: color_stack.last().copied(),
                bold: bold_depth > 0 || heading_level.is_some(),
                italic: italic_depth > 0,
                size: *size_stack.last().unwrap_or(&doc.body_font_size),
                link: active_link,
            });
        }
    }
    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
    doc
}

/// Paint a parsed `HtmlDoc` inside an `egui::Ui` scrollable container.
pub fn paint_html_doc(ui: &mut egui::Ui, doc: &HtmlDoc) {
    let default_col = rgba_to_color32(doc.text_color);
    let link_col = rgba_to_color32(doc.link_color);
    for block in &doc.blocks {
        match block {
            HtmlBlock::HorizontalRule => {
                ui.separator();
            }
            HtmlBlock::Heading { spans, centered, .. } | HtmlBlock::Paragraph { spans, centered } => {
                let layout = if *centered {
                    egui::Layout::left_to_right(egui::Align::Center)
                } else {
                    egui::Layout::left_to_right(egui::Align::Min)
                };
                ui.with_layout(layout.with_main_wrap(true), |ui| {
                    paint_spans_basic(ui, spans, default_col, link_col, doc.link_underline);
                });
                ui.add_space(2.0);
            }
            HtmlBlock::BulletItem(spans) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("•").color(default_col));
                    paint_spans_basic(ui, spans, default_col, link_col, doc.link_underline);
                });
            }
        }
    }
}

/// Paint a parsed `HtmlDoc` using `VguiRuntime`'s GDI-calibrated scheme font atlas and return
/// any clicked `<a href="...">` URL.
pub fn paint_html_doc_vgui(
    ui: &mut egui::Ui,
    doc: &HtmlDoc,
    runtime: &mut VguiRuntime,
    vfs: Option<&Vfs>,
    scheme_kind: VguiSchemeKind,
    screen_h: u32,
) -> Option<String> {
    let default_col = rgba_to_color32(doc.text_color);
    let link_col = rgba_to_color32(doc.link_color);
    let hover_col = rgba_to_color32(doc.link_hover_color);
    let mut clicked_url = None;
    let ctx = ui.ctx().clone();
    let scheme_fonts = runtime.scheme_fonts(scheme_kind).clone();
    let scheme_tag = scheme_kind.tag();
    let prev_item_spacing = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing = Vec2::ZERO;

    for (b_idx, block) in doc.blocks.iter().enumerate() {
        match block {
            HtmlBlock::HorizontalRule => {
                ui.separator();
            }
            HtmlBlock::Heading { spans, centered, .. } | HtmlBlock::Paragraph { spans, centered } => {
                if spans.is_empty() {
                    ui.add_space(doc.line_height.unwrap_or(14.0));
                    continue;
                }
                let layout = if *centered {
                    egui::Layout::left_to_right(egui::Align::Center)
                } else {
                    egui::Layout::left_to_right(egui::Align::Min)
                };
                ui.with_layout(layout.with_main_wrap(true), |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for (s_idx, sp) in spans.iter().enumerate() {
                        let font_name = if sp.size >= 18.0 {
                            "Title"
                        } else if sp.size < 13.0 {
                            "DefaultSmall"
                        } else {
                            "Default"
                        };
                        let base_tall = scheme_fonts
                            .resolve(font_name, screen_h)
                            .or_else(|| scheme_fonts.resolve("Default", screen_h))
                            .map_or(16.0, |v| v.tall.max(8.0));
                        let tall_scale = (sp.size / (base_tall * 0.82)).clamp(0.6, 3.0);
                        let measured = runtime.font_atlas.measure_text_with_scale(
                            &ctx,
                            vfs,
                            &scheme_fonts,
                            scheme_tag,
                            font_name,
                            screen_h,
                            &sp.text,
                            tall_scale,
                        );
                        let row_h = doc
                            .line_height
                            .unwrap_or_else(|| (sp.size * 1.25).round())
                            .max(measured.y);
                        let sz = Vec2::new(measured.x.max(4.0), row_h);
                        let (rect, resp) = ui.allocate_exact_size(
                            sz,
                            if sp.link.is_some() { Sense::click() } else { Sense::hover() },
                        );
                        if let Some(ref url) = sp.link {
                            let id = format!("html_link_{b_idx}_{s_idx}");
                            runtime.track_interactive(&id, &resp);
                            if resp.hovered() {
                                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                            }
                            if resp.clicked() {
                                clicked_url = Some(url.clone());
                            }
                            let col = if resp.hovered() { hover_col } else { link_col };
                            runtime.font_atlas.paint_text_with_scale(
                                ui.painter(),
                                &ctx,
                                vfs,
                                &scheme_fonts,
                                scheme_tag,
                                font_name,
                                screen_h,
                                rect,
                                &sp.text,
                                col,
                                TextAlignment::West,
                                false,
                                tall_scale,
                            );
                            let draw_ul = if resp.hovered() { doc.link_hover_underline } else { doc.link_underline };
                            if draw_ul {
                                let y = (rect.center().y + measured.y * 0.42).round().min(rect.bottom() - 1.0);
                                ui.painter().line_segment(
                                    [egui::Pos2::new(rect.left(), y), egui::Pos2::new(rect.right(), y)],
                                    Stroke::new(1.0, col),
                                );
                            }
                        } else {
                            let col = sp.color.map_or(default_col, rgba_to_color32);
                            runtime.font_atlas.paint_text_with_scale(
                                ui.painter(),
                                &ctx,
                                vfs,
                                &scheme_fonts,
                                scheme_tag,
                                font_name,
                                screen_h,
                                rect,
                                &sp.text,
                                col,
                                TextAlignment::West,
                                false,
                                tall_scale,
                            );
                        }
                    }
                });
            }
            HtmlBlock::BulletItem(spans) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("•").color(default_col));
                    paint_spans_basic(ui, spans, default_col, link_col, doc.link_underline);
                });
            }
        }
    }
    ui.spacing_mut().item_spacing = prev_item_spacing;
    clicked_url
}

fn paint_spans_basic(
    ui: &mut egui::Ui,
    spans: &[HtmlSpan],
    default_col: Color32,
    link_col: Color32,
    link_underline: bool,
) {
    for sp in spans {
        let col = sp.color.map_or(default_col, rgba_to_color32);
        let mut rt = RichText::new(&sp.text)
            .color(if sp.link.is_some() { link_col } else { col })
            .font(FontId::proportional(sp.size));
        if sp.bold {
            rt = rt.strong();
        }
        if sp.italic {
            rt = rt.italics();
        }
        if sp.link.is_some() && link_underline {
            rt = rt.underline();
        }
        ui.label(rt);
    }
}

fn apply_css_rules(css: &str, doc: &mut HtmlDoc) {
    for block in css.split('}') {
        let Some((sel_raw, decls)) = block.split_once('{') else {
            continue;
        };
        let sel = sel_raw.trim().to_ascii_lowercase();
        for decl in decls.split(';') {
            let Some((prop_raw, val_raw)) = decl.split_once(':') else {
                continue;
            };
            let prop = prop_raw.trim().to_ascii_lowercase();
            let val = val_raw.trim();
            match sel.as_str() {
                "body" => match prop.as_str() {
                    "background" | "background-color" => {
                        if let Some(c) = val.split_whitespace().find_map(parse_hex_color) {
                            doc.bg_color = Some(c);
                        }
                    }
                    "color" => {
                        if let Some(c) = parse_hex_color(val) {
                            doc.text_color = c;
                        }
                    }
                    "font" | "font-size" => {
                        for tok in val.split_whitespace() {
                            if let Some((sz_tok, lh_tok)) = tok.split_once('/') {
                                if let Some(px) = sz_tok
                                    .strip_suffix("px")
                                    .and_then(|v| v.parse::<f32>().ok())
                                {
                                    doc.body_font_size = px.clamp(10.0, 28.0);
                                }
                                if let Some(lh) = lh_tok
                                    .strip_suffix("px")
                                    .and_then(|v| v.parse::<f32>().ok())
                                {
                                    doc.line_height = Some(lh.clamp(10.0, 40.0));
                                }
                                break;
                            }
                            if let Some(px) = tok
                                .strip_suffix("px")
                                .and_then(|v| v.parse::<f32>().ok())
                            {
                                doc.body_font_size = px.clamp(10.0, 28.0);
                                break;
                            }
                        }
                    }
                    "line-height" => {
                        if let Ok(lh) = val
                            .trim_end_matches("px")
                            .trim()
                            .parse::<f32>()
                        {
                            doc.line_height = Some(lh.clamp(10.0, 40.0));
                        }
                    }
                    "margin" => {
                        if let Ok(m) = val.trim_end_matches("px").trim().parse::<f32>() {
                            doc.margin = m.clamp(0.0, 32.0);
                        }
                    }
                    _ => {}
                },
                "a" => match prop.as_str() {
                    "color" => {
                        if let Some(c) = parse_hex_color(val) {
                            doc.link_color = c;
                        }
                    }
                    "text-decoration" => {
                        doc.link_underline = !val.eq_ignore_ascii_case("none");
                    }
                    _ => {}
                },
                "a:hover" => match prop.as_str() {
                    "color" => {
                        if let Some(c) = parse_hex_color(val) {
                            doc.link_hover_color = c;
                        }
                    }
                    "text-decoration" => {
                        doc.link_hover_underline = !val.eq_ignore_ascii_case("none");
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

fn rgba_to_color32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    out.trim_end().to_string()
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn extract_attr(attrs: &str, key: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let needle = format!("{key}=");
    let idx = lower.find(&needle)?;
    let rest = attrs[idx + needle.len()..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        Some(stripped.split('"').next().unwrap_or("").to_string())
    } else if let Some(stripped) = rest.strip_prefix('\'') {
        Some(stripped.split('\'').next().unwrap_or("").to_string())
    } else {
        Some(rest.split_whitespace().next().unwrap_or("").to_string())
    }
}

fn parse_hex_color(raw: &str) -> Option<[u8; 4]> {
    let clean = raw.trim().trim_start_matches('#');
    match clean.to_ascii_lowercase().as_str() {
        "white" => return Some([255, 255, 255, 255]),
        "black" => return Some([0, 0, 0, 255]),
        "red" => return Some([255, 64, 64, 255]),
        "green" => return Some([80, 220, 80, 255]),
        "blue" => return Some([100, 160, 255, 255]),
        "yellow" | "gold" => return Some([255, 200, 50, 255]),
        "orange" => return Some([255, 160, 0, 255]),
        _ => {}
    }
    if clean.len() == 3
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&clean[0..1], 16),
            u8::from_str_radix(&clean[1..2], 16),
            u8::from_str_radix(&clean[2..3], 16),
        )
    {
        return Some([r * 17, g * 17, b * 17, 255]);
    }
    if clean.len() == 6
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&clean[0..2], 16),
            u8::from_str_radix(&clean[2..4], 16),
            u8::from_str_radix(&clean[4..6], 16),
        )
    {
        return Some([r, g, b, 255]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_retail_motd_css_and_clickable_link() {
        let retail_motd = r#"<!DOCTYPE HTML>
<html><head><title>Cstrike MOTD</title>
<style type="text/css">
body { background: #000; margin: 8px; color: #FFB000; font: normal 16px/20px Verdana, Tahoma, sans-serif; }
a { color: #FFF; text-decoration: underline; }
a:hover { color: #EEE; text-decoration: none; }
</style></head><body>
You are playing Counter-Strike v1.6<br>
Visit the official CS web site @<br>
www.counter-strike.net<br>
<a href="http://www.counter-strike.net">Visit Counter-Strike.net</a>
</body></html>"#;
        let doc = parse_html_doc(retail_motd);
        assert_eq!(doc.bg_color, Some([0, 0, 0, 255]));
        assert_eq!(doc.text_color, [255, 176, 0, 255]);
        assert_eq!(doc.link_color, [255, 255, 255, 255]);
        assert_eq!(doc.link_hover_color, [238, 238, 238, 255]);
        assert!(doc.link_underline);
        assert!(!doc.link_hover_underline);
        assert_eq!(doc.blocks.len(), 4);
        match &doc.blocks[3] {
            HtmlBlock::Paragraph { spans, .. } => {
                assert_eq!(spans[0].text, "Visit Counter-Strike.net");
                assert_eq!(spans[0].link.as_deref(), Some("http://www.counter-strike.net"));
            }
            other => panic!("expected link paragraph, got {other:?}"),
        }
    }
}
