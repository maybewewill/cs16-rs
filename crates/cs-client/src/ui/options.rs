//! Classic Counter-Strike 1.6 VGUI2 `Options` dialog (`OptionsSub*.res` + `MultiplayerAdvancedDialog.res`
//! + `ContentControlDialog.res` + `gfx/shell/kb_act.lst` + `logos/*.bmp`) bound live to `CVarRegistry`.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_scripts::{ScrOptionKind, SettingsScript};
use cs_cvar::CVarRegistry;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, ScrollArea, Stroke, TextureHandle, Vec2};

use crate::ui::parse_rgb;
use crate::ui::vgui2::{TrackerButtonCfg, VguiRuntime};
use crate::ui::vgui_assets::{TrackerWindowCfg, show_tracker_window};
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

/// Render context for [`OptionsUi`].
pub struct OptionsRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub palette: VguiPalette,
    pub steam_icon: Option<&'a TextureHandle>,
}

/// One row from Valve/CS 1.6 `gfx/shell/kb_act.lst`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbActEntry {
    pub command: String,
    pub label: String,
    pub is_header: bool,
}

const SPRAY_COLOR_PRESETS: &[(&str, Color32)] = &[
    ("Orange", Color32::from_rgb(255, 160, 32)),
    ("Yellow", Color32::from_rgb(245, 225, 48)),
    ("Blue", Color32::from_rgb(64, 150, 255)),
    ("Ltblue", Color32::from_rgb(100, 225, 255)),
    ("Green", Color32::from_rgb(64, 225, 96)),
    ("Red", Color32::from_rgb(235, 64, 56)),
    ("White", Color32::from_rgb(240, 240, 240)),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptionsTab {
    #[default]
    Multiplayer,
    Keyboard,
    Mouse,
    Audio,
    Video,
    Voice,
    Lock,
}

#[derive(Default)]
pub struct OptionsUi {
    pub open: bool,
    pub tab: OptionsTab,
    rebinding_cmd: Option<String>,
    pub advanced_open: bool,
    pub user_script: Option<SettingsScript>,
    pub kb_actions: Vec<KbActEntry>,
    pub spray_logos: Vec<String>,
    pub selected_spray: String,
    pub spray_color_idx: usize,
    pub lock_password: String,
    pub content_locked: bool,
    pub save_requested: bool,
    pub hd_models_changed: bool,
}

/// Parse Valve/CS 1.6 `gfx/shell/kb_act.lst` into section headers and bindable commands.
pub fn parse_kb_act_list(text: &str) -> Vec<KbActEntry> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let mut parts = Vec::new();
        let mut chars = trimmed.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
                continue;
            }
            if c == '"' {
                chars.next();
                let mut val = String::new();
                for ch in chars.by_ref() {
                    if ch == '"' {
                        break;
                    }
                    val.push(ch);
                }
                parts.push(val);
            } else {
                let mut val = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() {
                        break;
                    }
                    val.push(ch);
                    chars.next();
                }
                parts.push(val);
            }
        }
        if parts.len() < 2 {
            continue;
        }
        let cmd = parts[0].trim();
        let label = parts[1].trim();
        if label.chars().all(|c| c == '=') {
            continue;
        }
        let is_header = cmd.eq_ignore_ascii_case("blank");
        out.push(KbActEntry {
            command: if is_header { String::new() } else { cmd.to_string() },
            label: label.to_string(),
            is_header,
        });
    }
    out
}

const STANDARD_BIND_ACTIONS: &[(&str, &str)] = &[
    ("Move forward", "+forward"),
    ("Move back", "+back"),
    ("Move left (strafe)", "+moveleft"),
    ("Move right (strafe)", "+moveright"),
    ("Jump / Fly Up", "+jump"),
    ("Duck / Fly Down", "+duck"),
    ("Walk (slow)", "+speed"),
    ("Fire", "+attack"),
    ("Special / Zoom", "+attack2"),
    ("Reload", "+reload"),
    ("Use item", "+use"),
    ("Buy menu", "buy"),
    ("Command menu", "+commandmenu"),
    ("Use voice communication", "+voicerecord"),
    ("Drop weapon", "drop"),
    ("Last weapon used", "lastinv"),
    ("Display scores", "+showscores"),
    ("Toggle Console", "toggleconsole"),
];

