//! Recursive Valve KeyValues / VGUI2 `.res` layout parser and coordinate solver.
//! Supports UTF-16LE/UTF-8 files (`cstrike/resource/UI/*.res`, `valve/resource/*.res`,
//! `platform/servers/*.res`), proportional `c` / `r` / `f` offsets, `pinCorner`, and `autoResize`.

use std::collections::HashMap;

use cs_vfs::Vfs;

use crate::scheme::decode_valve_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Panel,
    Label,
    Button,
    MouseOverPanelButton,
    ImagePanel,
    Frame,
    ListPanel,
    RichText,
    ComboBox,
    Slider,
    CheckButton,
    TextEntry,
    PropertySheet,
    Html,
    ProgressBar,
    Divider,
    Menu,
    Unknown,
}

impl ControlKind {
    pub fn from_control_name(name: &str) -> Self {
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "panel" | "editablepanel" => Self::Panel,
            "label" => Self::Label,
            "button" => Self::Button,
            "mouseoverpanelbutton" => Self::MouseOverPanelButton,
            "imagepanel" | "cimagepanel" | "image" => Self::ImagePanel,
            "frame"
            | "cplayerlistdialog"
            | "cserverbrowserdialog"
            | "ccontentcontroldialog"
            | "coptionssubdialog" => Self::Frame,
            "listpanel" | "sectionedlistpanel" => Self::ListPanel,
            "richtext" => Self::RichText,
            "combobox" => Self::ComboBox,
            "slider" | "ccvarslider" => Self::Slider,
            "checkbutton" | "ccvarnegatedcheckbutton" | "ccvartogglecheckbutton" | "radiobutton" => {
                Self::CheckButton
            }
            "textentry" | "ccvartextentry" => Self::TextEntry,
            "propertysheet" | "propertydialog" => Self::PropertySheet,
            "html" => Self::Html,
            "progressbar" | "continuousprogressbar" => Self::ProgressBar,
            "divider" => Self::Divider,
            "menu" => Self::Menu,
            _ => {
                if lower.ends_with("dialog") || lower.ends_with("page") {
                    Self::Frame
                } else {
                    Self::Unknown
                }
            }
        }
    }
}

/// Valve VGUI2 coordinate token (`120`, `c-150`, `r80`, `f12`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VguiCoord {
    Abs(f32),
    CenterOffset(f32),
    RightOffset(f32),
    FillOffset(f32),
}

impl Default for VguiCoord {
    fn default() -> Self {
        Self::Abs(0.0)
    }
}

impl VguiCoord {
    pub fn parse(raw: &str) -> Self {
        let s = raw.trim().to_ascii_lowercase();
        if let Some(rest) = s.strip_prefix('c') {
            Self::CenterOffset(rest.parse::<f32>().unwrap_or(0.0))
        } else if let Some(rest) = s.strip_prefix('r') {
            Self::RightOffset(rest.parse::<f32>().unwrap_or(0.0))
        } else if let Some(rest) = s.strip_prefix('f') {
            Self::FillOffset(rest.parse::<f32>().unwrap_or(0.0))
        } else {
            Self::Abs(s.parse::<f32>().unwrap_or(0.0))
        }
    }

    pub fn resolve_pos(self, parent_origin: f32, parent_extent: f32, scale: f32) -> f32 {
        match self {
            Self::Abs(v) => parent_origin + v * scale,
            Self::CenterOffset(d) => parent_origin + parent_extent * 0.5 + d * scale,
            Self::RightOffset(d) => parent_origin + parent_extent - d * scale,
            Self::FillOffset(d) => parent_origin + d * scale,
        }
    }

