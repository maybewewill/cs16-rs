//! Declarative VGUI2 `.res` runtime: caches VFS `.res` documents (supporting custom server/mod
//! overrides and reload), resolves scheme colors & fonts (including `marlett.ttf`),
//! computes responsive widget rectangles, paints controls, and handles headless events.

use std::collections::HashMap;

use cs_assets::scheme::{LocalizationTable, load_image_rgba};
use cs_assets::vgui_font::{SchemeFontSet, load_marlett_font};
use cs_assets::vgui_res::{ControlKind, VguiControlSpec, VguiResDocument, resolve_control_rect};
use cs_vfs::Vfs;
use egui::{Color32, FontFamily, FontId, Pos2, Rect, TextureHandle, Vec2};

use crate::ui::vgui2::controls::{
    VguiComboState, VguiControlState, VguiListPanelState, VguiPropertySheetState,
    VguiRichTextState, VguiSliderState, draw_vgui_button, draw_vgui_image_panel, draw_vgui_label,
    draw_vgui_panel, draw_vgui_progress_bar,
};
use crate::ui::vgui_theme::VguiPalette;

#[derive(Debug, Clone, PartialEq)]
pub enum VguiEvent {
    Command { field: String, command: String },
    ListDoubleClicked { field: String, row: usize },
    TabChanged { field: String, tab: usize },
    ValueChanged { field: String },
}

#[derive(Debug, Clone)]
pub struct VguiLayoutNode {
    pub field_name: String,
    pub control_name: String,
    pub kind: ControlKind,
    pub rect: [f32; 4],
    pub visible: bool,
    pub enabled: bool,
    pub label: String,
    pub command: String,
}

/// Install Valve's `resource/marlett.ttf` (if available in VFS) into `egui` as `"marlett"` font family.
pub fn install_marlett_into_egui(ctx: &egui::Context, vfs: &Vfs) -> bool {
    let Some(tt) = load_marlett_font(vfs) else {
        return false;
    };
    let mut defs = ctx.fonts(|r| r.definitions().clone());
    defs.font_data.insert(
        "marlett".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(tt.raw_bytes().to_vec())),
    );
    defs.families.insert(
        FontFamily::Name("marlett".into()),
        vec!["marlett".to_owned()],
    );
    ctx.set_fonts(defs);
    true
}

pub fn resolve_vgui_text_color(spec: &VguiControlSpec, palette: VguiPalette) -> Color32 {
    if !spec.enabled {
        palette.dim_text
    } else if spec.bright_text {
        palette.bright_control_text
    } else if spec.dull_text {
        palette.dim_text
    } else {
        palette.base_text
    }
}

/// VGUI2 declarative `.res` layout interpreter + stateful widget bank.
pub struct VguiRuntime {
    docs: HashMap<String, VguiResDocument>,
    pub scheme_fonts: SchemeFontSet,
    pub states: HashMap<String, VguiControlState>,
    textures: HashMap<String, TextureHandle>,
    events: Vec<VguiEvent>,
}

impl Default for VguiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl VguiRuntime {
    pub fn new() -> Self {
        Self {
            docs: HashMap::new(),
            scheme_fonts: SchemeFontSet::default(),
            states: HashMap::new(),
            textures: HashMap::new(),
            events: Vec::new(),
        }
    }

    pub fn load_scheme_fonts(&mut self, vfs: &Vfs) {
        self.scheme_fonts = SchemeFontSet::load(vfs, "resource/TrackerScheme.res");
    }

    /// Clear cached `.res` layouts (e.g. after mounting server/mod resource packs).
    pub fn invalidate_cache(&mut self) {
        self.docs.clear();
        self.textures.clear();
    }

    /// Load or fetch cached `.res` document from VFS (automatically picks up custom server `.res` layers).
    pub fn load_res(&mut self, vfs: &Vfs, path: &str) -> Option<&VguiResDocument> {
        let key = path.replace('\\', "/").to_ascii_lowercase();
        if !self.docs.contains_key(&key) {
            let doc = VguiResDocument::load_from_vfs(vfs, &key)?;
            self.docs.insert(key.clone(), doc);
        }
        self.docs.get(&key)
    }

    pub fn insert_res(&mut self, path: &str, text: &str) {
        let key = path.replace('\\', "/").to_ascii_lowercase();
        let doc = VguiResDocument::parse(&key, text);
        self.docs.insert(key, doc);
    }