impl OptionsUi {
    pub fn load_resources(&mut self, vfs: &Vfs) {
        self.user_script = SettingsScript::load(vfs, "user.scr");
        if let Ok(lst) = vfs.read_string_lossy("gfx/shell/kb_act.lst") {
            let parsed = parse_kb_act_list(&lst);
            if !parsed.is_empty() {
                self.kb_actions = parsed;
            }
        }
        let all_bmps = vfs.list_dir("logos", Some("bmp"));
        let mut logos: Vec<String> = all_bmps
            .iter()
            .filter(|name| !name.eq_ignore_ascii_case("remapped.bmp"))
            .cloned()
            .collect();
        if logos.is_empty() {
            logos = all_bmps;
        }
        self.spray_logos = logos;
        if self.selected_spray.is_empty()
            || !self.spray_logos.iter().any(|l| l == &self.selected_spray)
        {
            self.selected_spray = self
                .spray_logos
                .iter()
                .find(|l| l.eq_ignore_ascii_case("lambda.bmp"))
                .or_else(|| self.spray_logos.first())
                .cloned()
                .unwrap_or_default();
        }
    }

    pub fn load_user_script(&mut self, vfs: &Vfs) {
        self.load_resources(vfs);
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
    ) {
        if !self.open {
            self.rebinding_cmd = None;
            return;
        }

        if let Some(v) = o.vfs {
            for res in [
                "resource/OptionsSubMultiplayer.res",
                "resource/OptionsSubKeyboard.res",
                "resource/OptionsSubMouse.res",
                "resource/OptionsSubAudio.res",
                "resource/OptionsSubVideo.res",
                "resource/OptionsSubVoice.res",
                "resource/ContentControlDialog.res",
                "resource/MultiplayerAdvancedDialog.res",
            ] {
                runtime.load_res(v, res);
            }
        }

        // Capture key press while rebinding an action in Keyboard tab
        if let Some(ref target_cmd) = self.rebinding_cmd.clone() {
            let captured = ctx.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Key {
                        key, pressed: true, ..
                    } = ev
                    {
                        if *key == egui::Key::Escape {
                            return Some(None);
                        }
                        return Some(Some(egui_key_to_goldsrc(*key)));
                    }
                }
                None
            });
            if let Some(maybe_key) = captured {
                if let Some(new_key) = maybe_key {
                    if let Some(old) = cvars.key_for_command(target_cmd).map(str::to_string) {
                        cvars.unbind(&old);
                    }
                    cvars.bind(&new_key, target_cmd);
                }
                self.rebinding_cmd = None;
            }
        }

        let palette = o.palette;
        let mut is_open = self.open;
        let mut close_now = false;
        let title = loc_or(o.loc, "#GameUI_Options", "Options");

        show_tracker_window(
            ctx,
            runtime,
            &mut is_open,
            &TrackerWindowCfg {
                id: "cs16_vgui_options",
                title: &title,
                default_pos: egui::pos2(160.0, 80.0),
                default_size: egui::vec2(556.0, 445.0),
                min_size: egui::vec2(530.0, 420.0),
                resizable: false,
                palette,
                steam_icon: o.steam_icon,
                vfs: o.vfs,
            },
            |ui, rt| {
                let tabs = [
                    (OptionsTab::Multiplayer, loc_or(o.loc, "#GameUI_Multiplayer", "Multiplayer")),
                    (OptionsTab::Keyboard, loc_or(o.loc, "#GameUI_Keyboard", "Keyboard")),
                    (OptionsTab::Mouse, loc_or(o.loc, "#GameUI_Mouse", "Mouse")),
                    (OptionsTab::Audio, loc_or(o.loc, "#GameUI_Audio", "Audio")),
                    (OptionsTab::Video, loc_or(o.loc, "#GameUI_Video", "Video")),
                    (OptionsTab::Voice, loc_or(o.loc, "#GameUI_Voice", "Voice")),
                    (OptionsTab::Lock, loc_or(o.loc, "#GameUI_Lock", "Lock")),
                ];
                ui.horizontal(|ui| {
                    for (t, label) in &tabs {
                        if rt.draw_tracker_tab(
                            ui,
                            o.vfs,
                            palette,
                            &format!("opt_tab_{label}"),
                            label,
                            self.tab == *t,
                        ) {
                            self.tab = *t;
                            self.rebinding_cmd = None;
                        }
                    }
                });

                let body_frame = egui::Frame::NONE
                    .fill(palette.control_bg)
                    .inner_margin(egui::Margin::same(10))
                    .stroke(Stroke::new(1.0, palette.border_dark));

                let body_resp = body_frame.show(ui, |ui| {
                    ui.set_min_size(egui::vec2(536.0, 336.0));
                    match self.tab {
                        OptionsTab::Multiplayer => self.show_multiplayer(ctx, ui, rt, cvars, o),
                        OptionsTab::Keyboard => self.show_keyboard(ui, rt, cvars, o),
                        OptionsTab::Mouse => self.show_mouse(ui, cvars),
                        OptionsTab::Audio => self.show_audio(ui, cvars),
                        OptionsTab::Video => self.show_video(ui, cvars),
                        OptionsTab::Voice => self.show_voice(ui, cvars),
                        OptionsTab::Lock => self.show_lock(ui, rt, cvars, o),
                    }
                });
                paint_vgui_bevel(ui.painter(), body_resp.response.rect, false, palette);

                ui.add_space(8.0);
                let ok_lbl = loc_or(o.loc, "#GameUI_OK", "OK");
                let cancel_lbl = loc_or(o.loc, "#GameUI_Cancel", "Cancel");
                let apply_lbl = loc_or(o.loc, "#GameUI_Apply", "Apply");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if rt
                        .draw_tracker_button(
                            ui,
                            o.vfs,
                            palette,
                            &TrackerButtonCfg {
                                id: "opt_apply",
                                label: &apply_lbl,
                                min_size: Vec2::new(74.0, 24.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        self.save_requested = true;
                    }
                    if rt
                        .draw_tracker_button(
                            ui,
                            o.vfs,
                            palette,
                            &TrackerButtonCfg {
                                id: "opt_cancel",
                                label: &cancel_lbl,
                                min_size: Vec2::new(74.0, 24.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        close_now = true;
                    }
                    if rt
                        .draw_tracker_button(
                            ui,
                            o.vfs,
                            palette,
                            &TrackerButtonCfg {
                                id: "opt_ok",
                                label: &ok_lbl,
                                min_size: Vec2::new(74.0, 24.0),
                                default_btn: true,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        self.save_requested = true;
                        close_now = true;
                    }
                });
            },
        );

        if !is_open || close_now {
            self.open = false;
            self.advanced_open = false;
        }

        if self.advanced_open {
            let mut adv = self.advanced_open;
            let mut adv_close = false;
            let adv_win = runtime.doc_window_size("resource/MultiplayerAdvancedDialog.res", [472.0, 376.0]);
            let adv_title = loc_or(o.loc, "#GameUI_MultiplayerAdvanced", "Multiplayer Advanced");
            let ok_lbl = loc_or(o.loc, "#GameUI_OK", "OK");
            let cancel_lbl = loc_or(o.loc, "#GameUI_Cancel", "Cancel");

            show_tracker_window(
                ctx,
                runtime,
                &mut adv,
                &TrackerWindowCfg {
                    id: "cs16_vgui_options_advanced",
                    title: &adv_title,
                    default_pos: Pos2::new(205.0, 115.0),
                    default_size: Vec2::new(adv_win[0].max(420.0), adv_win[1].max(340.0)),
                    min_size: Vec2::new(380.0, 280.0),
                    resizable: false,
                    palette,
                    steam_icon: o.steam_icon,
                    vfs: o.vfs,
                },
                |ui, rt| {
                    let list_resp = egui::Frame::NONE
                        .fill(palette.window_bg)
                        .inner_margin(egui::Margin::same(8))
                        .stroke(Stroke::new(1.0, palette.border_dark))
                        .show(ui, |ui| {
                            ScrollArea::vertical().max_height(274.0).show(ui, |ui| {
                                ui.set_min_width(410.0);
                                if let Some(ref scr) = self.user_script {
                                    render_scr_options(ui, cvars, scr, o.loc);
                                } else {
                                    cvar_checkbox(ui, cvars, "hud_centerid", "Center player ID names (hud_centerid)");
                                    cvar_checkbox(ui, cvars, "hud_takesshots", "Auto-take scoreboard screenshots");
                                    cvar_checkbox(ui, cvars, "cl_radartype", "Solid radar type (cl_radartype)");
                                    cvar_checkbox(ui, cvars, "cl_observercrosshair", "Show crosshair in observer mode");
                                }
                            });
                        });
                    paint_vgui_bevel(ui.painter(), list_resp.response.rect, false, palette);
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if rt
                            .draw_tracker_button(
                                ui,
                                o.vfs,
                                palette,
                                &TrackerButtonCfg {
                                    id: "opt_adv_cancel",
                                    label: &cancel_lbl,
                                    min_size: Vec2::new(72.0, 24.0),
                                    default_btn: false,
                                    enabled: true,
                                },
                            )
                            .clicked()
                        {
                            adv_close = true;
                        }
                        if rt
                            .draw_tracker_button(
                                ui,
                                o.vfs,
                                palette,
                                &TrackerButtonCfg {
                                    id: "opt_adv_ok",
                                    label: &ok_lbl,
                                    min_size: Vec2::new(72.0, 24.0),
                                    default_btn: true,
                                    enabled: true,
                                },
                            )
                            .clicked()
                        {
                            adv_close = true;
                        }
                    });
                },
            );
            if !adv || adv_close {
                self.advanced_open = false;
            }
        }
    }

    fn show_multiplayer(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
    ) {
        let palette = o.palette;
        let mut name = cvars.get_str("name").to_string();
        ui.horizontal(|ui| {
            let name_lbl = loc_or(o.loc, "#GameUI_PlayerName", "Player name");
            ui.label(format!("{name_lbl}:"));
            if ui.text_edit_singleline(&mut name).changed() {
                cvars.set("name", name);
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            // Left column: Spraypaint Logo preview (`LogoImage`) + selectors (`SpraypaintList` / `SpraypaintColor`)
            ui.vertical(|ui| {
                ui.set_min_width(245.0);
                let spray_lbl = loc_or(o.loc, "#GameUI_SpraypaintImage", "Spraypaint image");
                ui.label(RichText::new(spray_lbl).color(palette.bright_control_text));
                ui.horizontal(|ui| {
                    let (logo_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(64.0), egui::Sense::hover());
                    ui.painter().rect_filled(logo_rect, CornerRadius::ZERO, Color32::from_rgb(24, 26, 20));
                    paint_vgui_bevel(ui.painter(), logo_rect, false, palette);
                    let tint = SPRAY_COLOR_PRESETS
                        .get(self.spray_color_idx)
                        .map_or(Color32::WHITE, |(_, c)| *c);
                    if !self.selected_spray.is_empty()
                        && let Some(tex) =
                            rt.ensure_texture(ctx, o.vfs, &format!("logos/{}", self.selected_spray))
                    {
                        ui.painter().image(
                            tex.id(),
                            logo_rect.shrink(2.0),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            tint,
                        );
                    }
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        let sel_spray = if self.selected_spray.is_empty() {
                            "lambda.bmp"
                        } else {
                            &self.selected_spray
                        };
                        egui::ComboBox::from_id_salt("opt_spray_list")
                            .width(140.0)
                            .selected_text(sel_spray)
                            .show_ui(ui, |ui| {
                                for logo in &self.spray_logos {
                                    ui.selectable_value(&mut self.selected_spray, logo.clone(), logo);
                                }
                            });
                        let cur_color_name = SPRAY_COLOR_PRESETS
                            .get(self.spray_color_idx)
                            .map_or("Orange", |(n, _)| *n);
                        egui::ComboBox::from_id_salt("opt_spray_color")
                            .width(140.0)
                            .selected_text(cur_color_name)
                            .show_ui(ui, |ui| {
                                for (idx, (cname, _)) in SPRAY_COLOR_PRESETS.iter().enumerate() {
                                    ui.selectable_value(&mut self.spray_color_idx, idx, *cname);
                                }
                            });
                    });
                });
            });

            ui.add_space(12.0);
            // Right column: Crosshair preview (`OptionsSubMultiplayer.res`)
            ui.vertical(|ui| {
                let ch_lbl = loc_or(o.loc, "#GameUI_CrosshairDescription", "Crosshair appearance");
                ui.label(RichText::new(ch_lbl).color(palette.bright_control_text));
                ui.horizontal(|ui| {
                    let (preview_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(64.0), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(preview_rect, CornerRadius::ZERO, Color32::from_rgb(38, 42, 34));
                    paint_vgui_bevel(p, preview_rect, false, palette);
                    let (cr, cg, cb) = parse_rgb(cvars.get_str("cl_crosshair_color"));
                    let ca = if cvars.get_bool("cl_crosshair_translucent") { 195 } else { 255 };
                    let ch_col = Color32::from_rgba_unmultiplied(cr, cg, cb, ca);
                    let (len, gap) = match cvars.get_str("cl_crosshair_size").to_ascii_lowercase().as_str() {
                        "small" => (5.0f32, 3.0f32),
                        "large" => (10.0f32, 5.0f32),
                        _ => (7.5f32, 4.0f32),
                    };
                    let c = preview_rect.center();
                    let s = Stroke::new(1.5, ch_col);
                    p.line_segment([Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)], s);
                    p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], s);
                    p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)], s);
                    p.line_segment([Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)], s);

                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        let cur_size = cvars.get_str("cl_crosshair_size").to_string();
                        egui::ComboBox::from_id_salt("opt_ch_size")
                            .width(120.0)
                            .selected_text(&cur_size)
                            .show_ui(ui, |ui| {
                                for sz in ["auto", "small", "medium", "large"] {
                                    if ui.selectable_label(cur_size == sz, sz).clicked() {
                                        cvars.set("cl_crosshair_size", sz);
                                    }
                                }
                            });
                        let cur_color = cvars.get_str("cl_crosshair_color").to_string();
                        let presets = [
                            ("Green", "50 250 50"),
                            ("Red", "250 50 50"),
                            ("Blue", "50 50 250"),
                            ("Yellow", "250 250 50"),
                            ("Ltblue", "50 250 250"),
                            ("White", "255 255 255"),
                        ];
                        let preset_name = presets
                            .iter()
                            .find(|(_, v)| *v == cur_color)
                            .map(|(n, _)| *n)
                            .unwrap_or("Custom");
                        egui::ComboBox::from_id_salt("opt_ch_color")
                            .width(120.0)
                            .selected_text(preset_name)
                            .show_ui(ui, |ui| {
                                for (label, rgb) in presets {
                                    if ui.selectable_label(cur_color == rgb, label).clicked() {
                                        cvars.set("cl_crosshair_color", rgb);
                                    }
                                }
                            });
                        cvar_checkbox(
                            ui,
                            cvars,
                            "cl_crosshair_translucent",
                            &loc_or(o.loc, "#GameUI_Translucent", "Translucent"),
                        );
                        cvar_checkbox(ui, cvars, "cl_dynamiccrosshair", "Dynamic crosshair");
                    });
                });
            });
        });

        ui.separator();
        cvar_checkbox(ui, cvars, "cl_righthand", "Right-handed weapon models (cl_righthand)");
        cvar_checkbox(ui, cvars, "hud_fastswitch", "Fast weapon switch (hud_fastswitch)");
        ui.add_space(6.0);
        let adv_lbl = loc_or(o.loc, "#GameUI_AdvancedEllipsis", "Advanced...");
        if rt
            .draw_tracker_button(
                ui,
                o.vfs,
                palette,
                &TrackerButtonCfg {
                    id: "opt_mp_advanced",
                    label: &adv_lbl,
                    min_size: Vec2::new(120.0, 24.0),
                    default_btn: false,
                    enabled: true,
                },
            )
            .clicked()
        {
            self.advanced_open = true;
        }
    }

    fn show_keyboard(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
    ) {
        let palette = o.palette;
        if let Some(ref cmd) = self.rebinding_cmd {
            ui.label(
                RichText::new(format!("Press a key to bind to \"{cmd}\" (Esc to cancel)..."))
                    .color(palette.bright_control_text),
            );
        } else {
            ui.label(
                RichText::new("Double-click or click a key button to rebind:").color(palette.dim_text),
            );
        }

        let list_frame = egui::Frame::NONE
            .fill(palette.window_bg)
            .inner_margin(egui::Margin::same(6))
            .stroke(Stroke::new(1.0, palette.border_dark));
        let frame_resp = list_frame.show(ui, |ui| {
            ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                egui::Grid::new("kb_binds_grid")
                    .num_columns(3)
                    .min_col_width(145.0)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Action").color(palette.bright_control_text));
                        ui.label(RichText::new("Command").color(palette.bright_control_text));
                        ui.label(RichText::new("Key / Button").color(palette.bright_control_text));
                        ui.end_row();

                        if !self.kb_actions.is_empty() {
                            let entries = self.kb_actions.clone();
                            for entry in entries {
                                let label = loc_or(o.loc, &entry.label, &entry.label);
                                if entry.is_header {
                                    ui.label(
                                        RichText::new(label)
                                            .strong()
                                            .color(palette.bright_control_text),
                                    );
                                    ui.label("");
                                    ui.label("");
                                    ui.end_row();
                                    continue;
                                }
                                ui.label(label);
                                ui.label(RichText::new(&entry.command).monospace().color(palette.dim_text));
                                let current = cvars
                                    .key_for_command(&entry.command)
                                    .map(str::to_ascii_uppercase)
                                    .unwrap_or_else(|| "<unbound>".to_string());
                                let btn_label = if self.rebinding_cmd.as_deref() == Some(entry.command.as_str()) {
                                    "[ Press key... ]".to_string()
                                } else {
                                    current
                                };
                                if ui
                                    .add(
                                        egui::Button::new(btn_label)
                                            .min_size(egui::vec2(108.0, 20.0))
                                            .corner_radius(CornerRadius::ZERO),
                                    )
                                    .clicked()
                                {
                                    self.rebinding_cmd = Some(entry.command.clone());
                                }
                                ui.end_row();
                            }
                        } else {
                            for &(label, cmd) in STANDARD_BIND_ACTIONS {
                                ui.label(label);
                                ui.label(RichText::new(cmd).monospace().color(palette.dim_text));
                                let current = cvars
                                    .key_for_command(cmd)
                                    .map(str::to_ascii_uppercase)
                                    .unwrap_or_else(|| "<unbound>".to_string());
                                let btn_label = if self.rebinding_cmd.as_deref() == Some(cmd) {
                                    "[ Press key... ]".to_string()
                                } else {
                                    current
                                };
                                if ui
                                    .add(
                                        egui::Button::new(btn_label)
                                            .min_size(egui::vec2(108.0, 20.0))
                                            .corner_radius(CornerRadius::ZERO),
                                    )
                                    .clicked()
                                {
                                    self.rebinding_cmd = Some(cmd.to_string());
                                }
                                ui.end_row();
                            }
                        }
                    });
            });
        });
        paint_vgui_bevel(ui.painter(), frame_resp.response.rect, false, palette);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let defaults_lbl = loc_or(o.loc, "#GameUI_UseDefaults", "Use Defaults");
            if rt
                .draw_tracker_button(
                    ui,
                    o.vfs,
                    palette,
                    &TrackerButtonCfg {
                        id: "opt_kb_defaults",
                        label: &defaults_lbl,
                        min_size: Vec2::new(108.0, 24.0),
                        default_btn: false,
                        enabled: true,
                    },
                )
                .clicked()
            {
                cvars.unbind_all();
                cs_cvar::defaults::register_default_binds(cvars);
                self.rebinding_cmd = None;
            }
            if let Some(ref target) = self.rebinding_cmd.clone()
                && rt
                    .draw_tracker_button(
                        ui,
                        o.vfs,
                        palette,
                        &TrackerButtonCfg {
                            id: "opt_kb_clear",
                            label: "Clear Key",
                            min_size: Vec2::new(84.0, 24.0),
                            default_btn: false,
                            enabled: true,
                        },
                    )
                    .clicked()
            {
                if let Some(old) = cvars.key_for_command(target).map(str::to_string) {
                    cvars.unbind(&old);
                }
                self.rebinding_cmd = None;
            }
        });
    }

    fn show_mouse(&mut self, ui: &mut egui::Ui, cvars: &mut CVarRegistry) {
        let mut sens = cvars.get_f32("sensitivity").clamp(0.1, 20.0);
        ui.horizontal(|ui| {
            ui.label("Mouse sensitivity:");
            if ui.add(egui::Slider::new(&mut sens, 0.1..=20.0).fixed_decimals(2)).changed() {
                cvars.set("sensitivity", format!("{sens:.2}"));
            }
        });

        let mut invert = cvars.get_f32("m_pitch") < 0.0;
        if ui.checkbox(&mut invert, "Reverse mouse (invert Y)").changed() {
            let mag = cvars.get_f32("m_pitch").abs().max(0.001);
            cvars.set("m_pitch", format!("{}", if invert { -mag } else { mag }));
        }

        cvar_checkbox(ui, cvars, "m_rawinput", "Raw mouse input (m_rawinput)");
        cvar_checkbox(ui, cvars, "m_filter", "Mouse filter (m_filter)");
        cvar_checkbox(ui, cvars, "m_customaccel", "Custom mouse acceleration (m_customaccel)");

        let mut accel_scale = cvars.get_f32("m_customaccel_scale").clamp(0.0, 0.5);
        ui.horizontal(|ui| {
            ui.label("Custom accel scale:");
            if ui.add(egui::Slider::new(&mut accel_scale, 0.0..=0.5).fixed_decimals(3)).changed() {
                cvars.set("m_customaccel_scale", format!("{accel_scale:.3}"));
            }
        });

        let mut zoom_ratio = cvars.get_f32("zoom_sensitivity_ratio").clamp(0.1, 4.0);
        ui.horizontal(|ui| {
            ui.label("Zoom sensitivity ratio:");
            if ui.add(egui::Slider::new(&mut zoom_ratio, 0.1..=4.0).fixed_decimals(2)).changed() {
                cvars.set("zoom_sensitivity_ratio", format!("{zoom_ratio:.2}"));
            }
        });
    }

    fn show_audio(&mut self, ui: &mut egui::Ui, cvars: &mut CVarRegistry) {
        let mut vol = cvars.get_f32("volume").clamp(0.0, 1.0);
        ui.horizontal(|ui| {
            ui.label("Sound effects volume:");
            if ui.add(egui::Slider::new(&mut vol, 0.0..=1.0).fixed_decimals(2)).changed() {
                cvars.set("volume", format!("{vol:.2}"));
            }
        });

        let mut mp3 = cvars.get_f32("mp3volume").clamp(0.0, 1.0);
        ui.horizontal(|ui| {
            ui.label("MP3 music volume:");
            if ui.add(egui::Slider::new(&mut mp3, 0.0..=1.0).fixed_decimals(2)).changed() {
                cvars.set("mp3volume", format!("{mp3:.2}"));
            }
        });

        let mut fade_t = cvars.get_f32("mp3fadetime").clamp(0.0, 10.0);
        ui.horizontal(|ui| {
            ui.label("MP3 fade time (s):");
            if ui.add(egui::Slider::new(&mut fade_t, 0.0..=10.0).fixed_decimals(1)).changed() {
                cvars.set("mp3fadetime", format!("{fade_t:.1}"));
            }
        });

        cvar_checkbox(ui, cvars, "bgmvolume", "Enable CD / background music (bgmvolume)");
        cvar_checkbox(ui, cvars, "room_off", "Disable room reverb DSP (room_off)");

        ui.separator();
        ui.label("Voice Communication");
        cvar_checkbox(ui, cvars, "voice_enable", "Enable voice chat (voice_enable)");
        cvar_checkbox(ui, cvars, "voice_loopback", "Voice mic loopback test (voice_loopback)");
        let mut vscale = cvars.get_f32("voice_scale").clamp(0.0, 2.0);
        ui.horizontal(|ui| {
            ui.label("Voice receive volume:");
            if ui.add(egui::Slider::new(&mut vscale, 0.0..=2.0).fixed_decimals(2)).changed() {
                cvars.set("voice_scale", format!("{vscale:.2}"));
            }
        });
    }

    fn show_video(&mut self, ui: &mut egui::Ui, cvars: &mut CVarRegistry) {
        let cur_aspect = cvars.get_str("vid_aspect_mode").to_string();
        ui.horizontal(|ui| {
            ui.label("Display Mode:");
            egui::ComboBox::from_id_salt("opt_aspect")
                .selected_text(&cur_aspect)
                .show_ui(ui, |ui| {
                    for (id, label) in [
                        ("widescreen", "Widescreen (16:9 / Native)"),
                        ("4:3_bars", "Normal 4:3 (Black Bars)"),
                        ("4:3_stretch", "Normal 4:3 (Stretched)"),
                    ] {
                        if ui.selectable_label(cur_aspect == id, label).clicked() {
                            cvars.set("vid_aspect_mode", id);
                        }
                    }
                });
        });

        let cur_filter = cvars.get_str("gl_texturemode").to_string();
        ui.horizontal(|ui| {
            ui.label("Texture filtering:");
            egui::ComboBox::from_id_salt("opt_texmode")
                .selected_text(&cur_filter)
                .show_ui(ui, |ui| {
                    for mode in ["GL_LINEAR_MIPMAP_LINEAR", "GL_NEAREST"] {
                        if ui.selectable_label(cur_filter == mode, mode).clicked() {
                            cvars.set("gl_texturemode", mode);
                        }
                    }
                });
        });

        let mut fps = cvars.get_i32("fps_max").clamp(0, 1000);
        ui.horizontal(|ui| {
            ui.label("Max FPS (0 = unlimited):");
            if ui.add(egui::Slider::new(&mut fps, 0..=500)).changed() {
                cvars.set("fps_max", fps.to_string());
            }
        });

        let mut gamma = cvars.get_f32("gamma").clamp(1.0, 4.0);
        ui.horizontal(|ui| {
            ui.label("Gamma:");
            if ui.add(egui::Slider::new(&mut gamma, 1.5..=3.5).fixed_decimals(2)).changed() {
                cvars.set("gamma", format!("{gamma:.2}"));
            }
        });

        let mut bright = cvars.get_f32("brightness").clamp(0.4, 2.0);
        ui.horizontal(|ui| {
            ui.label("Brightness:");
            if ui.add(egui::Slider::new(&mut bright, 0.5..=1.8).fixed_decimals(2)).changed() {
                cvars.set("brightness", format!("{bright:.2}"));
            }
        });

        let mut fov = cvars.get_f32("default_fov").clamp(60.0, 120.0);
        ui.horizontal(|ui| {
            ui.label("Field of view (default_fov):");
            if ui.add(egui::Slider::new(&mut fov, 60.0..=120.0).fixed_decimals(0)).changed() {
                cvars.set("default_fov", format!("{fov:.0}"));
            }
        });

        let mut ng = cvars.get_i32("net_graph");
        ui.horizontal(|ui| {
            ui.label("net_graph:");
            for (val, label) in [(0, "Off"), (1, "1 (Full)"), (3, "3 (Compact)")] {
                if ui.selectable_label(ng == val, label).clicked() {
                    ng = val;
                    cvars.set("net_graph", val.to_string());
                }
            }
        });

        let mut hd = cvars.get_bool("cl_hd_models");
        if ui
            .checkbox(&mut hd, "Enable HD models if available (cstrike_hd)")
            .changed()
        {
            cvars.set("cl_hd_models", if hd { "1" } else { "0" });
            self.hd_models_changed = true;
        }
    }

    fn show_voice(&mut self, ui: &mut egui::Ui, cvars: &mut CVarRegistry) {
        cvar_checkbox(ui, cvars, "voice_enable", "Enable voice in this mod (voice_enable)");
        cvar_checkbox(ui, cvars, "voice_modenable", "Mod voice support (voice_modenable)");
        cvar_checkbox(ui, cvars, "voice_forcemicrecord", "Force microphone init (voice_forcemicrecord)");
        cvar_checkbox(ui, cvars, "voice_loopback", "Test microphone loopback (voice_loopback)");
        let mut vscale = cvars.get_f32("voice_scale").clamp(0.0, 2.0);
        ui.horizontal(|ui| {
            ui.label("Voice receive volume:");
            if ui.add(egui::Slider::new(&mut vscale, 0.0..=2.0).fixed_decimals(2)).changed() {
                cvars.set("voice_scale", format!("{vscale:.2}"));
            }
        });
        let mut avg = cvars.get_f32("voice_avggain").clamp(0.0, 2.0);
        ui.horizontal(|ui| {
            ui.label("Microphone gain target (voice_avggain):");
            if ui.add(egui::Slider::new(&mut avg, 0.0..=2.0).fixed_decimals(2)).changed() {
                cvars.set("voice_avggain", format!("{avg:.2}"));
            }
        });
    }

    fn show_lock(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
    ) {
        let palette = o.palette;
        ui.label(
            RichText::new("Content Control (ContentControlDialog.res)")
                .color(palette.bright_control_text),
        );
        ui.add_space(4.0);
        cvar_checkbox(ui, cvars, "violence_hblood", "Enable human blood (violence_hblood)");
        cvar_checkbox(ui, cvars, "violence_ablood", "Enable alien blood (violence_ablood)");
        cvar_checkbox(ui, cvars, "violence_hgibs", "Enable human gibs (violence_hgibs)");
        cvar_checkbox(ui, cvars, "violence_agibs", "Enable alien gibs (violence_agibs)");
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Content lock password:");
            ui.add(egui::TextEdit::singleline(&mut self.lock_password).password(true));
        });
        let btn_lbl = if self.content_locked {
            "Disable Content Lock"
        } else {
            "Enable Content Lock"
        };
        if rt
            .draw_tracker_button(
                ui,
                o.vfs,
                palette,
                &TrackerButtonCfg {
                    id: "opt_lock_toggle",
                    label: btn_lbl,
                    min_size: Vec2::new(140.0, 24.0),
                    default_btn: false,
                    enabled: true,
                },
            )
            .clicked()
        {
            self.content_locked = !self.content_locked;
            if self.content_locked {
                cvars.set("violence_hblood", "0");
                cvars.set("violence_ablood", "0");
                cvars.set("violence_hgibs", "0");
                cvars.set("violence_agibs", "0");
            }
        }
    }
}

