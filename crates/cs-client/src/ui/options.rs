//! Classic Counter-Strike 1.6 VGUI2 `Options` dialog (`OptionsSub*.res` + `MultiplayerAdvancedDialog.res`
//! + `ContentControlDialog.res` + `gfx/shell/kb_act.lst` + `logos/*.bmp`) bound live to `CVarRegistry`.
//!
//! Layout rules measured against the original Steam CS 1.6 client (build 10210, 2026-11-24) at
//! 800x600, 1280x720, 1280x800, and 1280x1024:
//! - Proportional scale `s = max(1.0, screen_h / 720.0)` (`TrackerScheme.res` `ProportionalBaseHeightHD`).
//! - Outer `OptionsDialog` is centered on screen at `(522 * s) x (444 * s)`:
//!   - 800x600 (`s = 1.000`): `522x444` at `(139, 78)`
//!   - 1280x720 (`s = 1.000`): `522x444` at `(379, 138)`
//!   - 1280x800 (`s = 1.111`): `580x493` at `(350, 153)`
//!   - 1280x1024 (`s = 1.422`): `742x631` at `(269, 196)`
//! - `PropertySheet` spans `(5, 55, 512, 352) * s` with tabs in order
//!   `Multiplayer | Audio | Video | Aim | Keyboard | Voice | Content`; active tab sits at `y = 29..55 * s`
//!   and merges into the sheet top border, inactive tabs sit at `y = 31..55 * s`, `tab_w = max(64*s, text_w + 16*s)`.
//! - Subpages (`OptionsSub*.res`) resolve all child controls relative to `(dlg_x + 5*s, dlg_y + 55*s)` scaled by `s`.
//! - Bottom buttons (`OK`, `Cancel`, `Apply`) sit at `(277, 411, 72, 24)*s`, `(357, 411, 72, 24)*s`, `(437, 411, 72, 24)*s`.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::{ControlKind, TextAlignment, VguiResDocument};
use cs_assets::vgui_scripts::{ScrOptionKind, SettingsScript};
use cs_cvar::CVarRegistry;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Pos2, Rect, ScrollArea, Sense, Stroke, TextureHandle, Vec2};

use crate::ui::parse_rgb;
use crate::ui::vgui2::{
    TrackerCtx, VguiRuntime, draw_tracker_button_rect, draw_tracker_check_button_rect,
    draw_tracker_close_button, draw_tracker_combo_box_rect, draw_tracker_list_panel_rect,
    draw_tracker_slider_rect, draw_tracker_text_entry_rect, paint_tracker_bevel_rect,
    paint_tracker_label_rect,
};
use crate::ui::vgui_theme::VguiPalette;

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

const VOLUME_SLIDER_MIN: f32 = 0.0;
const VOLUME_SLIDER_MAX: f32 = 2.0;
const GAMMA_SLIDER_MIN: f32 = 1.0;
const GAMMA_SLIDER_MAX: f32 = 3.0;
const LOW_LATENCY_CVAR: &str = "gl_lowlatency";

fn ensure_lowlatency_cvar(cvars: &mut CVarRegistry) {
    if cvars.get(LOW_LATENCY_CVAR).is_none() {
        cvars.register(
            LOW_LATENCY_CVAR,
            "1",
            cs_cvar::CVarFlags::ARCHIVE,
            "Lower input latency (0 or 1)",
        );
    }
}

const SPRAY_COLOR_PRESETS: &[(&str, Color32)] = &[
    ("Orange", Color32::from_rgb(255, 120, 24)),
    ("Yellow", Color32::from_rgb(245, 225, 48)),
    ("Blue", Color32::from_rgb(64, 150, 255)),
    ("Ltblue", Color32::from_rgb(100, 225, 255)),
    ("Green", Color32::from_rgb(64, 225, 96)),
    ("Red", Color32::from_rgb(235, 64, 56)),
    ("White", Color32::from_rgb(240, 240, 240)),
];

const CROSSHAIR_COLOR_PRESETS: &[(&str, &str)] = &[
    ("Green", "50 250 50"),
    ("Red", "250 50 50"),
    ("Blue", "50 50 250"),
    ("Yellow", "250 250 50"),
    ("Ltblue", "50 250 250"),
];

