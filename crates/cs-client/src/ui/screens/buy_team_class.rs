//! `.res`-driven CS 1.6 black-and-orange VGUI2 TeamMenu (`Teammenu.res`), ClassMenu
//! (`Classmenu_{CT,TER}.res`), and BuyMenu (`MainBuyMenu.res` +
//! `Buy{Pistols,Shotguns,SubMachineguns,Rifles,Machineguns,Equipment}_{CT,TER}.res`)
//! on `BackgroundPanel.res` + `ClientScheme.res` with `classes/<item>.res` previews and
//! `maps/<map>.txt` briefings.

use cs_assets::scheme::{ClassResPanel, LocalizationTable, load_map_briefing};
use cs_assets::vgui_res::{ControlKind, TextAlignment, VguiCoordMode};
use cs_vfs::Vfs;
use egui::{Color32, Key, Pos2, Rect, ScrollArea, Stroke, StrokeKind, Vec2};

use crate::ui::vgui2::controls::draw_vgui_image_panel;
use crate::ui::vgui2::font_atlas::VguiTextDrawOpts;
use crate::ui::vgui2::runtime::{
    VguiLayoutNode, VguiSchemeKind, modal_backdrop, scheme_color_or,
};
use crate::ui::vgui2::{VguiRuntime, paint_html_doc_vgui, parse_html_doc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VguiMenuKind {
    Team,
    ClassCt,
    ClassT,
    Buy,
}

impl VguiMenuKind {
    /// Map `VGUIMenu` ID (`ReGameDLL_CS cdll_dll.h` / `cs-game/src/menus.rs`:
    /// `VGUI_Menu_Team=2`, `VGUI_Menu_Class_T=26`, `VGUI_Menu_Class_CT=27`,
    /// `VGUI_Menu_Buy..=VGUI_Menu_Buy_Item=28..=34`) to a client VGUI2 screen kind.
    pub fn from_vgui_menu_id(menu_id: u8, name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower.contains("team") || menu_id == 2 {
            Some(Self::Team)
        } else if lower.contains("class_ct") || menu_id == 27 {
            Some(Self::ClassCt)
        } else if lower.contains("class_te") || menu_id == 3 || menu_id == 26 {
            Some(Self::ClassT)
        } else if lower.contains("buy") || (28..=34).contains(&menu_id) {
            Some(Self::Buy)
        } else {
            None
        }
    }
}

pub struct BuyMenuRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub map_name: &'a str,
    pub is_ct: bool,
    pub allowed_keys: u16,
}

#[derive(Debug, Clone, Default)]
pub struct BuyClassTeamMenuUi {
    pub active_res: Option<String>,
    pub hovered_class: Option<String>,
    pub focus_index: usize,
    cached_preview: Option<(String, ClassResPanel)>,
}

impl BuyClassTeamMenuUi {
    pub fn open_kind(&mut self, kind: VguiMenuKind, _is_ct: bool) {
        let path = match kind {
            VguiMenuKind::Team => "resource/UI/TeamMenu.res",
            VguiMenuKind::ClassCt => "resource/UI/ClassMenu_CT.res",
            VguiMenuKind::ClassT => "resource/UI/ClassMenu_TER.res",
            VguiMenuKind::Buy => "resource/UI/MainBuyMenu.res",
        };
        if self.active_res.as_deref() != Some(path) {
            self.active_res = Some(path.to_string());
            self.hovered_class = None;
            self.focus_index = 0;
        }
    }

