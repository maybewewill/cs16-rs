//! `.res`-driven CS 1.6 black-and-orange VGUI2 TeamMenu (`Teammenu.res`), ClassMenu
//! (`Classmenu_{CT,TER}.res`), and BuyMenu (`MainBuyMenu.res` +
//! `Buy{Pistols,Shotguns,SubMachineguns,Rifles,Machineguns,Equipment}_{CT,TER}.res`)
//! on `BackgroundPanel.res` + `ClientScheme.res` with `classes/<item>.res` previews and
//! `maps/<map>.txt` briefings.

use cs_assets::scheme::{ClassResPanel, LocalizationTable, load_map_briefing};
use cs_assets::vgui_res::{ControlKind, TextAlignment, VguiCoordMode};
use cs_vfs::Vfs;
use egui::{Color32, Key, Pos2, Rect, ScrollArea, Stroke, Vec2};

use crate::ui::vgui2::VguiRuntime;
use crate::ui::vgui2::controls::draw_vgui_image_panel;
use crate::ui::vgui2::runtime::{VguiLayoutNode, modal_backdrop, scheme_color_or};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VguiMenuKind {
    Team,
    ClassCt,
    ClassT,
    Buy,
    BuyPistols,
    BuyShotguns,
    BuySubMachineguns,
    BuyRifles,
    BuyMachineguns,
    BuyEquipment,
}

impl VguiMenuKind {
    /// Map `VGUIMenu` ID (`ReGameDLL_CS cdll_dll.h` / `cs-game/src/menus.rs`:
    /// `VGUI_Menu_Team=2`, `VGUI_Menu_Class_T=26`, `VGUI_Menu_Class_CT=27`,
    /// `VGUI_Menu_Buy=28`, `Buy_Pistol=29`, `Buy_Shotgun=30`, `Buy_Rifle=31`,
    /// `Buy_SubMachineGun=32`, `Buy_MachineGun=33`, `Buy_Item=34`) to a client VGUI2 screen kind.
    pub fn from_vgui_menu_id(menu_id: u8, name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower.contains("team") || menu_id == 2 {
            Some(Self::Team)
        } else if lower.contains("class_ct") || menu_id == 27 {
            Some(Self::ClassCt)
        } else if lower.contains("class_te") || menu_id == 3 || menu_id == 26 {
            Some(Self::ClassT)
        } else if menu_id == 29 || lower.contains("pistol") {
            Some(Self::BuyPistols)
        } else if menu_id == 30 || lower.contains("shotgun") {
            Some(Self::BuyShotguns)
        } else if menu_id == 31 || lower.contains("rifle") {
            Some(Self::BuyRifles)
        } else if menu_id == 32 || lower.contains("submachine") || lower.contains("smg") {
            Some(Self::BuySubMachineguns)
        } else if menu_id == 33 || lower.contains("machinegun") {
            Some(Self::BuyMachineguns)
        } else if menu_id == 34 || lower.contains("equip") || lower.contains("buy_item") {
            Some(Self::BuyEquipment)
        } else if lower.contains("buy") || menu_id == 28 || (4..=10).contains(&menu_id) {
            Some(Self::Buy)
        } else {
            None
        }
    }

    pub fn is_buy(self) -> bool {
        matches!(
            self,
            Self::Buy
                | Self::BuyPistols
                | Self::BuyShotguns
                | Self::BuySubMachineguns
                | Self::BuyRifles
                | Self::BuyMachineguns
                | Self::BuyEquipment
        )
    }

    pub fn res_path(self, is_ct: bool) -> &'static str {
        match self {
            Self::Team => "resource/UI/TeamMenu.res",
            Self::ClassCt => "resource/UI/ClassMenu_CT.res",
            Self::ClassT => "resource/UI/ClassMenu_TER.res",
            Self::Buy => "resource/UI/MainBuyMenu.res",
            Self::BuyPistols => {
                if is_ct {
                    "resource/UI/BuyPistols_CT.res"
                } else {
                    "resource/UI/BuyPistols_TER.res"
                }
            }
            Self::BuyShotguns => {
                if is_ct {
                    "resource/UI/BuyShotguns_CT.res"
                } else {
                    "resource/UI/BuyShotguns_TER.res"
                }
            }
            Self::BuySubMachineguns => {
                if is_ct {
                    "resource/UI/BuySubMachineguns_CT.res"
                } else {
                    "resource/UI/BuySubMachineguns_TER.res"
                }
            }
            Self::BuyRifles => {
                if is_ct {
                    "resource/UI/BuyRifles_CT.res"
                } else {
                    "resource/UI/BuyRifles_TER.res"
                }
            }
            Self::BuyMachineguns => {
                if is_ct {
                    "resource/UI/BuyMachineguns_CT.res"
                } else {
                    "resource/UI/BuyMachineguns_TER.res"
                }
            }
            Self::BuyEquipment => {
                if is_ct {
                    "resource/UI/BuyEquipment_CT.res"
                } else {
                    "resource/UI/BuyEquipment_TER.res"
                }
            }
        }
    }
}