const RESOLUTIONS: &[&str] = &[
    "640 x 480",
    "800 x 600",
    "1024 x 768",
    "1280 x 720",
    "1280 x 800",
    "1280 x 1024",
    "1920 x 1080",
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

impl OptionsTab {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "multiplayer" => Some(Self::Multiplayer),
            "audio" => Some(Self::Audio),
            "video" => Some(Self::Video),
            "mouse" | "aim" => Some(Self::Mouse),
            "keyboard" => Some(Self::Keyboard),
            "voice" => Some(Self::Voice),
            "content" | "lock" => Some(Self::Lock),
            _ => None,
        }
    }
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
    drag_offset: Vec2,
    adv_drag_offset: Vec2,
    content_dialog_open: bool,
    content_pw2: String,
    selected_bind_row: Option<usize>,
    sens_text_buf: String,
    sens_last_val: f32,
    auto_aim: bool,
    windowed: bool,
    widescreen_fov: bool,
    vsync: bool,
    use_shaders: bool,
    renderer_idx: usize,
    sound_quality_idx: usize,
    mic_boost: bool,
    addons_folder: bool,
    video_inited: bool,
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
        if !self.video_inited {
            self.video_inited = true;
            self.windowed = true;
            self.widescreen_fov = true;
            self.vsync = true;
            self.use_shaders = true;
            self.auto_aim = true;
            self.mic_boost = true;
        }

        if let Some(v) = o.vfs {
            for res in [
                "resource/OptionsSubMultiplayer.res",
                "resource/OptionsSubKeyboard.res",
                "resource/OptionsSubMouse.res",
                "resource/OptionsSubAudio.res",
                "resource/OptionsSubVideo.res",
                "resource/OptionsSubVoice.res",
                "resource/OptionsSubAdvanced.res",
                "resource/ContentControlDialog.res",
                "resource/MultiplayerAdvancedDialog.res",
            ] {
                runtime.load_res(v, res);
            }
        }

        self.handle_rebind_input(ctx, cvars);

        let screen = ctx.content_rect();
        let s = (screen.height() / 720.0).max(1.0);
        let screen_h = screen.height().round().max(480.0) as u32;
        let tc = TrackerCtx {
            ctx,
            vfs: o.vfs,
            palette: o.palette,
            screen_h,
            scale: s,
        };

        let dlg_w = (522.0 * s).round();
        let dlg_h = (444.0 * s).round();
        let default_x = screen.left() + ((screen.width() - dlg_w) * 0.5).floor();
        let default_y = screen.top() + ((screen.height() - dlg_h) * 0.5).floor();
        let max_x = (screen.right() - dlg_w).max(screen.left());
        let max_y = (screen.bottom() - dlg_h).max(screen.top());
        let dlg_min = Pos2::new(
            (default_x + self.drag_offset.x).clamp(screen.left(), max_x).round(),
            (default_y + self.drag_offset.y).clamp(screen.top(), max_y).round(),
        );
        let dlg_rect = Rect::from_min_size(dlg_min, Vec2::new(dlg_w, dlg_h));

        let mut close_now = false;
        let title = loc_or(o.loc, "#GameUI_Options", "Options");

        egui::Area::new(egui::Id::unique("cs16_vgui_options"))
            .order(egui::Order::Foreground)
            .fixed_pos(dlg_rect.min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(dlg_rect.size(), Sense::hover());
                let p = ui.painter();
                p.rect_filled(dlg_rect, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(p, dlg_rect, true, tc.palette);

                // Titlebar (`0..29*s`): `icon_steam.tga`, `DefaultBold` title, `[X]` close button.
                let title_drag = Rect::from_min_size(
                    dlg_min,
                    Vec2::new((dlg_w - 28.0 * s).max(40.0), (29.0 * s).round()),
                );
                let drag_resp = ui.interact(
                    title_drag,
                    ui.make_persistent_id("opt_title_drag"),
                    Sense::drag(),
                );
                if drag_resp.dragged() {
                    self.drag_offset += drag_resp.drag_delta();
                }
                if let Some(icon) = o.steam_icon {
                    let isz = (16.0 * s).round();
                    let ir = Rect::from_min_size(
                        dlg_min + Vec2::new((7.0 * s).round(), (8.0 * s).round()),
                        Vec2::splat(isz),
                    );
                    ui.painter().image(
                        icon.id(),
                        ir,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                let title_rect = Rect::from_min_size(
                    dlg_min + Vec2::new((28.0 * s).round(), (4.0 * s).round()),
                    Vec2::new((240.0 * s).round(), (22.0 * s).round()),
                );
                paint_tracker_label_rect(
                    ui.painter(),
                    runtime,
                    &tc,
                    title_rect,
                    &title,
                    "DefaultBold",
                    TextAlignment::West,
                    tc.palette.bright_text,
                    false,
                    true,
                );

                // Close `[X]` button: measured `(497, 8, 18, 18) * s`
                let close_rect = Rect::from_min_size(
                    dlg_min + Vec2::new((497.0 * s).round(), (8.0 * s).round()),
                    Vec2::splat((18.0 * s).round()),
                );
                if draw_tracker_close_button(ui, runtime, &tc, "opt_close", close_rect) {
                    close_now = true;
                }

                // `PropertySheet`: `(5, 55, 512, 352) * s`
                let sheet_rect = Rect::from_min_size(
                    dlg_min + Vec2::new((5.0 * s).round(), (55.0 * s).round()),
                    Vec2::new((512.0 * s).round(), (352.0 * s).round()),
                );
                ui.painter()
                    .rect_filled(sheet_rect, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(ui.painter(), sheet_rect, true, tc.palette);

                // Exact tab strip order measured on original CS 1.6:
                // Multiplayer | Audio | Video | Aim | Keyboard | Voice | Content
                let tab_specs = [
                    (OptionsTab::Multiplayer, loc_or(o.loc, "#GameUI_Multiplayer", "Multiplayer")),
                    (OptionsTab::Audio, loc_or(o.loc, "#GameUI_Audio", "Audio")),
                    (OptionsTab::Video, loc_or(o.loc, "#GameUI_Video", "Video")),
                    (OptionsTab::Mouse, loc_or(o.loc, "#GameUI_Mouse", "Aim")),
                    (OptionsTab::Keyboard, loc_or(o.loc, "#GameUI_Keyboard", "Keyboard")),
                    (OptionsTab::Voice, loc_or(o.loc, "#GameUI_Voice", "Voice")),
                    (OptionsTab::Lock, loc_or(o.loc, "#GameUI_Content", "Content")),
                ];
                let mut tx = dlg_min.x + (5.0 * s).round();
                let fonts = runtime.tracker_scheme.fonts.clone();
                let tab_pad_w = (16.0 * s).round();
                let min_tab_w = (64.0 * s).round();
                let tab_gap = (1.0 * s).round().max(1.0);
                for (tab_val, label) in &tab_specs {
                    let tw = runtime
                        .font_atlas
                        .measure_text_with_scale(ctx, o.vfs, &fonts, "tracker", "Default", screen_h, label, s)
                        .x;
                    let tab_w = (tw + tab_pad_w).round().max(min_tab_w);
                    let active = self.tab == *tab_val;
                    let top_y = dlg_min.y + if active { (29.0 * s).round() } else { (31.0 * s).round() };
                    let tab_rect = Rect::from_min_max(
                        Pos2::new(tx, top_y),
                        Pos2::new(tx + tab_w, sheet_rect.top()),
                    );
                    let tid = ui.make_persistent_id(("opt_tab", label));
                    let tresp = ui.interact(tab_rect, tid, Sense::click());
                    runtime.track_interactive(&format!("opt_tab_{label}"), &tresp);

                    let tp = ui.painter();
                    tp.rect_filled(tab_rect, CornerRadius::ZERO, tc.palette.control_bg);
                    tp.rect_filled(
                        Rect::from_min_max(tab_rect.min, Pos2::new(tab_rect.right(), tab_rect.top() + 1.0)),
                        0.0,
                        tc.palette.border_bright,
                    );
                    tp.rect_filled(
                        Rect::from_min_max(tab_rect.min, Pos2::new(tab_rect.left() + 1.0, tab_rect.bottom())),
                        0.0,
                        tc.palette.border_bright,
                    );
                    tp.rect_filled(
                        Rect::from_min_max(Pos2::new(tab_rect.right() - 1.0, tab_rect.top()), tab_rect.max),
                        0.0,
                        tc.palette.border_dark,
                    );
                    if active {
                        // Erase the sheet's top bright bevel underneath the active tab so it merges seamlessly.
                        tp.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(tab_rect.left() + 1.0, sheet_rect.top()),
                                Pos2::new(tab_rect.right() - 1.0, sheet_rect.top() + 1.0),
                            ),
                            0.0,
                            tc.palette.control_bg,
                        );
                    }
                    let fg = if active {
                        tc.palette.bright_control_text
                    } else {
                        tc.palette.bright_text
                    };
                    let inset_x = (8.0 * s).round();
                    let lbl_rect = Rect::from_min_max(
                        Pos2::new(tab_rect.left() + inset_x, tab_rect.top() + (1.0 * s).round()),
                        Pos2::new(tab_rect.right() - (2.0 * s).round(), tab_rect.bottom()),
                    );
                    paint_tracker_label_rect(
                        tp,
                        runtime,
                        &tc,
                        lbl_rect,
                        label,
                        "Default",
                        TextAlignment::West,
                        fg,
                        false,
                        true,
                    );
                    if tresp.clicked() {
                        self.tab = *tab_val;
                        self.rebinding_cmd = None;
                    }
                    tx += tab_w + tab_gap;
                }

                // Active subpage resolved relative to `(dlg_min.x + 5*s, dlg_min.y + 55*s)`.
                let page_origin = sheet_rect.min;
                match self.tab {
                    OptionsTab::Multiplayer => {
                        self.show_multiplayer(ui, runtime, &tc, cvars, o, page_origin);
                    }
                    OptionsTab::Audio => self.show_audio(ui, runtime, &tc, cvars, o, page_origin),
                    OptionsTab::Video => self.show_video(ui, runtime, &tc, cvars, o, page_origin),
                    OptionsTab::Mouse => self.show_mouse(ui, runtime, &tc, cvars, o, page_origin),
                    OptionsTab::Keyboard => {
                        self.show_keyboard(ui, runtime, &tc, cvars, o, page_origin);
                    }
                    OptionsTab::Voice => self.show_voice(ui, runtime, &tc, cvars, o, page_origin),
                    OptionsTab::Lock => self.show_content(ui, runtime, &tc, cvars, o, page_origin),
                }

                // Bottom buttons: `OK` `(277,411,72,24)*s`, `Cancel` `(357,411,72,24)*s`, `Apply` `(437,411,72,24)*s`
                let ok_lbl = loc_or(o.loc, "#GameUI_OK", "OK");
                let cancel_lbl = loc_or(o.loc, "#GameUI_Cancel", "Cancel");
                let apply_lbl = loc_or(o.loc, "#GameUI_Apply", "Apply");
                let ok_rect = dlg_subrect(dlg_min, s, 277.0, 411.0, 72.0, 24.0);
                let cancel_rect = dlg_subrect(dlg_min, s, 357.0, 411.0, 72.0, 24.0);
                let apply_rect = dlg_subrect(dlg_min, s, 437.0, 411.0, 72.0, 24.0);
                if draw_tracker_button_rect(
                    ui, runtime, &tc, "opt_ok", ok_rect, &ok_lbl, TextAlignment::West, false, true,
                )
                .clicked()
                {
                    self.save_requested = true;
                    close_now = true;
                }
                if draw_tracker_button_rect(
                    ui, runtime, &tc, "opt_cancel", cancel_rect, &cancel_lbl, TextAlignment::West, false, true,
                )
                .clicked()
                {
                    close_now = true;
                }
                if draw_tracker_button_rect(
                    ui, runtime, &tc, "opt_apply", apply_rect, &apply_lbl, TextAlignment::West, false, true,
                )
                .clicked()
                {
                    self.save_requested = true;
                }
            });

        if close_now {
            self.open = false;
            self.advanced_open = false;
            self.content_dialog_open = false;
        }

        if self.advanced_open {
            self.show_multiplayer_advanced(ctx, runtime, &tc, cvars, o);
        }
        if self.content_dialog_open {
            self.show_content_control_dialog(ctx, runtime, &tc, cvars, o);
        }
    }

    fn handle_rebind_input(&mut self, ctx: &egui::Context, cvars: &mut CVarRegistry) {
        let Some(target_cmd) = self.rebinding_cmd.clone() else {
            return;
        };
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
                if let Some(old) = cvars.key_for_command(&target_cmd).map(str::to_string) {
                    cvars.unbind(&old);
                }
                cvars.bind(&new_key, &target_cmd);
            }
            self.rebinding_cmd = None;
        }
    }

    fn show_multiplayer(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
        origin: Pos2,
    ) {
        let doc = rt.doc("resource/OptionsSubMultiplayer.res").cloned();
        let rect_or = |field: &str, fx: f32, fy: f32, fw: f32, fh: f32| -> Rect {
            res_rect(doc.as_ref(), origin, tc.scale, field, fx, fy, fw, fh)
        };
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };

        // `NameLabel` (bright/associated with `NameEntry`, which has tabPosition 1)
        let (name_txt, _, wrap, align) = label_or("NameLabel", "#GameUI_PlayerName");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("NameLabel", 40.0, 28.0, 150.0, 24.0),
            &name_txt,
            "Default",
            align,
            tc.palette.bright_control_text,
            wrap,
            true,
        );

        let mut player_name = cvars.get_str("name").to_string();
        if draw_tracker_text_entry_rect(
            ui,
            rt,
            tc,
            "opt_mp_name",
            rect_or("NameEntry", 40.0, 52.0, 196.0, 24.0),
            &mut player_name,
            false,
            false,
            true,
        ) {
            cvars.set("name", player_name);
        }

        // `Label2`: `#GameUI_SpraypaintImage`
        let (spray_lbl, dull, wrap, align) = label_or("Label2", "#GameUI_SpraypaintImage");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label2", 40.0, 94.0, 200.0, 24.0),
            &spray_lbl,
            "Default",
            align,
            if dull { tc.palette.dim_text } else { tc.palette.base_text },
            wrap,
            true,
        );

        // `LogoImage` (`40, 120, 64, 64` + 1px `InsetBorder` outside/inside)
        let logo_rect = rect_or("LogoImage", 40.0, 120.0, 64.0, 64.0);
        ui.painter().rect_filled(logo_rect, CornerRadius::ZERO, Color32::BLACK);
        let tint = SPRAY_COLOR_PRESETS
            .get(self.spray_color_idx)
            .map_or(Color32::WHITE, |(_, c)| *c);
        if !self.selected_spray.is_empty()
            && let Some(tex) =
                rt.ensure_texture(tc.ctx, o.vfs, &format!("logos/{}", self.selected_spray))
        {
            ui.painter().image(
                tex.id(),
                logo_rect.shrink(1.0),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                tint,
            );
        }
        paint_tracker_bevel_rect(ui.painter(), logo_rect.expand(1.0), false, tc.palette);

        // `SpraypaintList`
        let spray_names: Vec<String> = if self.spray_logos.is_empty() {
            vec!["lambda".to_string()]
        } else {
            self.spray_logos
                .iter()
                .map(|s| s.strip_suffix(".bmp").or_else(|| s.strip_suffix(".BMP")).unwrap_or(s).to_string())
                .collect()
        };
        let mut spray_idx = self
            .spray_logos
            .iter()
            .position(|l| l == &self.selected_spray)
            .unwrap_or(0);
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_spray_list",
            rect_or("SpraypaintList", 112.0, 120.0, 124.0, 24.0),
            &spray_names,
            &mut spray_idx,
            true,
        ) && let Some(chosen) = self.spray_logos.get(spray_idx)
        {
            self.selected_spray = chosen.clone();
        }

        // `SpraypaintColor`
        let color_items: Vec<String> = SPRAY_COLOR_PRESETS
            .iter()
            .map(|(n, _)| (*n).to_string())
            .collect();
        draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_spray_color",
            rect_or("SpraypaintColor", 112.0, 160.0, 124.0, 24.0),
            &color_items,
            &mut self.spray_color_idx,
            true,
        );

        // `Label3`: `#GameUI_SpraypaintServerNote` (`wrap 1`, `dulltext 1`)
        let (note_lbl, _, _, align) = label_or("Label3", "#GameUI_SpraypaintServerNote");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label3", 40.0, 187.0, 230.0, 64.0),
            &note_lbl,
            "Default",
            align,
            tc.palette.dim_text,
            true,
            true,
        );

        // `Advanced` button (`Default = 1`)
        let (adv_lbl, _, _, adv_align) = label_or("Advanced", "#GameUI_AdvancedEllipsis");
        if draw_tracker_button_rect(
            ui,
            rt,
            tc,
            "opt_mp_advanced",
            rect_or("Advanced", 40.0, 260.0, 120.0, 24.0),
            &adv_lbl,
            adv_align,
            true,
            true,
        )
        .clicked()
        {
            self.advanced_open = true;
        }

        // Right side: Crosshair appearance (`CrosshairLabel`, `CrosshairImage`, Size, Color, Translucent)
        let (ch_lbl, _, wrap, align) = label_or("CrosshairLabel", "#GameUI_CrosshairDescription");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("CrosshairLabel", 281.0, 94.0, 181.0, 24.0),
            &ch_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );

        let ch_rect = rect_or("CrosshairImage", 281.0, 120.0, 64.0, 64.0);
        if let Some(bg) = rt.ensure_texture(tc.ctx, o.vfs, "gfx/vgui/crosshair.tga") {
            ui.painter().image(
                bg.id(),
                ch_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            ui.painter()
                .rect_filled(ch_rect, CornerRadius::ZERO, Color32::from_rgb(72, 68, 56));
        }
        paint_tracker_bevel_rect(ui.painter(), ch_rect.expand(1.0), false, tc.palette);
        paint_crosshair_preview(ui.painter(), ch_rect, cvars, tc.scale);

        let size_items = vec![
            loc_or(o.loc, "#GameUI_Auto", "Auto-size"),
            loc_or(o.loc, "#GameUI_Small", "Small"),
            loc_or(o.loc, "#GameUI_Medium", "Medium"),
            loc_or(o.loc, "#GameUI_Large", "Large"),
        ];
        let size_vals = ["auto", "small", "medium", "large"];
        let cur_sz = cvars.get_str("cl_crosshair_size").to_ascii_lowercase();
        let mut sz_idx = size_vals.iter().position(|&v| v == cur_sz).unwrap_or(0);
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_ch_size",
            rect_or("CrosshairSizeComboBox", 354.0, 120.0, 108.0, 24.0),
            &size_items,
            &mut sz_idx,
            true,
        ) {
            cvars.set("cl_crosshair_size", size_vals[sz_idx]);
        }

        let cur_ch_col = cvars.get_str("cl_crosshair_color").to_string();
        let ch_col_items: Vec<String> = CROSSHAIR_COLOR_PRESETS
            .iter()
            .map(|(n, _)| (*n).to_string())
            .collect();
        let mut ch_col_idx = CROSSHAIR_COLOR_PRESETS
            .iter()
            .position(|(_, v)| *v == cur_ch_col)
            .unwrap_or(0);
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_ch_color",
            rect_or("CrosshairColorComboBox", 354.0, 160.0, 108.0, 24.0),
            &ch_col_items,
            &mut ch_col_idx,
            true,
        ) {
            cvars.set("cl_crosshair_color", CROSSHAIR_COLOR_PRESETS[ch_col_idx].1);
        }

        let (tr_lbl, _, _, _) = label_or("CrosshairTranslucencyCheckbox", "#GameUI_Translucent");
        let mut translucent = cvars.get_bool("cl_crosshair_translucent");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_ch_translucent",
            rect_or("CrosshairTranslucencyCheckbox", 281.0, 188.0, 181.0, 24.0),
            &mut translucent,
            &tr_lbl,
            true,
        ) {
            cvars.set("cl_crosshair_translucent", if translucent { "1" } else { "0" });
        }
    }

    fn show_audio(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
        origin: Pos2,
    ) {
        let doc = rt.doc("resource/OptionsSubAudio.res").cloned();
        let rect_or = |field: &str, fx: f32, fy: f32, fw: f32, fh: f32| -> Rect {
            res_rect(doc.as_ref(), origin, tc.scale, field, fx, fy, fw, fh)
        };
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };

        let (sfx_lbl, _, wrap, align) = label_or("sfx label", "#GameUI_SoundEffectVolume");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("sfx label", 42.0, 14.0, 160.0, 24.0),
            &sfx_lbl,
            "Default",
            align,
            tc.palette.bright_control_text,
            wrap,
            true,
        );
        let mut vol = cvars.get_f32("volume").clamp(VOLUME_SLIDER_MIN, VOLUME_SLIDER_MAX);
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_audio_sfx",
            rect_or("SFX Slider", 40.0, 37.0, 420.0, 36.0),
            &mut vol,
            VOLUME_SLIDER_MIN,
            VOLUME_SLIDER_MAX,
            "",
            "",
        ) {
            cvars.set("volume", format!("{vol:.2}"));
        }

        let (mp3_lbl, _, wrap, align) = label_or("mp3 label", "#GameUI_MP3Volume");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("mp3 label", 40.0, 76.0, 140.0, 24.0),
            &mp3_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut mp3 = cvars.get_f32("mp3volume").clamp(0.0, 1.0);
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_audio_mp3",
            rect_or("MP3 Volume", 40.0, 98.0, 420.0, 36.0),
            &mut mp3,
            0.0,
            1.0,
            "",
            "",
        ) {
            cvars.set("mp3volume", format!("{mp3:.2}"));
        }

        let (sq_lbl, _, wrap, align) = label_or("Label1", "#GameUI_SoundQuality");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label1", 40.0, 200.0, 179.0, 24.0),
            &sq_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let sq_items = vec![
            loc_or(o.loc, "#GameUI_High", "High"),
            loc_or(o.loc, "#GameUI_Low", "Low"),
        ];
        draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_audio_quality",
            rect_or("Sound Quality", 40.0, 226.0, 180.0, 24.0),
            &sq_items,
            &mut self.sound_quality_idx,
            true,
        );

        let (miles_lbl, _, _, align) = label_or(
            "MilesAudioLabel",
            "* MPEG Layer-3 playback supplied with the Miles Sound System.",
        );
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("MilesAudioLabel", 6.0, 270.0, 480.0, 72.0),
            &miles_lbl,
            "Default",
            align,
            tc.palette.dim_text,
            true,
            true,
        );
    }

    fn show_video(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
        origin: Pos2,
    ) {
        let doc = rt.doc("resource/OptionsSubVideo.res").cloned();
        let rect_or = |field: &str, fx: f32, fy: f32, fw: f32, fh: f32| -> Rect {
            res_rect(doc.as_ref(), origin, tc.scale, field, fx, fy, fw, fh)
        };
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };

        let (r_lbl, _, wrap, align) = label_or("Label2", "#GameUI_Renderer");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label2", 40.0, 16.0, 80.0, 24.0),
            &r_lbl,
            "Default",
            align,
            tc.palette.bright_control_text,
            wrap,
            true,
        );
        let renderers = vec![
            loc_or(o.loc, "#GameUI_OpenGL", "OpenGL"),
            loc_or(o.loc, "#GameUI_Software", "Software"),
        ];
        draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_vid_renderer",
            rect_or("Renderer", 140.0, 16.0, 180.0, 24.0),
            &renderers,
            &mut self.renderer_idx,
            true,
        );

        let (res_lbl, _, wrap, align) = label_or("Label1", "#GameUI_Resolution");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label1", 40.0, 50.0, 80.0, 24.0),
            &res_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let cur_sz = format!(
            "{} x {}",
            tc.ctx.content_rect().width().round() as u32,
            tc.ctx.content_rect().height().round() as u32
        );
        let mut res_items: Vec<String> = RESOLUTIONS.iter().map(|s| (*s).to_string()).collect();
        if !res_items.contains(&cur_sz) {
            res_items.push(cur_sz.clone());
        }
        let mut res_idx = res_items.iter().position(|r| r == &cur_sz).unwrap_or(4);
        draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_vid_res",
            rect_or("Resolution", 140.0, 50.0, 180.0, 24.0),
            &res_items,
            &mut res_idx,
            true,
        );

        let (dm_lbl, _, wrap, align) = label_or("Label4", "#GameUI_DisplayMode");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label4", 40.0, 84.0, 80.0, 24.0),
            &dm_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let aspect_items = vec![
            loc_or(o.loc, "#GameUI_AspectNormal", "Normal"),
            loc_or(o.loc, "#GameUI_AspectWide", "Widescreen"),
            "Normal (4:3 Bars)".to_string(),
        ];
        let aspect_vals = ["4:3_stretch", "widescreen", "4:3_bars"];
        let cur_aspect = cvars.get_str("vid_aspect_mode").to_string();
        let mut aspect_idx = aspect_vals.iter().position(|&v| v == cur_aspect).unwrap_or(1);
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_vid_aspect",
            rect_or("AspectRatio", 140.0, 84.0, 180.0, 24.0),
            &aspect_items,
            &mut aspect_idx,
            true,
        ) {
            cvars.set("vid_aspect_mode", aspect_vals[aspect_idx]);
        }

        let (win_lbl, _, _, _) = label_or("Windowed", "#GameUI_Windowed");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_win",
            rect_or("Windowed", 140.0, 116.0, 200.0, 24.0),
            &mut self.windowed,
            &win_lbl,
            true,
        );
        let (wfov_lbl, _, _, _) = label_or("WidescreenFOV", "#GameUI_WidescreenFOV");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_wfov",
            rect_or("WidescreenFOV", 140.0, 140.0, 280.0, 24.0),
            &mut self.widescreen_fov,
            &wfov_lbl,
            true,
        );
        let (vs_lbl, _, _, _) = label_or("VSync", "#GameUI_VSync");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_vsync",
            rect_or("VSync", 140.0, 164.0, 280.0, 24.0),
            &mut self.vsync,
            &vs_lbl,
            true,
        );
        let (tf_lbl, _, _, _) = label_or("TextureFiltering", "#GameUI_TextureFiltering");
        let mut tex_filter = !cvars.get_str("gl_texturemode").eq_ignore_ascii_case("GL_NEAREST");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_texfilter",
            rect_or("TextureFiltering", 140.0, 188.0, 280.0, 24.0),
            &mut tex_filter,
            &tf_lbl,
            true,
        ) {
            cvars.set(
                "gl_texturemode",
                if tex_filter { "GL_LINEAR_MIPMAP_LINEAR" } else { "GL_NEAREST" },
            );
        }
        let (sh_lbl, _, _, _) = label_or("UseShaders", "#GameUI_UseShaders");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_shaders",
            rect_or("UseShaders", 140.0, 212.0, 280.0, 24.0),
            &mut self.use_shaders,
            &sh_lbl,
            true,
        );

        let (br_lbl, _, wrap, align) = label_or("brightness label", "#GameUI_Brightness");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("brightness label", 40.0, 242.0, 80.0, 24.0),
            &br_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut bright = cvars.get_f32("brightness").clamp(0.0, 2.0);
        let (dark_s, light_s) = slider_edge_labels(doc.as_ref(), o.loc, "Brightness", "Dark", "Light");
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_vid_bright",
            rect_or("Brightness", 140.0, 242.0, 300.0, 36.0),
            &mut bright,
            0.0,
            2.0,
            &dark_s,
            &light_s,
        ) {
            cvars.set("brightness", format!("{bright:.2}"));
        }

        let (gm_lbl, _, wrap, align) = label_or("Gamma label", "#GameUI_Gamma");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Gamma label", 40.0, 283.0, 80.0, 24.0),
            &gm_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut gamma = cvars.get_f32("gamma").clamp(GAMMA_SLIDER_MIN, GAMMA_SLIDER_MAX);
        let (low_s, high_s) = slider_edge_labels(doc.as_ref(), o.loc, "Gamma", "Low", "High");
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_vid_gamma",
            rect_or("Gamma", 140.0, 283.0, 300.0, 36.0),
            &mut gamma,
            GAMMA_SLIDER_MIN,
            GAMMA_SLIDER_MAX,
            &low_s,
            &high_s,
        ) {
            cvars.set("gamma", format!("{gamma:.2}"));
        }

        let (note_lbl, _, _, align) = label_or("Label5", "#GameUI_VideoRestart");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label5", 40.0, 315.0, 450.0, 48.0),
            &note_lbl,
            "Default",
            align,
            tc.palette.dim_text,
            true,
            true,
        );
    }

    fn show_mouse(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
        origin: Pos2,
    ) {
        let doc = rt.doc("resource/OptionsSubMouse.res").cloned();
        let rect_or = |field: &str, fx: f32, fy: f32, fw: f32, fh: f32| -> Rect {
            res_rect(doc.as_ref(), origin, tc.scale, field, fx, fy, fw, fh)
        };
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };

        let mut invert = cvars.get_f32("m_pitch") < 0.0;
        let (rev_lbl, _, _, _) = label_or("ReverseMouse", "#GameUI_ReverseMouse");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_m_rev",
            rect_or("ReverseMouse", 30.0, 32.0, 160.0, 28.0),
            &mut invert,
            &rev_lbl,
            true,
        ) {
            let mag = cvars.get_f32("m_pitch").abs().max(0.001);
            cvars.set("m_pitch", format!("{}", if invert { -mag } else { mag }));
        }
        let (rev_desc, _, _, align) = label_or("Reverse Mouse label", "#GameUI_ReverseMouseLabel");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Reverse Mouse label", 208.0, 32.0, 300.0, 24.0),
            &rev_desc,
            "Default",
            align,
            tc.palette.dim_text,
            false,
            true,
        );

        let mut mfilter = cvars.get_bool("m_filter");
        let (mf_lbl, _, _, _) = label_or("MouseFilter", "#GameUI_MouseFilter");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_m_filter",
            rect_or("MouseFilter", 30.0, 56.0, 160.0, 28.0),
            &mut mfilter,
            &mf_lbl,
            true,
        ) {
            cvars.set("m_filter", if mfilter { "1" } else { "0" });
        }
        let (mf_desc, _, _, align) = label_or("MouseFilterLabel", "#GameUI_MouseFilterLabel");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("MouseFilterLabel", 208.0, 56.0, 300.0, 24.0),
            &mf_desc,
            "Default",
            align,
            tc.palette.dim_text,
            false,
            true,
        );

        let (aa_lbl, _, _, _) = label_or("Auto-Aim", "#GameUI_AutoAim");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_m_autoaim",
            rect_or("Auto-Aim", 30.0, 80.0, 160.0, 24.0),
            &mut self.auto_aim,
            &aa_lbl,
            true,
        );
        let (aa_desc, _, _, align) = label_or("AutoaimLabel", "#GameUI_AutoaimLabel");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("AutoaimLabel", 208.0, 80.0, 300.0, 24.0),
            &aa_desc,
            "Default",
            align,
            tc.palette.dim_text,
            false,
            true,
        );

        let mut raw = cvars.get_bool("m_rawinput");
        let (raw_lbl, _, _, _) = label_or("RawInput", "#GameUI_RawInput");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_m_raw",
            rect_or("RawInput", 30.0, 104.0, 160.0, 24.0),
            &mut raw,
            &raw_lbl,
            true,
        ) {
            cvars.set("m_rawinput", if raw { "1" } else { "0" });
        }
        let (raw_desc, _, _, align) = label_or("RawInputLabel", "#GameUI_RawInputLabel");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("RawInputLabel", 208.0, 104.0, 300.0, 24.0),
            &raw_desc,
            "Default",
            align,
            tc.palette.dim_text,
            false,
            true,
        );

        // `GameUI.dll:FUN_10034f90`: LowLatency checkbox binds to `gl_lowlatency`.
        ensure_lowlatency_cvar(cvars);
        let mut low_latency = cvars.get_bool(LOW_LATENCY_CVAR);
        let (ll_lbl, _, _, _) = label_or("LowLatency", "#GameUI_LowLatency");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_m_lowlat",
            rect_or("LowLatency", 30.0, 128.0, 280.0, 24.0),
            &mut low_latency,
            &ll_lbl,
            true,
        ) {
            cvars.set(LOW_LATENCY_CVAR, if low_latency { "1" } else { "0" });
        }
        let (ll_desc, _, _, align) = label_or("LowLatencyLabel", "#GameUI_LowLatencyLabel");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("LowLatencyLabel", 208.0, 128.0, 300.0, 24.0),
            &ll_desc,
            "Default",
            align,
            tc.palette.dim_text,
            false,
            true,
        );

        let (sens_lbl, _, wrap, align) = label_or("Label3", "#GameUI_MouseSensitivity");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label3", 34.0, 200.0, 164.0, 24.0),
            &sens_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut sens = cvars.get_f32("sensitivity").clamp(0.20, 20.00);
        if (sens - self.sens_last_val).abs() > 1e-4 || self.sens_text_buf.is_empty() {
            self.sens_last_val = sens;
            self.sens_text_buf = format!("{sens:.1}");
        }
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_m_sens",
            rect_or("Slider", 34.0, 222.0, 272.0, 40.0),
            &mut sens,
            0.20,
            20.00,
            "0.20",
            "20.00",
        ) {
            cvars.set("sensitivity", format!("{sens:.2}"));
            self.sens_last_val = sens;
            self.sens_text_buf = format!("{sens:.1}");
        }
        if draw_tracker_text_entry_rect(
            ui,
            rt,
            tc,
            "opt_m_sens_val",
            rect_or("SensitivityLabel", 320.0, 222.0, 48.0, 24.0),
            &mut self.sens_text_buf,
            false,
            true,
            true,
        ) && let Ok(v) = self.sens_text_buf.parse::<f32>()
        {
            let clamped = v.clamp(0.1, 20.0);
            cvars.set("sensitivity", format!("{clamped:.2}"));
            self.sens_last_val = clamped;
        }
    }

    fn show_keyboard(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
        origin: Pos2,
    ) {
        let doc = rt.doc("resource/OptionsSubKeyboard.res").cloned();
        let rect_or = |field: &str, fx: f32, fy: f32, fw: f32, fh: f32| -> Rect {
            res_rect(doc.as_ref(), origin, tc.scale, field, fx, fy, fw, fh)
        };
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };

        let mut row_cmds: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<String>> = Vec::new();
        if !self.kb_actions.is_empty() {
            for entry in &self.kb_actions {
                if entry.is_header {
                    continue;
                }
                let lbl = loc_or(o.loc, &entry.label, &entry.label);
                let bound = if self.rebinding_cmd.as_deref() == Some(entry.command.as_str()) {
                    "<press key>".to_string()
                } else {
                    cvars
                        .key_for_command(&entry.command)
                        .map(pretty_key_name)
                        .unwrap_or_default()
                };
                row_cmds.push(entry.command.clone());
                rows.push(vec![lbl, bound, String::new()]);
            }
        } else {
            for &(lbl, cmd) in STANDARD_BIND_ACTIONS {
                let bound = if self.rebinding_cmd.as_deref() == Some(cmd) {
                    "<press key>".to_string()
                } else {
                    cvars
                        .key_for_command(cmd)
                        .map(pretty_key_name)
                        .unwrap_or_default()
                };
                row_cmds.push(cmd.to_string());
                rows.push(vec![lbl.to_string(), bound, String::new()]);
            }
        }
        if self.selected_bind_row.is_none() && !rows.is_empty() {
            self.selected_bind_row = Some(0);
        }

        let col_action = loc_or(o.loc, "#GameUI_Action", "ACTION");
        let col_key = loc_or(o.loc, "#GameUI_KeyButton", "KEY/BUTTON");
        let col_alt = loc_or(o.loc, "#GameUI_Alternate", "ALTERNATE");
        if let Some(dbl) = draw_tracker_list_panel_rect(
            ui,
            rt,
            tc,
            "opt_kb_list",
            rect_or("listpanel_keybindlist", 12.0, 12.0, 480.0, 258.0),
            &[(&col_action, 224.0), (&col_key, 120.0), (&col_alt, 114.0)],
            &rows,
            &mut self.selected_bind_row,
        ) && let Some(cmd) = row_cmds.get(dbl)
        {
            self.rebinding_cmd = Some(cmd.clone());
        }

        let (def_lbl, _, _, def_align) = label_or("Defaults", "#GameUI_UseDefaults");
        if draw_tracker_button_rect(
            ui,
            rt,
            tc,
            "opt_kb_defaults",
            rect_or("Defaults", 12.0, 278.0, 105.0, 24.0),
            &def_lbl,
            def_align,
            false,
            true,
        )
        .clicked()
        {
            cvars.unbind_all();
            cs_cvar::defaults::register_default_binds(cvars);
            self.rebinding_cmd = None;
        }

        let (chg_lbl, _, _, chg_align) = label_or("ChangeKeyButton", "#GameUI_SetNewKey");
        if draw_tracker_button_rect(
            ui,
            rt,
            tc,
            "opt_kb_edit",
            rect_or("ChangeKeyButton", 310.0, 278.0, 84.0, 24.0),
            &chg_lbl,
            chg_align,
            true,
            true,
        )
        .clicked()
            && let Some(idx) = self.selected_bind_row
            && let Some(cmd) = row_cmds.get(idx)
        {
            self.rebinding_cmd = Some(cmd.clone());
        }

        let (clr_lbl, _, _, clr_align) = label_or("ClearKeyButton", "#GameUI_ClearKey");
        if draw_tracker_button_rect(
            ui,
            rt,
            tc,
            "opt_kb_clear",
            rect_or("ClearKeyButton", 408.0, 278.0, 84.0, 24.0),
            &clr_lbl,
            clr_align,
            false,
            true,
        )
        .clicked()
            && let Some(idx) = self.selected_bind_row
            && let Some(cmd) = row_cmds.get(idx)
            && let Some(old) = cvars.key_for_command(cmd).map(str::to_string)
        {
            cvars.unbind(&old);
            self.rebinding_cmd = None;
        }
    }

    fn show_voice(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
        origin: Pos2,
    ) {
        let doc = rt.doc("resource/OptionsSubVoice.res").cloned();
        let rect_or = |field: &str, fx: f32, fy: f32, fw: f32, fh: f32| -> Rect {
            res_rect(doc.as_ref(), origin, tc.scale, field, fx, fy, fw, fh)
        };
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };

        let mut venable = cvars.get_bool("voice_modenable");
        let (ve_lbl, _, _, _) = label_or("voice_modenable", "#GameUI_EnableVoice");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_v_modenable",
            rect_or("voice_modenable", 33.0, 32.0, 300.0, 28.0),
            &mut venable,
            &ve_lbl,
            true,
        ) {
            cvars.set("voice_modenable", if venable { "1" } else { "0" });
            cvars.set("voice_enable", if venable { "1" } else { "0" });
        }

        let (mb_lbl, _, _, _) = label_or("MicBoost", "#GameUI_BoostMicrophone");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_v_micboost",
            rect_or("MicBoost", 33.0, 66.0, 250.0, 28.0),
            &mut self.mic_boost,
            &mb_lbl,
            true,
        );

        let (tx_lbl, _, wrap, align) = label_or("Microphonelabel", "#GameUI_VoiceTransmitVolume");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Microphonelabel", 42.0, 102.0, 180.0, 24.0),
            &tx_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut avg = cvars.get_f32("voice_avggain").clamp(0.0, 1.0);
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_v_tx",
            rect_or("VoiceVolume", 40.0, 122.0, 420.0, 42.0),
            &mut avg,
            0.0,
            1.0,
            "",
            "",
        ) {
            cvars.set("voice_avggain", format!("{avg:.2}"));
        }

        let (rx_lbl, _, wrap, align) = label_or("ReceiveLabel", "#GameUI_VoiceReceiveVolume");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("ReceiveLabel", 40.0, 160.0, 180.0, 24.0),
            &rx_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut vscale = cvars.get_f32("voice_scale").clamp(0.0, 2.0);
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_v_rx",
            rect_or("VoiceReceive", 40.0, 180.0, 420.0, 42.0),
            &mut vscale,
            0.0,
            2.0,
            "",
            "",
        ) {
            cvars.set("voice_scale", format!("{vscale:.2}"));
        }

        let meter_rect = rect_or("MicMeter", 40.0, 226.0, 158.0, 32.0);
        let tex_path = if cvars.get_bool("voice_loopback") {
            "resource/mic_meter_live.tga"
        } else {
            "resource/mic_meter_dead.tga"
        };
        if let Some(tex) = rt.ensure_texture(tc.ctx, o.vfs, tex_path) {
            ui.painter().image(
                tex.id(),
                meter_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            ui.painter()
                .rect_filled(meter_rect, CornerRadius::ZERO, tc.palette.window_bg);
            paint_tracker_bevel_rect(ui.painter(), meter_rect, false, tc.palette);
        }

        let (tm_lbl, _, _, tm_align) = label_or("TestMicrophone", "#GameUI_TestMicrophone");
        if draw_tracker_button_rect(
            ui,
            rt,
            tc,
            "opt_v_test",
            rect_or("TestMicrophone", 40.0, 258.0, 160.0, 24.0),
            &tm_lbl,
            tm_align,
            false,
            true,
        )
        .clicked()
        {
            let next = !cvars.get_bool("voice_loopback");
            cvars.set("voice_loopback", if next { "1" } else { "0" });
        }

        let (mv_lbl, _, _, align) = label_or("MilesVoiceLabel", "* Uses Miles Sound System.");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("MilesVoiceLabel", 43.0, 296.0, 400.0, 48.0),
            &mv_lbl,
            "Default",
            align,
            tc.palette.dim_text,
            true,
            true,
        );
    }

    fn show_content(
        &mut self,
        ui: &mut egui::Ui,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
        origin: Pos2,
    ) {
        let doc = rt.doc("resource/OptionsSubAdvanced.res").cloned();
        let rect_or = |field: &str, fx: f32, fy: f32, fw: f32, fh: f32| -> Rect {
            res_rect(doc.as_ref(), origin, tc.scale, field, fx, fy, fw, fh)
        };
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };

        let mut orig_models = !cvars.get_bool("cl_hd_models");
        let (om_lbl, _, _, _) = label_or("OriginalModels", "#GameUI_OriginalModels");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_c_orig_models",
            rect_or("OriginalModels", 40.0, 40.0, 280.0, 24.0),
            &mut orig_models,
            &om_lbl,
            true,
        ) {
            cvars.set("cl_hd_models", if orig_models { "0" } else { "1" });
            self.hd_models_changed = true;
        }

        let (ad_lbl, _, _, _) = label_or("AddonsFolder", "#GameUI_AddonsFolder");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_c_addons",
            rect_or("AddonsFolder", 40.0, 64.0, 280.0, 24.0),
            &mut self.addons_folder,
            &ad_lbl,
            true,
        );

        let (cl_lbl, _, _, cl_align) = label_or("ContentLockButton", "#GameUI_ContentLock");
        if draw_tracker_button_rect(
            ui,
            rt,
            tc,
            "opt_c_lock_btn",
            rect_or("ContentLockButton", 40.0, 152.0, 120.0, 24.0),
            &cl_lbl,
            cl_align,
            false,
            true,
        )
        .clicked()
        {
            self.content_dialog_open = true;
        }

        let (cl_desc, _, _, align) = label_or("ContentlockLabel", "#GameUI_ContentLockLabel");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("ContentlockLabel", 172.0, 124.0, 320.0, 80.0),
            &cl_desc,
            "Default",
            align,
            tc.palette.dim_text,
            true,
            true,
        );

        let (note_lbl, _, _, align) = label_or("Label5", "#GameUI_ContentRestart");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            rect_or("Label5", 32.0, 306.0, 450.0, 60.0),
            &note_lbl,
            "Default",
            align,
            tc.palette.dim_text,
            true,
            true,
        );
    }

    fn show_multiplayer_advanced(
        &mut self,
        ctx: &egui::Context,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
    ) {
        let doc = rt.doc("resource/MultiplayerAdvancedDialog.res").cloned();
        let (rw, rh) = doc_frame_size(doc.as_ref(), 472.0, 376.0);
        let s = tc.scale;
        let screen = ctx.content_rect();
        let w = (rw * s).round();
        let h = (rh * s).round();
        let def_pos = Pos2::new(
            screen.left() + ((screen.width() - w) * 0.5).floor() + 24.0 * s,
            screen.top() + ((screen.height() - h) * 0.5).floor() + 24.0 * s,
        );
        let dlg_min = Pos2::new(
            (def_pos.x + self.adv_drag_offset.x).round(),
            (def_pos.y + self.adv_drag_offset.y).round(),
        );
        let dlg_rect = Rect::from_min_size(dlg_min, Vec2::new(w, h));
        let mut close_adv = false;
        let adv_title = loc_or(o.loc, "#GameUI_MultiplayerAdvanced", "Multiplayer Advanced");

        egui::Area::new(egui::Id::unique("cs16_vgui_options_advanced"))
            .order(egui::Order::Foreground)
            .fixed_pos(dlg_min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(dlg_rect.size(), Sense::hover());
                let p = ui.painter();
                p.rect_filled(dlg_rect, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(p, dlg_rect, true, tc.palette);
                let drag = ui.interact(
                    Rect::from_min_size(dlg_min, Vec2::new(w - 28.0 * s, 28.0 * s)),
                    ui.make_persistent_id("opt_adv_drag"),
                    Sense::drag(),
                );
                if drag.dragged() {
                    self.adv_drag_offset += drag.drag_delta();
                }
                paint_tracker_label_rect(
                    p,
                    rt,
                    tc,
                    Rect::from_min_size(dlg_min + Vec2::new(10.0 * s, 4.0 * s), Vec2::new(w - 40.0 * s, 22.0 * s)),
                    &adv_title,
                    "DefaultBold",
                    TextAlignment::West,
                    tc.palette.bright_text,
                    false,
                    true,
                );
                let close_r = dlg_subrect(dlg_min, s, rw - 24.0, 8.0, 18.0, 18.0);
                if draw_tracker_close_button(ui, rt, tc, "opt_adv_x", close_r) {
                    close_adv = true;
                }

                let list_r = res_rect(doc.as_ref(), dlg_min, s, "PanelListPanel", 16.0, 56.0, 442.0, 274.0);
                ui.painter().rect_filled(list_r, CornerRadius::ZERO, tc.palette.window_bg);
                paint_tracker_bevel_rect(ui.painter(), list_r, false, tc.palette);
                ui.scope_builder(egui::UiBuilder::new().max_rect(list_r.shrink(8.0)), |ui| {
                    ScrollArea::vertical().id_salt("opt_adv_scr").show(ui, |ui| {
                        if let Some(ref scr) = self.user_script {
                            render_scr_options(ui, cvars, scr, o.loc);
                        } else {
                            cvar_checkbox(ui, cvars, "cl_righthand", "Right-handed weapon models");
                            cvar_checkbox(ui, cvars, "hud_fastswitch", "Fast weapon switch");
                            cvar_checkbox(ui, cvars, "hud_centerid", "Center player ID names");
                            cvar_checkbox(ui, cvars, "cl_radartype", "Solid radar type");
                        }
                    });
                });

                let ok_lbl = loc_or(o.loc, "#GameUI_OK", "OK");
                let cancel_lbl = loc_or(o.loc, "#GameUI_Cancel", "Cancel");
                let ok_r = res_rect(doc.as_ref(), dlg_min, s, "OK", 308.0, 338.0, 72.0, 24.0);
                let cancel_r = res_rect(doc.as_ref(), dlg_min, s, "Cancel", 388.0, 338.0, 72.0, 24.0);
                if draw_tracker_button_rect(
                    ui, rt, tc, "opt_adv_ok", ok_r, &ok_lbl, TextAlignment::West, true, true,
                )
                .clicked()
                    || draw_tracker_button_rect(
                        ui, rt, tc, "opt_adv_cancel", cancel_r, &cancel_lbl, TextAlignment::West, false, true,
                    )
                    .clicked()
                {
                    close_adv = true;
                }
            });
        if close_adv {
            self.advanced_open = false;
        }
    }

    fn show_content_control_dialog(
        &mut self,
        ctx: &egui::Context,
        rt: &mut VguiRuntime,
        tc: &TrackerCtx<'_>,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
    ) {
        let doc = rt.doc("resource/ContentControlDialog.res").cloned();
        let (rw, rh) = doc_frame_size(doc.as_ref(), 328.0, 250.0);
        let s = tc.scale;
        let screen = ctx.content_rect();
        let w = (rw * s).round();
        let h = (rh * s).round();
        let dlg_min = Pos2::new(
            (screen.left() + (screen.width() - w) * 0.5).round(),
            (screen.top() + (screen.height() - h) * 0.5).round(),
        );
        let dlg_rect = Rect::from_min_size(dlg_min, Vec2::new(w, h));
        let mut close_cc = false;

        egui::Area::new(egui::Id::unique("cs16_vgui_content_control"))
            .order(egui::Order::Foreground)
            .fixed_pos(dlg_min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(dlg_rect.size(), Sense::hover());
                let p = ui.painter();
                p.rect_filled(dlg_rect, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(p, dlg_rect, true, tc.palette);
                let title = loc_or(o.loc, "#GameUI_ContentLock", "Content lock");
                paint_tracker_label_rect(
                    p,
                    rt,
                    tc,
                    Rect::from_min_size(dlg_min + Vec2::new(10.0 * s, 4.0 * s), Vec2::new(w - 40.0 * s, 22.0 * s)),
                    &title,
                    "DefaultBold",
                    TextAlignment::West,
                    tc.palette.bright_text,
                    false,
                    true,
                );
                let status_tok = if self.content_locked {
                    "#GameUI_ContentStatusEnabled"
                } else {
                    "#GameUI_ContentStatusDisabled"
                };
                let status_txt = loc_or(o.loc, status_tok, "Enter a password to enable the content lock.");
                paint_tracker_label_rect(
                    ui.painter(),
                    rt,
                    tc,
                    res_rect(doc.as_ref(), dlg_min, s, "ContentStatus", 30.0, 30.0, 280.0, 36.0),
                    &status_txt,
                    "Default",
                    TextAlignment::NorthWest,
                    tc.palette.base_text,
                    true,
                    true,
                );
                let (_, _, _, _) = res_label(doc.as_ref(), o.loc, "PasswordPrompt", "");
                let p1_lbl = loc_or(o.loc, "#GameUI_PasswordPrompt", "Enter a password");
                paint_tracker_label_rect(
                    ui.painter(),
                    rt,
                    tc,
                    res_rect(doc.as_ref(), dlg_min, s, "PasswordPrompt", 32.0, 68.0, 280.0, 28.0),
                    &p1_lbl,
                    "Default",
                    TextAlignment::West,
                    tc.palette.base_text,
                    false,
                    true,
                );
                draw_tracker_text_entry_rect(
                    ui,
                    rt,
                    tc,
                    "opt_cc_pw1",
                    res_rect(doc.as_ref(), dlg_min, s, "Password", 32.0, 98.0, 166.0, 24.0),
                    &mut self.lock_password,
                    true,
                    false,
                    true,
                );
                let p2_lbl = loc_or(o.loc, "#GameUI_PasswordReentryPrompt", "Re-enter password");
                paint_tracker_label_rect(
                    ui.painter(),
                    rt,
                    tc,
                    res_rect(doc.as_ref(), dlg_min, s, "PasswordReentryPrompt", 32.0, 124.0, 280.0, 28.0),
                    &p2_lbl,
                    "Default",
                    TextAlignment::West,
                    tc.palette.base_text,
                    false,
                    true,
                );
                draw_tracker_text_entry_rect(
                    ui,
                    rt,
                    tc,
                    "opt_cc_pw2",
                    res_rect(doc.as_ref(), dlg_min, s, "Password2", 32.0, 154.0, 166.0, 24.0),
                    &mut self.content_pw2,
                    true,
                    false,
                    true,
                );

                let ok_lbl = loc_or(o.loc, "#GameUI_OK", "OK");
                let cancel_lbl = loc_or(o.loc, "#GameUI_Cancel", "Cancel");
                if draw_tracker_button_rect(
                    ui,
                    rt,
                    tc,
                    "opt_cc_ok",
                    res_rect(doc.as_ref(), dlg_min, s, "Ok", 166.0, 216.0, 72.0, 24.0),
                    &ok_lbl,
                    TextAlignment::West,
                    true,
                    true,
                )
                .clicked()
                    && !self.lock_password.is_empty()
                    && self.lock_password == self.content_pw2
                {
                    self.content_locked = !self.content_locked;
                    let v = if self.content_locked { "0" } else { "1" };
                    cvars.set("violence_hblood", v);
                    cvars.set("violence_ablood", v);
                    cvars.set("violence_hgibs", v);
                    cvars.set("violence_agibs", v);
                    close_cc = true;
                }
                if draw_tracker_button_rect(
                    ui,
                    rt,
                    tc,
                    "opt_cc_cancel",
                    res_rect(doc.as_ref(), dlg_min, s, "Cancel", 248.0, 216.0, 72.0, 24.0),
                    &cancel_lbl,
                    TextAlignment::West,
                    false,
                    true,
                )
                .clicked()
                {
                    close_cc = true;
                }
            });
        if close_cc {
            self.content_dialog_open = false;
        }
    }
}

