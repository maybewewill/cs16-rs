//! `.res`-driven CS 1.6 VGUI2 TeamMenu (`Teammenu.res`), ClassMenu (`Classmenu_{CT,TER}.res`),
//! and BuyMenu (`MainBuyMenu.res` + `Buy{Pistols,Shotguns,SubMachineguns,Rifles,Machineguns,Equipment}_{CT,TER}.res`)
//! with `classes/<item>.res` hover preview and `maps/<map>.txt` briefing.

use cs_assets::scheme::{ClassResPanel, LocalizationTable, load_map_briefing};
use cs_assets::vgui_res::ControlKind;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Key, Pos2, Rect, RichText, ScrollArea, Stroke, Vec2};

use crate::ui::vgui2::{VguiRuntime, resolve_vgui_text_color};
use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::VguiPalette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VguiMenuKind {
    Team,
    ClassCt,
    ClassT,
    Buy,
}

impl VguiMenuKind {
    pub fn from_vgui_menu_id(menu_id: u8, name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower.contains("team") || menu_id == 2 {
            Some(Self::Team)
        } else if lower.contains("class_ct") || menu_id == 26 {
            Some(Self::ClassCt)
        } else if lower.contains("class_te") || menu_id == 3 || menu_id == 27 {
            Some(Self::ClassT)
        } else if lower.contains("buy") || (4..=10).contains(&menu_id) {
            Some(Self::Buy)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BuyClassTeamMenuUi {
    pub active_res: Option<String>,
    pub hovered_class: Option<String>,
    cached_preview: Option<(String, ClassResPanel)>,
}

impl BuyClassTeamMenuUi {
    pub fn open_kind(&mut self, kind: VguiMenuKind, is_ct: bool) {
        let path = match kind {
            VguiMenuKind::Team => "resource/UI/TeamMenu.res",
            VguiMenuKind::ClassCt => "resource/UI/ClassMenu_CT.res",
            VguiMenuKind::ClassT => "resource/UI/ClassMenu_TER.res",
            VguiMenuKind::Buy => {
                if is_ct {
                    "resource/UI/BuyMenu.res"
                } else {
                    "resource/UI/MainBuyMenu.res"
                }
            }
        };
        if self.active_res.as_deref() != Some(path) {
            self.active_res = Some(path.to_string());
            self.hovered_class = None;
        }
    }

    pub fn close(&mut self) {
        self.active_res = None;
        self.hovered_class = None;
    }

    /// Resolve `.res` sub-navigation (`Resource/UI/BuyPistols.res` -> `_CT` / `_TER` variant if present)
    /// or return a server stringcmd to send (`jointeam 2`, `ak47`, `vest`, etc.).
    pub fn handle_command(&mut self, cmd: &str, is_ct: bool, vfs: Option<&Vfs>) -> Option<String> {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.eq_ignore_ascii_case("vguicancel") || trimmed.eq_ignore_ascii_case("close") {
            self.close();
            return None;
        }
        let lower = trimmed.to_ascii_lowercase().replace('\\', "/");
        if lower.ends_with(".res") {
            let suffix = if is_ct { "_ct.res" } else { "_ter.res" };
            let candidate_suffixed = lower.trim_end_matches(".res").to_string() + suffix;
            if let Some(v) = vfs {
                if v.exists(&candidate_suffixed) {
                    self.active_res = Some(candidate_suffixed);
                    return None;
                }
            }
            self.active_res = Some(lower);
            return None;
        }
        self.close();
        Some(trimmed.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        vfs: Option<&Vfs>,
        loc: Option<&LocalizationTable>,
        map_name: &str,
        is_ct: bool,
        palette: VguiPalette,
        steam_icon: Option<&egui::TextureHandle>,
    ) -> Option<String> {
        let res_path = self.active_res.clone()?;
        if let Some(v) = vfs {
            runtime.load_res(v, &res_path);
        }

        let mut open = true;
        let mut out_cmd: Option<String> = None;
        let title = if res_path.to_ascii_lowercase().contains("team") {
            "Select Team"
        } else if res_path.to_ascii_lowercase().contains("class") {
            "Select Appearance"
        } else {
            "Buy Menu"
        };

        show_steam2003_window(
            ctx,
            "cs16_vgui_buy_team_class",
            title,
            &mut open,
            Pos2::new(110.0, 70.0),
            Vec2::new(610.0, 440.0),
            Vec2::new(520.0, 360.0),
            false,
            palette,
            steam_icon,
            |ui| {
                let area = ui.available_rect_before_wrap();
                let nodes = runtime.compute_layout(
                    &res_path,
                    [area.left(), area.top(), area.width(), area.height()],
                    loc,
                );
                let mut buttons: Vec<(String, String, String, Rect, bool)> = Vec::new();
                for n in &nodes {
                    if !n.visible {
                        continue;
                    }
                    let rect = Rect::from_min_size(
                        Pos2::new(n.rect[0], n.rect[1]),
                        Vec2::new(n.rect[2].max(16.0), n.rect[3].max(16.0)),
                    );
                    match n.kind {
                        ControlKind::Label => {
                            let col = resolve_vgui_label_color(palette, n.field_name == "Title");
                            ui.painter().text(
                                Pos2::new(rect.left(), rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &n.label,
                                egui::FontId::proportional(if n.field_name.eq_ignore_ascii_case("title") {
                                    18.0
                                } else {
                                    13.0
                                }),
                                col,
                            );
                        }
                        ControlKind::Button | ControlKind::MouseOverPanelButton => {
                            buttons.push((
                                n.field_name.clone(),
                                n.label.clone(),
                                n.command.clone(),
                                rect,
                                n.enabled,
                            ));
                        }
                        _ => {}
                    }
                }

                // Numbered hotkeys 1..=9 and 0 -> buttons in order.
                let pressed_digit = ctx.input(|i| {
                    [
                        (Key::Num1, 0usize),
                        (Key::Num2, 1),
                        (Key::Num3, 2),
                        (Key::Num4, 3),
                        (Key::Num5, 4),
                        (Key::Num6, 5),
                        (Key::Num7, 6),
                        (Key::Num8, 7),
                        (Key::Num9, 8),
                    ]
                    .into_iter()
                    .find_map(|(k, idx)| i.key_pressed(k).then_some(idx))
                });
                if let Some(idx) = pressed_digit
                    && let Some((_, _, cmd, _, true)) = buttons.get(idx)
                    && let Some(send) = self.handle_command(cmd, is_ct, vfs)
                {
                    out_cmd = Some(send);
                }

                for (field, label, cmd, rect, enabled) in buttons {
                    let resp = ui.put(
                        rect,
                        egui::Button::new(RichText::new(&label).color(palette.base_text))
                            .corner_radius(CornerRadius::ZERO),
                    );
                    if resp.hovered() {
                        self.hovered_class = Some(field);
                    }
                    if enabled && resp.clicked() && let Some(send) = self.handle_command(&cmd, is_ct, vfs) {
                        out_cmd = Some(send);
                    }
                }

                // Right-side info pane (`maps/<map>.txt` for TeamMenu or `classes/<hover>.res`).
                let info_rect = Rect::from_min_size(
                    Pos2::new(area.left() + area.width() * 0.42, area.top() + 58.0),
                    Vec2::new(area.width() * 0.55, area.height() - 72.0),
                );
                ui.painter().rect_filled(info_rect, CornerRadius::ZERO, palette.window_bg);
                ui.painter().rect_stroke(
                    info_rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, palette.border_dark),
                    egui::StrokeKind::Inside,
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(info_rect.shrink(8.0)), |ui| {
                    if let Some(hover) = self.hovered_class.clone()
                        && let Some(panel) = self.preview_panel(vfs, loc, &hover)
                    {
                        if let Some(ref img_path) = panel.image_path
                            && let Some(tex) = runtime.ensure_texture(ctx, vfs, img_path)
                        {
                            ui.image(egui::load::SizedTexture::new(tex.id(), Vec2::new(220.0, 70.0)));
                            ui.add_space(6.0);
                        }
                        for lbl in &panel.labels {
                            let col = if lbl.bright {
                                palette.bright_control_text
                            } else {
                                palette.base_text
                            };
                            ui.label(RichText::new(&lbl.text).color(col).size(if lbl.small_font {
                                12.0
                            } else {
                                13.5
                            }));
                        }
                    } else if res_path.to_ascii_lowercase().contains("team")
                        && let Some(v) = vfs
                        && let Some(brief) = load_map_briefing(v, map_name)
                    {
                        ScrollArea::vertical().show(ui, |ui| {
                            ui.label(RichText::new(brief).color(palette.base_text));
                        });
                    }
                });
            },
        );
        if !open {
            self.close();
        }
        out_cmd
    }

    fn preview_panel(
        &mut self,
        vfs: Option<&Vfs>,
        loc: Option<&LocalizationTable>,
        name: &str,
    ) -> Option<ClassResPanel> {
        if let Some((ref cached_name, ref panel)) = self.cached_preview
            && cached_name.eq_ignore_ascii_case(name)
        {
            return Some(panel.clone());
        }
        let v = vfs?;
        let def_loc = LocalizationTable::default();
        let l = loc.unwrap_or(&def_loc);
        let panel = ClassResPanel::load(v, name, l)?;
        self.cached_preview = Some((name.to_string(), panel.clone()));
        Some(panel)
    }
}

fn resolve_vgui_label_color(palette: VguiPalette, title: bool) -> Color32 {
    if title {
        palette.bright_control_text
    } else {
        let _ = resolve_vgui_text_color;
        palette.base_text
    }
}