    pub fn close(&mut self) {
        self.active_res = None;
        self.hovered_class = None;
        self.focus_index = 0;
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
            if let Some(v) = vfs
                && v.exists(&candidate_suffixed)
            {
                self.active_res = Some(candidate_suffixed);
                self.hovered_class = None;
                self.focus_index = 0;
                return None;
            }
            self.active_res = Some(lower);
            self.hovered_class = None;
            self.focus_index = 0;
            return None;
        }
        self.close();
        Some(trimmed.to_string())
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        b: &BuyMenuRenderCtx<'_>,
    ) -> Option<String> {
        let res_path = self.active_res.clone()?;
        if let Some(v) = b.vfs {
            runtime.load_res(v, &res_path);
            runtime.load_res(v, "resource/UI/BackgroundPanel.res");
        }

        modal_backdrop(ctx, "cs16_vgui_buy_backdrop");

        let screen = ctx.content_rect();
        let screen_h = screen.height().round().max(480.0) as u32;
        let lower_res = res_path.to_ascii_lowercase();
        let is_team_menu = lower_res.contains("teammenu");
        let is_class_menu = lower_res.contains("classmenu");
        let is_main_buy = lower_res.contains("mainbuymenu");

        let mut out_cmd: Option<String> = None;

        egui::Area::new(egui::Id::unique("cs16_vgui_buy_team_class"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                let vp_rect = runtime.paint_background_panel(ctx, ui.painter(), screen, b.vfs, "");
                let vp = [vp_rect.left(), vp_rect.top(), vp_rect.width(), vp_rect.height()];
                let nodes = runtime.compute_layout_mode(
                    &res_path,
                    vp,
                    VguiCoordMode::Proportional640,
                    b.loc,
                );

                let border_col = scheme_color_or(
                    &runtime.client_scheme,
                    "BorderBright",
                    Color32::from_rgba_unmultiplied(188, 112, 0, 128),
                );
                let dark_bg = scheme_color_or(
                    &runtime.client_scheme,
                    "ControlDarkBG",
                    Color32::from_rgba_unmultiplied(0, 0, 0, 140),
                );
                let title_col = scheme_color_or(
                    &runtime.client_scheme,
                    "TitleText",
                    Color32::from_rgb(255, 174, 0),
                );
                let label_col = scheme_color_or(
                    &runtime.client_scheme,
                    "ControlText",
                    Color32::from_rgb(255, 176, 0),
                );

                let sy = (screen.height() / 480.0).max(0.5);
                let row_step = 32.0 * sy;
                let mut info_rect: Option<Rect> = None;
                let mut buttons: Vec<VguiLayoutNode> = Vec::new();
                let mut hidden_rows = 0usize;
                let mut cancel_cmd: Option<String> = None;
                let client_fonts = runtime.client_scheme.fonts.clone();

                for n in &nodes {
                    if n.field_name.eq_ignore_ascii_case("MapInfo")
                        || n.field_name.eq_ignore_ascii_case("ClassInfo")
                        || n.field_name.eq_ignore_ascii_case("ItemInfo")
                    {
                        if n.rect[2] > 20.0 && n.rect[3] > 20.0 && n.rect[0] < screen.right() {
                            info_rect = Some(Rect::from_min_size(
                                Pos2::new(n.rect[0], n.rect[1]),
                                Vec2::new(n.rect[2], n.rect[3]),
                            ));
                        }
                        continue;
                    }
                    if !n.visible {
                        continue;
                    }
                    let rect = Rect::from_min_size(
                        Pos2::new(n.rect[0], n.rect[1]),
                        Vec2::new(n.rect[2].max(16.0), n.rect[3].max(1.0)),
                    );
                    match n.kind {
                        ControlKind::Divider => {
                            ui.painter().rect_filled(rect, 0.0, border_col);
                        }
                        ControlKind::Label => {
                            if n.label.is_empty() || n.label.starts_with('#') {
                                continue;
                            }
                            let is_title = n.font.eq_ignore_ascii_case("title")
                                || n.field_name.eq_ignore_ascii_case("title")
                                || n.field_name.eq_ignore_ascii_case("joinTeam")
                                || n.field_name.eq_ignore_ascii_case("joinClass");
                            let font_name = if is_title { "Title" } else { "Default" };
                            let col = if is_title { title_col } else { label_col };
                            runtime.font_atlas.paint_text(
                                ui.painter(),
                                ctx,
                                b.vfs,
                                &client_fonts,
                                "client",
                                font_name,
                                screen_h,
                                rect,
                                &n.label,
                                col,
                                VguiTextDrawOpts {
                                    align: n.align,
                                    process_hotkey: false,
                                    proportional_scale: true,
                                },
                            );
                        }
                        ControlKind::Button | ControlKind::MouseOverPanelButton => {
                            if is_class_menu && is_hidden_cz_class_button(b.vfs, &n.field_name) {
                                hidden_rows += 1;
                                continue;
                            }
                            if is_team_menu && is_hidden_team_button(n, b.allowed_keys, b.map_name) {
                                hidden_rows += 1;
                                continue;
                            }
                            if n.command.eq_ignore_ascii_case("vguicancel") {
                                cancel_cmd = Some(n.command.clone());
                            }
                            let mut node = n.clone();
                            if hidden_rows > 0 {
                                node.rect[1] -= hidden_rows as f32 * row_step;
                            }
                            buttons.push(node);
                        }
                        _ => {}
                    }
                }

                // Keyboard navigation: Tab / Up / Down, Enter, 1..9, and 0 (Cancel).
                let key_action = ctx.input(|i| {
                    let digit_map = [
                        (Key::Num1, 0usize),
                        (Key::Num2, 1),
                        (Key::Num3, 2),
                        (Key::Num4, 3),
                        (Key::Num5, 4),
                        (Key::Num6, 5),
                        (Key::Num7, 6),
                        (Key::Num8, 7),
                        (Key::Num9, 8),
                    ];
                    if let Some(idx) = digit_map.into_iter().find_map(|(k, d)| i.key_pressed(k).then_some(d)) {
                        return KeyNav::Digit(idx);
                    }
                    if i.key_pressed(Key::Num0) {
                        return KeyNav::Cancel;
                    }
                    if i.key_pressed(Key::ArrowDown) || (i.key_pressed(Key::Tab) && !i.modifiers.shift) {
                        return KeyNav::Next;
                    }
                    if i.key_pressed(Key::ArrowUp) || (i.key_pressed(Key::Tab) && i.modifiers.shift) {
                        return KeyNav::Prev;
                    }
                    if i.key_pressed(Key::Enter) {
                        return KeyNav::Activate;
                    }
                    KeyNav::None
                });

                if !buttons.is_empty() {
                    if self.focus_index >= buttons.len() {
                        self.focus_index = 0;
                    }
                    match key_action {
                        KeyNav::Next => {
                            self.focus_index = (self.focus_index + 1) % buttons.len();
                        }
                        KeyNav::Prev => {
                            self.focus_index = (self.focus_index + buttons.len() - 1) % buttons.len();
                        }
                        KeyNav::Digit(slot_idx) => {
                            let digit_num = slot_idx + 1;
                            let target = if is_team_menu {
                                buttons
                                    .iter()
                                    .find(|btn| parse_team_slot_from_cmd(&btn.command) == Some(digit_num))
                                    .or_else(|| buttons.get(slot_idx))
                            } else {
                                buttons.get(slot_idx)
                            };
                            if let Some(btn) = target
                                && btn.enabled
                                && let Some(send) = self.handle_command(&btn.command, b.is_ct, b.vfs)
                            {
                                out_cmd = Some(send);
                            }
                        }
                        KeyNav::Cancel => {
                            let cmd = cancel_cmd.as_deref().unwrap_or("vguicancel");
                            out_cmd = self.handle_command(cmd, b.is_ct, b.vfs);
                        }
                        KeyNav::Activate => {
                            if let Some(btn) = buttons.get(self.focus_index)
                                && btn.enabled
                                && let Some(send) = self.handle_command(&btn.command, b.is_ct, b.vfs)
                            {
                                out_cmd = Some(send);
                            }
                        }
                        KeyNav::None => {}
                    }
                }

                for (idx, btn) in buttons.iter().enumerate() {
                    let rect = Rect::from_min_size(
                        Pos2::new(btn.rect[0], btn.rect[1]),
                        Vec2::new(btn.rect[2].max(16.0), btn.rect[3].max(18.0)),
                    );
                    let font_name = if btn.font.is_empty() { "Default" } else { btn.font.as_str() };
                    let resp = runtime.draw_client_button(
                        ui,
                        ctx,
                        b.vfs,
                        &format!("buy_btn_{idx}_{}", btn.field_name),
                        rect,
                        &btn.label,
                        btn.default_button,
                        btn.enabled,
                        idx == self.focus_index,
                        font_name,
                        btn.align,
                        screen_h,
                    );
                    if resp.hovered() {
                        self.focus_index = idx;
                        self.hovered_class = Some(btn.field_name.clone());
                    }
                    if btn.enabled && resp.clicked()
                        && let Some(send) = self.handle_command(&btn.command, b.is_ct, b.vfs)
                    {
                        out_cmd = Some(send);
                    }
                }

                // Right-side info pane (`MapInfo` HTML briefing on TeamMenu, or `ClassInfo`/`ItemInfo` preview).
                if !is_main_buy && let Some(info_box) = info_rect {
                    let clamped_box = Rect::from_min_max(
                        info_box.min,
                        Pos2::new(
                            info_box.right().min(screen.right() - 48.0),
                            info_box.bottom().min(screen.bottom() - 40.0),
                        ),
                    );
                    if is_team_menu {
                        ui.painter().rect_filled(clamped_box, 0.0, dark_bg);
                        ui.painter().rect_stroke(
                            clamped_box,
                            0.0,
                            Stroke::new(1.0, border_col),
                            StrokeKind::Inside,
                        );
                        if let Some(v) = b.vfs
                            && let Some(brief) = load_map_briefing(v, b.map_name)
                        {
                            let mut html = parse_html_doc(&brief.replace('\n', "<br>"));
                            html.text_color = runtime
                                .client_scheme
                                .resolve_color("MapDescriptionText")
                                .unwrap_or([255, 176, 0, 255]);
                            let inner = clamped_box.shrink(8.0);
                            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                                ScrollArea::vertical()
                                    .id_salt("cs16_team_map_briefing")
                                    .max_height(inner.height())
                                    .show(ui, |ui| {
                                        paint_html_doc_vgui(
                                            ui,
                                            &html,
                                            runtime,
                                            b.vfs,
                                            VguiSchemeKind::Client,
                                            screen_h,
                                        );
                                    });
                            });
                        }
                    } else if let Some(hover) = self
                        .hovered_class
                        .clone()
                        .or_else(|| buttons.first().map(|btn| btn.field_name.clone()))
                        && let Some(panel) = self.preview_panel(b.vfs, b.loc, &hover)
                    {
                        let sx = (screen.width() / 640.0).max(0.5);
                        let sy = (screen.height() / 480.0).max(0.5);
                        let origin = clamped_box.min;

                        let bg_r = Rect::from_min_size(
                            Pos2::new(origin.x + panel.image_box[0] * sx, origin.y + panel.image_box[1] * sy),
                            Vec2::new(panel.image_box[2] * sx, panel.image_box[3] * sy),
                        );
                        if is_class_menu || panel.image_path.is_some() {
                            ui.painter().rect_filled(bg_r, 0.0, dark_bg);
                            ui.painter().rect_stroke(
                                bg_r,
                                0.0,
                                Stroke::new(1.0, border_col),
                                StrokeKind::Inside,
                            );
                        }
                        if let Some(ref img_path) = panel.image_path
                            && let Some(tex) = runtime.ensure_texture(ctx, b.vfs, img_path)
                        {
                            let img_r = Rect::from_min_size(
                                Pos2::new(
                                    origin.x + panel.image_rect[0] * sx,
                                    origin.y + panel.image_rect[1] * sy,
                                ),
                                Vec2::new(panel.image_rect[2] * sx, panel.image_rect[3] * sy),
                            );
                            draw_vgui_image_panel(ui.painter(), img_r, Some(&tex), Color32::WHITE);
                        }
                        for lbl in &panel.labels {
                            let lr = Rect::from_min_size(
                                Pos2::new(origin.x + lbl.x * sx, origin.y + lbl.y * sy),
                                Vec2::new((lbl.w * sx).max(60.0), (lbl.h * sy).max(16.0)),
                            );
                            let col = if lbl.bright {
                                title_col
                            } else {
                                scheme_color_or(
                                    &runtime.client_scheme,
                                    "LabelDimText",
                                    Color32::from_rgba_unmultiplied(255, 176, 0, 200),
                                )
                            };
                            let fname = if lbl.small_font { "DefaultSmall" } else { "Default" };
                            let max_w = lr.width().min(clamped_box.right() - lr.left()).max(60.0);
                            let line_h = runtime
                                .font_atlas
                                .measure_text(ctx, b.vfs, &client_fonts, "client", fname, screen_h, "Ay")
                                .y
                                .max(14.0);
                            let lines = wrap_preview_lines(
                                &mut runtime.font_atlas,
                                ctx,
                                b.vfs,
                                &client_fonts,
                                fname,
                                screen_h,
                                &lbl.text,
                                max_w,
                            );
                            for (line_idx, line) in lines.iter().enumerate() {
                                let y = lr.top() + line_idx as f32 * line_h;
                                if y + line_h > clamped_box.bottom() {
                                    break;
                                }
                                let line_rect = Rect::from_min_size(
                                    Pos2::new(lr.left(), y),
                                    Vec2::new(max_w, line_h),
                                );
                                runtime.font_atlas.paint_text(
                                    ui.painter(),
                                    ctx,
                                    b.vfs,
                                    &client_fonts,
                                    "client",
                                    fname,
                                    screen_h,
                                    line_rect,
                                    line,
                                    col,
                                    VguiTextDrawOpts {
                                        align: TextAlignment::NorthWest,
                                        process_hotkey: false,
                                        proportional_scale: false,
                                    },
                                );
                            }
                        }
                    }
                }
            });

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