fn dlg_subrect(origin: Pos2, s: f32, x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(
        origin + Vec2::new((x * s).round(), (y * s).round()),
        Vec2::new((w * s).round(), (h * s).round()),
    )
}

fn doc_frame_size(doc: Option<&VguiResDocument>, fb_w: f32, fb_h: f32) -> (f32, f32) {
    if let Some(d) = doc
        && let Some(root) = d.controls.iter().find(|c| matches!(c.kind, ControlKind::Frame))
    {
        return (root.wide.raw_or(fb_w), root.tall.raw_or(fb_h));
    }
    (fb_w, fb_h)
}

#[allow(clippy::too_many_arguments)]
fn res_rect(
    doc: Option<&VguiResDocument>,
    origin: Pos2,
    s: f32,
    field: &str,
    fx: f32,
    fy: f32,
    fw: f32,
    fh: f32,
) -> Rect {
    let (x, y, w, h) = if let Some(spec) = doc.and_then(|d| d.find(field)) {
        (
            spec.xpos.resolve_pos(0.0, 512.0, 1.0),
            spec.ypos.resolve_pos(0.0, 352.0, 1.0),
            if spec.wide_specified { spec.wide.resolve_size(0.0, 512.0, 1.0) } else { fw },
            if spec.tall_specified { spec.tall.resolve_size(0.0, 352.0, 1.0) } else { fh },
        )
    } else {
        (fx, fy, fw, fh)
    };
    dlg_subrect(origin, s, x, y, w, h)
}

