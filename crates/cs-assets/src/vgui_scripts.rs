//! GoldSrc text scripts driving CS 1.6 VGUI2 menus & HUD:
//! - `overviews/<map>.txt` (`OverviewSpec`)
//! - `settings.scr` & `user.scr` (`SettingsScript`)
//! - `commandmenu.txt` & `spectatormenu.txt` (`CommandMenuTree`)
//! - `titles.txt` (`TitlesTable`)
//! - `resource/GameMenu.res` (`GameMenuEntry`)

use std::collections::HashMap;

use cs_vfs::Vfs;

use crate::scheme::decode_valve_text;

/// `overviews/<map>.txt` tactical map projection specification.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewSpec {
    pub zoom: f32,
    pub origin: [f32; 3],
    pub rotated: bool,
    pub image: String,
    pub height: f32,
}

impl OverviewSpec {
    pub fn load(vfs: &Vfs, map_name: &str) -> Option<Self> {
        let clean = map_name.strip_suffix(".bsp").unwrap_or(map_name);
        let path = format!("overviews/{clean}.txt");
        let text = vfs.read_string_lossy(&path).ok()?;
        Some(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let tokens = tokenize_scr(text);
        let mut spec = Self {
            zoom: 1.0,
            origin: [0.0; 3],
            rotated: false,
            image: String::new(),
            height: 0.0,
        };
        let mut i = 0usize;
        while i < tokens.len() {
            match tokens[i].to_ascii_uppercase().as_str() {
                "ZOOM" if i + 1 < tokens.len() => {
                    spec.zoom = tokens[i + 1].parse().unwrap_or(1.0);
                    i += 2;
                }
                "ORIGIN" if i + 3 < tokens.len() => {
                    spec.origin = [
                        tokens[i + 1].parse().unwrap_or(0.0),
                        tokens[i + 2].parse().unwrap_or(0.0),
                        tokens[i + 3].parse().unwrap_or(0.0),
                    ];
                    i += 4;
                }
                "ROTATED" if i + 1 < tokens.len() => {
                    spec.rotated = tokens[i + 1] == "1";
                    i += 2;
                }
                "IMAGE" if i + 1 < tokens.len() => {
                    spec.image = tokens[i + 1].replace('\\', "/");
                    i += 2;
                }
                "HEIGHT" if i + 1 < tokens.len() => {
                    spec.height = tokens[i + 1].parse().unwrap_or(0.0);
                    i += 2;
                }
                _ => i += 1,
            }
        }
        spec
    }

    /// Project world `[x, y]` coordinates into normalized `[0..1, 0..1]` overview image UV space.
    /// Uses standard GoldSrc `CHudSpectator::UpdateMap`: `4096 / zoom` world width, `1024x768` 4:3 frame.
    pub fn world_to_overview_uv(&self, world_xy: [f32; 2]) -> [f32; 2] {
        let zoom = self.zoom.max(0.05);
        let x_scale = (4.0 / 3.0) * 1024.0 / zoom; // world span X (~5.46 * 256 / zoom)
        let y_scale = 1024.0 / zoom;
        let dx = world_xy[0] - self.origin[0];
        let dy = world_xy[1] - self.origin[1];
        if self.rotated {
            [0.5 + dx / (y_scale * (4.0 / 3.0) * 3.0), 0.5 - dy / (x_scale * 2.25)]
        } else {
            let half_w = 4096.0 / zoom;
            let half_h = half_w * 0.75;
            [0.5 + dx / (half_w * 2.0), 0.5 - dy / (half_h * 2.0)]
        }
    }
}

/// Option types in Valve `settings.scr` / `user.scr`.
#[derive(Debug, Clone, PartialEq)]
pub enum ScrOptionKind {
    Bool,
    Number { min: f32, max: f32 },
    String,
    List(Vec<(String, String)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScrOption {
    pub cvar: String,
    pub prompt: String,
    pub kind: ScrOptionKind,
    pub default: String,
}

#[derive(Debug, Clone, Default)]
pub struct SettingsScript {
    pub version: String,
    pub options: Vec<ScrOption>,
}

impl SettingsScript {
    pub fn load(vfs: &Vfs, path: &str) -> Option<Self> {
        let bytes = vfs.read_bytes(path).ok()?;
        let text = decode_valve_text(&bytes);
        Some(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let tokens = tokenize_scr(text);
        let mut script = Self {
            version: "1.0".to_string(),
            options: Vec::new(),
        };
        let mut i = 0usize;
        while i < tokens.len() {
            if tokens[i].eq_ignore_ascii_case("VERSION") && i + 1 < tokens.len() {
                script.version = tokens[i + 1].clone();
                i += 2;
                continue;
            }
            if tokens[i].eq_ignore_ascii_case("DESCRIPTION") && i + 2 < tokens.len() {
                i += 2; // skip DESCRIPTION name
                if i < tokens.len() && tokens[i] == "{" {
                    i += 1;
                    while i < tokens.len() && tokens[i] != "}" {
                        let cvar = tokens[i].clone();
                        i += 1;
                        if i < tokens.len() && tokens[i] == "{" {
                            i += 1;
                            if let Some(opt) = parse_scr_option(cvar, &tokens, &mut i) {
                                script.options.push(opt);
                            }
                        }
                    }
                    if i < tokens.len() && tokens[i] == "}" {
                        i += 1;
                    }
                    continue;
                }
            }
            i += 1;
        }
        script
    }
}

fn parse_scr_option(cvar: String, tokens: &[String], i: &mut usize) -> Option<ScrOption> {
    let prompt = tokens.get(*i)?.clone();
    *i += 1;
    let mut kind = ScrOptionKind::String;
    if tokens.get(*i).map(String::as_str) == Some("{") {
        *i += 1;
        let ty = tokens.get(*i).map(|s| s.to_ascii_uppercase()).unwrap_or_default();
        *i += 1;
        match ty.as_str() {
            "BOOL" => kind = ScrOptionKind::Bool,
            "NUMBER" => {
                let min = tokens.get(*i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
                let max = tokens.get(*i + 1).and_then(|s| s.parse::<f32>().ok()).unwrap_or(-1.0);
                *i += 2;
                kind = ScrOptionKind::Number { min, max };
            }
            "LIST" => {
                let mut pairs = Vec::new();
                while *i + 1 < tokens.len() && tokens[*i] != "}" {
                    let lbl = tokens[*i].clone();
                    let val = tokens[*i + 1].clone();
                    *i += 2;
                    pairs.push((lbl, val));
                }
                kind = ScrOptionKind::List(pairs);
            }
            _ => {}
        }
        while *i < tokens.len() && tokens[*i] != "}" {
            *i += 1;
        }
        if *i < tokens.len() {
            *i += 1;
        }
    }
    let mut default = String::new();
    if tokens.get(*i).map(String::as_str) == Some("{") {
        *i += 1;
        if *i < tokens.len() && tokens[*i] != "}" {
            default = tokens[*i].clone();
            *i += 1;
        }
        while *i < tokens.len() && tokens[*i] != "}" {
            *i += 1;
        }
        if *i < tokens.len() {
            *i += 1;
        }
    }
    while *i < tokens.len() && tokens[*i] != "}" {
        *i += 1;
    }
    if *i < tokens.len() {
        *i += 1;
    }
    Some(ScrOption {
        cvar,
        prompt,
        kind,
        default,
    })
}

/// Entry inside `commandmenu.txt` or `spectatormenu.txt`.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandMenuAction {
    Command(String),
    Submenu(Vec<CommandMenuItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandMenuItem {
    pub key: String,
    pub label: String,
    pub map_filter: Option<String>,
    pub custom: bool,
    pub toggle_default: Option<bool>,
    pub action: CommandMenuAction,
}

#[derive(Debug, Clone, Default)]
pub struct CommandMenuTree {
    pub items: Vec<CommandMenuItem>,
}

impl CommandMenuTree {
    pub fn load(vfs: &Vfs, path: &str) -> Option<Self> {
        let bytes = vfs.read_bytes(path).ok()?;
        let text = decode_valve_text(&bytes);
        Some(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let tokens = tokenize_scr(text);
        let mut cursor = 0usize;
        Self {
            items: parse_cmd_items(&tokens, &mut cursor),
        }
    }
}

fn parse_cmd_items(tokens: &[String], cursor: &mut usize) -> Vec<CommandMenuItem> {
    let mut out = Vec::new();
    while *cursor < tokens.len() {
        if tokens[*cursor] == "}" {
            *cursor += 1;
            break;
        }
        let mut custom = false;
        let mut map_filter = None;
        let mut toggle_default = None;
        loop {
            let Some(tok) = tokens.get(*cursor) else {
                return out;
            };
            if tok.eq_ignore_ascii_case("CUSTOM") {
                custom = true;
                *cursor += 1;
            } else if tok.eq_ignore_ascii_case("MAP") && *cursor + 1 < tokens.len() {
                map_filter = Some(tokens[*cursor + 1].clone());
                *cursor += 2;
            } else if tok.eq_ignore_ascii_case("TOGGLE") && *cursor + 1 < tokens.len() {
                toggle_default = Some(tokens[*cursor + 1].eq_ignore_ascii_case("true") || tokens[*cursor + 1] == "1");
                *cursor += 2;
            } else {
                break;
            }
        }
        let Some(key) = tokens.get(*cursor).cloned() else { break };
        *cursor += 1;
        let Some(label) = tokens.get(*cursor).cloned() else { break };
        *cursor += 1;
        if tokens.get(*cursor).map(String::as_str) == Some("{") {
            *cursor += 1;
            let children = parse_cmd_items(tokens, cursor);
            out.push(CommandMenuItem {
                key,
                label,
                map_filter,
                custom,
                toggle_default,
                action: CommandMenuAction::Submenu(children),
            });
        } else if let Some(cmd) = tokens.get(*cursor).cloned() {
            *cursor += 1;
            out.push(CommandMenuItem {
                key,
                label,
                map_filter,
                custom,
                toggle_default,
                action: CommandMenuAction::Command(cmd),
            });
        }
    }
    out
}

/// Entry from `titles.txt` (`HudText` / `TE_TEXTMESSAGE`).
#[derive(Debug, Clone, PartialEq)]
pub struct TitleEntry {
    pub name: String,
    pub position: [f32; 2],
    pub effect: u8,
    pub color: [u8; 4],
    pub color2: [u8; 4],
    pub fadein: f32,
    pub fadeout: f32,
    pub holdtime: f32,
    pub fxtime: f32,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct TitlesTable {
    entries: HashMap<String, TitleEntry>,
}

impl TitlesTable {
    pub fn load(vfs: &Vfs) -> Self {
        let Ok(bytes) = vfs.read_bytes("titles.txt") else {
            return Self::default();
        };
        let text = decode_valve_text(&bytes);
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Self {
        let mut position = [-1.0f32, -1.0];
        let mut effect = 0u8;
        let mut color = [255u8, 180, 30, 255];
        let mut color2 = [255u8, 255, 255, 255];
        let mut fadein = 0.25f32;
        let mut fadeout = 0.5f32;
        let mut holdtime = 3.0f32;
        let mut fxtime = 0.25f32;
        let mut entries = HashMap::new();

        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0usize;
        while i < lines.len() {
            let line = lines[i].split("//").next().unwrap_or("").trim();
            i += 1;
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('$') {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if let Some(&directive) = parts.first() {
                    match directive.to_ascii_lowercase().as_str() {
                        "position" if parts.len() >= 3 => {
                            position = [parts[1].parse().unwrap_or(-1.0), parts[2].parse().unwrap_or(-1.0)];
                        }
                        "effect" if parts.len() >= 2 => effect = parts[1].parse().unwrap_or(0),
                        "color" if parts.len() >= 4 => {
                            color = [
                                parts[1].parse().unwrap_or(255),
                                parts[2].parse().unwrap_or(180),
                                parts[3].parse().unwrap_or(30),
                                255,
                            ];
                        }
                        "color2" if parts.len() >= 4 => {
                            color2 = [
                                parts[1].parse().unwrap_or(255),
                                parts[2].parse().unwrap_or(255),
                                parts[3].parse().unwrap_or(255),
                                255,
                            ];
                        }
                        "fadein" if parts.len() >= 2 => fadein = parts[1].parse().unwrap_or(0.25),
                        "fadeout" if parts.len() >= 2 => fadeout = parts[1].parse().unwrap_or(0.5),
                        "holdtime" if parts.len() >= 2 => holdtime = parts[1].parse().unwrap_or(3.0),
                        "fxtime" if parts.len() >= 2 => fxtime = parts[1].parse().unwrap_or(0.25),
                        _ => {}
                    }
                }
                continue;
            }
            // Look ahead for `{ ... }` block
            let mut j = i;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j < lines.len() && lines[j].trim() == "{" {
                j += 1;
                let mut body = Vec::new();
                while j < lines.len() && lines[j].trim() != "}" {
                    body.push(lines[j].trim_end());
                    j += 1;
                }
                if j < lines.len() && lines[j].trim() == "}" {
                    j += 1;
                }
                i = j;
                let name = line.trim_matches('"').to_string();
                entries.insert(
                    name.to_ascii_lowercase(),
                    TitleEntry {
                        name,
                        position,
                        effect,
                        color,
                        color2,
                        fadein,
                        fadeout,
                        holdtime,
                        fxtime,
                        text: body.join("\n").trim().to_string(),
                    },
                );
            }
        }
        Self { entries }
    }

    pub fn get(&self, name: &str) -> Option<&TitleEntry> {
        self.entries
            .get(&name.trim().trim_start_matches('#').to_ascii_lowercase())
    }
}

/// Entry from `resource/GameMenu.res`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameMenuEntry {
    pub label: String,
    pub command: String,
    pub only_in_game: bool,
    pub not_single: bool,
    pub not_multi: bool,
}

impl GameMenuEntry {
    pub fn load(vfs: &Vfs) -> Vec<Self> {
        let Ok(bytes) = vfs.read_bytes("resource/GameMenu.res") else {
            return Vec::new();
        };
        let text = decode_valve_text(&bytes);
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Vec<Self> {
        let doc = crate::vgui_res::VguiResDocument::parse("resource/GameMenu.res", text);
        doc.controls
            .into_iter()
            .filter_map(|c| {
                let label = c.props.get("label").cloned().unwrap_or(c.label_text);
                if label.is_empty() && c.command.is_empty() {
                    return None;
                }
                Some(Self {
                    label,
                    command: c.command,
                    only_in_game: c.props.get("onlyingame").map(String::as_str) == Some("1"),
                    not_single: c.props.get("notsingle").map(String::as_str) == Some("1"),
                    not_multi: c.props.get("notmulti").map(String::as_str) == Some("1"),
                })
            })
            .collect()
    }
}

fn tokenize_scr(text: &str) -> Vec<String> {
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
            while let Some(ch) = chars.next() {
                if ch == '\\' {
                    if let Some(esc) = chars.next() {
                        match esc {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            '"' => s.push('"'),
                            other => {
                                s.push('\\');
                                s.push(other);
                            }
                        }
                    }
                } else if ch == '"' {
                    break;
                } else {
                    s.push(ch);
                }
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
    fn parses_overview_scr_cmdmenu_and_titles() {
        let ov = OverviewSpec::parse("global { ZOOM 1.50 ORIGIN -223 1097 -192 ROTATED 0 } layer { IMAGE \"overviews/de_dust2.bmp\" HEIGHT -192 }");
        assert!((ov.zoom - 1.5).abs() < 1e-4);
        assert_eq!(ov.image, "overviews/de_dust2.bmp");
        let uv = ov.world_to_overview_uv([-223.0, 1097.0]);
        assert!((uv[0] - 0.5).abs() < 1e-4 && (uv[1] - 0.5).abs() < 1e-4);

        let scr = SettingsScript::parse(
            r#"VERSION 1.0 DESCRIPTION SERVER_OPTIONS { "mp_roundtime" { "Round" { NUMBER 1 15 } { "5" } } "mp_footsteps" { "Steps" { BOOL } { "1" } } }"#,
        );
        assert_eq!(scr.options.len(), 2);
        assert_eq!(scr.options[1].kind, ScrOptionKind::Bool);

        let cm = CommandMenuTree::parse(r#""1" "Help" { CUSTOM "1" "Brief" "!MAPBRIEFING" } "2" "Team" "chooseteam""#);
        assert_eq!(cm.items.len(), 2);

        let titles = TitlesTable::parse("$position -1 0.4\n$color 255 180 0\n\nGAMEOVER\n{\nMatch Finished\n}\n");
        assert_eq!(titles.get("GAMEOVER").unwrap().text, "Match Finished");
    }
}