pub struct BuyMenuRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub map_name: &'a str,
    pub is_ct: bool,
    pub allowed_keys: u16,
    pub can_cancel: bool,
    pub money: i32,
}

#[derive(Debug, Clone, Default)]
pub struct BuyClassTeamMenuUi {
    pub active_res: Option<String>,
    pub hovered_class: Option<String>,
    pub focus_index: usize,
    cached_preview: Option<(String, ClassResPanel)>,
    map_scroll_y: f32,
}

impl BuyClassTeamMenuUi {
    pub fn is_open(&self) -> bool {
        self.active_res.is_some()
    }

    pub fn is_buy_menu(&self) -> bool {
        self.active_res.as_deref().is_some_and(|p| {
            let l = p.to_ascii_lowercase();
            l.contains("buymenu") || l.contains("/buy")
        })
    }

    /// Open `kind` without clobbering active `CCSBuyMenu` sub-menu navigation (`MainBuyMenu.res`
    /// -> `BuyRifles_CT.res` etc.) when the server's `VGUIMenu(Buy)` message remains active.
    pub fn open_kind(&mut self, kind: VguiMenuKind, is_ct: bool) {
        if kind == VguiMenuKind::Buy && self.is_buy_menu() {
            return;
        }
        let path = kind.res_path(is_ct);
        if self
            .active_res
            .as_deref()
            .is_none_or(|cur| !cur.eq_ignore_ascii_case(path))
        {
            self.active_res = Some(path.to_string());
            self.hovered_class = None;
            self.focus_index = 0;
            self.map_scroll_y = 0.0;
        }
    }

    /// Force-open `kind` (e.g. a freshly issued `buy` or `buyequip` server menu).
    pub fn open_kind_fresh(&mut self, kind: VguiMenuKind, is_ct: bool) {
        let path = kind.res_path(is_ct);
        self.active_res = Some(path.to_string());
        self.hovered_class = None;
        self.focus_index = 0;
        self.map_scroll_y = 0.0;
    }

    pub fn close(&mut self) {
        self.active_res = None;
        self.hovered_class = None;
        self.focus_index = 0;
        self.map_scroll_y = 0.0;
    }