fn res_label(
    doc: Option<&VguiResDocument>,
    loc: Option<&LocalizationTable>,
    field: &str,
    fallback_tok: &str,
) -> (String, bool, bool, TextAlignment) {
    if let Some(spec) = doc.and_then(|d| d.find(field)) {
        let raw = if spec.label_text.is_empty() { fallback_tok } else { &spec.label_text };
        return (
            loc_or(loc, raw, raw),
            spec.dull_text,
            spec.wrap,
            spec.text_alignment,
        );
    }
    (loc_or(loc, fallback_tok, fallback_tok), false, false, TextAlignment::West)
}

fn slider_edge_labels(
    doc: Option<&VguiResDocument>,
    loc: Option<&LocalizationTable>,
    field: &str,
    fb_left: &str,
    fb_right: &str,
) -> (String, String) {
    let Some(spec) = doc.and_then(|d| d.find(field)) else {
        return (fb_left.to_string(), fb_right.to_string());
    };
    let l = if spec.left_text.is_empty() {
        fb_left.to_string()
    } else {
        loc_or(loc, &spec.left_text, fb_left)
    };
    let r = if spec.right_text.is_empty() {
        fb_right.to_string()
    } else {
        loc_or(loc, &spec.right_text, fb_right)
    };
    (l, r)
}

