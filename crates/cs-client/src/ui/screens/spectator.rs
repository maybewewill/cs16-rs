//! `.res`-driven CS 1.6 Spectator letterbox & bottom control bar
//! (`Resource/UI/Spectator.res`, `BottomSpectator.res`, `spectatormenu.txt`, `ClientScheme.res`).

use std::collections::BTreeMap;

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_assets::vgui_scripts::{CommandMenuAction, CommandMenuItem, CommandMenuTree};
use cs_clientcore::hud::HudState;
use cs_cvar::{CVarRegistry, EngineAction};
use cs_vfs::Vfs;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

use crate::ui::hud::HudSprites;
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

/// One player card in the HLTV-style CT/T spectator sidebar (`spec_hud_bars 1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecSidebarPlayer {
    pub player_id: u8,
    pub slot_key: u8,
    pub name: String,
    pub team: String,
    pub alive: bool,
    pub health: i32,
    pub money: Option<i32>,
    pub armor: Option<i16>,
    pub has_helmet: bool,
    pub has_defuser: bool,
    pub has_c4: bool,
    pub is_vip: bool,
    pub weapon: Option<String>,
    pub frags: i16,
    pub deaths: i16,
    pub observed: bool,
}

/// Build CT and Terrorist sidebar player entries from `HudState` and entity weapon models.
pub fn collect_spec_sidebar_players(
    hud: &HudState,
    names: &BTreeMap<u8, String>,
    player_weapons: &BTreeMap<u8, String>,
    observed_target: u8,
) -> Vec<SpecSidebarPlayer> {
    let mut ct = Vec::new();
    let mut t = Vec::new();
    for (&player_id, score) in &hud.players {
        let team = if score.team.eq_ignore_ascii_case("CT") {
            "CT"
        } else if score.team.eq_ignore_ascii_case("TERRORIST") {
            "TERRORIST"
        } else if !score.team.is_empty() {
            continue;
        } else if score.team_id == 2 {
            "CT"
        } else if score.team_id == 1 {
            "TERRORIST"
        } else {
            continue;
        };
        let slot0 = player_id.saturating_sub(1);
        let name = names
            .get(&slot0)
            .cloned()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Player {player_id}"));
        let alive = (score.attrib & 1) == 0;
        let has_c4 = (score.attrib & 2) != 0;
        let is_vip = (score.attrib & 4) != 0;
        let observed = observed_target != 0 && observed_target == player_id;
        let health = if !alive {
            0
        } else if let Some(&hp) = hud.player_health.get(&player_id) {
            hp.max(0)
        } else if observed && let Some((hp, _)) = hud.spec_health {
            i32::from(hp)
        } else {
            100
        };
        let money = hud.player_money.get(&player_id).copied();
        let (armor, has_helmet, has_defuser) = if observed {
            (
                (hud.armor > 0).then_some(hud.armor),
                hud.armor > 0 && hud.armor_type >= 1,
                (hud.item_status & 2) != 0,
            )
        } else {
            (None, false, false)
        };
        let weapon = player_weapons.get(&player_id).cloned();
        let entry = SpecSidebarPlayer {
            player_id,
            slot_key: 0,
            name,
            team: team.to_string(),
            alive,
            health,
            money,
            armor,
            has_helmet,
            has_defuser,
            has_c4,
            is_vip,
            weapon,
            frags: score.frags,
            deaths: score.deaths,
            observed,
        };
        if team == "CT" {
            ct.push(entry);
        } else {
            t.push(entry);
        }
    }
    let assign_keys = |list: &mut [SpecSidebarPlayer], start: usize| {
        for (idx, item) in list.iter_mut().enumerate() {
            let num = start + idx + 1;
            item.slot_key = if num == 10 { 0 } else { (num % 10) as u8 };
        }
    };
    let ct_len = ct.len();
    assign_keys(&mut ct, 0);
    assign_keys(&mut t, ct_len);
    ct.extend(t);
    ct
}

