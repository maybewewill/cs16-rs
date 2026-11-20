//! 1-to-1 Counter-Strike 1.6 `TeamMenu.res` -> `ClassMenu_CT.res` / `ClassMenu_TER.res`
//! (with real `maps/<map>.txt` mission briefing and real `gfx/vgui/*.tga` class portraits)
//! plus `Y`/`U` classic chat input.

use cs_assets::bsp::SpawnTeam;
use cs_sim::rules::PlayerModelSkin;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, FontId, Key, RichText, Stroke};

use crate::ui::vgui_assets::{
    VguiAssetBank, VirtualCanvas, cs16_vgui_button, paint_cs16_background_panel,
    render_class_res_panel,
};
use crate::ui::vgui_theme::VguiPalette;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TeamMenuStep {
    #[default]
    ChooseTeam,
    ChooseClass(SpawnTeam),
}

#[derive(Default)]
pub struct TeamMenuUi {
    pub open: bool,
    step: TeamMenuStep,
    hovered_class_res: Option<String>,
}

impl TeamMenuUi {
    pub fn open_team_select(&mut self) {
        self.open = true;
        self.step = TeamMenuStep::ChooseTeam;
        self.hovered_class_res = None;
    }

    pub fn open_class_select(&mut self, team: SpawnTeam) {
        self.open = true;
        self.step = TeamMenuStep::ChooseClass(team);
        self.hovered_class_res = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.step = TeamMenuStep::ChooseTeam;
        self.hovered_class_res = None;
    }

