//! Classic Counter-Strike 1.6 VGUI2 `Options` dialog bound live to `CVarRegistry`.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_scripts::{ScrOptionKind, SettingsScript};
use cs_cvar::CVarRegistry;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Pos2, RichText, ScrollArea, Stroke, TextureHandle};

use crate::ui::parse_rgb;
use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel, vgui_tab};

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
    pub lock_password: String,
    pub content_locked: bool,
    pub save_requested: bool,
    pub hd_models_changed: bool,
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
    pub fn load_user_script(&mut self, vfs: &Vfs) {
        self.user_script = SettingsScript::load(vfs, "user.scr");
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        cvars: &mut CVarRegistry,
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) {
        if !self.open {
            self.rebinding_cmd = None;
            return;
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
                            return Some(None); // cancel rebind
                        }
                        return Some(Some(egui_key_to_goldsrc(*key)));
                    }
                }
                None
            });
            if let Some(maybe_key) = captured {
                if let Some(new_key) = maybe_key {
                    // Remove old bind for this command
                    if let Some(old) = cvars.key_for_command(target_cmd).map(str::to_string) {
                        cvars.unbind(&old);
                    }
                    cvars.bind(&new_key, target_cmd);
                }
                self.rebinding_cmd = None;
            }
        }

        let mut is_open = self.open;
        let mut close_now = false;
        show_steam2003_window(
            ctx,
            "cs16_vgui_options",
            "Options",
            &mut is_open,
            egui::pos2(160.0, 90.0),
            egui::vec2(530.0, 425.0),
            egui::vec2(520.0, 410.0),
            false,
            palette,
            steam_icon,
            |ui| {
                // Classic VGUI2 top tab strip (`OptionsSub*.res` — 7 tabs)
                ui.horizontal(|ui| {
                    for (t, label) in [
                        (OptionsTab::Multiplayer, "Multiplayer"),
                        (OptionsTab::Keyboard, "Keyboard"),
                        (OptionsTab::Mouse, "Mouse"),
                        (OptionsTab::Audio, "Audio"),
                        (OptionsTab::Video, "Video"),
                        (OptionsTab::Voice, "Voice"),
                        (OptionsTab::Lock, "Lock"),
                    ] {
                        if vgui_tab(ui, label, self.tab == t, palette) {
                            self.tab = t;
                            self.rebinding_cmd = None;
                        }
                    }
                });

                let body_frame = egui::Frame::NONE
                    .fill(palette.control_bg)
                    .inner_margin(egui::Margin::same(10))
                    .stroke(Stroke::new(1.0, palette.border_dark));

                let body_resp = body_frame.show(ui, |ui| {
                    ui.set_min_size(egui::vec2(525.0, 320.0));
                    match self.tab {
                        OptionsTab::Multiplayer => self.show_multiplayer(ui, cvars, palette),
                        OptionsTab::Keyboard => self.show_keyboard(ui, cvars, palette),
                        OptionsTab::Mouse => self.show_mouse(ui, cvars),
                        OptionsTab::Audio => self.show_audio(ui, cvars),
                        OptionsTab::Video => self.show_video(ui, cvars),
                        OptionsTab::Voice => self.show_voice(ui, cvars),
                        OptionsTab::Lock => self.show_lock(ui, cvars, palette),
                    }
                });
                paint_vgui_bevel(ui.painter(), body_resp.response.rect, false, palette);

                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("Apply").min_size(egui::vec2(74.0, 24.0)).corner_radius(CornerRadius::ZERO))
                        .clicked()
                    {
                        self.save_requested = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(74.0, 24.0)).corner_radius(CornerRadius::ZERO))
                        .clicked()
                    {
                        close_now = true;
                    }
                    if ui
                        .add(egui::Button::new("OK").min_size(egui::vec2(74.0, 24.0)).corner_radius(CornerRadius::ZERO))
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
            show_steam2003_window(
                ctx,
                "cs16_vgui_options_advanced",
                "Multiplayer Advanced",
                &mut adv,
                Pos2::new(210.0, 120.0),
                egui::vec2(440.0, 340.0),
                egui::vec2(380.0, 260.0),
                false,
                palette,
                steam_icon,
                |ui| {
                    let loc_default = LocalizationTable::default();
                    let _ = &loc_default;
                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        if let Some(ref scr) = self.user_script {
                            render_scr_options(ui, cvars, scr, None);
                        } else {
                            cvar_checkbox(ui, cvars, "hud_centerid", "Center player ID names (hud_centerid)");
                            cvar_checkbox(ui, cvars, "hud_takesshots", "Auto-take scoreboard screenshots");
                            cvar_checkbox(ui, cvars, "cl_radartype", "Solid radar type (cl_radartype)");
                            cvar_checkbox(ui, cvars, "cl_observercrosshair", "Show crosshair in observer mode");
                        }
                    });
                },
            );
            if !adv {
                self.advanced_open = false;
            }
        }
    }

    fn show_multiplayer(&mut self, ui: &mut egui::Ui, cvars: &mut CVarRegistry, palette: VguiPalette) {
        let mut name = cvars.get_str("name").to_string();
        ui.horizontal(|ui| {
            ui.label("Player name:");
            if ui.text_edit_singleline(&mut name).changed() {
                cvars.set("name", name);
            }
        });

        ui.separator();
        ui.label(RichText::new("Crosshair appearance").color(palette.bright_control_text));

        ui.horizontal(|ui| {
            // Iconic CS 1.6 live Crosshair preview square (`OptionsSubMultiplayer.res`)
            let (preview_rect, _) =
                ui.allocate_exact_size(egui::vec2(86.0, 86.0), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(preview_rect, CornerRadius::ZERO, Color32::from_rgb(38, 42, 34));
            paint_vgui_bevel(p, preview_rect, false, palette);
            let (cr, cg, cb) = parse_rgb(cvars.get_str("cl_crosshair_color"));
            let ca = if cvars.get_bool("cl_crosshair_translucent") { 195 } else { 255 };
            let ch_col = Color32::from_rgba_unmultiplied(cr, cg, cb, ca);
            let (len, gap) = match cvars.get_str("cl_crosshair_size").to_ascii_lowercase().as_str() {
                "small" => (6.0f32, 4.0f32),
                "large" => (12.0f32, 6.0f32),
                _ => (9.0f32, 5.0f32),
            };
            let c = preview_rect.center();
            let s = Stroke::new(1.5, ch_col);
            p.line_segment([Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)], s);
            p.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], s);
            p.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)], s);
            p.line_segment([Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)], s);

            ui.add_space(8.0);
            ui.vertical(|ui| {
                let cur_size = cvars.get_str("cl_crosshair_size").to_string();
                ui.horizontal(|ui| {
                    ui.label("Size:");
                    egui::ComboBox::from_id_salt("opt_ch_size")
                        .selected_text(&cur_size)
                        .show_ui(ui, |ui| {
                            for s in ["auto", "small", "medium", "large"] {
                                if ui.selectable_label(cur_size == s, s).clicked() {
                                    cvars.set("cl_crosshair_size", s);
                                }
                            }
                        });
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
                ui.horizontal(|ui| {
                    ui.label("Color:");
                    egui::ComboBox::from_id_salt("opt_ch_color")
                        .selected_text(preset_name)
                        .show_ui(ui, |ui| {
                            for (label, rgb) in presets {
                                if ui.selectable_label(cur_color == rgb, label).clicked() {
                                    cvars.set("cl_crosshair_color", rgb);
                                }
                            }
                        });
                });

                cvar_checkbox(ui, cvars, "cl_crosshair_translucent", "Translucent");
                cvar_checkbox(ui, cvars, "cl_dynamiccrosshair", "Dynamic crosshair");
            });
        });

        ui.separator();
        cvar_checkbox(ui, cvars, "cl_righthand", "Right-handed weapon models (cl_righthand)");
        cvar_checkbox(ui, cvars, "hud_fastswitch", "Fast weapon switch (hud_fastswitch)");
        ui.add_space(6.0);
        if ui
            .add(egui::Button::new("Advanced...").corner_radius(CornerRadius::ZERO))
            .clicked()
        {
            self.advanced_open = true;
        }
    }

    fn show_keyboard(&mut self, ui: &mut egui::Ui, cvars: &mut CVarRegistry, palette: VguiPalette) {
        if let Some(ref cmd) = self.rebinding_cmd {
            ui.label(
                RichText::new(format!("Press a key to bind to \"{cmd}\" (Esc to cancel)..."))
                    .color(palette.bright_control_text),
            );
        } else {
            ui.label(RichText::new("Click a key column to rebind:").color(palette.dim_text));
        }

        let list_frame = egui::Frame::NONE
            .fill(palette.window_bg)
            .inner_margin(egui::Margin::same(6))
            .stroke(Stroke::new(1.0, palette.border_dark));
        list_frame.show(ui, |ui| {
            ScrollArea::vertical().max_height(235.0).show(ui, |ui| {
                egui::Grid::new("kb_binds_grid")
                    .num_columns(3)
                    .min_col_width(140.0)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Action").color(palette.bright_control_text));
                        ui.label(RichText::new("Command").color(palette.bright_control_text));
                        ui.label(RichText::new("Key").color(palette.bright_control_text));
                        ui.end_row();

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
                                .add(egui::Button::new(btn_label).min_size(egui::vec2(100.0, 20.0)).corner_radius(CornerRadius::ZERO))
                                .clicked()
                            {
                                self.rebinding_cmd = Some(cmd.to_string());
                            }
                            ui.end_row();
                        }
                    });
            });
        });

        ui.add_space(6.0);
        if ui
            .add(egui::Button::new("Use Defaults").corner_radius(CornerRadius::ZERO))
            .clicked()
        {
            cvars.unbind_all();
            cs_cvar::defaults::register_default_binds(cvars);
            self.rebinding_cmd = None;
        }
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

        let mut fov = cvars.get_f32("fov_default").clamp(70.0, 110.0);
        ui.horizontal(|ui| {
            ui.label("Field of view (fov_default):");
            if ui.add(egui::Slider::new(&mut fov, 70.0..=110.0).fixed_decimals(0)).changed() {
                cvars.set("fov_default", format!("{fov:.0}"));
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

    fn show_lock(&mut self, ui: &mut egui::Ui, cvars: &mut CVarRegistry, palette: VguiPalette) {
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
        let btn_lbl = if self.content_locked { "Disable Content Lock" } else { "Enable Content Lock" };
        if ui.button(btn_lbl).clicked() {
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