#[derive(Default)]
pub struct SpectatorRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub ct_score: i16,
    pub t_score: i16,
    pub round_timer_secs: u32,
    pub players: &'a [(u8, String)],
    pub spec_health: Option<(u8, u8)>,
    pub sidebar_players: &'a [SpecSidebarPlayer],
    pub hud_sprites: Option<&'a HudSprites>,
    /// Skip the top/bottom letterbox bars (e.g. POV `.dem` playback with `spec_hud_bars 1`).
    pub suppress_letterbox: bool,
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
            menu_visible: false,
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
        let sy = (screen.height() / 480.0).max(0.5);

        // `CSpectatorGUI` (`FUN_100aff00`): topbar / bottombar height = `GetProportionalScaledValue(64)`
        // (`107 px` at 1280x800, `80 px` at 800x600), translucent black fill, no orange edge lines.
        let bar_h = (64.0 * sy).round();
        let top_rect = Rect::from_min_size(screen.min, Vec2::new(screen.width(), bar_h));
        let bot_rect = Rect::from_min_size(
            Pos2::new(screen.left(), screen.bottom() - bar_h),
            Vec2::new(screen.width(), bar_h),
        );

        if cvars.get_bool("spec_hud_bars") && !s.sidebar_players.is_empty() {
            let side_top = if s.suppress_letterbox {
                screen.top() + (72.0 * sy).round()
            } else {
                top_rect.bottom() + (10.0 * sy).round()
            };
            let side_bottom = if s.suppress_letterbox {
                screen.bottom() - (72.0 * sy).round()
            } else {
                bot_rect.top() - (10.0 * sy).round()
            };
            draw_spec_hud_bars(
                ctx,
                screen,
                side_top,
                side_bottom,
                sy,
                s.sidebar_players,
                s.hud_sprites,
                &mut actions,
            );
        }

        if s.suppress_letterbox {
            return actions;
        }

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

        painter.rect_filled(top_rect, 0.0, Color32::from_black_alpha(205));
        painter.rect_filled(bot_rect, 0.0, Color32::from_black_alpha(205));

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

        // `Spectator.res` right-anchored proportional coords (`rN = screen.right - N * sy`).
        let top_r = |r_off: f32, y: f32, w: f32, h: f32| -> Rect {
            Rect::from_min_size(
                Pos2::new(
                    (screen.right() - r_off * sy).round(),
                    (screen.top() + y * sy).round(),
                ),
                Vec2::new((w * sy).round().max(1.0), (h * sy).round().max(1.0)),
            )
        };
        let ct_lbl_rect = top_r(233.0, 12.0, 150.0, 14.0);
        let ct_val_rect = top_r(80.0, 12.0, 16.0, 14.0);
        let t_lbl_rect = top_r(233.0, 24.0, 150.0, 14.0);
        let t_val_rect = top_r(80.0, 24.0, 16.0, 14.0);
        let div_rect = top_r(64.0, 12.0, 1.0, 30.0);
        let timer_img_rect = top_r(56.0, 26.0, 12.0, 12.0);
        let timer_lbl_rect = top_r(42.0, 24.0, 40.0, 14.0);

        let ct_text = loc_lbl("#Cstrike_Spec_CT_Score", "Counter-Terrorists :");
        let t_text = loc_lbl("#Cstrike_Spec_Ter_Score", "Terrorists :");
        let timer_text = format!("{}:{:02}", s.round_timer_secs / 60, s.round_timer_secs % 60);
        for (rect, txt, align) in [
            (ct_lbl_rect, ct_text.as_str(), TextAlignment::East),
            (t_lbl_rect, t_text.as_str(), TextAlignment::East),
            (timer_lbl_rect, timer_text.as_str(), TextAlignment::West),
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
                amber,
                VguiTextDrawOpts {
                    align,
                    process_hotkey: false,
                    proportional_scale: false,
                },
            );
        }
        for (rect, val) in [(ct_val_rect, s.ct_score), (t_val_rect, s.t_score)] {
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
                amber,
                VguiTextDrawOpts {
                    align: TextAlignment::West,
                    process_hotkey: false,
                    proportional_scale: false,
                },
            );
        }
        painter.rect_filled(Rect::from_min_size(div_rect.min, Vec2::new(1.0, div_rect.height())), 0.0, border_col);
        if let Some(tex) = runtime.ensure_texture(ctx, s.vfs, "gfx/vgui/timer") {
            draw_vgui_image_panel(&painter, timer_img_rect, Some(&tex), amber);
        }

        if !self.menu_visible {
            let toggle_hit = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::unique("cs16_spec_bot_hit"),
            ));
            let _ = toggle_hit;
            egui::Area::new(egui::Id::unique("cs16_spectator_bottom_toggle"))
                .order(egui::Order::Middle)
                .fixed_pos(bot_rect.min)
                .show(ctx, |ui| {
                    let (_, resp) = ui.allocate_exact_size(bot_rect.size(), egui::Sense::click());
                    if resp.clicked() {
                        self.menu_visible = true;
                    }
                });
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