    pub fn set_state(&mut self, field: &str, state: VguiControlState) {
        self.states.insert(field.to_ascii_lowercase(), state);
    }

    pub fn get_state(&self, field: &str) -> Option<&VguiControlState> {
        self.states.get(&field.to_ascii_lowercase())
    }

    pub fn get_state_mut(&mut self, field: &str) -> Option<&mut VguiControlState> {
        self.states.get_mut(&field.to_ascii_lowercase())
    }

    pub fn take_events(&mut self) -> Vec<VguiEvent> {
        std::mem::take(&mut self.events)
    }

    /// Deterministic headless event API for automated tests & keyboard shortcuts.
    pub fn trigger_button(&mut self, doc_path: &str, field: &str) -> Option<String> {
        let key = doc_path.replace('\\', "/").to_ascii_lowercase();
        let doc = self.docs.get(&key)?;
        let spec = doc.find(field)?;
        if !spec.visible || !spec.enabled {
            return None;
        }
        let cmd = spec.command.clone();
        self.events.push(VguiEvent::Command {
            field: spec.field_name.clone(),
            command: cmd.clone(),
        });
        Some(cmd)
    }

    pub fn sort_list(&mut self, field: &str, col: usize) -> bool {
        let Some(VguiControlState::List(list)) = self.get_state_mut(field) else {
            return false;
        };
        list.sort_by(col);
        true
    }

    pub fn select_tab(&mut self, field: &str, tab: usize) -> bool {
        let Some(VguiControlState::PropertySheet(ps)) = self.get_state_mut(field) else {
            return false;
        };
        if tab < ps.tabs.len() {
            ps.active_tab = tab;
            self.events.push(VguiEvent::TabChanged {
                field: field.to_string(),
                tab,
            });
            true
        } else {
            false
        }
    }

    pub fn set_text_entry(&mut self, field: &str, text: String) {
        self.set_state(field, VguiControlState::Text(text));
        self.events.push(VguiEvent::ValueChanged {
            field: field.to_string(),
        });
    }

    pub fn toggle_check(&mut self, field: &str) -> bool {
        let next = match self.get_state(field) {
            Some(VguiControlState::Check(cur)) => !cur,
            _ => true,
        };
        self.set_state(field, VguiControlState::Check(next));
        self.events.push(VguiEvent::ValueChanged {
            field: field.to_string(),
        });
        next
    }

    pub fn set_slider(&mut self, field: &str, value: f32) {
        let key = field.to_ascii_lowercase();
        match self.states.get_mut(&key) {
            Some(VguiControlState::Slider(s)) => {
                s.value = value.clamp(s.min, s.max);
            }
            _ => {
                self.states.insert(
                    key,
                    VguiControlState::Slider(VguiSliderState {
                        value,
                        min: 0.0,
                        max: 1.0,
                    }),
                );
            }
        }
        self.events.push(VguiEvent::ValueChanged {
            field: field.to_string(),
        });
    }

    pub fn select_combo(&mut self, field: &str, idx: usize) -> bool {
        let Some(VguiControlState::Combo(cb)) = self.get_state_mut(field) else {
            return false;
        };
        if idx < cb.items.len() {
            cb.selected = idx;
            self.events.push(VguiEvent::ValueChanged {
                field: field.to_string(),
            });
            true
        } else {
            false
        }
    }