    /// Resolve `CCSBuyMenu` sub-navigation (both `.res` paths like `Resource/UI/BuyRifles.res`
    /// and bare category commands like `pistols`, `shotguns`, `submachineguns`, `rifles`,
    /// `machineguns`, `equipment`, `buyequip`) into `_CT` / `_TER` pages while keeping the menu
    /// open, or return a server stringcmd (`jointeam 2`, `ak47`, `m4a1`, `vest`, `primammo`,
    /// `secammo`, `autobuy`, `rebuy`, `menuselect N`, etc.) and close the menu.
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
        if let Some(sub_res) = resolve_buy_subpage_command(&lower, is_ct, vfs) {
            self.active_res = Some(sub_res);
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
                let vp = [
                    vp_rect.left(),
                    vp_rect.top(),
                    vp_rect.width(),
                    vp_rect.height(),
                ];
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

                let s = (vp_rect.height() / 480.0).max(0.5);
                let mut info_rect: Option<Rect> = None;
                let mut buttons: Vec<VguiLayoutNode> = Vec::new();
                let mut cancel_cmd: Option<String> = None;
                let client_fonts = runtime.client_scheme.fonts.clone();

                for n in &nodes {
                    if n.field_name.eq_ignore_ascii_case("MapInfo")
                        || n.field_name.eq_ignore_ascii_case("ClassInfo")
                        || n.field_name.eq_ignore_ascii_case("ItemInfo")
                    {
                        if n.rect[2] > 20.0 && n.rect[3] > 20.0 && n.rect[0] < vp_rect.right() {
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
                    match n.kind {
                        ControlKind::Divider => {
                            let div_rect = if n.rect[2] <= n.rect[3] {
                                Rect::from_min_size(
                                    Pos2::new(n.rect[0].round(), n.rect[1].round()),
                                    Vec2::new(1.0, n.rect[3].round().max(1.0)),
                                )
                            } else {
                                Rect::from_min_size(
                                    Pos2::new(n.rect[0].round(), n.rect[1].round()),
                                    Vec2::new(n.rect[2].round().max(1.0), 1.0),
                                )
                            };
                            ui.painter().rect_filled(div_rect, 0.0, border_col);
                        }
                        ControlKind::Label => {
                            if n.label.is_empty() || n.label.starts_with('#') {
                                continue;
                            }
                            let rect = Rect::from_min_size(
                                Pos2::new(n.rect[0], n.rect[1]),
                                Vec2::new(n.rect[2].max(16.0), n.rect[3].max(1.0)),
                            );
                            let is_title = n.font.eq_ignore_ascii_case("title")
                                || n.field_name.eq_ignore_ascii_case("title")
                                || n.field_name.eq_ignore_ascii_case("joinTeam")
                                || n.field_name.eq_ignore_ascii_case("joinClass");
                            let font_name = if is_title { "Title" } else { "Default" };
                            let col = if is_title { title_col } else { label_col };
                            runtime.font_atlas.paint_text_with_scale(
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
                                n.align,
                                false,
                                if is_title { s } else { 1.0 },
                            );
                        }
                        ControlKind::Button | ControlKind::MouseOverPanelButton => {
                            if is_class_menu && is_hidden_cz_class_button(b.vfs, &n.field_name) {
                                continue;
                            }
                            if is_team_menu && is_hidden_team_button(n, b.allowed_keys, b.map_name)
                            {
                                continue;
                            }
                            if n.command.eq_ignore_ascii_case("vguicancel") {
                                if (is_team_menu || is_class_menu) && !b.can_cancel {
                                    continue;
                                }
                                cancel_cmd = Some(n.command.clone());
                            }
                            buttons.push(n.clone());
                        }
                        _ => {}
                    }
                }

                let is_buy_submenu = !is_team_menu && !is_class_menu && !is_main_buy;
                let is_unaffordable = |btn: &VguiLayoutNode| -> bool {
                    is_buy_submenu && btn.cost.is_some_and(|c| b.money < c as i32)
                };

                // Keyboard navigation: Tab / Up / Down, Enter, 1..9, 0 (Cancel), and &A / &R (Autobuy / Rebuy).
                let key_action = ctx.input(|i| {
                    let digit_map = [
                        (Key::Num1, '1'),
                        (Key::Num2, '2'),
                        (Key::Num3, '3'),
                        (Key::Num4, '4'),
                        (Key::Num5, '5'),
                        (Key::Num6, '6'),
                        (Key::Num7, '7'),
                        (Key::Num8, '8'),
                        (Key::Num9, '9'),
                    ];
                    if let Some(ch) = digit_map
                        .into_iter()
                        .find_map(|(k, c)| i.key_pressed(k).then_some(c))
                    {
                        return KeyNav::Hotkey(ch);
                    }
                    if i.key_pressed(Key::Num0) {
                        return KeyNav::Cancel;
                    }
                    if is_main_buy && i.key_pressed(Key::A) {
                        return KeyNav::Hotkey('a');
                    }
                    if is_main_buy && i.key_pressed(Key::R) {
                        return KeyNav::Hotkey('r');
                    }
                    if i.key_pressed(Key::ArrowDown)
                        || (i.key_pressed(Key::Tab) && !i.modifiers.shift)
                    {
                        return KeyNav::Next;
                    }
                    if i.key_pressed(Key::ArrowUp) || (i.key_pressed(Key::Tab) && i.modifiers.shift)
                    {
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
                            self.hovered_class = Some(buttons[self.focus_index].field_name.clone());
                        }
                        KeyNav::Prev => {
                            self.focus_index =
                                (self.focus_index + buttons.len() - 1) % buttons.len();
                            self.hovered_class = Some(buttons[self.focus_index].field_name.clone());
                        }
                        KeyNav::Hotkey(ch) => {
                            if let Some(btn) = find_button_by_hotkey(&buttons, ch, is_team_menu)
                                && btn.enabled
                                && !is_unaffordable(btn)
                                && let Some(send) =
                                    self.handle_command(&btn.command, b.is_ct, b.vfs)
                            {
                                out_cmd = Some(send);
                            }
                        }
                        KeyNav::Cancel => {
                            let cmd = buttons
                                .iter()
                                .find(|btn| extract_vgui_hotkey(&btn.label) == Some('0'))
                                .map(|btn| btn.command.as_str())
                                .or(cancel_cmd.as_deref())
                                .unwrap_or("vguicancel");
                            out_cmd = self.handle_command(cmd, b.is_ct, b.vfs);
                        }
                        KeyNav::Activate => {
                            if let Some(btn) = buttons.get(self.focus_index)
                                && btn.enabled
                                && !is_unaffordable(btn)
                                && let Some(send) =
                                    self.handle_command(&btn.command, b.is_ct, b.vfs)
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
                    let font_name = if btn.font.is_empty() {
                        "Default"
                    } else {
                        btn.font.as_str()
                    };
                    let unaffordable = is_unaffordable(btn);
                    let resp = runtime.draw_client_button_styled(
                        ui,
                        ctx,
                        b.vfs,
                        &format!("buy_btn_{idx}_{}", btn.field_name),
                        rect,
                        &btn.label,
                        btn.default_button,
                        btn.enabled,
                        idx == self.focus_index,
                        unaffordable,
                        font_name,
                        btn.align,
                        screen_h,
                    );
                    if resp.hovered() {
                        self.focus_index = idx;
                        self.hovered_class = Some(btn.field_name.clone());
                    }
                    if btn.enabled
                        && !unaffordable
                        && resp.clicked()
                        && let Some(send) = self.handle_command(&btn.command, b.is_ct, b.vfs)
                    {
                        out_cmd = Some(send);
                    }
                }

                // Right-side info pane (`MapInfo` briefing on TeamMenu, or `ClassInfo`/`ItemInfo` preview).
                if !is_main_buy && let Some(info_box) = info_rect {
                    let clamped_box = Rect::from_min_max(
                        Pos2::new(info_box.left().round(), info_box.top().round()),
                        Pos2::new(
                            info_box.right().min(vp_rect.right() - 48.0 * s).round(),
                            info_box.bottom().min(vp_rect.bottom() - 40.0 * s).round(),
                        ),
                    );
                    if is_team_menu {
                        ui.painter().rect_filled(
                            clamped_box,
                            0.0,
                            Color32::from_rgba_unmultiplied(0, 0, 0, 235),
                        );
                        draw_crisp_border(ui.painter(), clamped_box, border_col);

                        let sb_w = (21.0 * s).round().clamp(18.0, 36.0);
                        let sb_rect = Rect::from_min_max(
                            Pos2::new(clamped_box.right() - sb_w, clamped_box.top() + 1.0),
                            Pos2::new(clamped_box.right() - 1.0, clamped_box.bottom() - 1.0),
                        );
                        ui.painter().rect_filled(
                            sb_rect,
                            0.0,
                            Color32::from_rgba_unmultiplied(0, 0, 0, 220),
                        );
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                sb_rect.min,
                                Pos2::new(sb_rect.left() + 1.0, sb_rect.bottom()),
                            ),
                            0.0,
                            border_col,
                        );
                        let top_arrow = Rect::from_min_size(sb_rect.min, Vec2::splat(sb_w));
                        let bot_arrow = Rect::from_min_size(
                            Pos2::new(sb_rect.left(), sb_rect.bottom() - sb_w),
                            Vec2::splat(sb_w),
                        );
                        draw_scrollbar_arrow(
                            ui,
                            top_arrow,
                            border_col,
                            true,
                            &mut self.map_scroll_y,
                        );
                        draw_scrollbar_arrow(
                            ui,
                            bot_arrow,
                            border_col,
                            false,
                            &mut self.map_scroll_y,
                        );

                        if let Some(v) = b.vfs
                            && let Some(brief) = load_map_briefing(v, b.map_name)
                        {
                            let map_col = scheme_color_or(
                                &runtime.client_scheme,
                                "MapDescriptionText",
                                Color32::from_rgb(255, 176, 0),
                            );
                            let text_rect = Rect::from_min_max(
                                Pos2::new(clamped_box.left() + 6.0, clamped_box.top() + 4.0),
                                Pos2::new(sb_rect.left() - 6.0, clamped_box.bottom() - 4.0),
                            );
                            let line_h = 14.0;
                            let lines = wrap_preview_lines(
                                &mut runtime.font_atlas,
                                ctx,
                                b.vfs,
                                &client_fonts,
                                "Default",
                                screen_h,
                                &brief,
                                text_rect.width(),
                            );
                            let max_scroll =
                                ((lines.len() as f32 * line_h) - text_rect.height()).max(0.0);
                            self.map_scroll_y = self.map_scroll_y.clamp(0.0, max_scroll);
                            let scroll_out = ui.scope_builder(
                                egui::UiBuilder::new().max_rect(text_rect),
                                |ui| {
                                    ScrollArea::vertical()
                                        .id_salt("cs16_team_map_briefing")
                                        .max_height(text_rect.height())
                                        .vertical_scroll_offset(self.map_scroll_y)
                                        .scroll_bar_visibility(
                                            egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                        )
                                        .show(ui, |ui| {
                                            let (alloc, _) = ui.allocate_exact_size(
                                                Vec2::new(
                                                    text_rect.width(),
                                                    (lines.len() as f32 * line_h).max(1.0),
                                                ),
                                                egui::Sense::hover(),
                                            );
                                            let clipped = ui.painter().with_clip_rect(text_rect);
                                            for (idx, line) in lines.iter().enumerate() {
                                                if line.is_empty() {
                                                    continue;
                                                }
                                                let y = (alloc.top() + idx as f32 * line_h).round();
                                                let line_r = Rect::from_min_size(
                                                    Pos2::new(alloc.left(), y),
                                                    Vec2::new(alloc.width(), line_h + 4.0),
                                                );
                                                runtime.font_atlas.paint_text_with_scale(
                                                    &clipped,
                                                    ctx,
                                                    b.vfs,
                                                    &client_fonts,
                                                    "client",
                                                    "Default",
                                                    screen_h,
                                                    line_r,
                                                    line,
                                                    map_col,
                                                    TextAlignment::NorthWest,
                                                    false,
                                                    1.0,
                                                );
                                            }
                                        })
                                },
                            );
                            self.map_scroll_y = scroll_out.inner.state.clamped_offset().y;
                        }
                    } else if let Some(hover) = self.hovered_class.clone().or_else(|| {
                        buttons
                            .get(self.focus_index)
                            .or_else(|| buttons.first())
                            .map(|btn| btn.field_name.clone())
                    }) && let Some(panel) = self.preview_panel(b.vfs, b.loc, &hover)
                    {
                        let origin = clamped_box.min;

                        let bg_r = Rect::from_min_size(
                            Pos2::new(
                                (origin.x + panel.image_box[0] * s).round(),
                                (origin.y + panel.image_box[1] * s).round(),
                            ),
                            Vec2::new(
                                (panel.image_box[2] * s).round(),
                                (panel.image_box[3] * s).round(),
                            ),
                        );
                        if is_class_menu || panel.image_path.is_some() {
                            let img_bg =
                                scheme_color_or(&runtime.client_scheme, "WindowBG", dark_bg);
                            ui.painter().rect_filled(bg_r, 0.0, img_bg);
                            draw_crisp_border(ui.painter(), bg_r, border_col);
                        }
                        if let Some(ref img_path) = panel.image_path
                            && let Some(tex) = runtime.ensure_texture(ctx, b.vfs, img_path)
                        {
                            let img_r = Rect::from_min_size(
                                Pos2::new(
                                    origin.x + panel.image_rect[0] * s,
                                    origin.y + panel.image_rect[1] * s,
                                ),
                                Vec2::new(panel.image_rect[2] * s, panel.image_rect[3] * s),
                            );
                            draw_vgui_image_panel(ui.painter(), img_r, Some(&tex), Color32::WHITE);
                        }
                        for lbl in &panel.labels {
                            let lr = Rect::from_min_size(
                                Pos2::new(origin.x + lbl.x * s, origin.y + lbl.y * s),
                                Vec2::new((lbl.w * s).max(60.0), (lbl.h * s).max(16.0)),
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
                            let fname = if lbl.small_font {
                                "DefaultSmall"
                            } else {
                                "Default"
                            };
                            let max_w = lr.width().min(clamped_box.right() - lr.left()).max(60.0);
                            let min_lh = if lbl.small_font { 12.0 } else { 14.0 };
                            let line_h = runtime
                                .font_atlas
                                .measure_text_with_scale(
                                    ctx,
                                    b.vfs,
                                    &client_fonts,
                                    "client",
                                    fname,
                                    screen_h,
                                    "Ay",
                                    1.0,
                                )
                                .y
                                .max(min_lh);
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
                                runtime.font_atlas.paint_text_with_scale(
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
                                    TextAlignment::NorthWest,
                                    false,
                                    1.0,
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
        let lower = name.to_ascii_lowercase();
        let panel = ClassResPanel::load(v, name, l)
            .or_else(|| ClassResPanel::load(v, &lower, l))
            .or_else(|| {
                let img_path = format!("gfx/vgui/{lower}.tga");
                v.exists(&img_path).then(|| ClassResPanel {
                    image_path: Some(img_path),
                    image_box: [0.0, 0.0, 300.0, 80.0],
                    image_rect: [0.0, 8.0, 256.0, 64.0],
                    labels: Vec::new(),
                })
            })?;
        self.cached_preview = Some((name.to_string(), panel.clone()));
        Some(panel)
    }
}

/// Extract the VGUI2 `&X` hotkey character (lowercased) from a localized button label
/// such as `"&4 RIFLES"` -> `Some('4')`, `"&A AUTO-BUY"` -> `Some('a')`, `"&0 CANCEL"` -> `Some('0')`.
pub fn extract_vgui_hotkey(label: &str) -> Option<char> {
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            match chars.peek().copied() {
                Some('&') => {
                    chars.next();
                }
                Some(next) if next.is_ascii_alphanumeric() => {
                    return Some(next.to_ascii_lowercase());
                }
                _ => {}
            }
        }
    }
    None
}

/// Match a pressed hotkey (`'1'..='9'`, `'a'`, `'r'`) against `buttons` using localized `&X`
/// hotkeys first, falling back to team-slot (`jointeam N`) or positional index only when the
/// menu's labels carry no `&` accelerators.
pub fn find_button_by_hotkey(
    buttons: &[VguiLayoutNode],
    ch: char,
    is_team_menu: bool,
) -> Option<&VguiLayoutNode> {
    let needle = ch.to_ascii_lowercase();
    if let Some(btn) = buttons
        .iter()
        .find(|btn| extract_vgui_hotkey(&btn.label) == Some(needle))
    {
        return Some(btn);
    }
    if needle == 'a' {
        return buttons
            .iter()
            .find(|btn| btn.command.eq_ignore_ascii_case("autobuy"));
    }
    if needle == 'r' {
        return buttons
            .iter()
            .find(|btn| btn.command.eq_ignore_ascii_case("rebuy"));
    }
    if let Some(d) = needle.to_digit(10).filter(|&d| (1..=9).contains(&d)) {
        let slot_num = d as usize;
        let slot_idx = slot_num - 1;
        if is_team_menu {
            return buttons
                .iter()
                .find(|btn| parse_team_slot_from_cmd(&btn.command) == Some(slot_num))
                .or_else(|| buttons.get(slot_idx));
        }
        return buttons.get(slot_idx);
    }
    None
}

fn resolve_buy_subpage_command(lower: &str, is_ct: bool, vfs: Option<&Vfs>) -> Option<String> {
    let suffix = if is_ct { "_ct.res" } else { "_ter.res" };
    let (base_res, is_bare_category) = match lower {
        "pistols" => ("resource/ui/buypistols.res", true),
        "shotguns" => ("resource/ui/buyshotguns.res", true),
        "submachineguns" | "smg" => ("resource/ui/buysubmachineguns.res", true),
        "rifles" => ("resource/ui/buyrifles.res", true),
        "machineguns" => ("resource/ui/buymachineguns.res", true),
        "equipment" | "buyequip" => ("resource/ui/buyequipment.res", true),
        "mainbuymenu" | "buy" => return Some("resource/ui/mainbuymenu.res".to_string()),
        _ if lower.ends_with(".res") => (lower, false),
        _ => return None,
    };
    if base_res.ends_with("_ct.res")
        || base_res.ends_with("_ter.res")
        || base_res.ends_with("mainbuymenu.res")
    {
        return Some(base_res.to_string());
    }
    let candidate_suffixed = base_res.trim_end_matches(".res").to_string() + suffix;
    if let Some(v) = vfs {
        if v.exists(&candidate_suffixed) {
            return Some(candidate_suffixed);
        }
        if v.exists(base_res) {
            return Some(base_res.to_string());
        }
    }
    if is_bare_category {
        Some(candidate_suffixed)
    } else {
        Some(base_res.to_string())
    }
}

fn draw_crisp_border(painter: &egui::Painter, rect: Rect, color: Color32) {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    painter.rect_filled(
        Rect::from_min_max(r.min, Pos2::new(r.right(), r.top() + 1.0)),
        0.0,
        color,
    );
    painter.rect_filled(
        Rect::from_min_max(r.min, Pos2::new(r.left() + 1.0, r.bottom())),
        0.0,
        color,
    );
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(r.left(), r.bottom() - 1.0), r.max),
        0.0,
        color,
    );
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(r.right() - 1.0, r.top()), r.max),
        0.0,
        color,
    );
}

fn draw_scrollbar_arrow(
    ui: &mut egui::Ui,
    rect: Rect,
    border_col: Color32,
    up: bool,
    scroll_y: &mut f32,
) {
    let resp = ui.interact(
        rect,
        ui.make_persistent_id(if up { "cs16_sb_up" } else { "cs16_sb_dn" }),
        egui::Sense::click(),
    );
    if resp.clicked() {
        *scroll_y = (*scroll_y + if up { -28.0 } else { 28.0 }).max(0.0);
    }
    draw_crisp_border(ui.painter(), rect, border_col);
    let half_w = (rect.width() * 0.19).round().max(3.0);
    let half_h = (rect.height() * 0.10).round().max(2.0);
    let cx = rect.center().x.round();
    let cy = rect.center().y.round();
    let pts = if up {
        vec![
            Pos2::new(cx, cy - half_h),
            Pos2::new(cx - half_w, cy + half_h),
            Pos2::new(cx + half_w, cy + half_h),
        ]
    } else {
        vec![
            Pos2::new(cx - half_w, cy - half_h),
            Pos2::new(cx + half_w, cy - half_h),
            Pos2::new(cx, cy + half_h),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        pts,
        Color32::from_rgb(176, 174, 174),
        Stroke::NONE,
    ));
}

#[derive(Debug, Clone, Copy)]
enum KeyNav {
    None,
    Next,
    Prev,
    Activate,
    Cancel,
    Hotkey(char),
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
                .measure_text_with_scale(
                    ctx,
                    vfs,
                    scheme_fonts,
                    "client",
                    font_name,
                    screen_h,
                    &candidate,
                    1.0,
                )
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

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN_BUY_RES: &str = r#"
    "Resource/UI/MainBuyMenu.res"
    {
        "pistols" { "ControlName" "MouseOverPanelButton" "fieldName" "pistols" "xpos" "76" "ypos" "116" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&1 PISTOLS" "command" "Resource/UI/BuyPistols.res" }
        "rifles"  { "ControlName" "MouseOverPanelButton" "fieldName" "rifles"  "xpos" "76" "ypos" "212" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&4 RIFLES"  "command" "Resource/UI/BuyRifles.res" }
        "CancelButton" { "ControlName" "Button" "fieldName" "CancelButton" "xpos" "76" "ypos" "380" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&0 CANCEL" "command" "vguicancel" }
        "AutobuyButton" { "ControlName" "Button" "fieldName" "AutobuyButton" "xpos" "250" "ypos" "380" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&A AUTO-BUY" "command" "autobuy" }
        "RebuyButton"   { "ControlName" "Button" "fieldName" "RebuyButton"   "xpos" "410" "ypos" "380" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&R RE-BUY PREVIOUS" "command" "rebuy" }
    }
    "#;

    const BUY_RIFLES_TER_RES: &str = r#"
    "Resource/UI/BuyRifles_TER.res"
    {
        "galil" { "ControlName" "MouseOverPanelButton" "fieldName" "galil" "xpos" "76" "ypos" "116" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&1 IDF DEFENDER" "command" "galil" "cost" "2000" }
        "ak47"  { "ControlName" "MouseOverPanelButton" "fieldName" "ak47"  "xpos" "76" "ypos" "148" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&2 CV-47"        "command" "ak47"  "cost" "2500" }
        "CancelButton" { "ControlName" "Button" "fieldName" "CancelButton" "xpos" "76" "ypos" "380" "wide" "148" "tall" "20" "visible" "1" "enabled" "1" "labelText" "&0 CANCEL" "command" "vguicancel" }
    }
    "#;

    fn press_key(
        ui: &mut BuyClassTeamMenuUi,
        rt: &mut VguiRuntime,
        vfs: Option<&Vfs>,
        key: Key,
        is_ct: bool,
        money: i32,
    ) -> Option<String> {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
            ..Default::default()
        };
        raw.events.push(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut out_cmd = None;
        let mut out = ctx.run_ui(raw, |egui_ui| {
            out_cmd = ui.show(
                egui_ui.ctx(),
                rt,
                &BuyMenuRenderCtx {
                    vfs,
                    loc: None,
                    map_name: "de_dust2",
                    is_ct,
                    allowed_keys: 0x3FF,
                    can_cancel: true,
                    money,
                },
            );
        });
        out.textures_delta.clear();
        out_cmd
    }

    #[test]
    fn extract_hotkey_parses_ampersand_digits_letters_and_skips_escaped() {
        assert_eq!(extract_vgui_hotkey("&4 RIFLES"), Some('4'));
        assert_eq!(extract_vgui_hotkey("&0 CANCEL"), Some('0'));
        assert_eq!(extract_vgui_hotkey("&A AUTO-BUY"), Some('a'));
        assert_eq!(extract_vgui_hotkey("A && B &2 NEXT"), Some('2'));
        assert_eq!(extract_vgui_hotkey("Plain Label"), None);
    }

    #[test]
    fn b_4_2_navigates_submenus_stays_open_and_enforces_affordability() {
        let vfs = Vfs::from_memory([
            (
                "resource/UI/MainBuyMenu.res".to_string(),
                MAIN_BUY_RES.as_bytes().to_vec(),
            ),
            (
                "resource/UI/BuyRifles_TER.res".to_string(),
                BUY_RIFLES_TER_RES.as_bytes().to_vec(),
            ),
        ]);
        let mut rt = VguiRuntime::new();
        rt.insert_res("resource/UI/MainBuyMenu.res", MAIN_BUY_RES);
        rt.insert_res("resource/UI/BuyRifles_TER.res", BUY_RIFLES_TER_RES);

        let mut menu = BuyClassTeamMenuUi::default();
        menu.open_kind(VguiMenuKind::Buy, false);
        assert!(menu.is_open());

        // Press `4` (`&4 RIFLES`) on MainBuyMenu -> navigates to BuyRifles_TER.res and STAYS OPEN.
        let cmd1 = press_key(&mut menu, &mut rt, Some(&vfs), Key::Num4, false, 800);
        assert_eq!(cmd1, None);
        assert_eq!(
            menu.active_res.as_deref(),
            Some("resource/ui/buyrifles_ter.res")
        );

        // Next frame: server's VGUIMenu(Buy) still active must NOT reset BuyRifles_TER back to MainBuyMenu.
        menu.open_kind(VguiMenuKind::Buy, false);
        assert_eq!(
            menu.active_res.as_deref(),
            Some("resource/ui/buyrifles_ter.res")
        );

        // With $800 (< $2500 AK-47 cost), pressing `2` is blocked and keeps the menu open.
        let blocked = press_key(&mut menu, &mut rt, Some(&vfs), Key::Num2, false, 800);
        assert_eq!(blocked, None);
        assert!(menu.is_open());

        // With $3000 (>= $2500), pressing `2` (`&2 CV-47`) buys `ak47` and closes the menu.
        let bought = press_key(&mut menu, &mut rt, Some(&vfs), Key::Num2, false, 3000);
        assert_eq!(bought.as_deref(), Some("ak47"));
        assert!(!menu.is_open());

        // Bare category commands (`rifles`, `buyequip`) also resolve to team `.res` subpages.
        menu.open_kind(VguiMenuKind::Buy, true);
        assert_eq!(menu.handle_command("rifles", true, None), None);
        assert_eq!(
            menu.active_res.as_deref(),
            Some("resource/ui/buyrifles_ct.res")
        );
        assert_eq!(menu.handle_command("buyequip", true, None), None);
        assert_eq!(
            menu.active_res.as_deref(),
            Some("resource/ui/buyequipment_ct.res")
        );
        assert_eq!(menu.handle_command("vguicancel", true, None), None);
        assert!(!menu.is_open());
    }
}