#[allow(clippy::too_many_arguments)]
fn draw_spec_hud_bars(
    ctx: &egui::Context,
    screen: Rect,
    top_y: f32,
    bottom_y: f32,
    sy: f32,
    players: &[SpecSidebarPlayer],
    sprites: Option<&HudSprites>,
    actions: &mut Vec<EngineAction>,
) {
    let ct: Vec<&SpecSidebarPlayer> = players.iter().filter(|p| p.team == "CT").collect();
    let t: Vec<&SpecSidebarPlayer> = players.iter().filter(|p| p.team == "TERRORIST").collect();
    let max_cards = ((bottom_y - top_y) / (38.0 * sy)).floor().max(1.0) as usize;
    let card_w = (192.0 * sy).round().clamp(150.0, 260.0);
    let card_h = (34.0 * sy).round().clamp(28.0, 46.0);
    let gap = (4.0 * sy).round().max(2.0);
    let margin_x = (8.0 * sy).round().max(4.0);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Middle,
        egui::Id::unique("cs16_spec_hud_bars"),
    ));
    let font_sm = egui::FontId::proportional((11.0 * sy).clamp(10.0, 15.0));
    let font_xs = egui::FontId::monospace((10.0 * sy).clamp(9.0, 13.0));

    let mut draw_team_column = |list: &[&SpecSidebarPlayer], left_x: f32, is_ct: bool| {
        let accent = if is_ct {
            Color32::from_rgb(64, 144, 248)
        } else {
            Color32::from_rgb(235, 92, 52)
        };
        let fill_alive = if is_ct {
            Color32::from_rgba_unmultiplied(18, 28, 44, 210)
        } else {
            Color32::from_rgba_unmultiplied(44, 22, 18, 210)
        };
        let fill_dead = Color32::from_rgba_unmultiplied(16, 16, 18, 150);

        for (i, p) in list.iter().take(max_cards).enumerate() {
            let y = top_y + (i as f32) * (card_h + gap);
            let rect = Rect::from_min_size(Pos2::new(left_x, y), Vec2::new(card_w, card_h));
            let bg = if p.alive { fill_alive } else { fill_dead };
            painter.rect_filled(rect, 2.0, bg);

            // Proportional HP fill bar across bottom half of alive card.
            if p.alive {
                let hp_frac = (p.health as f32 / 100.0).clamp(0.0, 1.0);
                let hp_w = rect.width() * hp_frac;
                let hp_rect = Rect::from_min_size(
                    Pos2::new(rect.left(), rect.bottom() - (4.0 * sy).max(3.0)),
                    Vec2::new(hp_w, (4.0 * sy).max(3.0)),
                );
                let hp_col = if p.health > 50 {
                    accent
                } else if p.health > 20 {
                    Color32::from_rgb(235, 185, 50)
                } else {
                    Color32::from_rgb(225, 55, 45)
                };
                painter.rect_filled(hp_rect, 0.0, hp_col);
            }

            let border = if p.observed {
                Stroke::new(2.0, Color32::from_rgb(255, 200, 60))
            } else if p.alive {
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 140))
            } else {
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(90, 90, 90, 100))
            };
            painter.rect_stroke(rect, 2.0, border, egui::StrokeKind::Inside);

            let text_col = if p.alive {
                Color32::from_rgb(240, 240, 240)
            } else {
                Color32::from_rgb(140, 140, 140)
            };
            let hdr_name: String = p.name.chars().take(15).collect();
            let top_txt = format!("[{}] {}", p.slot_key, hdr_name);
            painter.text(
                Pos2::new(rect.left() + 5.0, rect.top() + 3.0),
                egui::Align2::LEFT_TOP,
                top_txt,
                font_sm.clone(),
                text_col,
            );
            let kd_txt = format!("{} / {}", p.frags, p.deaths);
            painter.text(
                Pos2::new(rect.right() - 5.0, rect.top() + 3.0),
                egui::Align2::RIGHT_TOP,
                kd_txt,
                font_xs.clone(),
                Color32::from_rgb(200, 200, 200),
            );

            // Row 2: HP, $money, armor/helmet/defuser/C4 badges, primary weapon silhouette.
            let row2_y = rect.top() + (16.0 * sy).round();
            let hp_txt = if p.alive {
                format!("{} HP", p.health)
            } else {
                "DEAD".to_string()
            };
            painter.text(
                Pos2::new(rect.left() + 5.0, row2_y),
                egui::Align2::LEFT_TOP,
                hp_txt,
                font_xs.clone(),
                if p.alive { accent } else { Color32::from_rgb(130, 130, 130) },
            );

            let mut badge_x = rect.left() + (52.0 * sy).round();
            if let Some(money) = p.money {
                painter.text(
                    Pos2::new(badge_x, row2_y),
                    egui::Align2::LEFT_TOP,
                    format!("${money}"),
                    font_xs.clone(),
                    Color32::from_rgb(125, 220, 115),
                );
                badge_x += (44.0 * sy).round();
            }

            let mut badges = Vec::new();
            if p.armor.is_some_and(|a| a > 0) {
                badges.push(if p.has_helmet { "A+H" } else { "A" });
            }
            if p.has_defuser {
                badges.push("DEF");
            }
            if p.has_c4 {
                badges.push("C4");
            }
            if p.is_vip {
                badges.push("VIP");
            }
            if !badges.is_empty() {
                let bstr = badges.join(" ");
                painter.text(
                    Pos2::new(badge_x, row2_y),
                    egui::Align2::LEFT_TOP,
                    bstr,
                    font_xs.clone(),
                    Color32::from_rgb(255, 196, 64),
                );
            }

            if let Some(wpn) = &p.weapon {
                let mut drew_spr = false;
                if let Some(sp) = sprites {
                    let icon_scale = (0.28 * sy).clamp(0.22, 0.42);
                    let pos = Pos2::new(rect.right() - (46.0 * sy).round(), row2_y - 2.0);
                    if sp.draw_weapon_card(&painter, wpn, pos, icon_scale, text_col).is_some() {
                        drew_spr = true;
                    }
                }
                if !drew_spr {
                    painter.text(
                        Pos2::new(rect.right() - 5.0, row2_y),
                        egui::Align2::RIGHT_TOP,
                        wpn.to_ascii_uppercase(),
                        font_xs.clone(),
                        text_col,
                    );
                }
            }

            egui::Area::new(egui::Id::unique(("cs16_spec_card", p.player_id)))
                .order(egui::Order::Middle)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (_, resp) = ui.allocate_exact_size(rect.size(), egui::Sense::click());
                    if resp.clicked() {
                        actions.push(EngineAction::Forward(vec!["follow".into(), p.name.clone()]));
                    }
                });
        }
    };

    draw_team_column(&ct, screen.left() + margin_x, true);
    draw_team_column(&t, screen.right() - card_w - margin_x, false);
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