fn paint_crosshair_preview(
    painter: &egui::Painter,
    preview_rect: Rect,
    cvars: &CVarRegistry,
    scale: f32,
) {
    let (cr, cg, cb) = parse_rgb(cvars.get_str("cl_crosshair_color"));
    let ca = if cvars.get_bool("cl_crosshair_translucent") { 195 } else { 255 };
    let ch_col = Color32::from_rgba_unmultiplied(cr, cg, cb, ca);
    let (len, gap) = match cvars.get_str("cl_crosshair_size").to_ascii_lowercase().as_str() {
        "small" => (5.0 * scale, 3.0 * scale),
        "large" => (10.0 * scale, 5.0 * scale),
        _ => (7.5 * scale, 4.0 * scale),
    };
    let c = preview_rect.center();
    let st = Stroke::new((1.0 * scale).max(1.0), ch_col);
    painter.line_segment([Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)], st);
    painter.line_segment([Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)], st);
    painter.line_segment([Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)], st);
    painter.line_segment([Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)], st);
}

fn pretty_key_name(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "uparrow" => "Up".into(),
        "downarrow" => "Down".into(),
        "leftarrow" => "Left".into(),
        "rightarrow" => "Right".into(),
        "shift" | "lshift" => "Left Shift".into(),
        "ctrl" | "lctrl" => "Left Ctrl".into(),
        "alt" | "lalt" => "Left Alt".into(),
        "space" => "Space".into(),
        "pgup" => "PageUp".into(),
        "pgdn" => "PageDown".into(),
        "mouse1" => "Mouse1".into(),
        "mouse2" => "Mouse2".into(),
        "tab" => "Tab".into(),
        "escape" => "Esc".into(),
        other => {
            if other.len() == 1 {
                other.to_ascii_uppercase()
            } else {
                raw.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run_options_frame(
        opts: &mut OptionsUi,
        cvars: &mut CVarRegistry,
        rt: &mut VguiRuntime,
    ) -> usize {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            opts.show(
                ui.ctx(),
                rt,
                cvars,
                &OptionsRenderCtx {
                    vfs: None,
                    loc: None,
                    palette: VguiPalette::from(cs_assets::scheme::VguiColors::default()),
                    steam_icon: None,
                },
            );
        });
        out.textures_delta.clear();
        out.shapes.len()
    }

    #[test]
    fn audio_volume_slider_allows_up_to_2_0_and_video_gamma_down_to_1_0() {
        let mut opts = OptionsUi {
            open: true,
            ..Default::default()
        };
        let mut rt = VguiRuntime::new();
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("volume", "1.75");
        cvars.set("gamma", "1.25");

        opts.tab = OptionsTab::Audio;
        assert!(run_options_frame(&mut opts, &mut cvars, &mut rt) > 0);
        assert!(
            (cvars.get_f32("volume") - 1.75).abs() < 1e-4,
            "volume slider range 0.0..=2.0 must not clamp 1.75"
        );
        assert!((VOLUME_SLIDER_MIN - 0.0).abs() < 1e-6);
        assert!((VOLUME_SLIDER_MAX - 2.0).abs() < 1e-6);

        opts.tab = OptionsTab::Video;
        assert!(run_options_frame(&mut opts, &mut cvars, &mut rt) > 0);
        assert!(
            (cvars.get_f32("gamma") - 1.25).abs() < 1e-4,
            "gamma slider range 1.0..=3.0 must not clamp 1.25"
        );
        assert!((GAMMA_SLIDER_MIN - 1.0).abs() < 1e-6);
        assert!((GAMMA_SLIDER_MAX - 3.0).abs() < 1e-6);
    }

    #[test]
    fn mouse_tab_low_latency_binds_to_gl_lowlatency_not_m_customaccel() {
        let mut opts = OptionsUi {
            open: true,
            tab: OptionsTab::Mouse,
            ..Default::default()
        };
        let mut rt = VguiRuntime::new();
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("m_customaccel", "1");

        assert!(run_options_frame(&mut opts, &mut cvars, &mut rt) > 0);
        assert!(cvars.get("gl_lowlatency").is_some());
        assert!(cvars.get_bool("gl_lowlatency"));
        assert!(cvars.get_bool("m_customaccel"));

        assert!(cvars.set("gl_lowlatency", "0"));
        assert!(!cvars.get_bool("gl_lowlatency"));
        assert!(cvars.get_bool("m_customaccel"));
    }
}
