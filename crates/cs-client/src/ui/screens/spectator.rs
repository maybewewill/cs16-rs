//! `.res`-driven CS 1.6 Spectator letterbox & bottom control bar
//! (`Resource/UI/Spectator.res`, `BottomSpectator.res`, `spectatormenu.txt`, `ClientScheme.res`).

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::{TextAlignment, VguiCoordMode};
use cs_assets::vgui_scripts::{CommandMenuAction, CommandMenuItem, CommandMenuTree};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_vfs::Vfs;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

use crate::ui::vgui2::controls::draw_vgui_image_panel;
use crate::ui::vgui2::font_atlas::VguiTextDrawOpts;
use crate::ui::vgui2::runtime::{VguiRuntime, scheme_color_or};

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

pub struct SpectatorRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub ct_score: i16,
    pub t_score: i16,
    pub round_timer_secs: u32,
    pub players: &'a [(u8, String)],
    pub spec_health: Option<(u8, u8)>,
}

pub struct SpectatorVguiUi {
    pub menu_visible: bool,
    pub mode: SpectatorMode,
    pub target_index: usize,
    pub settings_popup: bool,
    pub player_popup: bool,
    pub view_popup: bool,
    spec_menu_tree: Option<CommandMenuTree>,
}

impl Default for SpectatorVguiUi {
    fn default() -> Self {
        Self {
            menu_visible: true,
            mode: SpectatorMode::ChaseFree,
            target_index: 0,
            settings_popup: false,
            player_popup: false,
            view_popup: false,
            spec_menu_tree: None,
        }
    }
}

