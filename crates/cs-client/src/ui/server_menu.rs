//! The menus the server asks for with `ShowMenu`: the team and class choice, the buy menus,
//! the radio menus. The client draws the text and answers with `menuselect <n>`; nothing about
//! teams or items is known here.

use cs_assets::scheme::LocalizationTable;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};

const AMBER: Color32 = Color32::from_rgb(255, 176, 0);

/// The lines of a menu text: `#Token` lines looked up, the colour escapes (`\w` white, `\y`
/// yellow, `\r` red, `\d` grey, `\R` right-align) removed.
pub fn menu_lines(raw: &str, loc: Option<&LocalizationTable>) -> Vec<String> {
    let resolved = match loc {
        Some(l) if raw.starts_with('#') => l.resolve(raw).to_string(),
        _ => raw.to_string(),
    };
    resolved
        .lines()
        .map(|line| {
            let line = match loc {
                Some(l) if line.starts_with('#') => l.resolve(line),
                _ => line,
            };
            let mut out = String::new();
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' && chars.peek().is_some_and(|n| "wyrdRWYD".contains(*n)) {
                    chars.next();
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect()
}

/// The key `1..=9`, `0` (as 10) a menu line answers with, if the mask enables it.
pub fn menu_key_enabled(keys: u16, digit: u8) -> bool {
    let bit = if digit == 0 { 9 } else { digit - 1 };
    keys & (1 << bit) != 0
}

/// Parsed GoldSrc/AMX text menu span colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuSpanColor {
    White,
    Yellow,
    Red,
    Grey,
}

impl MenuSpanColor {
    pub fn to_color32(self) -> Color32 {
        match self {
            Self::White => Color32::from_rgb(240, 240, 240),
            Self::Yellow => Color32::from_rgb(255, 210, 64),
            Self::Red => Color32::from_rgb(255, 72, 56),
            Self::Grey => Color32::from_rgb(138, 138, 138),
        }
    }
}

/// One styled line of a GoldSrc/AMX Mod X menu: left colour spans plus optional `\R` right-aligned text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledMenuLine {
    pub spans: Vec<(MenuSpanColor, String)>,
    pub right: Option<String>,
}

/// Parse `ShowMenu` text preserving `\w` white, `\y` yellow, `\r` red, `\d` grey, and `\R` right-align.
pub fn menu_styled_lines(raw: &str, loc: Option<&LocalizationTable>) -> Vec<StyledMenuLine> {
    let resolved = match loc {
        Some(l) if raw.starts_with('#') => l.resolve(raw).to_string(),
        _ => raw.to_string(),
    };
    let mut color = MenuSpanColor::Yellow;
    resolved
        .lines()
        .map(|line| {
            let line = match loc {
                Some(l) if line.starts_with('#') => l.resolve(line),
                _ => line,
            };
            let mut spans: Vec<(MenuSpanColor, String)> = Vec::new();
            let mut cur = String::new();
            let mut right: Option<String> = None;
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\'
                    && let Some(&tag) = chars.peek()
                    && "wyrdRWYD".contains(tag)
                {
                    chars.next();
                    if tag == 'R' {
                        if !cur.is_empty() {
                            spans.push((color, std::mem::take(&mut cur)));
                        }
                        right = Some(String::new());
                        continue;
                    }
                    let next_col = match tag {
                        'w' | 'W' => MenuSpanColor::White,
                        'y' | 'Y' => MenuSpanColor::Yellow,
                        'r' => MenuSpanColor::Red,
                        'd' | 'D' => MenuSpanColor::Grey,
                        _ => color,
                    };
                    if next_col != color && !cur.is_empty() && right.is_none() {
                        spans.push((color, std::mem::take(&mut cur)));
                    }
                    color = next_col;
                } else if let Some(r) = right.as_mut() {
                    r.push(c);
                } else {
                    cur.push(c);
                }
            }
            if !cur.is_empty() {
                spans.push((color, cur));
            }
            StyledMenuLine { spans, right }
        })
        .collect()
}

/// Draws the menu on the left of the screen with full AMX `\w`/`\y`/`\r`/`\d` colour spans and `\R` alignment.
pub fn draw_styled_text_menu(ctx: &egui::Context, lines: &[StyledMenuLine], scale: f32) {
    let screen = ctx.content_rect();
    let font = FontId::proportional(14.0 * scale);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_server_menu_styled"),
    ));
    let line_h = 18.0 * scale;
    let width = 348.0 * scale;
    let height = line_h * lines.len() as f32 + 20.0 * scale;
    let rect = Rect::from_min_size(
        Pos2::new(24.0 * scale, screen.center().y - height * 0.5),
        Vec2::new(width, height),
    );
    painter.rect(
        rect,
        0.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, 172),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 170)),
        egui::StrokeKind::Inside,
    );
    let mut y = rect.top() + 10.0 * scale;
    for line in lines {
        let mut x = rect.left() + 12.0 * scale;
        for (col, text) in &line.spans {
            let galley = painter.layout_no_wrap(text.clone(), font.clone(), col.to_color32());
            let adv = galley.size().x;
            painter.galley(Pos2::new(x, y), galley, col.to_color32());
            x += adv;
        }
        if let Some(r_text) = &line.right {
            painter.text(
                Pos2::new(rect.right() - 12.0 * scale, y),
                Align2::RIGHT_TOP,
                r_text,
                font.clone(),
                MenuSpanColor::Yellow.to_color32(),
            );
        }
        y += line_h;
    }
}

/// Draws the menu on the left of the screen, as the text menus of the original are.
pub fn draw_text_menu(ctx: &egui::Context, lines: &[String], scale: f32) {
    let screen = ctx.content_rect();
    let font = FontId::proportional(14.0 * scale);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_server_menu"),
    ));
    let line_h = 18.0 * scale;
    let width = 330.0 * scale;
    let height = line_h * lines.len() as f32 + 20.0 * scale;
    let rect = Rect::from_min_size(
        Pos2::new(24.0 * scale, screen.center().y - height * 0.5),
        Vec2::new(width, height),
    );
    painter.rect(
        rect,
        0.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, 160),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 160)),
        egui::StrokeKind::Inside,
    );
    let mut y = rect.top() + 10.0 * scale;
    for line in lines {
        painter.text(
            Pos2::new(rect.left() + 12.0 * scale, y),
            Align2::LEFT_TOP,
            line,
            font.clone(),
            AMBER,
        );
        y += line_h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_escapes_and_lines() {
        let lines = menu_lines("\\yBuy\n\n\\w1. \\rAK-47\\R$2500", None);
        assert_eq!(lines, ["Buy", "", "1. AK-47$2500"]);
    }

    #[test]
    fn the_key_mask_selects_digits() {
        let keys = 0b10_0000_0011;
        assert!(menu_key_enabled(keys, 1));
        assert!(menu_key_enabled(keys, 2));
        assert!(!menu_key_enabled(keys, 3));
        assert!(menu_key_enabled(keys, 0));
    }
}