fn loc_or(loc: Option<&LocalizationTable>, token: &str, fallback: &str) -> String {
    if let Some(l) = loc {
        let res = l.resolve(token);
        if !res.is_empty() && !res.starts_with('#') {
            return res.to_string();
        }
    }
    fallback.to_string()
}

pub fn render_scr_options(
    ui: &mut egui::Ui,
    cvars: &mut CVarRegistry,
    scr: &SettingsScript,
    loc: Option<&LocalizationTable>,
) {
    for opt in &scr.options {
        let prompt = loc.map_or(opt.prompt.as_str(), |l| l.resolve(&opt.prompt));
        match &opt.kind {
            ScrOptionKind::Bool => cvar_checkbox(ui, cvars, &opt.cvar, prompt),
            ScrOptionKind::String => {
                let mut v = cvars.get_str(&opt.cvar).to_string();
                ui.horizontal(|ui| {
                    ui.label(prompt);
                    if ui.text_edit_singleline(&mut v).changed() {
                        cvars.set(&opt.cvar, v);
                    }
                });
            }
            ScrOptionKind::Number { min, max } => {
                let mut val = cvars.get_f32(&opt.cvar);
                let hi = if *max > *min { *max } else { (*min + 100.0).max(16000.0) };
                ui.horizontal(|ui| {
                    ui.label(prompt);
                    if ui.add(egui::DragValue::new(&mut val).range(*min..=hi)).changed() {
                        cvars.set(&opt.cvar, format!("{val}"));
                    }
                });
            }
            ScrOptionKind::List(items) => {
                let cur = cvars.get_str(&opt.cvar).to_string();
                let sel_lbl = items
                    .iter()
                    .find(|(_, v)| v == &cur)
                    .map(|(k, _)| loc.map_or(k.as_str(), |l| l.resolve(k)))
                    .unwrap_or(&cur);
                ui.horizontal(|ui| {
                    ui.label(prompt);
                    egui::ComboBox::from_id_salt(&opt.cvar)
                        .selected_text(sel_lbl)
                        .show_ui(ui, |ui| {
                            for (lbl, val) in items {
                                let txt = loc.map_or(lbl.as_str(), |l| l.resolve(lbl));
                                if ui.selectable_label(&cur == val, txt).clicked() {
                                    cvars.set(&opt.cvar, val.clone());
                                }
                            }
                        });
                });
            }
        }
    }
}

fn cvar_checkbox(ui: &mut egui::Ui, cvars: &mut CVarRegistry, name: &str, label: &str) {
    let mut val = cvars.get_bool(name);
    if ui.checkbox(&mut val, label).changed() {
        cvars.set(name, if val { "1" } else { "0" });
    }
}

fn egui_key_to_goldsrc(key: egui::Key) -> String {
    match key {
        egui::Key::Space => "space".into(),
        egui::Key::Tab => "tab".into(),
        egui::Key::Enter => "enter".into(),
        egui::Key::Backspace => "backspace".into(),
        egui::Key::Backtick => "`".into(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}