    pub fn resolve_size(self, pos_rel: f32, parent_extent: f32, scale: f32) -> f32 {
        match self {
            Self::Abs(v) | Self::CenterOffset(v) => v * scale,
            Self::RightOffset(margin) | Self::FillOffset(margin) => {
                (parent_extent - pos_rel - margin * scale).max(0.0)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PinCorner {
    #[default]
    TopLeft = 0,
    TopRight = 1,
    BottomLeft = 2,
    BottomRight = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoResize {
    #[default]
    None = 0,
    Right = 1,
    Down = 2,
    DownAndRight = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlignment {
    NorthWest,
    North,
    NorthEast,
    #[default]
    West,
    Center,
    East,
    SouthWest,
    South,
    SouthEast,
}

impl TextAlignment {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "north-west" | "northwest" => Self::NorthWest,
            "north" => Self::North,
            "north-east" | "northeast" => Self::NorthEast,
            "center" => Self::Center,
            "east" => Self::East,
            "south-west" | "southwest" => Self::SouthWest,
            "south" => Self::South,
            "south-east" | "southeast" => Self::SouthEast,
            _ => Self::West,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VguiControlSpec {
    pub block_key: String,
    pub field_name: String,
    pub control_name: String,
    pub kind: ControlKind,
    pub xpos: VguiCoord,
    pub ypos: VguiCoord,
    pub wide: VguiCoord,
    pub tall: VguiCoord,
    pub zpos: i32,
    pub visible: bool,
    pub enabled: bool,
    pub tab_position: i32,
    pub pin_corner: PinCorner,
    pub auto_resize: AutoResize,
    pub label_text: String,
    pub text_alignment: TextAlignment,
    pub font: String,
    pub command: String,
    pub image: String,
    pub fill_color: Option<String>,
    pub bright_text: bool,
    pub dull_text: bool,
    pub default_button: bool,
    pub wrap: bool,
    pub scale_image: bool,
    pub text_hidden: bool,
    pub editable: bool,
    pub numeric_only: bool,
    pub title_bar_visible: bool,
    pub props: HashMap<String, String>,
    pub children: Vec<VguiControlSpec>,
}

#[derive(Debug, Clone, Default)]
pub struct VguiResDocument {
    pub source_path: String,
    pub root_name: String,
    pub controls: Vec<VguiControlSpec>,
}

impl VguiResDocument {
    pub fn load_from_vfs(vfs: &Vfs, path: &str) -> Option<Self> {
        let bytes = vfs.read_bytes(path).ok()?;
        let text = decode_valve_text(&bytes);
        Some(Self::parse(path, &text))
    }

    pub fn parse(source_path: &str, text: &str) -> Self {
        let tokens = tokenize_kv(text);
        let mut cursor = 0usize;
        let mut doc = Self {
            source_path: source_path.to_string(),
            root_name: String::new(),
            controls: Vec::new(),
        };
        while cursor < tokens.len() {
            let key = tokens[cursor].clone();
            cursor += 1;
            if cursor < tokens.len() && tokens[cursor] == "{" {
                doc.root_name = key;
                cursor += 1;
                doc.controls = parse_block_controls(&tokens, &mut cursor);
                break;
            }
        }
        doc
    }

    pub fn find(&self, field_or_key: &str) -> Option<&VguiControlSpec> {
        let target = field_or_key.to_ascii_lowercase();
        fn find_rec<'a>(items: &'a [VguiControlSpec], target: &str) -> Option<&'a VguiControlSpec> {
            for c in items {
                if c.field_name.to_ascii_lowercase() == target
                    || c.block_key.to_ascii_lowercase() == target
                {
                    return Some(c);
                }
                if let Some(sub) = find_rec(&c.children, target) {
                    return Some(sub);
                }
            }
            None
        }
        find_rec(&self.controls, &target)
    }
}

/// Compute screen `[x, y, w, h]` for `spec` inside `parent_rect = [px, py, pw, ph]` scaled from `base_w x base_h`.
pub fn resolve_control_rect(
    spec: &VguiControlSpec,
    parent_rect: [f32; 4],
    base_w: f32,
    base_h: f32,
) -> [f32; 4] {
    let [px, py, pw, ph] = parent_rect;
    let sx = if base_w > 0.0 { pw / base_w } else { 1.0 };
    let sy = if base_h > 0.0 { ph / base_h } else { 1.0 };
    let scale = sx.min(sy).max(0.5);
    let x = spec.xpos.resolve_pos(px, pw, scale);
    let y = spec.ypos.resolve_pos(py, ph, scale);
    let mut w = spec.wide.resolve_size(x - px, pw, scale);
    let mut h = spec.tall.resolve_size(y - py, ph, scale);
    match spec.auto_resize {
        AutoResize::Right => w = (px + pw - x - 8.0).max(w),
        AutoResize::Down => h = (py + ph - y - 8.0).max(h),
        AutoResize::DownAndRight => {
            w = (px + pw - x - 8.0).max(w);
            h = (py + ph - y - 8.0).max(h);
        }
        AutoResize::None => {}
    }
    [x, y, w.max(0.0), h.max(0.0)]
}

fn parse_block_controls(tokens: &[String], cursor: &mut usize) -> Vec<VguiControlSpec> {
    let mut controls = Vec::new();
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
            controls.push(parse_one_control(key, tokens, cursor));
        } else if *cursor < tokens.len() {
            // Skip stray key/value pair at root level
            *cursor += 1;
        }
    }
    controls.sort_by_key(|c| c.zpos);
    controls
}

fn parse_one_control(key: String, tokens: &[String], cursor: &mut usize) -> VguiControlSpec {
    let mut props: HashMap<String, String> = HashMap::new();
    let mut children = Vec::new();
    while *cursor < tokens.len() {
        let tok = &tokens[*cursor];
        if tok == "}" {
            *cursor += 1;
            break;
        }
        let k = tok.clone();
        *cursor += 1;
        if *cursor < tokens.len() && tokens[*cursor] == "{" {
            *cursor += 1;
            children.push(parse_one_control(k, tokens, cursor));
        } else if *cursor < tokens.len() {
            let v = tokens[*cursor].clone();
            *cursor += 1;
            props.insert(k.to_ascii_lowercase(), v);
        }
    }

    let get = |name: &str| props.get(name).map(String::as_str).unwrap_or("");
    let control_name = if !get("controlname").is_empty() {
        get("controlname").to_string()
    } else {
        "Panel".to_string()
    };
    let field_name = if !get("fieldname").is_empty() {
        get("fieldname").to_string()
    } else {
        key.clone()
    };
    let kind = ControlKind::from_control_name(&control_name);
    let mut image = get("image").replace('\\', "/");
    if !image.is_empty() && !image.contains('.') {
        image.push_str(".tga");
    }

    VguiControlSpec {
        block_key: key,
        field_name,
        control_name,
        kind,
        xpos: props.get("xpos").map_or(VguiCoord::Abs(0.0), |s| VguiCoord::parse(s)),
        ypos: props.get("ypos").map_or(VguiCoord::Abs(0.0), |s| VguiCoord::parse(s)),
        wide: props.get("wide").map_or(VguiCoord::Abs(64.0), |s| VguiCoord::parse(s)),
        tall: props.get("tall").map_or(VguiCoord::Abs(24.0), |s| VguiCoord::parse(s)),
        zpos: get("zpos").parse().unwrap_or(0),
        visible: get("visible") != "0",
        enabled: get("enabled") != "0",
        tab_position: get("tabposition").parse().unwrap_or(0),
        pin_corner: match get("pincorner").parse::<u8>().unwrap_or(0) {
            1 => PinCorner::TopRight,
            2 => PinCorner::BottomLeft,
            3 => PinCorner::BottomRight,
            _ => PinCorner::TopLeft,
        },
        auto_resize: match get("autoresize").parse::<u8>().unwrap_or(0) {
            1 => AutoResize::Right,
            2 => AutoResize::Down,
            3 => AutoResize::DownAndRight,
            _ => AutoResize::None,
        },
        label_text: get("labeltext").to_string(),
        text_alignment: TextAlignment::parse(get("textalignment")),
        font: get("font").to_string(),
        command: get("command").to_string(),
        image,
        fill_color: props.get("fillcolor").cloned(),
        bright_text: get("brighttext") == "1",
        dull_text: get("dulltext") == "1",
        default_button: get("default") == "1",
        wrap: get("wrap") == "1",
        scale_image: get("scaleimage") != "0",
        text_hidden: get("texthidden") == "1",
        editable: get("editable") != "0",
        numeric_only: get("numericinputonly") == "1",
        title_bar_visible: get("settitlebarvisible") != "0",
        props,
        children,
    }
}

fn tokenize_kv(text: &str) -> Vec<String> {
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
            let mut bare = String::from("/");
            while let Some(&nc) = chars.peek() {
                if nc.is_whitespace() || nc == '{' || nc == '}' || nc == '"' {
                    break;
                }
                bare.push(nc);
                chars.next();
            }
            out.push(bare);
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
                            '\\' => s.push('\\'),
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
    fn parses_res_coords_controls_and_nested_children() {
        let res = r##"
        "Resource/UI/Sample.res"
        {
            "Frame" { "ControlName" "Frame" "fieldName" "MainFrame" "wide" "640" "tall" "480" }
            "CenterLbl" { "ControlName" "Label" "xpos" "c-100" "ypos" "r40" "wide" "200" "tall" "24" "labelText" "#Hello" }
            "Holder" {
                "ControlName" "Panel"
                "ChildBtn" { "ControlName" "Button" "Command" "okay" "Default" "1" }
            }
        }
        "##;
        let doc = VguiResDocument::parse("Resource/UI/Sample.res", res);
        let lbl = doc.find("CenterLbl").expect("CenterLbl");
        assert_eq!(lbl.kind, ControlKind::Label);
        assert_eq!(lbl.xpos, VguiCoord::CenterOffset(-100.0));
        assert_eq!(lbl.ypos, VguiCoord::RightOffset(40.0));
        let rect = resolve_control_rect(lbl, [0.0, 0.0, 800.0, 600.0], 640.0, 480.0);
        assert!((rect[0] - 275.0).abs() < 1.0); // 400 - 100*1.25 = 275
        let child = doc.find("ChildBtn").expect("nested ChildBtn");
        assert_eq!(child.command, "okay");
        assert!(child.default_button);
    }
}