    /// Render the authentic 1-to-1 `Choose a Team` -> `Select Your Appearance` CS 1.6 VGUI2 flow.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        map_name: &str,
        vfs: Option<&Vfs>,
        bank: &mut VguiAssetBank,
    ) -> Option<(SpawnTeam, PlayerModelSkin)> {
        if !self.open {
            return None;
        }

        let pressed_digit = ctx.input(|i| {
            for (k, d) in [
                (Key::Num0, 0u8),
                (Key::Num1, 1),
                (Key::Num2, 2),
                (Key::Num3, 3),
                (Key::Num4, 4),
                (Key::Num5, 5),
                (Key::Num6, 6),
            ] {
                if i.key_pressed(k) {
                    return Some(d);
                }
            }
            None
        });

        if pressed_digit == Some(0) {
            self.close();
            return None;
        }

        match self.step {
            TeamMenuStep::ChooseTeam => {
                if pressed_digit == Some(1) {
                    self.step = TeamMenuStep::ChooseClass(SpawnTeam::Terrorist);
                    self.hovered_class_res = None;
                } else if pressed_digit == Some(2) {
                    self.step = TeamMenuStep::ChooseClass(SpawnTeam::CounterTerrorist);
                    self.hovered_class_res = None;
                } else if pressed_digit == Some(5) {
                    self.close();
                    return Some((SpawnTeam::CounterTerrorist, PlayerModelSkin::Urban));
                }
            }
            TeamMenuStep::ChooseClass(team) => {
                if let Some(d) = pressed_digit {
                    let classes = team_classes(team);
                    if d == 5 {
                        self.close();
                        return Some((team, classes[0].skin));
                    }
                    if let Some(c) = classes.iter().find(|c| c.hotkey == d) {
                        self.close();
                        return Some((team, c.skin));
                    }
                }
            }
        }

        let screen = ctx.content_rect();
        let vc = VirtualCanvas::from_screen(screen);
        let cp = bank.client_palette;
        let mut chosen: Option<(SpawnTeam, PlayerModelSkin)> = None;

        egui::Area::new(egui::Id::unique("cs16_vgui_team_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                match self.step {
                    TeamMenuStep::ChooseTeam => {
                        paint_cs16_background_panel(ui.painter(), vc, "Choose a Team", bank);

                        // Exact `TeamMenu.res` left buttons at x=76, y=116..276
                        let btns: [(&str, &str, f32, Option<SpawnTeam>, bool); 5] = [
                            ("1", "Terrorist Forces", 116.0, Some(SpawnTeam::Terrorist), true),
                            ("2", "CT Forces", 148.0, Some(SpawnTeam::CounterTerrorist), true),
                            ("5", "Auto-Assign", 212.0, None, true),
                            ("6", "Spectate", 244.0, None, false),
                            ("0", "Cancel", 276.0, None, true),
                        ];
                        for (hk, label, y, team_opt, enabled) in btns {
                            let rect = vc.rect(76.0, y, 148.0, 20.0);
                            let (clicked, _) = cs16_vgui_button(
                                ui,
                                rect,
                                &format!("team_btn_{hk}"),
                                Some(hk),
                                label,
                                enabled,
                                false,
                                vc.scale,
                                cp,
                            );
                            if clicked {
                                if let Some(t) = team_opt {
                                    self.step = TeamMenuStep::ChooseClass(t);
                                    self.hovered_class_res = None;
                                } else if hk == "5" {
                                    chosen = Some((SpawnTeam::CounterTerrorist, PlayerModelSkin::Urban));
                                } else if hk == "0" {
                                    self.close();
                                }
                            }
                        }

                        // Exact `MapInfo` panel at (244, 116, 316, 286) from `TeamMenu.res`!
                        let map_rect = vc.rect(244.0, 116.0, 340.0, 286.0);
                        ui.painter().rect(
                            map_rect,
                            CornerRadius::ZERO,
                            Color32::from_rgba_unmultiplied(0, 0, 0, 145),
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 150)),
                            egui::StrokeKind::Inside,
                        );
                        let briefing = bank.get_map_briefing(vfs, map_name);
                        let pad = 10.0 * vc.scale;
                        let galley = ui.painter().layout(
                            briefing,
                            FontId::proportional((9.5 * vc.scale).clamp(11.0, 16.0)),
                            cp.base_text,
                            map_rect.width() - pad * 2.0,
                        );
                        ui.painter().galley(
                            map_rect.min + egui::vec2(pad, pad),
                            galley,
                            cp.base_text,
                        );
                    }
                    TeamMenuStep::ChooseClass(team) => {
                        paint_cs16_background_panel(
                            ui.painter(),
                            vc,
                            "Select Your Appearance",
                            bank,
                        );

                        let classes = team_classes(team);
                        let active_res = self
                            .hovered_class_res
                            .clone()
                            .unwrap_or_else(|| classes[0].res_name.to_string());

                        for (idx, cls) in classes.iter().enumerate() {
                            let y = 116.0 + (idx as f32) * 32.0;
                            let rect = vc.rect(76.0, y, 148.0, 20.0);
                            let hk = cls.hotkey.to_string();
                            let is_active = active_res.eq_ignore_ascii_case(cls.res_name);
                            let (clicked, hovered) = cs16_vgui_button(
                                ui,
                                rect,
                                &format!("class_btn_{}", cls.res_name),
                                Some(&hk),
                                cls.label,
                                true,
                                is_active,
                                vc.scale,
                                cp,
                            );
                            if hovered {
                                self.hovered_class_res = Some(cls.res_name.to_string());
                            }
                            if clicked {
                                chosen = Some((team, cls.skin));
                            }
                        }

                        // Auto-select row 5
                        let auto_res = match team {
                            SpawnTeam::CounterTerrorist => "autoselect_ct",
                            SpawnTeam::Terrorist => "autoselect_t",
                        };
                        let auto_rect = vc.rect(76.0, 244.0, 148.0, 20.0);
                        let (auto_clicked, auto_hovered) = cs16_vgui_button(
                            ui,
                            auto_rect,
                            "class_btn_auto",
                            Some("5"),
                            "Auto-Select",
                            true,
                            active_res.eq_ignore_ascii_case(auto_res),
                            vc.scale,
                            cp,
                        );
                        if auto_hovered {
                            self.hovered_class_res = Some(auto_res.to_string());
                        }
                        if auto_clicked {
                            chosen = Some((team, classes[0].skin));
                        }

                        let cancel_rect = vc.rect(76.0, 380.0, 148.0, 20.0);
                        let (cancel_clicked, _) = cs16_vgui_button(
                            ui,
                            cancel_rect,
                            "class_btn_cancel",
                            Some("0"),
                            "Cancel",
                            true,
                            false,
                            vc.scale,
                            cp,
                        );
                        if cancel_clicked {
                            self.close();
                        }

                        // Real `ClassInfo` panel at (244, 116) with 256×196 Valve portrait TGA + unit bio!
                        render_class_res_panel(
                            ctx,
                            ui.painter(),
                            vc,
                            244.0,
                            116.0,
                            &active_res,
                            vfs,
                            bank,
                        );
                    }
                }
            });

        if chosen.is_some() {
            self.close();
        }
        chosen
    }
}

