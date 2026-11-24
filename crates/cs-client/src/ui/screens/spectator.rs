//! `.res`-driven CS 1.6 Spectator letterbox & bottom control bar
//! (`Resource/UI/Spectator.res`, `BottomSpectator.res`, `spectatormenu.txt`).

use cs_assets::vgui_scripts::CommandMenuTree;
use cs_cvar::{CVarRegistry, EngineAction};
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Vec2};

use crate::ui::vgui2::VguiRuntime;
use crate::ui::vgui_theme::VguiPalette;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpectatorMode {
    None = 0,
    ChaseLocked = 1,
    #[default]
    ChaseFree = 2,
    FreeLook = 3,
    FirstPerson = 4,
    MapOverviewFree = 5,
    MapOverviewChase = 6,
}

impl SpectatorMode {
    pub const ALL: [(Self, &'static str); 6] = [
        (Self::ChaseLocked, "Locked Chase Cam"),
        (Self::ChaseFree, "Free Chase Cam"),
        (Self::FreeLook, "Free Look"),
        (Self::FirstPerson, "First Person"),
        (Self::MapOverviewFree, "Free Overview Map"),
        (Self::MapOverviewChase, "Chase Overview Map"),
    ];

    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::ChaseLocked,
            2 => Self::ChaseFree,
            3 => Self::FreeLook,
            4 => Self::FirstPerson,
            5 => Self::MapOverviewFree,
            6 => Self::MapOverviewChase,
            _ => Self::ChaseFree,
        }
    }

    pub fn label(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(m, _)| *m == self)
            .map_or("Free Chase Cam", |(_, l)| *l)
    }
}

pub struct SpectatorVguiUi {
    pub menu_visible: bool,
    pub mode: SpectatorMode,
    pub target_index: usize,
    pub settings_popup: bool,
    spec_menu_tree: Option<CommandMenuTree>,
}

impl Default for SpectatorVguiUi {
    fn default() -> Self {
        Self {
            menu_visible: true,
            mode: SpectatorMode::ChaseFree,
            target_index: 0,
            settings_popup: false,
            spec_menu_tree: None,
        }
    }
}

