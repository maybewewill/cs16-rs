//! Structured GoldSrc VGUI2 HTML subset parser & renderer (`MOTD.res` / `TeamMenu.res` `HTML`).
//! Handles `<body>`, `<h1>..<h6>`, `<p>`, `<br>`, `<b>`/`<strong>`, `<i>`, `<font color/size>`,
//! `<center>`, `<ul>`/`<li>`, `<hr>`, `<a href>`, HTML entities, and drops `<style>`/`<script>`.

use egui::{Color32, FontId, RichText};

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
    pub blocks: Vec<HtmlBlock>,
}

impl Default for HtmlDoc {
    fn default() -> Self {
        Self {
            bg_color: Some([20, 22, 18, 255]),
            text_color: [216, 222, 211, 255],
            blocks: Vec::new(),
        }
    }
}

pub fn parse_html_doc(html: &str) -> HtmlDoc {
    let mut doc = HtmlDoc::default();
    let mut current_spans: Vec<HtmlSpan> = Vec::new();
    let mut color_stack: Vec<[u8; 4]> = Vec::new();
    let mut size_stack: Vec<f32> = vec![13.5];
    let mut bold_depth = 0u32;
    let mut italic_depth = 0u32;
    let mut center_depth = 0u32;
    let mut active_link: Option<String> = None;
    let mut heading_level: Option<u8> = None;
    let mut in_li = false;
    let mut skip_depth = 0u32;

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
            if skip_depth == 0 && !text_buf.is_empty() {
                let decoded = decode_entities(&text_buf);
                let clean = collapse_ws(&decoded);
                if !clean.is_empty() {
                    current_spans.push(HtmlSpan {
                        text: clean,
                        color: color_stack.last().copied(),
                        bold: bold_depth > 0 || heading_level.is_some(),
                        italic: italic_depth > 0,
                        size: *size_stack.last().unwrap_or(&13.5),
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
            let is_close = trimmed.starts_with('/');
            let body = trimmed.trim_start_matches('/').trim();
            let mut parts = body.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or("").to_ascii_lowercase();
            let attrs = parts.next().unwrap_or("");

            if matches!(name.as_str(), "style" | "script" | "title" | "head") {
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
                }
                ("br", _) => {
                    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
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
    if skip_depth == 0 && !text_buf.is_empty() {
        let clean = collapse_ws(&decode_entities(&text_buf));
        if !clean.is_empty() {
            current_spans.push(HtmlSpan {
                text: clean,
                color: color_stack.last().copied(),
                bold: bold_depth > 0 || heading_level.is_some(),
                italic: italic_depth > 0,
                size: *size_stack.last().unwrap_or(&13.5),
                link: active_link,
            });
        }
    }
    flush(&mut doc.blocks, &mut current_spans, heading_level, in_li, center_depth > 0);
    doc
}

/// Paint a parsed `HtmlDoc` inside an `egui::Ui` scrollable container.
pub fn paint_html_doc(ui: &mut egui::Ui, doc: &HtmlDoc) {
    let default_col = Color32::from_rgba_unmultiplied(
        doc.text_color[0],
        doc.text_color[1],
        doc.text_color[2],
        doc.text_color[3],
    );
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
                    paint_spans(ui, spans, default_col);
                });
                ui.add_space(4.0);
            }
            HtmlBlock::BulletItem(spans) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("•").color(default_col));
                    paint_spans(ui, spans, default_col);
                });
            }
        }
    }
}

fn paint_spans(ui: &mut egui::Ui, spans: &[HtmlSpan], default_col: Color32) {
    for sp in spans {
        let col = sp.color.map_or(default_col, |[r, g, b, a]| {
            Color32::from_rgba_unmultiplied(r, g, b, a)
        });
        let mut rt = RichText::new(&sp.text)
            .color(if sp.link.is_some() { Color32::from_rgb(130, 195, 255) } else { col })
            .font(FontId::proportional(sp.size));
        if sp.bold {
            rt = rt.strong();
        }
        if sp.italic {
            rt = rt.italics();
        }
        if sp.link.is_some() {
            rt = rt.underline();
        }
        ui.label(rt);
    }
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
