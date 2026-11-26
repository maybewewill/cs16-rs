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
use crate::ui::vgui_theme::VguiPalette;
use crate::ui::vgui2::{
    TrackerCtx, VguiRuntime, draw_tracker_button_rect, draw_tracker_check_button_rect,
    draw_tracker_close_button, draw_tracker_combo_box_rect, draw_tracker_list_panel_rect,
    draw_tracker_slider_rect, draw_tracker_text_entry_rect, paint_tracker_bevel_rect,
    paint_tracker_label_rect,
};

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

/// Video settings emitted on **Apply** / **OK** so the native window shell can resize, toggle
/// borderless fullscreen, and recreate the `wgpu` surface/backend live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoApplyRequest {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub backend: String,
}

#[derive(Debug, Clone, Default)]
struct OptionsSnapshot {
    cvars: Vec<(String, String)>,
    binds: Vec<(String, String)>,
    hd_models: bool,
}

const VIDEO_BACKENDS: &[(&str, &str)] = &[
    ("auto", "Auto / Best"),
    ("vulkan", "Vulkan"),
    ("dx12", "Direct3D 12"),
    ("metal", "Metal"),
    ("gl", "OpenGL"),
];

const TEXTURE_STYLES: &[(&str, &str, &str)] = &[
    ("GL_LINEAR_MIPMAP_LINEAR", "0", "OpenGL 1.6 Trilinear"),
    ("GL_LINEAR", "0", "OpenGL Bilinear"),
    ("GL_NEAREST", "1", "Software Retro Nearest"),
];

const ASPECT_MODES: &[(&str, &str)] = &[
    ("4:3_stretch", "Normal (4:3)"),
    ("widescreen", "Widescreen (16:9)"),
    ("16:10", "Widescreen (16:10)"),
    ("4:3_bars", "Normal (4:3 Bars)"),
];

const MANAGED_OPTION_CVARS: &[&str] = &[
    "name",
    "cl_logofile",
    "cl_logocolor",
    "cl_crosshair_size",
    "cl_crosshair_color",
    "cl_crosshair_translucent",
    "cl_dynamiccrosshair",
    "cl_righthand",
    "hud_fastswitch",
    "hud_centerid",
    "cl_radartype",
    "volume",
    "mp3volume",
    "suitvolume",
    "hisound",
    "room_off",
    "r_refdll",
    "width",
    "height",
    "fullscreen",
    "vid_aspect_mode",
    "gl_texturemode",
    "r_software_mode",
    "gl_widescreen_yfov",
    "gl_vsync",
    "gl_lowlatency",
    "gl_use_shaders",
    "brightness",
    "gamma",
    "cl_hd_models",
    "m_pitch",
    "m_filter",
    "sv_aim",
    "m_rawinput",
    "sensitivity",
    "zoom_sensitivity_ratio",
    "voice_enable",
    "voice_modenable",
    "voice_forcemicrecord",
    "voice_avggain",
    "voice_scale",
    "voice_loopback",
    "cl_allowdownload",
    "violence_hblood",
    "violence_ablood",
    "violence_hgibs",
    "violence_agibs",
];

fn ensure_cvar(cvars: &mut CVarRegistry, name: &str, default: &str, desc: &str) {
    if cvars.get(name).is_none() {
        cvars.register(name, default, cs_cvar::CVarFlags::ARCHIVE, desc);
    }
}

fn ensure_option_cvars(cvars: &mut CVarRegistry) {
    ensure_cvar(cvars, LOW_LATENCY_CVAR, "1", "Lower input latency (0 or 1)");
    ensure_cvar(
        cvars,
        "r_refdll",
        "auto",
        "Renderer GPU backend (auto/vulkan/dx12/metal/gl)",
    );
    ensure_cvar(cvars, "width", "1280", "Window physical width");
    ensure_cvar(cvars, "height", "720", "Window physical height");
    ensure_cvar(
        cvars,
        "fullscreen",
        "0",
        "Borderless fullscreen mode (0 or 1)",
    );
    ensure_cvar(
        cvars,
        "gl_widescreen_yfov",
        "1",
        "Widescreen Hor+ FOV scaling",
    );
    ensure_cvar(cvars, "gl_use_shaders", "1", "Enable GPU lighting shaders");
    ensure_cvar(cvars, "hisound", "1", "High-quality audio mixing (0 or 1)");
    ensure_cvar(cvars, "room_off", "0", "Disable GoldSrc room DSP reverb");
    ensure_cvar(cvars, "sv_aim", "0", "Classic auto-aim assist");
    ensure_cvar(
        cvars,
        "voice_forcemicrecord",
        "1",
        "Boost / force microphone capture",
    );
    ensure_cvar(
        cvars,
        "cl_logofile",
        "lambda",
        "Spraypaint decal image name",
    );
    ensure_cvar(
        cvars,
        "cl_logocolor",
        "Orange",
        "Spraypaint decal color preset",
    );
    ensure_cvar(
        cvars,
        "cl_allowdownload",
        "1",
        "Allow custom addon/server downloads",
    );
}