impl SpectatorVguiUi {
    pub fn load_assets(&mut self, vfs: &Vfs) {
        self.spec_menu_tree = CommandMenuTree::load(vfs, "spectatormenu.txt");
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        vfs: Option<&Vfs>,
        cvars: &mut CVarRegistry,
        ct_score: i16,
        t_score: i16,
        round_timer_secs: u32,
        players: &[(u8, String)],
        spec_health: Option<(u8, u8)>,
        palette: VguiPalette,
    ) -> Vec<EngineAction> {
        let mut actions = Vec::new();
        if let Some(v) = vfs {
            runtime.load_res(v, "resource/UI/Spectator.res");
            runtime.load_res(v, "resource/UI/BottomSpectator.res");
        }

        let screen = ctx.content_rect();
        let scale = (screen.height() / 480.0).clamp(0.85, 1.8);
        let top_h = (62.0 * scale).round();
        let bot_h = (62.0 * scale).round();

        let top_rect = Rect::from_min_size(screen.min, Vec2::new(screen.width(), top_h));
        let bot_rect = Rect::from_min_size(
            Pos2::new(screen.left(), screen.bottom() - bot_h),
            Vec2::new(screen.width(), bot_h),
        );

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::unique("cs16_spectator_letterbox"),
        ));
        painter.rect_filled(top_rect, 0.0, Color32::from_black_alpha(228));
        painter.rect_filled(bot_rect, 0.0, Color32::from_black_alpha(228));
        painter.line_segment(
            [Pos2::new(top_rect.left(), top_rect.bottom()), Pos2::new(top_rect.right(), top_rect.bottom())],
            Stroke::new(1.0, Color32::from_rgb(160, 110, 20)),
        );
        painter.line_segment(
            [Pos2::new(bot_rect.left(), bot_rect.top()), Pos2::new(bot_rect.right(), bot_rect.top())],
            Stroke::new(1.0, Color32::from_rgb(160, 110, 20)),
        );

        // Top-right CT / T score + timer (`Spectator.res` CTScoreLabel / TERScoreLabel)
        let f_score = FontId::proportional(13.0 * (scale * 0.75).max(1.0));
        painter.text(
            Pos2::new(screen.right() - 18.0, top_rect.top() + 12.0),
            egui::Align2::RIGHT_TOP,
            format!("Counter-Terrorists: {ct_score}   Terrorists: {t_score}"),
            f_score.clone(),
            Color32::from_rgb(255, 180, 30),
        );
        let mins = round_timer_secs / 60;
        let secs = round_timer_secs % 60;
        painter.text(
            Pos2::new(screen.right() - 18.0, top_rect.top() + 32.0),
            egui::Align2::RIGHT_TOP,
            format!("Time: {mins}:{secs:02}"),
            f_score,
            Color32::from_rgb(216, 222, 211),
        );

        if !self.menu_visible {
            return actions;
        }

        // Bottom bar interactive controls (`BottomSpectator.res`)
        egui::Area::new(egui::Id::unique("cs16_spectator_bottom_controls"))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(bot_rect.left() + 10.0, bot_rect.top() + 16.0))
            .show(ctx, |ui| {
                ui.set_min_width((bot_rect.width() - 20.0).max(320.0));
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new("Options").corner_radius(CornerRadius::ZERO))
                        .clicked()
                    {
                        self.settings_popup = !self.settings_popup;
                    }

                    ui.add_space(16.0);
                    if ui.add(egui::Button::new("<").corner_radius(CornerRadius::ZERO)).clicked()
                        && !players.is_empty()
                    {
                        self.target_index =
                            (self.target_index + players.len() - 1) % players.len();
                    }

                    let target_label = if players.is_empty() {
                        "No active players".to_string()
                    } else {
                        let idx = self.target_index.min(players.len() - 1);
                        let (slot, name) = &players[idx];
                        if let Some((hp, tgt)) = spec_health
                            && (tgt == 0 || tgt == *slot)
                        {
                            format!("{name} ({hp} HP)")
                        } else {
                            name.clone()
                        }
                    };
                    egui::ComboBox::from_id_salt("spec_player_combo")
                        .width(240.0)
                        .selected_text(target_label)
                        .show_ui(ui, |ui| {
                            for (i, (_, name)) in players.iter().enumerate() {
                                ui.selectable_value(&mut self.target_index, i, name);
                            }
                        });

                    if ui.add(egui::Button::new(">").corner_radius(CornerRadius::ZERO)).clicked()
                        && !players.is_empty()
                    {
                        self.target_index = (self.target_index + 1) % players.len();
                    }

                    ui.add_space(16.0);
                    egui::ComboBox::from_id_salt("spec_view_combo")
                        .width(155.0)
                        .selected_text(self.mode.label())
                        .show_ui(ui, |ui| {
                            for (m, label) in SpectatorMode::ALL {
                                if ui.selectable_label(self.mode == m, label).clicked() {
                                    self.mode = m;
                                    actions.push(EngineAction::SpecMode(m as i32));
                                }
                            }
                        });
                });
            });

        if self.settings_popup {
            egui::Window::new("Spectator Settings")
                .collapsible(false)
                .resizable(false)
                .fixed_pos(Pos2::new(bot_rect.left() + 10.0, bot_rect.top() - 140.0))
                .show(ctx, |ui| {
                    for (cvar, lbl) in [
                        ("spec_autodirector", "Auto Director"),
                        ("spec_drawnames", "Player Names"),
                        ("spec_drawcone", "Overview View Cones"),
                        ("spec_drawstatus", "Status Bar"),
                        ("cl_observercrosshair", "Observer Crosshair"),
                    ] {
                        let mut on = cvars.get_bool(cvar);
                        if ui.checkbox(&mut on, lbl).changed() {
                            cvars.set(cvar, if on { "1" } else { "0" });
                        }
                    }
                    let _ = palette;
                });
        }

        actions
    }
}