    pub fn ensure_texture(&mut self, ctx: &egui::Context, vfs: Option<&Vfs>, rel_path: &str) -> Option<TextureHandle> {
        let norm = rel_path.replace('\\', "/");
        if let Some(tex) = self.textures.get(&norm) {
            return Some(tex.clone());
        }
        let img = load_image_rgba(vfs?, &norm).ok()?;
        let col = egui::ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        );
        let tex = ctx.load_texture(format!("vgui_{norm}"), col, egui::TextureOptions::LINEAR);
        self.textures.insert(norm, tex.clone());
        Some(tex)
    }

    /// Compute flattened resolved layout nodes for a `.res` file inside `viewport` (`640x480` base).
    pub fn compute_layout(
        &self,
        doc_path: &str,
        viewport: [f32; 4],
        loc: Option<&LocalizationTable>,
    ) -> Vec<VguiLayoutNode> {
        let key = doc_path.replace('\\', "/").to_ascii_lowercase();
        let Some(doc) = self.docs.get(&key) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        fn walk(
            controls: &[VguiControlSpec],
            parent: [f32; 4],
            loc: Option<&LocalizationTable>,
            out: &mut Vec<VguiLayoutNode>,
        ) {
            for c in controls {
                let rect = resolve_control_rect(c, parent, 640.0, 480.0);
                let label = loc.map_or(c.label_text.as_str(), |l| l.resolve(&c.label_text)).to_string();
                out.push(VguiLayoutNode {
                    field_name: c.field_name.clone(),
                    control_name: c.control_name.clone(),
                    kind: c.kind,
                    rect,
                    visible: c.visible,
                    enabled: c.enabled,
                    label,
                    command: c.command.clone(),
                });
                walk(&c.children, rect, loc, out);
            }
        }
        walk(&doc.controls, viewport, loc, &mut out);
        out
    }

    /// Paint a `.res` document's visible controls directly into `ui` and return emitted commands.
    pub fn paint_res_controls(
        &mut self,
        ui: &mut egui::Ui,
        doc_path: &str,
        origin_rect: Rect,
        loc: Option<&LocalizationTable>,
        vfs: Option<&Vfs>,
        palette: VguiPalette,
    ) -> Vec<VguiEvent> {
        let viewport = [
            origin_rect.left(),
            origin_rect.top(),
            origin_rect.width(),
            origin_rect.height(),
        ];
        let nodes = self.compute_layout(doc_path, viewport, loc);
        let key = doc_path.replace('\\', "/").to_ascii_lowercase();
        let specs = self.docs.get(&key).cloned();
        let mut out = Vec::new();
        for n in &nodes {
            if !n.visible {
                continue;
            }
            let rect = Rect::from_min_size(Pos2::new(n.rect[0], n.rect[1]), Vec2::new(n.rect[2], n.rect[3]));
            let spec = specs.as_ref().and_then(|d| d.find(&n.field_name));
            match n.kind {
                ControlKind::Panel => {
                    draw_vgui_panel(ui.painter(), rect, palette.control_bg, palette, false);
                }
                ControlKind::Divider => {
                    ui.painter().rect_filled(rect, 0.0, palette.border_dark);
                }
                ControlKind::Label => {
                    let fg = spec.map_or(palette.base_text, |s| resolve_vgui_text_color(s, palette));
                    let sz = spec
                        .and_then(|s| self.scheme_fonts.resolve(&s.font, origin_rect.height() as u32))
                        .map_or(13.5, |f| f.tall.min(24.0));
                    let align = spec.map_or(Default::default(), |s| s.text_alignment);
                    draw_vgui_label(ui.painter(), rect, &n.label, align, FontId::proportional(sz), fg);
                }
                ControlKind::Button | ControlKind::MouseOverPanelButton => {
                    let def = spec.is_some_and(|s| s.default_button);
                    if draw_vgui_button(ui, rect, &n.label, def, n.enabled, palette).clicked() {
                        out.push(VguiEvent::Command {
                            field: n.field_name.clone(),
                            command: n.command.clone(),
                        });
                    }
                }
                ControlKind::ImagePanel => {
                    let tex = spec
                        .filter(|s| !s.image.is_empty())
                        .and_then(|s| self.ensure_texture(ui.ctx(), vfs, &s.image));
                    draw_vgui_image_panel(ui.painter(), rect, tex.as_ref(), Color32::WHITE);
                }
                ControlKind::ProgressBar => {
                    let frac = match self.get_state(&n.field_name) {
                        Some(VguiControlState::Progress(f)) => *f,
                        _ => 0.0,
                    };
                    draw_vgui_progress_bar(ui.painter(), rect, frac, palette);
                }
                _ => {}
            }
        }
        out
    }
}

pub fn default_list_state(cols: &[(&str, u32)]) -> VguiListPanelState {
    VguiListPanelState {
        columns: cols
            .iter()
            .map(|(n, w)| super::controls::ListColumn {
                name: (*n).to_string(),
                width: *w,
            })
            .collect(),
        rows: Vec::new(),
        sort_col: 0,
        sort_dir: None,
        selected_row: None,
    }
}

pub fn default_combo_state(items: &[&str], selected: usize) -> VguiComboState {
    VguiComboState {
        items: items.iter().map(|s| (*s).to_string()).collect(),
        selected,
    }
}

pub fn default_sheet_state(tabs: &[&str]) -> VguiPropertySheetState {
    VguiPropertySheetState {
        tabs: tabs.iter().map(|s| (*s).to_string()).collect(),
        active_tab: 0,
    }
}

pub fn default_rich_text() -> VguiRichTextState {
    VguiRichTextState::default()
}