fn ensure_lowlatency_cvar(cvars: &mut CVarRegistry) {
    ensure_cvar(cvars, LOW_LATENCY_CVAR, "1", "Lower input latency (0 or 1)");
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

const RESOLUTIONS: &[(u32, u32, &str)] = &[
    (640, 480, "640 x 480"),
    (800, 600, "800 x 600"),
    (1024, 768, "1024 x 768"),
    (1280, 720, "1280 x 720"),
    (1280, 800, "1280 x 800"),
    (1280, 1024, "1280 x 1024"),
    (1600, 900, "1600 x 900"),
    (1920, 1080, "1920 x 1080"),
    (2560, 1440, "2560 x 1440"),
];

fn backend_idx_from_cvar(raw: &str) -> usize {
    let lower = raw.trim().to_ascii_lowercase();
    VIDEO_BACKENDS
        .iter()
        .position(|(k, _)| *k == lower)
        .unwrap_or(0)
}

fn texture_style_idx_from_cvars(cvars: &CVarRegistry) -> usize {
    let mode = cvars.get_str("gl_texturemode").trim().to_ascii_uppercase();
    if cvars.get_bool("r_software_mode")
        || mode == "GL_NEAREST"
        || mode == "GL_NEAREST_MIPMAP_NEAREST"
    {
        2
    } else if mode == "GL_LINEAR" || mode == "GL_LINEAR_MIPMAP_NEAREST" {
        1
    } else {
        0
    }
}

fn parse_resolution_label(label: &str) -> Option<(u32, u32)> {
    let mut parts = label.split('x');
    let w = parts.next()?.trim().parse::<u32>().ok()?;
    let h = parts.next()?.trim().parse::<u32>().ok()?;
    (w >= 320 && h >= 240).then_some((w, h))
}

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
    rebind_arm_frame: bool,
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
    pub video_apply_requested: Option<VideoApplyRequest>,
    pub gpu_adapter: String,
    pub gpu_backend: String,
    drag_offset: Vec2,
    adv_drag_offset: Vec2,
    content_dialog_open: bool,
    content_pw2: String,
    selected_bind_row: Option<usize>,
    sens_text_buf: String,
    sens_last_val: f32,
    zoom_sens_buf: String,
    zoom_sens_last_val: f32,
    selected_res_w: u32,
    selected_res_h: u32,
    backend_idx: usize,
    texture_style_idx: usize,
    windowed: bool,
    was_open: bool,
    snapshot: Option<OptionsSnapshot>,
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
            command: if is_header {
                String::new()
            } else {
                cmd.to_string()
            },
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

    pub fn set_gpu_info(&mut self, adapter: &str, backend: &str) {
        if self.gpu_adapter != adapter {
            self.gpu_adapter = adapter.to_string();
        }
        if self.gpu_backend != backend {
            self.gpu_backend = backend.to_string();
        }
    }

    fn capture_snapshot(&self, cvars: &CVarRegistry) -> OptionsSnapshot {
        let mut names: Vec<&str> = MANAGED_OPTION_CVARS.to_vec();
        if let Some(ref scr) = self.user_script {
            for opt in &scr.options {
                names.push(opt.cvar.as_str());
            }
        }
        let mut saved = Vec::with_capacity(names.len());
        for name in names {
            if let Some(v) = cvars.get(name) {
                saved.push((name.to_string(), v.value.clone()));
            }
        }
        let binds = cvars
            .binds()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        OptionsSnapshot {
            cvars: saved,
            binds,
            hd_models: cvars.get_bool("cl_hd_models"),
        }
    }

    fn sync_from_cvars(&mut self, cvars: &mut CVarRegistry, screen: Rect) {
        ensure_option_cvars(cvars);
        self.windowed = !cvars.get_bool("fullscreen");
        self.backend_idx = backend_idx_from_cvar(cvars.get_str("r_refdll"));
        self.texture_style_idx = texture_style_idx_from_cvars(cvars);
        let cur_w = screen.width().round().max(640.0) as u32;
        let cur_h = screen.height().round().max(480.0) as u32;
        let cw = cvars.get_i32("width");
        let ch = cvars.get_i32("height");
        if cw >= 320 && ch >= 240 {
            self.selected_res_w = cw as u32;
            self.selected_res_h = ch as u32;
        } else {
            self.selected_res_w = cur_w;
            self.selected_res_h = cur_h;
        }
        let logo_cvar = cvars.get_str("cl_logofile").trim().to_ascii_lowercase();
        if !logo_cvar.is_empty()
            && let Some(found) = self.spray_logos.iter().find(|l| {
                let stem = l
                    .strip_suffix(".bmp")
                    .or_else(|| l.strip_suffix(".BMP"))
                    .unwrap_or(l);
                stem.eq_ignore_ascii_case(&logo_cvar) || l.eq_ignore_ascii_case(&logo_cvar)
            })
        {
            self.selected_spray = found.clone();
        }
        let logo_col = cvars.get_str("cl_logocolor").trim();
        if let Some(idx) = SPRAY_COLOR_PRESETS
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(logo_col))
        {
            self.spray_color_idx = idx;
        }
    }

    /// Commit pending video/display selections into `cvars`, emit [`VideoApplyRequest`],
    /// request `config.cfg` persistence, and refresh the Cancel rollback snapshot.
    pub fn apply_changes(&mut self, cvars: &mut CVarRegistry) {
        ensure_option_cvars(cvars);
        let w = self.selected_res_w.max(640);
        let h = self.selected_res_h.max(480);
        let fullscreen = !self.windowed;
        let backend = VIDEO_BACKENDS
            .get(self.backend_idx)
            .map_or("auto", |(k, _)| *k)
            .to_string();
        cvars.set("width", w.to_string());
        cvars.set("height", h.to_string());
        cvars.set("fullscreen", if fullscreen { "1" } else { "0" });
        cvars.set("r_refdll", &backend);
        self.video_apply_requested = Some(VideoApplyRequest {
            width: w,
            height: h,
            fullscreen,
            backend,
        });
        self.save_requested = true;
        self.snapshot = Some(self.capture_snapshot(cvars));
    }

    /// Revert all un-applied Option CVars & keybinds back to the snapshot taken on dialog open
    /// (or last Apply), then close the Options dialog cleanly.
    pub fn cancel_and_close(&mut self, cvars: &mut CVarRegistry) {
        if let Some(snap) = self.snapshot.take() {
            let cur_hd = cvars.get_bool("cl_hd_models");
            for (k, v) in snap.cvars {
                cvars.set(&k, v);
            }
            if cur_hd != snap.hd_models {
                self.hd_models_changed = true;
            }
            cvars.unbind_all();
            for (k, cmd) in snap.binds {
                cvars.bind(&k, &cmd);
            }
        }
        self.open = false;
        self.was_open = false;
        self.advanced_open = false;
        self.content_dialog_open = false;
        self.rebinding_cmd = None;
        self.rebind_arm_frame = false;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        o: &OptionsRenderCtx<'_>,
    ) {
        if !self.open {
            self.was_open = false;
            self.rebinding_cmd = None;
            self.rebind_arm_frame = false;
            return;
        }
        let screen = ctx.content_rect();
        if !self.was_open {
            self.was_open = true;
            self.sync_from_cvars(cvars, screen);
            self.snapshot = Some(self.capture_snapshot(cvars));
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
            (default_x + self.drag_offset.x)
                .clamp(screen.left(), max_x)
                .round(),
            (default_y + self.drag_offset.y)
                .clamp(screen.top(), max_y)
                .round(),
        );
        let dlg_rect = Rect::from_min_size(dlg_min, Vec2::new(dlg_w, dlg_h));

        let mut close_cancel = false;
        let mut close_ok = false;
        let mut do_apply = false;
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
                    close_cancel = true;
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
                    (
                        OptionsTab::Multiplayer,
                        loc_or(o.loc, "#GameUI_Multiplayer", "Multiplayer"),
                    ),
                    (OptionsTab::Audio, loc_or(o.loc, "#GameUI_Audio", "Audio")),
                    (OptionsTab::Video, loc_or(o.loc, "#GameUI_Video", "Video")),
                    (OptionsTab::Mouse, loc_or(o.loc, "#GameUI_Mouse", "Aim")),
                    (
                        OptionsTab::Keyboard,
                        loc_or(o.loc, "#GameUI_Keyboard", "Keyboard"),
                    ),
                    (OptionsTab::Voice, loc_or(o.loc, "#GameUI_Voice", "Voice")),
                    (
                        OptionsTab::Lock,
                        loc_or(o.loc, "#GameUI_Content", "Content"),
                    ),
                ];
                let mut tx = dlg_min.x + (5.0 * s).round();
                let fonts = runtime.tracker_scheme.fonts.clone();
                let tab_pad_w = (16.0 * s).round();
                let min_tab_w = (64.0 * s).round();
                let tab_gap = (1.0 * s).round().max(1.0);
                for (tab_val, label) in &tab_specs {
                    let tw = runtime
                        .font_atlas
                        .measure_text_with_scale(
                            ctx, o.vfs, &fonts, "tracker", "Default", screen_h, label, s,
                        )
                        .x;
                    let tab_w = (tw + tab_pad_w).round().max(min_tab_w);
                    let active = self.tab == *tab_val;
                    let top_y = dlg_min.y
                        + if active {
                            (29.0 * s).round()
                        } else {
                            (31.0 * s).round()
                        };
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
                        Rect::from_min_max(
                            tab_rect.min,
                            Pos2::new(tab_rect.right(), tab_rect.top() + 1.0),
                        ),
                        0.0,
                        tc.palette.border_bright,
                    );
                    tp.rect_filled(
                        Rect::from_min_max(
                            tab_rect.min,
                            Pos2::new(tab_rect.left() + 1.0, tab_rect.bottom()),
                        ),
                        0.0,
                        tc.palette.border_bright,
                    );
                    tp.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(tab_rect.right() - 1.0, tab_rect.top()),
                            tab_rect.max,
                        ),
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
                        Pos2::new(
                            tab_rect.left() + inset_x,
                            tab_rect.top() + (1.0 * s).round(),
                        ),
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
                    ui,
                    runtime,
                    &tc,
                    "opt_ok",
                    ok_rect,
                    &ok_lbl,
                    TextAlignment::West,
                    false,
                    true,
                )
                .clicked()
                {
                    close_ok = true;
                }
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "opt_cancel",
                    cancel_rect,
                    &cancel_lbl,
                    TextAlignment::West,
                    false,
                    true,
                )
                .clicked()
                {
                    close_cancel = true;
                }
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "opt_apply",
                    apply_rect,
                    &apply_lbl,
                    TextAlignment::West,
                    false,
                    true,
                )
                .clicked()
                {
                    do_apply = true;
                }
            });

        if close_ok {
            self.apply_changes(cvars);
            self.open = false;
            self.was_open = false;
            self.advanced_open = false;
            self.content_dialog_open = false;
            self.snapshot = None;
        } else if close_cancel {
            self.cancel_and_close(cvars);
        } else if do_apply {
            self.apply_changes(cvars);
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
        if self.rebind_arm_frame {
            self.rebind_arm_frame = false;
            return;
        }
        let captured = ctx.input(|i| {
            for ev in &i.events {
                match ev {
                    egui::Event::Key {
                        key, pressed: true, ..
                    } => {
                        if *key == egui::Key::Escape {
                            return Some(None);
                        }
                        return Some(Some(egui_key_to_goldsrc(*key)));
                    }
                    egui::Event::PointerButton {
                        button,
                        pressed: true,
                        ..
                    } => {
                        return Some(Some(egui_mouse_button_to_goldsrc(*button).to_string()));
                    }
                    egui::Event::MouseWheel { delta, .. } if delta.y.abs() > 0.0 => {
                        let key = if delta.y > 0.0 {
                            "mwheelup"
                        } else {
                            "mwheeldown"
                        };
                        return Some(Some(key.to_string()));
                    }
                    _ => {}
                }
            }
            if i.modifiers.ctrl {
                return Some(Some("ctrl".to_string()));
            }
            if i.modifiers.shift {
                return Some(Some("shift".to_string()));
            }
            if i.modifiers.alt {
                return Some(Some("alt".to_string()));
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
            if dull {
                tc.palette.dim_text
            } else {
                tc.palette.base_text
            },
            wrap,
            true,
        );

        // `LogoImage` (`40, 120, 64, 64` + 1px `InsetBorder` outside/inside)
        let logo_rect = rect_or("LogoImage", 40.0, 120.0, 64.0, 64.0);
        ui.painter()
            .rect_filled(logo_rect, CornerRadius::ZERO, Color32::BLACK);
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
                .map(|s| {
                    s.strip_suffix(".bmp")
                        .or_else(|| s.strip_suffix(".BMP"))
                        .unwrap_or(s)
                        .to_string()
                })
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
            let stem = chosen
                .strip_suffix(".bmp")
                .or_else(|| chosen.strip_suffix(".BMP"))
                .unwrap_or(chosen);
            cvars.set("cl_logofile", stem);
        }

        // `SpraypaintColor`
        let color_items: Vec<String> = SPRAY_COLOR_PRESETS
            .iter()
            .map(|(n, _)| (*n).to_string())
            .collect();
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_spray_color",
            rect_or("SpraypaintColor", 112.0, 160.0, 124.0, 24.0),
            &color_items,
            &mut self.spray_color_idx,
            true,
        ) && let Some((name, _)) = SPRAY_COLOR_PRESETS.get(self.spray_color_idx)
        {
            cvars.set("cl_logocolor", *name);
        }

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
            cvars.set(
                "cl_crosshair_translucent",
                if translucent { "1" } else { "0" },
            );
        }

        let mut dyn_ch = cvars.get_bool("cl_dynamiccrosshair");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_ch_dynamic",
            dlg_subrect(origin, tc.scale, 281.0, 214.0, 181.0, 24.0),
            &mut dyn_ch,
            "Dynamic crosshair",
            true,
        ) {
            cvars.set("cl_dynamiccrosshair", if dyn_ch { "1" } else { "0" });
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
        let mut vol = cvars
            .get_f32("volume")
            .clamp(VOLUME_SLIDER_MIN, VOLUME_SLIDER_MAX);
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

        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, tc.scale, 40.0, 138.0, 180.0, 22.0),
            "HEV Suit / Radio volume",
            "Default",
            TextAlignment::West,
            tc.palette.base_text,
            false,
            true,
        );
        let mut suit = cvars.get_f32("suitvolume").clamp(0.0, 1.0);
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_audio_suit",
            dlg_subrect(origin, tc.scale, 40.0, 160.0, 420.0, 34.0),
            &mut suit,
            0.0,
            1.0,
            "",
            "",
        ) {
            cvars.set("suitvolume", format!("{suit:.2}"));
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
        let mut sq_idx = if cvars.get_bool("hisound") { 0 } else { 1 };
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_audio_quality",
            rect_or("Sound Quality", 40.0, 226.0, 180.0, 24.0),
            &sq_items,
            &mut sq_idx,
            true,
        ) {
            cvars.set("hisound", if sq_idx == 0 { "1" } else { "0" });
        }

        let mut room_off = cvars.get_bool("room_off");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_audio_room_off",
            dlg_subrect(origin, tc.scale, 240.0, 226.0, 220.0, 24.0),
            &mut room_off,
            "Disable room DSP / reverb",
            true,
        ) {
            cvars.set("room_off", if room_off { "1" } else { "0" });
        }

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
        let label_or = |field: &str, fb: &str| -> (String, bool, bool, TextAlignment) {
            res_label(doc.as_ref(), o.loc, field, fb)
        };
        let s = tc.scale;

        // 1. Renderer Backend (`Auto / Best`, `Vulkan`, `Direct3D 12`, `Metal`, `OpenGL`) + live GPU info
        let (r_lbl, _, wrap, align) = label_or("Label2", "#GameUI_Renderer");
        let label_align = align;
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, s, 28.0, 14.0, 108.0, 24.0),
            &r_lbl,
            "Default",
            label_align,
            tc.palette.bright_control_text,
            wrap,
            true,
        );
        let backend_items: Vec<String> = VIDEO_BACKENDS
            .iter()
            .map(|(k, l)| {
                if *k == "auto" && !self.gpu_backend.is_empty() {
                    format!("Auto ({})", self.gpu_backend)
                } else {
                    (*l).to_string()
                }
            })
            .collect();
        draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_vid_backend",
            dlg_subrect(origin, s, 140.0, 14.0, 168.0, 24.0),
            &backend_items,
            &mut self.backend_idx,
            true,
        );
        if !self.gpu_adapter.is_empty() {
            let gpu_line = if self.gpu_backend.is_empty() {
                self.gpu_adapter.clone()
            } else {
                format!("{} ({})", self.gpu_adapter, self.gpu_backend)
            };
            paint_tracker_label_rect(
                ui.painter(),
                rt,
                tc,
                dlg_subrect(origin, s, 314.0, 12.0, 190.0, 30.0),
                &gpu_line,
                "DefaultSmall",
                TextAlignment::NorthWest,
                tc.palette.dim_text,
                true,
                true,
            );
        }

        // 2. Resolution (live physical pixels on Apply/OK)
        let (res_lbl, _, wrap, align) = label_or("Label1", "#GameUI_Resolution");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, s, 28.0, 44.0, 108.0, 24.0),
            &res_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut res_items: Vec<String> = RESOLUTIONS
            .iter()
            .map(|(_, _, l)| (*l).to_string())
            .collect();
        let cur_sel = format!("{} x {}", self.selected_res_w, self.selected_res_h);
        if !res_items.contains(&cur_sel) {
            res_items.push(cur_sel.clone());
        }
        let mut res_idx = res_items.iter().position(|r| r == &cur_sel).unwrap_or(3);
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_vid_res",
            dlg_subrect(origin, s, 140.0, 44.0, 168.0, 24.0),
            &res_items,
            &mut res_idx,
            true,
        ) && let Some(chosen) = res_items.get(res_idx)
            && let Some((w, h)) = parse_resolution_label(chosen)
        {
            self.selected_res_w = w;
            self.selected_res_h = h;
        }

        // 3. Aspect Ratio (`Normal 4:3` / `Widescreen 16:9` / `16:10` / `4:3 Bars`)
        let (dm_lbl, _, wrap, align) = label_or("Label4", "#GameUI_DisplayMode");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, s, 28.0, 74.0, 108.0, 24.0),
            &dm_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let aspect_items: Vec<String> =
            ASPECT_MODES.iter().map(|(_, l)| (*l).to_string()).collect();
        let cur_aspect = cvars.get_str("vid_aspect_mode").to_string();
        let mut aspect_idx = ASPECT_MODES
            .iter()
            .position(|(v, _)| v.eq_ignore_ascii_case(&cur_aspect))
            .unwrap_or(1);
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_vid_aspect",
            dlg_subrect(origin, s, 140.0, 74.0, 168.0, 24.0),
            &aspect_items,
            &mut aspect_idx,
            true,
        ) {
            cvars.set("vid_aspect_mode", ASPECT_MODES[aspect_idx].0);
        }

        // 4. Texture Filter Style (`OpenGL 1.6 Trilinear` / `OpenGL Bilinear` / `Software Retro Nearest`)
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, s, 28.0, 104.0, 108.0, 24.0),
            "Texture Style",
            "Default",
            label_align,
            tc.palette.base_text,
            false,
            true,
        );
        let tex_items: Vec<String> = TEXTURE_STYLES
            .iter()
            .map(|(_, _, l)| (*l).to_string())
            .collect();
        self.texture_style_idx = texture_style_idx_from_cvars(cvars);
        if draw_tracker_combo_box_rect(
            ui,
            rt,
            tc,
            "opt_vid_texstyle",
            dlg_subrect(origin, s, 140.0, 104.0, 168.0, 24.0),
            &tex_items,
            &mut self.texture_style_idx,
            true,
        ) {
            let (mode, sw, _) = TEXTURE_STYLES[self.texture_style_idx];
            cvars.set("gl_texturemode", mode);
            cvars.set("r_software_mode", sw);
        }

        // 5. Video checkboxes (2 crisp well-spaced columns, every checkbox wired live to CVars)
        let (win_lbl, _, _, _) = label_or("Windowed", "#GameUI_Windowed");
        draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_win",
            dlg_subrect(origin, s, 28.0, 134.0, 236.0, 22.0),
            &mut self.windowed,
            &win_lbl,
            true,
        );

        let (wfov_lbl, _, _, _) = label_or("WidescreenFOV", "#GameUI_WidescreenFOV");
        let mut wfov = cvars.get_bool("gl_widescreen_yfov");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_wfov",
            dlg_subrect(origin, s, 28.0, 158.0, 236.0, 22.0),
            &mut wfov,
            &wfov_lbl,
            true,
        ) {
            cvars.set("gl_widescreen_yfov", if wfov { "1" } else { "0" });
        }

        let (vs_lbl, _, _, _) = label_or("VSync", "#GameUI_VSync");
        let mut vsync = cvars.get_bool("gl_vsync");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_vsync",
            dlg_subrect(origin, s, 28.0, 182.0, 236.0, 22.0),
            &mut vsync,
            &vs_lbl,
            true,
        ) {
            cvars.set("gl_vsync", if vsync { "1" } else { "0" });
        }

        ensure_lowlatency_cvar(cvars);
        let mut lowlat = cvars.get_bool(LOW_LATENCY_CVAR);
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_lowlat",
            dlg_subrect(origin, s, 278.0, 134.0, 218.0, 22.0),
            &mut lowlat,
            "Low video latency",
            true,
        ) {
            cvars.set(LOW_LATENCY_CVAR, if lowlat { "1" } else { "0" });
        }

        let (sh_lbl, _, _, _) = label_or("UseShaders", "#GameUI_UseShaders");
        let mut shaders = cvars.get_bool("gl_use_shaders");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_shaders",
            dlg_subrect(origin, s, 278.0, 158.0, 218.0, 22.0),
            &mut shaders,
            &sh_lbl,
            true,
        ) {
            cvars.set("gl_use_shaders", if shaders { "1" } else { "0" });
        }

        let mut hd_models = cvars.get_bool("cl_hd_models");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_vid_hd_models",
            dlg_subrect(origin, s, 278.0, 182.0, 218.0, 22.0),
            &mut hd_models,
            "Enable HD models",
            true,
        ) {
            cvars.set("cl_hd_models", if hd_models { "1" } else { "0" });
            self.hd_models_changed = true;
        }

        let (br_lbl, _, wrap, align) = label_or("brightness label", "#GameUI_Brightness");
        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, s, 28.0, 218.0, 108.0, 24.0),
            &br_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut bright = cvars.get_f32("brightness").clamp(0.0, 2.0);
        let (dark_s, light_s) =
            slider_edge_labels(doc.as_ref(), o.loc, "Brightness", "Dark", "Light");
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_vid_bright",
            dlg_subrect(origin, s, 140.0, 218.0, 300.0, 36.0),
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
            dlg_subrect(origin, s, 28.0, 260.0, 108.0, 24.0),
            &gm_lbl,
            "Default",
            align,
            tc.palette.base_text,
            wrap,
            true,
        );
        let mut gamma = cvars
            .get_f32("gamma")
            .clamp(GAMMA_SLIDER_MIN, GAMMA_SLIDER_MAX);
        let (low_s, high_s) = slider_edge_labels(doc.as_ref(), o.loc, "Gamma", "Low", "High");
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_vid_gamma",
            dlg_subrect(origin, s, 140.0, 260.0, 300.0, 36.0),
            &mut gamma,
            GAMMA_SLIDER_MIN,
            GAMMA_SLIDER_MAX,
            &low_s,
            &high_s,
        ) {
            cvars.set("gamma", format!("{gamma:.2}"));
        }

        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, s, 28.0, 304.0, 460.0, 40.0),
            "Resolution, display mode, and GPU backend changes apply live on Apply / OK.",
            "DefaultSmall",
            TextAlignment::NorthWest,
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
        let mut auto_aim = cvars.get_bool("sv_aim");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_m_autoaim",
            rect_or("Auto-Aim", 30.0, 80.0, 160.0, 24.0),
            &mut auto_aim,
            &aa_lbl,
            true,
        ) {
            cvars.set("sv_aim", if auto_aim { "1" } else { "0" });
        }
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

        paint_tracker_label_rect(
            ui.painter(),
            rt,
            tc,
            dlg_subrect(origin, tc.scale, 34.0, 266.0, 200.0, 22.0),
            "Zoom sensitivity ratio",
            "Default",
            TextAlignment::West,
            tc.palette.base_text,
            false,
            true,
        );
        let mut zsens = cvars.get_f32("zoom_sensitivity_ratio").clamp(0.20, 3.00);
        if (zsens - self.zoom_sens_last_val).abs() > 1e-4 || self.zoom_sens_buf.is_empty() {
            self.zoom_sens_last_val = zsens;
            self.zoom_sens_buf = format!("{zsens:.2}");
        }
        if draw_tracker_slider_rect(
            ui,
            rt,
            tc,
            "opt_m_zsens",
            dlg_subrect(origin, tc.scale, 34.0, 288.0, 272.0, 38.0),
            &mut zsens,
            0.20,
            3.00,
            "0.20",
            "3.00",
        ) {
            cvars.set("zoom_sensitivity_ratio", format!("{zsens:.2}"));
            self.zoom_sens_last_val = zsens;
            self.zoom_sens_buf = format!("{zsens:.2}");
        }
        if draw_tracker_text_entry_rect(
            ui,
            rt,
            tc,
            "opt_m_zsens_val",
            dlg_subrect(origin, tc.scale, 320.0, 288.0, 48.0, 24.0),
            &mut self.zoom_sens_buf,
            false,
            true,
            true,
        ) && let Ok(v) = self.zoom_sens_buf.parse::<f32>()
        {
            let clamped = v.clamp(0.1, 5.0);
            cvars.set("zoom_sensitivity_ratio", format!("{clamped:.2}"));
            self.zoom_sens_last_val = clamped;
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
            self.rebind_arm_frame = true;
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
            self.rebind_arm_frame = true;
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
        let mut mic_boost = cvars.get_bool("voice_forcemicrecord");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_v_micboost",
            rect_or("MicBoost", 33.0, 66.0, 250.0, 28.0),
            &mut mic_boost,
            &mb_lbl,
            true,
        ) {
            cvars.set("voice_forcemicrecord", if mic_boost { "1" } else { "0" });
        }

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
        let mut addons = cvars.get_bool("cl_allowdownload");
        if draw_tracker_check_button_rect(
            ui,
            rt,
            tc,
            "opt_c_addons",
            rect_or("AddonsFolder", 40.0, 64.0, 280.0, 24.0),
            &mut addons,
            &ad_lbl,
            true,
        ) {
            cvars.set("cl_allowdownload", if addons { "1" } else { "0" });
        }

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
                    Rect::from_min_size(
                        dlg_min + Vec2::new(10.0 * s, 4.0 * s),
                        Vec2::new(w - 40.0 * s, 22.0 * s),
                    ),
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

                let list_r = res_rect(
                    doc.as_ref(),
                    dlg_min,
                    s,
                    "PanelListPanel",
                    16.0,
                    56.0,
                    442.0,
                    274.0,
                );
                ui.painter()
                    .rect_filled(list_r, CornerRadius::ZERO, tc.palette.window_bg);
                paint_tracker_bevel_rect(ui.painter(), list_r, false, tc.palette);
                ui.scope_builder(egui::UiBuilder::new().max_rect(list_r.shrink(8.0)), |ui| {
                    ScrollArea::vertical()
                        .id_salt("opt_adv_scr")
                        .show(ui, |ui| {
                            if let Some(ref scr) = self.user_script {
                                render_scr_options(ui, cvars, scr, o.loc);
                            } else {
                                cvar_checkbox(
                                    ui,
                                    cvars,
                                    "cl_righthand",
                                    "Right-handed weapon models",
                                );
                                cvar_checkbox(ui, cvars, "hud_fastswitch", "Fast weapon switch");
                                cvar_checkbox(ui, cvars, "hud_centerid", "Center player ID names");
                                cvar_checkbox(ui, cvars, "cl_radartype", "Solid radar type");
                            }
                        });
                });

                let ok_lbl = loc_or(o.loc, "#GameUI_OK", "OK");
                let cancel_lbl = loc_or(o.loc, "#GameUI_Cancel", "Cancel");
                let ok_r = res_rect(doc.as_ref(), dlg_min, s, "OK", 308.0, 338.0, 72.0, 24.0);
                let cancel_r =
                    res_rect(doc.as_ref(), dlg_min, s, "Cancel", 388.0, 338.0, 72.0, 24.0);
                if draw_tracker_button_rect(
                    ui,
                    rt,
                    tc,
                    "opt_adv_ok",
                    ok_r,
                    &ok_lbl,
                    TextAlignment::West,
                    true,
                    true,
                )
                .clicked()
                    || draw_tracker_button_rect(
                        ui,
                        rt,
                        tc,
                        "opt_adv_cancel",
                        cancel_r,
                        &cancel_lbl,
                        TextAlignment::West,
                        false,
                        true,
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
                    Rect::from_min_size(
                        dlg_min + Vec2::new(10.0 * s, 4.0 * s),
                        Vec2::new(w - 40.0 * s, 22.0 * s),
                    ),
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
                let status_txt = loc_or(
                    o.loc,
                    status_tok,
                    "Enter a password to enable the content lock.",
                );
                paint_tracker_label_rect(
                    ui.painter(),
                    rt,
                    tc,
                    res_rect(
                        doc.as_ref(),
                        dlg_min,
                        s,
                        "ContentStatus",
                        30.0,
                        30.0,
                        280.0,
                        36.0,
                    ),
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
                    res_rect(
                        doc.as_ref(),
                        dlg_min,
                        s,
                        "PasswordPrompt",
                        32.0,
                        68.0,
                        280.0,
                        28.0,
                    ),
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
                    res_rect(
                        doc.as_ref(),
                        dlg_min,
                        s,
                        "Password",
                        32.0,
                        98.0,
                        166.0,
                        24.0,
                    ),
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
                    res_rect(
                        doc.as_ref(),
                        dlg_min,
                        s,
                        "PasswordReentryPrompt",
                        32.0,
                        124.0,
                        280.0,
                        28.0,
                    ),
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
                    res_rect(
                        doc.as_ref(),
                        dlg_min,
                        s,
                        "Password2",
                        32.0,
                        154.0,
                        166.0,
                        24.0,
                    ),
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
        && let Some(root) = d
            .controls
            .iter()
            .find(|c| matches!(c.kind, ControlKind::Frame))
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
            if spec.wide_specified {
                spec.wide.resolve_size(0.0, 512.0, 1.0)
            } else {
                fw
            },
            if spec.tall_specified {
                spec.tall.resolve_size(0.0, 352.0, 1.0)
            } else {
                fh
            },
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
        let raw = if spec.label_text.is_empty() {
            fallback_tok
        } else {
            &spec.label_text
        };
        return (
            loc_or(loc, raw, raw),
            spec.dull_text,
            spec.wrap,
            spec.text_alignment,
        );
    }
    (
        loc_or(loc, fallback_tok, fallback_tok),
        false,
        false,
        TextAlignment::West,
    )
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
    let ca = if cvars.get_bool("cl_crosshair_translucent") {
        195
    } else {
        255
    };
    let ch_col = Color32::from_rgba_unmultiplied(cr, cg, cb, ca);
    let (len, gap) = match cvars
        .get_str("cl_crosshair_size")
        .to_ascii_lowercase()
        .as_str()
    {
        "small" => (5.0 * scale, 3.0 * scale),
        "large" => (10.0 * scale, 5.0 * scale),
        _ => (7.5 * scale, 4.0 * scale),
    };
    let c = preview_rect.center();
    let st = Stroke::new((1.0 * scale).max(1.0), ch_col);
    painter.line_segment(
        [Pos2::new(c.x, c.y - gap - len), Pos2::new(c.x, c.y - gap)],
        st,
    );
    painter.line_segment(
        [Pos2::new(c.x, c.y + gap), Pos2::new(c.x, c.y + gap + len)],
        st,
    );
    painter.line_segment(
        [Pos2::new(c.x - gap - len, c.y), Pos2::new(c.x - gap, c.y)],
        st,
    );
    painter.line_segment(
        [Pos2::new(c.x + gap, c.y), Pos2::new(c.x + gap + len, c.y)],
        st,
    );
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
                let hi = if *max > *min {
                    *max
                } else {
                    (*min + 100.0).max(16000.0)
                };
                ui.horizontal(|ui| {
                    ui.label(prompt);
                    if ui
                        .add(egui::DragValue::new(&mut val).range(*min..=hi))
                        .changed()
                    {
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

fn egui_mouse_button_to_goldsrc(button: egui::PointerButton) -> &'static str {
    match button {
        egui::PointerButton::Primary => "mouse1",
        egui::PointerButton::Secondary => "mouse2",
        egui::PointerButton::Middle => "mouse3",
        egui::PointerButton::Extra1 => "mouse4",
        egui::PointerButton::Extra2 => "mouse5",
    }
}

fn egui_key_to_goldsrc(key: egui::Key) -> String {
    match key {
        egui::Key::Space => "space".into(),
        egui::Key::Tab => "tab".into(),
        egui::Key::Enter => "enter".into(),
        egui::Key::Backspace => "backspace".into(),
        egui::Key::Backtick => "`".into(),
        egui::Key::ArrowUp => "uparrow".into(),
        egui::Key::ArrowDown => "downarrow".into(),
        egui::Key::ArrowLeft => "leftarrow".into(),
        egui::Key::ArrowRight => "rightarrow".into(),
        egui::Key::Insert => "ins".into(),
        egui::Key::Delete => "del".into(),
        egui::Key::Home => "home".into(),
        egui::Key::End => "end".into(),
        egui::Key::PageUp => "pgup".into(),
        egui::Key::PageDown => "pgdn".into(),
        egui::Key::Comma => ",".into(),
        egui::Key::Period => ".".into(),
        egui::Key::Minus => "-".into(),
        egui::Key::Equals => "=".into(),
        egui::Key::OpenBracket => "[".into(),
        egui::Key::CloseBracket => "]".into(),
        egui::Key::Semicolon => ";".into(),
        egui::Key::Quote => "'".into(),
        egui::Key::Slash => "/".into(),
        egui::Key::Backslash => "\\".into(),
        egui::Key::Num0 => "0".into(),
        egui::Key::Num1 => "1".into(),
        egui::Key::Num2 => "2".into(),
        egui::Key::Num3 => "3".into(),
        egui::Key::Num4 => "4".into(),
        egui::Key::Num5 => "5".into(),
        egui::Key::Num6 => "6".into(),
        egui::Key::Num7 => "7".into(),
        egui::Key::Num8 => "8".into(),
        egui::Key::Num9 => "9".into(),
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

    #[test]
    fn options_cancel_reverts_cvars_and_binds_while_apply_emits_video_request() {
        let mut opts = OptionsUi {
            open: true,
            ..Default::default()
        };
        let mut rt = VguiRuntime::new();
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("volume", "0.80");
        cvars.set("sensitivity", "3.00");
        cvars.unbind("k");
        cvars.bind("v", "+voicerecord");

        // Opening Options captures snapshot.
        assert!(run_options_frame(&mut opts, &mut cvars, &mut rt) > 0);
        cvars.set("volume", "0.20");
        cvars.set("hisound", "0");
        cvars.set("room_off", "1");
        cvars.set("sv_aim", "1");
        cvars.set("zoom_sensitivity_ratio", "0.85");
        cvars.set("voice_forcemicrecord", "0");
        cvars.set("cl_allowdownload", "0");
        cvars.set("gl_texturemode", "GL_NEAREST");
        cvars.set("r_software_mode", "1");
        cvars.unbind("v");
        cvars.bind("mouse4", "+voicerecord");

        // Cancel reverts EVERY CVar & bind cleanly.
        opts.cancel_and_close(&mut cvars);
        assert!(!opts.open);
        assert!((cvars.get_f32("volume") - 0.80).abs() < 1e-3);
        assert!(cvars.get_bool("hisound"));
        assert!(!cvars.get_bool("room_off"));
        assert!(!cvars.get_bool("sv_aim"));
        assert!(cvars.get_bool("cl_allowdownload"));
        assert_eq!(cvars.bind_for_key("v"), Some("+voicerecord"));
        assert_eq!(cvars.bind_for_key("mouse4"), None);

        // Re-open, change Video resolution/fullscreen/backend, and Apply.
        opts.open = true;
        opts.tab = OptionsTab::Video;
        opts.set_gpu_info("TestGPU", "Vulkan");
        assert!(run_options_frame(&mut opts, &mut cvars, &mut rt) > 0);
        opts.selected_res_w = 1920;
        opts.selected_res_h = 1080;
        opts.windowed = false;
        opts.backend_idx = 1; // Vulkan
        opts.apply_changes(&mut cvars);

        assert!(opts.save_requested);
        assert_eq!(
            opts.video_apply_requested,
            Some(VideoApplyRequest {
                width: 1920,
                height: 1080,
                fullscreen: true,
                backend: "vulkan".into(),
            })
        );
        assert_eq!(cvars.get_i32("width"), 1920);
        assert_eq!(cvars.get_i32("height"), 1080);
        assert!(cvars.get_bool("fullscreen"));
        assert_eq!(cvars.get_str("r_refdll"), "vulkan");
    }

    #[test]
    fn keyboard_rebind_captures_mouse_buttons_and_wheel_after_arm_frame() {
        let mut opts = OptionsUi {
            open: true,
            tab: OptionsTab::Keyboard,
            rebinding_cmd: Some("+jump".into()),
            rebind_arm_frame: true,
            ..Default::default()
        };
        let mut cvars = CVarRegistry::new_with_defaults();
        let ctx = egui::Context::default();

        // Arm frame ignores the originating mouse1 click.
        let mut raw_arm = egui::RawInput::default();
        raw_arm.events.push(egui::Event::PointerButton {
            pos: Pos2::ZERO,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let mut out = ctx.run_ui(raw_arm, |ui| opts.handle_rebind_input(ui.ctx(), &mut cvars));
        out.textures_delta.clear();
        assert_eq!(opts.rebinding_cmd.as_deref(), Some("+jump"));

        // Next frame captures mouse wheel down -> `mwheeldown`.
        let mut raw_wheel = egui::RawInput::default();
        raw_wheel.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        });
        let mut out = ctx.run_ui(raw_wheel, |ui| {
            opts.handle_rebind_input(ui.ctx(), &mut cvars)
        });
        out.textures_delta.clear();
        assert_eq!(opts.rebinding_cmd, None);
        assert_eq!(cvars.key_for_command("+jump"), Some("mwheeldown"));

        // Capture mouse side button `mouse4`.
        opts.rebinding_cmd = Some("+voicerecord".into());
        let mut raw_btn = egui::RawInput::default();
        raw_btn.events.push(egui::Event::PointerButton {
            pos: Pos2::ZERO,
            button: egui::PointerButton::Extra1,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let mut out = ctx.run_ui(raw_btn, |ui| opts.handle_rebind_input(ui.ctx(), &mut cvars));
        out.textures_delta.clear();
        assert_eq!(cvars.key_for_command("+voicerecord"), Some("mouse4"));
    }
}