struct ClassEntry {
    hotkey: u8,
    label: &'static str,
    res_name: &'static str,
    skin: PlayerModelSkin,
}

fn team_classes(team: SpawnTeam) -> [ClassEntry; 4] {
    match team {
        SpawnTeam::CounterTerrorist => [
            ClassEntry {
                hotkey: 1,
                label: "SEAL Team 6",
                res_name: "urban",
                skin: PlayerModelSkin::Urban,
            },
            ClassEntry {
                hotkey: 2,
                label: "GSG-9",
                res_name: "gsg9",
                skin: PlayerModelSkin::Gsg9,
            },
            ClassEntry {
                hotkey: 3,
                label: "SAS",
                res_name: "sas",
                skin: PlayerModelSkin::Sas,
            },
            ClassEntry {
                hotkey: 4,
                label: "GIGN",
                res_name: "gign",
                skin: PlayerModelSkin::Gign,
            },
        ],
        SpawnTeam::Terrorist => [
            ClassEntry {
                hotkey: 1,
                label: "Phoenix Connexion",
                res_name: "terror",
                skin: PlayerModelSkin::Terror,
            },
            ClassEntry {
                hotkey: 2,
                label: "Elite Crew",
                res_name: "leet",
                skin: PlayerModelSkin::Leet,
            },
            ClassEntry {
                hotkey: 3,
                label: "Arctic Avengers",
                res_name: "arctic",
                skin: PlayerModelSkin::Arctic,
            },
            ClassEntry {
                hotkey: 4,
                label: "Guerilla Warfare",
                res_name: "guerilla",
                skin: PlayerModelSkin::Guerilla,
            },
        ],
    }
}

#[derive(Default)]
pub struct ChatInputUi {
    pub open: bool,
    pub team_only: bool,
    buffer: String,
    request_focus: bool,
}

impl ChatInputUi {
    pub fn open_chat(&mut self, team_only: bool) {
        self.open = true;
        self.team_only = team_only;
        self.buffer.clear();
        self.request_focus = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, _palette: VguiPalette) -> Option<(bool, String)> {
        if !self.open {
            return None;
        }
        let mut submitted = None;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("cs16_chat_input"))
            .fixed_pos(egui::pos2(24.0, screen.bottom() - 108.0))
            .show(ctx, |ui| {
                // Authentic translucent black CS 1.6 say prompt
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 175))
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 160)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let label = if self.team_only { "say_team :" } else { "say :" };
                            ui.label(RichText::new(label).color(Color32::from_rgb(255, 176, 0)));
                            let edit = egui::TextEdit::singleline(&mut self.buffer)
                                .desired_width(340.0)
                                .frame(egui::Frame::NONE)
                                .text_color(Color32::from_rgb(255, 220, 120))
                                .lock_focus(true);
                            let resp = ui.add(edit);
                            if self.request_focus {
                                resp.request_focus();
                                self.request_focus = false;
                            }
                            if ui.input(|i| i.key_pressed(Key::Enter)) {
                                let text = self.buffer.trim().to_string();
                                if !text.is_empty() {
                                    submitted = Some((self.team_only, text));
                                }
                                self.buffer.clear();
                                self.open = false;
                            } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                                self.buffer.clear();
                                self.open = false;
                            }
                        });
                    });
            });
        submitted
    }
}