impl SpectatorVguiUi {
    pub fn load_assets(&mut self, vfs: &Vfs) {
        self.spec_menu_tree = CommandMenuTree::load(vfs, "spectatormenu.txt");
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        s: &SpectatorRenderCtx<'_>,
    ) -> Vec<EngineAction> {
        let mut actions = Vec::new();
        if let Some(v) = s.vfs {
            runtime.load_res(v, "resource/UI/Spectator.res");
            runtime.load_res(v, "resource/UI/BottomSpectator.res");
        }

        let screen = ctx.content_rect();
        let screen_h = screen.height().round().max(480.0) as u32;
        let vp = [screen.left(), screen.top(), screen.width(), screen.height()];
        let top_nodes = runtime.compute_layout_mode(
            "resource/UI/Spectator.res",
            vp,
            VguiCoordMode::Proportional640,
            s.loc,
        );
        let sy = (screen.height() / 480.0).max(0.5);

        let rect_of = |field: &str, fb: Rect| -> Rect {
            top_nodes
                .iter()
                .find(|n| n.field_name.eq_ignore_ascii_case(field))
                .map_or(fb, |n| {
                    Rect::from_min_size(Pos2::new(n.rect[0], n.rect[1]), Vec2::new(n.rect[2], n.rect[3]))
                })
        };

        let top_rect = rect_of("topbar", Rect::from_min_size(screen.min, Vec2::new(screen.width(), (66.0 * sy).round())));
        let bot_h = rect_of("BottomBar", Rect::from_min_size(Pos2::ZERO, Vec2::new(screen.width(), (66.0 * sy).round())))
            .height()
            .clamp(48.0, 120.0);
        let bot_rect = Rect::from_min_size(
            Pos2::new(screen.left(), screen.bottom() - bot_h),
            Vec2::new(screen.width(), bot_h),
        );

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::unique("cs16_spectator_letterbox"),
        ));
        let border_col = scheme_color_or(
            &runtime.client_scheme,
            "BorderBright",
            Color32::from_rgba_unmultiplied(188, 112, 0, 160),
        );
        let amber = scheme_color_or(
            &runtime.client_scheme,
            "ControlText",
            Color32::from_rgb(255, 176, 0),
        );
        let dim_col = scheme_color_or(
            &runtime.client_scheme,
            "LabelDimText",
            Color32::from_rgba_unmultiplied(255, 176, 0, 180),
        );
        let ct_col = scheme_color_or(&runtime.client_scheme, "team2", Color32::from_rgb(153, 204, 255));
        let t_col = scheme_color_or(&runtime.client_scheme, "team1", Color32::from_rgb(255, 64, 64));

        painter.rect_filled(top_rect, 0.0, Color32::from_black_alpha(228));
        painter.rect_filled(bot_rect, 0.0, Color32::from_black_alpha(228));
        painter.line_segment(
            [Pos2::new(top_rect.left(), top_rect.bottom()), Pos2::new(top_rect.right(), top_rect.bottom())],
            Stroke::new(1.0, border_col),
        );
        painter.line_segment(
            [Pos2::new(bot_rect.left(), bot_rect.top()), Pos2::new(bot_rect.right(), bot_rect.top())],
            Stroke::new(1.0, border_col),
        );

        let client_fonts = runtime.client_scheme.fonts.clone();
        let loc_lbl = |token: &str, fb: &str| -> String {
            if let Some(l) = s.loc {
                let r = l.resolve(token);
                if !r.is_empty() && !r.starts_with('#') {
                    return r.to_string();
                }
            }
            fb.to_string()
        };

        let ct_lbl_rect = rect_of("CTScoreLabel", Rect::from_min_size(Pos2::new(screen.right() - 233.0, 12.0), Vec2::new(150.0, 16.0)));
        let ct_val_rect = rect_of("CTScoreValue", Rect::from_min_size(Pos2::new(screen.right() - 80.0, 12.0), Vec2::new(20.0, 16.0)));
        let t_lbl_rect = rect_of("TERScoreLabel", Rect::from_min_size(Pos2::new(screen.right() - 233.0, 26.0), Vec2::new(150.0, 16.0)));
        let t_val_rect = rect_of("TERScoreValue", Rect::from_min_size(Pos2::new(screen.right() - 80.0, 26.0), Vec2::new(20.0, 16.0)));
        let div_rect = rect_of("DividerBar", Rect::from_min_size(Pos2::new(screen.right() - 64.0, 12.0), Vec2::new(1.0, 30.0)));
        let timer_img_rect = rect_of("timerimage", Rect::from_min_size(Pos2::new(screen.right() - 56.0, 26.0), Vec2::new(12.0, 12.0)));
        let timer_lbl_rect = rect_of("timerlabel", Rect::from_min_size(Pos2::new(screen.right() - 42.0, 24.0), Vec2::new(40.0, 16.0)));

        let ct_text = loc_lbl("#Cstrike_Spec_CT_Score", "Counter-Terrorists :");
        let t_text = loc_lbl("#Cstrike_Spec_Ter_Score", "Terrorists :");
        for (rect, txt, col, align) in [
            (ct_lbl_rect, ct_text.as_str(), ct_col, TextAlignment::East),
            (t_lbl_rect, t_text.as_str(), t_col, TextAlignment::East),
            (timer_lbl_rect, &format!("{:02}:{:02}", s.round_timer_secs / 60, s.round_timer_secs % 60), dim_col, TextAlignment::West),
        ] {
            runtime.font_atlas.paint_text(
                &painter,
                ctx,
                s.vfs,
                &client_fonts,
                "client",
                "DefaultSmall",
                screen_h,
                rect,
                txt,
                col,
                VguiTextDrawOpts {
                    align,
                    process_hotkey: false,
                    proportional_scale: false,
                },
            );
        }
        for (rect, val, col) in [(ct_val_rect, s.ct_score, ct_col), (t_val_rect, s.t_score, t_col)] {
            runtime.font_atlas.paint_text(
                &painter,
                ctx,
                s.vfs,
                &client_fonts,
                "client",
                "DefaultSmall",
                screen_h,
                Rect::from_min_size(rect.min, Vec2::new(rect.width().max(24.0), rect.height())),
                &val.to_string(),
                col,
                VguiTextDrawOpts {
                    align: TextAlignment::West,
                    process_hotkey: false,
                    proportional_scale: false,
                },
            );
        }
        painter.rect_filled(Rect::from_min_size(div_rect.min, Vec2::new(div_rect.width().max(1.0), div_rect.height())), 0.0, border_col);
        if let Some(tex) = runtime.ensure_texture(ctx, s.vfs, "gfx/vgui/timer") {
            draw_vgui_image_panel(&painter, timer_img_rect, Some(&tex), amber);
        }

        if !self.menu_visible {
            return actions;
        }

        let sx = (screen.width() / 640.0).max(0.5);
        let bottom_doc = runtime.doc("resource/UI/BottomSpectator.res").cloned();
        let bot_rect_of = |field: &str, fb: Rect| -> Rect {
            if let Some(ref doc) = bottom_doc
                && let Some(spec) = doc.find(field)
            {
                let x = spec.xpos.resolve_pos(bot_rect.left(), bot_rect.width(), sx.min(sy));
                let y = bot_rect.top() + match spec.ypos {
                    cs_assets::vgui_res::VguiCoord::Abs(v) => v * sy,
                    other => other.resolve_pos(0.0, bot_rect.height(), sy),
                };
                let w = spec.wide.resolve_size(x - bot_rect.left(), bot_rect.width(), sx);
                let h = spec.tall.resolve_size(0.0, bot_rect.height(), sy);
                return Rect::from_min_size(Pos2::new(x, y), Vec2::new(w.max(18.0), h.max(18.0)));
            }
            fb
        };

        let r_settings = bot_rect_of("settingscombo", Rect::from_min_size(Pos2::new(bot_rect.left() + 8.0, bot_rect.top() + 20.0), Vec2::new(135.0, 22.0)));
        let r_prev = bot_rect_of("playerprev", Rect::from_min_size(Pos2::new(bot_rect.center().x - 175.0, bot_rect.top() + 20.0), Vec2::new(22.0, 22.0)));
        let r_player = bot_rect_of("playercombo", Rect::from_min_size(Pos2::new(bot_rect.center().x - 150.0, bot_rect.top() + 20.0), Vec2::new(300.0, 22.0)));
        let r_next = bot_rect_of("playernext", Rect::from_min_size(Pos2::new(bot_rect.center().x + 155.0, bot_rect.top() + 20.0), Vec2::new(22.0, 22.0)));
        let r_view = bot_rect_of("viewcombo", Rect::from_min_size(Pos2::new(bot_rect.right() - 144.0, bot_rect.top() + 20.0), Vec2::new(135.0, 22.0)));

        let target_label = if s.players.is_empty() {
            "Spectating".to_string()
        } else {
            let idx = self.target_index.min(s.players.len() - 1);
            let (slot, name) = &s.players[idx];
            if let Some((hp, tgt)) = s.spec_health
                && (tgt == 0 || tgt == *slot)
            {
                format!("{name} ({hp})")
            } else {
                name.clone()
            }
        };

        egui::Area::new(egui::Id::unique("cs16_spectator_bottom_controls"))
            .order(egui::Order::Foreground)
            .fixed_pos(bot_rect.min)
            .show(ctx, |ui| {
                ui.set_min_size(bot_rect.size());
                let opt_label = loc_lbl("#Valve_Settings", "Options");
                if runtime
                    .draw_client_button(
                        ui,
                        ctx,
                        s.vfs,
                        "spec_settings_btn",
                        r_settings,
                        &opt_label,
                        false,
                        true,
                        self.settings_popup,
                        "DefaultSmall",
                        TextAlignment::Center,
                        screen_h,
                    )
                    .clicked()
                {
                    self.settings_popup = !self.settings_popup;
                    self.player_popup = false;
                    self.view_popup = false;
                }
                if runtime
                    .draw_client_button(
                        ui,
                        ctx,
                        s.vfs,
                        "spec_prev_btn",
                        r_prev,
                        "<",
                        false,
                        !s.players.is_empty(),
                        false,
                        "DefaultSmall",
                        TextAlignment::Center,
                        screen_h,
                    )
                    .clicked()
                    && !s.players.is_empty()
                {
                    self.target_index = (self.target_index + s.players.len() - 1) % s.players.len();
                    actions.push(EngineAction::Forward(vec!["spec_prev".into()]));
                }
                if runtime
                    .draw_client_button(
                        ui,
                        ctx,
                        s.vfs,
                        "spec_player_btn",
                        r_player,
                        &target_label,
                        false,
                        !s.players.is_empty(),
                        self.player_popup,
                        "DefaultSmall",
                        TextAlignment::Center,
                        screen_h,
                    )
                    .clicked()
                    && !s.players.is_empty()
                {
                    self.player_popup = !self.player_popup;
                    self.settings_popup = false;
                    self.view_popup = false;
                }
                if runtime
                    .draw_client_button(
                        ui,
                        ctx,
                        s.vfs,
                        "spec_next_btn",
                        r_next,
                        ">",
                        false,
                        !s.players.is_empty(),
                        false,
                        "DefaultSmall",
                        TextAlignment::Center,
                        screen_h,
                    )
                    .clicked()
                    && !s.players.is_empty()
                {
                    self.target_index = (self.target_index + 1) % s.players.len();
                    actions.push(EngineAction::Forward(vec!["spec_next".into()]));
                }
                if runtime
                    .draw_client_button(
                        ui,
                        ctx,
                        s.vfs,
                        "spec_view_btn",
                        r_view,
                        self.mode.label(),
                        false,
                        true,
                        self.view_popup,
                        "DefaultSmall",
                        TextAlignment::Center,
                        screen_h,
                    )
                    .clicked()
                {
                    self.view_popup = !self.view_popup;
                    self.settings_popup = false;
                    self.player_popup = false;
                }
            });

        if self.settings_popup {
            let popup_pos = Pos2::new(r_settings.left(), bot_rect.top() - 180.0);
            egui::Area::new(egui::Id::unique("cs16_spec_settings_popup"))
                .order(egui::Order::Foreground)
                .fixed_pos(popup_pos)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 227))
                        .stroke(Stroke::new(1.0, border_col))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            if let Some(ref tree) = self.spec_menu_tree {
                                let entries = flatten_spec_items(&tree.items);
                                for (idx, item) in entries.iter().enumerate() {
                                    let lbl = loc_lbl(&item.label, &item.label);
                                    let checked = item.is_toggle && cvars.get_bool(&item.command);
                                    let show_lbl = if checked { format!("[x] {lbl}") } else { lbl };
                                    let (rr, _) = ui.allocate_exact_size(Vec2::new(190.0, 20.0), egui::Sense::hover());
                                    if runtime
                                        .draw_client_button(
                                            ui,
                                            ctx,
                                            s.vfs,
                                            &format!("spec_opt_{idx}"),
                                            rr,
                                            &show_lbl,
                                            false,
                                            true,
                                            checked,
                                            "DefaultSmall",
                                            TextAlignment::West,
                                            screen_h,
                                        )
                                        .clicked()
                                    {
                                        if item.command == "spec_menu 0" {
                                            self.settings_popup = false;
                                        } else if item.is_toggle {
                                            let next = !cvars.get_bool(&item.command);
                                            cvars.set(&item.command, if next { "1" } else { "0" });
                                        } else {
                                            actions.extend(cvars.execute_script(&item.command));
                                        }
                                    }
                                }
                            } else {
                                for (idx, (cvar, lbl)) in [
                                    ("spec_autodirector", "Auto Director"),
                                    ("spec_drawnames", "Player Names"),
                                    ("spec_drawcone", "Overview View Cones"),
                                    ("spec_drawstatus", "Status Bar"),
                                    ("cl_observercrosshair", "Observer Crosshair"),
                                ]
                                .iter()
                                .enumerate()
                                {
                                    let on = cvars.get_bool(cvar);
                                    let (rr, _) = ui.allocate_exact_size(Vec2::new(190.0, 20.0), egui::Sense::hover());
                                    if runtime
                                        .draw_client_button(
                                            ui,
                                            ctx,
                                            s.vfs,
                                            &format!("spec_fb_{idx}"),
                                            rr,
                                            &format!("{} {lbl}", if on { "[x]" } else { "[ ]" }),
                                            false,
                                            true,
                                            on,
                                            "DefaultSmall",
                                            TextAlignment::West,
                                            screen_h,
                                        )
                                        .clicked()
                                    {
                                        cvars.set(cvar, if on { "0" } else { "1" });
                                    }
                                }
                            }
                        });
                });
        }

        if self.view_popup {
            let popup_pos = Pos2::new(r_view.left(), bot_rect.top() - 148.0);
            egui::Area::new(egui::Id::unique("cs16_spec_view_popup"))
                .order(egui::Order::Foreground)
                .fixed_pos(popup_pos)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 227))
                        .stroke(Stroke::new(1.0, border_col))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            for (m, label) in SpectatorMode::ALL {
                                let (rr, _) = ui.allocate_exact_size(Vec2::new(r_view.width().max(145.0), 20.0), egui::Sense::hover());
                                if runtime
                                    .draw_client_button(
                                        ui,
                                        ctx,
                                        s.vfs,
                                        &format!("spec_mode_{}", m as i32),
                                        rr,
                                        label,
                                        false,
                                        true,
                                        self.mode == m,
                                        "DefaultSmall",
                                        TextAlignment::West,
                                        screen_h,
                                    )
                                    .clicked()
                                {
                                    self.mode = m;
                                    self.view_popup = false;
                                    actions.push(EngineAction::SpecMode(m as i32));
                                }
                            }
                        });
                });
        }

        if self.player_popup && !s.players.is_empty() {
            let h_pop = (s.players.len().min(12) as f32 * 22.0 + 10.0).max(32.0);
            egui::Area::new(egui::Id::unique("cs16_spec_player_popup"))
                .order(egui::Order::Foreground)
                .fixed_pos(Pos2::new(r_player.left(), bot_rect.top() - h_pop))
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 227))
                        .stroke(Stroke::new(1.0, border_col))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            for (i, (_, name)) in s.players.iter().enumerate() {
                                let (rr, _) = ui.allocate_exact_size(Vec2::new(r_player.width().max(200.0), 20.0), egui::Sense::hover());
                                if runtime
                                    .draw_client_button(
                                        ui,
                                        ctx,
                                        s.vfs,
                                        &format!("spec_pl_{i}"),
                                        rr,
                                        name,
                                        false,
                                        true,
                                        self.target_index == i,
                                        "DefaultSmall",
                                        TextAlignment::West,
                                        screen_h,
                                    )
                                    .clicked()
                                {
                                    self.target_index = i;
                                    self.player_popup = false;
                                }
                            }
                        });
                });
        }

        actions
    }
}

struct FlatSpecItem {
    label: String,
    command: String,
    is_toggle: bool,
}

fn flatten_spec_items(items: &[CommandMenuItem]) -> Vec<FlatSpecItem> {
    let mut out = Vec::new();
    for it in items {
        match &it.action {
            CommandMenuAction::Command(cmd) => {
                out.push(FlatSpecItem {
                    label: it.label.clone(),
                    command: cmd.clone(),
                    is_toggle: it.toggle_default.is_some(),
                });
            }
            CommandMenuAction::Submenu(sub) => {
                out.extend(flatten_spec_items(sub));
            }
        }
    }
    out
}