#[derive(Debug, Clone, Copy)]
enum KeyNav {
    None,
    Next,
    Prev,
    Activate,
    Cancel,
    Digit(usize),
}

fn parse_team_slot_from_cmd(cmd: &str) -> Option<usize> {
    let mut parts = cmd.split_whitespace();
    let verb = parts.next()?;
    if verb.eq_ignore_ascii_case("jointeam") {
        parts.next()?.parse::<usize>().ok()
    } else {
        None
    }
}

/// Hide Condition Zero-only class buttons unless their model exists in the active VFS.
fn is_hidden_cz_class_button(vfs: Option<&Vfs>, field_name: &str) -> bool {
    let model = if field_name.eq_ignore_ascii_case("spetsnaz") {
        "models/player/spetsnaz/spetsnaz.mdl"
    } else if field_name.eq_ignore_ascii_case("militia") {
        "models/player/militia/militia.mdl"
    } else {
        return false;
    };
    !vfs.is_some_and(|v| v.exists(model))
}

/// Filter TeamMenu buttons by server `allowed_keys` bitmask or `vipbutton` on non-`as_` maps.
fn is_hidden_team_button(node: &VguiLayoutNode, allowed_keys: u16, map_name: &str) -> bool {
    if node.field_name.eq_ignore_ascii_case("vipbutton") {
        return if allowed_keys != 0 {
            (allowed_keys & (1u16 << 2)) == 0
        } else {
            !map_name.to_ascii_lowercase().starts_with("as_")
        };
    }
    let Some(slot) = parse_team_slot_from_cmd(&node.command) else {
        return false;
    };
    slot < 10 && allowed_keys != 0 && (allowed_keys & (1u16 << (slot - 1))) == 0
}

/// Split `text` on newlines and word-wrap each paragraph to fit within `max_width`.
#[allow(clippy::too_many_arguments)]
fn wrap_preview_lines(
    font_atlas: &mut crate::ui::vgui2::font_atlas::VguiFontAtlas,
    ctx: &egui::Context,
    vfs: Option<&Vfs>,
    scheme_fonts: &cs_assets::vgui_font::SchemeFontSet,
    font_name: &str,
    screen_h: u32,
    text: &str,
    max_width: f32,
) -> Vec<String> {
    let mut out = Vec::new();
    for raw_para in text.split('\n') {
        let para = raw_para.trim_end_matches('\r');
        if para.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in para.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
                continue;
            }
            let candidate = format!("{current} {word}");
            let w = font_atlas
                .measure_text(ctx, vfs, scheme_fonts, "client", font_name, screen_h, &candidate)
                .x;
            if w <= max_width {
                current = candidate;
            } else {
                out.push(current);
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}
