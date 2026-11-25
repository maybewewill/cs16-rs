//! Ergonomic builders for classic ShowMenu screens, host-managed [`PaginatedMenu`]s,
//! [`CustomWeaponBuilder`], and authentic CS 1.6 VGUI2 / GoldSrc 0px-corner [`HudCanvas`].

use cs_mod_api::{CustomWeaponSpec, HudDrawCmd, MenuItemSpec};

use crate::bridge::HostBridge;

/// Classic AMX Mod X `newmenus.inc` menu property keys (`MPROP_*`) matching `cs-mod-api::services::mprop`.
pub mod mprop {
    pub const PERPAGE: i32 = 1;
    pub const BACKNAME: i32 = 2;
    pub const NEXTNAME: i32 = 3;
    pub const EXITNAME: i32 = 4;
    pub const TITLE: i32 = 5;
    pub const EXIT: i32 = 6;
    pub const NOCOLORS: i32 = 7;
    pub const NUMBER_COLOR: i32 = 8;
    pub const SHOWPAGE: i32 = 9;
    pub const MEXIT_ALL: i32 = 1;
    pub const MEXIT_NEVER: i32 = -1;
}

/// Sentinel `item_index` passed to `cs_mod_menu` when a player presses `0. Exit`.
pub const MENU_EXIT: i32 = -3;

/// Authentic Valve Steam 2003 / CS 1.6 `TrackerScheme.res` VGUI2 & GoldSrc HUD RGBA palette
/// (strict 0px sharp corners, olive-drab panels, 1px bevel borders, amber HUD accents).
pub mod vgui {
    pub const CONTROL_BG: [u8; 4] = [76, 88, 68, 235];
    pub const CONTROL_DARK_BG: [u8; 4] = [90, 106, 80, 235];
    pub const WINDOW_BG: [u8; 4] = [62, 70, 55, 240];
    pub const SELECTION_BG: [u8; 4] = [149, 136, 49, 255];
    pub const BORDER_BRIGHT: [u8; 4] = [136, 145, 128, 255];
    pub const BORDER_DARK: [u8; 4] = [40, 46, 34, 255];
    pub const BASE_TEXT: [u8; 4] = [216, 222, 211, 255];
    pub const BRIGHT_TEXT: [u8; 4] = [255, 255, 255, 255];
    pub const DIM_TEXT: [u8; 4] = [160, 170, 149, 255];
    pub const BRIGHT_CONTROL_TEXT: [u8; 4] = [196, 181, 80, 255];
    pub const HUD_AMBER: [u8; 4] = [255, 160, 0, 255];
    pub const HUD_RED: [u8; 4] = [255, 64, 64, 255];
    pub const HUD_GREEN: [u8; 4] = [116, 212, 116, 255];
    pub const HUD_CT_BLUE: [u8; 4] = [153, 204, 255, 255];
}

/// Fluent builder for classic raw GoldSrc `ShowMenu` screens (v1 compatible).
#[derive(Debug, Default, Clone)]
pub struct MenuBuilder {
    title: String,
    items: Vec<(u8, String)>,
    time_sec: i8,
}

impl MenuBuilder {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
            time_sec: 15,
        }
    }

    pub fn display_time(mut self, seconds: i8) -> Self {
        self.time_sec = seconds;
        self
    }

    pub fn item(mut self, key_1_to_9: u8, label: impl Into<String>) -> Self {
        if (1..=9).contains(&key_1_to_9) {
            self.items.push((key_1_to_9, label.into()));
        }
        self
    }

    pub fn build(&self) -> (u16, i8, String) {
        let mut mask = 0u16;
        let mut text = format!("\\y{}\\w\n", self.title);
        for (k, label) in &self.items {
            mask |= 1u16 << (k - 1);
            text.push_str(&format!("{k}. {label}\n"));
        }
        mask |= 1u16 << 9;
        text.push_str("\n0. Exit\n");
        (mask, self.time_sec, text)
    }

    pub fn send_to(&self, bridge: &mut impl HostBridge, slot: u8) {
        let (mask, time_sec, text) = self.build();
        bridge.show_menu(slot, mask, time_sec, &text);
    }
}

/// Fluent builder backed by the host `MenuManager` (`1..7`, `8. Back`, `9. More`, `0. Exit`),
/// automatically isolating menu ownership per player slot across all loaded plugins.
#[derive(Debug, Clone)]
pub struct PaginatedMenu {
    title: String,
    callback_id: i32,
    items: Vec<MenuItemSpec>,
    per_page: Option<u8>,
    allow_exit: bool,
    back_label: Option<String>,
    next_label: Option<String>,
    exit_label: Option<String>,
    number_color: Option<String>,
}

impl PaginatedMenu {
    pub fn new(title: impl Into<String>, callback_id: i32) -> Self {
        Self {
            title: title.into(),
            callback_id,
            items: Vec::new(),
            per_page: None,
            allow_exit: true,
            back_label: None,
            next_label: None,
            exit_label: None,
            number_color: None,
        }
    }

    pub fn item(mut self, label: impl Into<String>, info: impl Into<String>) -> Self {
        self.items.push(MenuItemSpec::selectable(label, info, true));
        self
    }

    pub fn item_enabled(
        mut self,
        label: impl Into<String>,
        info: impl Into<String>,
        enabled: bool,
    ) -> Self {
        self.items
            .push(MenuItemSpec::selectable(label, info, enabled));
        self
    }

    pub fn blank(mut self) -> Self {
        self.items.push(MenuItemSpec::blank());
        self
    }

    pub fn text(mut self, line: impl Into<String>) -> Self {
        self.items.push(MenuItemSpec::text(line));
        self
    }

    pub fn per_page(mut self, count: u8) -> Self {
        self.per_page = Some(count);
        self
    }

    pub fn allow_exit(mut self, allow: bool) -> Self {
        self.allow_exit = allow;
        self
    }

    pub fn back_label(mut self, label: impl Into<String>) -> Self {
        self.back_label = Some(label.into());
        self
    }

    pub fn next_label(mut self, label: impl Into<String>) -> Self {
        self.next_label = Some(label.into());
        self
    }

    pub fn exit_label(mut self, label: impl Into<String>) -> Self {
        self.exit_label = Some(label.into());
        self
    }

    pub fn number_color(mut self, escape: impl Into<String>) -> Self {
        self.number_color = Some(escape.into());
        self
    }

    /// Registers this menu in the host `MenuManager` and returns its positive `menu_id` (`None` on failure).
    pub fn create(&self, host: &mut impl HostBridge) -> Option<u32> {
        let raw_id = host.menu_create(&self.title, self.callback_id);
        let id = u32::try_from(raw_id).ok()?;
        for item in &self.items {
            if item.is_blank {
                host.menu_add_blank(id);
            } else if item.is_text {
                host.menu_add_text(id, &item.label);
            } else {
                host.menu_add_item(id, &item.label, &item.info, item.enabled);
            }
        }
        if let Some(pp) = self.per_page {
            host.menu_set_prop_int(id, mprop::PERPAGE, i32::from(pp));
        }
        if !self.allow_exit {
            host.menu_set_prop_int(id, mprop::EXIT, mprop::MEXIT_NEVER);
        }
        if let Some(ref s) = self.back_label {
            host.menu_set_prop_str(id, mprop::BACKNAME, s);
        }
        if let Some(ref s) = self.next_label {
            host.menu_set_prop_str(id, mprop::NEXTNAME, s);
        }
        if let Some(ref s) = self.exit_label {
            host.menu_set_prop_str(id, mprop::EXITNAME, s);
        }
        if let Some(ref s) = self.number_color {
            host.menu_set_prop_str(id, mprop::NUMBER_COLOR, s);
        }
        Some(id)
    }

    /// Creates and immediately displays this menu to `slot` starting on `page` (`time_sec = -1` for permanent).
    pub fn display_to(
        &self,
        host: &mut impl HostBridge,
        slot: u8,
        page: u16,
        time_sec: i8,
    ) -> Option<u32> {
        let id = self.create(host)?;
        if host.menu_display(slot, id, page, time_sec) >= 0 {
            Some(id)
        } else {
            None
        }
    }
}

/// Fluent builder for declarative custom weapons (`register_custom_weapon` / `give_custom_weapon`).
#[derive(Debug, Clone)]
pub struct CustomWeaponBuilder {
    spec: CustomWeaponSpec,
}

impl CustomWeaponBuilder {
    pub fn new(id_name: impl Into<String>, base_weapon: u8) -> Self {
        let id = id_name.into();
        let spec = CustomWeaponSpec {
            display_name: id.clone(),
            id_name: id,
            base_weapon,
            ..CustomWeaponSpec::default()
        };
        Self { spec }
    }

    pub fn display_name(mut self, name: impl Into<String>) -> Self {
        self.spec.display_name = name.into();
        self
    }

    pub fn v_model(mut self, path: impl Into<String>) -> Self {
        self.spec.v_model = Some(path.into());
        self
    }

    pub fn p_model(mut self, path: impl Into<String>) -> Self {
        self.spec.p_model = Some(path.into());
        self
    }

    pub fn w_model(mut self, path: impl Into<String>) -> Self {
        self.spec.w_model = Some(path.into());
        self
    }

    pub fn fire_sound(mut self, sample: impl Into<String>) -> Self {
        self.spec.fire_sound = Some(sample.into());
        self
    }

    pub fn damage_mult(mut self, mult: f32) -> Self {
        self.spec.damage_mult = mult;
        self
    }

    pub fn clip_size(mut self, clip: i32) -> Self {
        self.spec.clip_size = Some(clip.max(0));
        self
    }

    pub fn deploy_speed_mult(mut self, mult: f32) -> Self {
        self.spec.deploy_speed_mult = mult;
        self
    }

    pub fn recoil_mult(mut self, mult: f32) -> Self {
        self.spec.recoil_mult = mult;
        self
    }

    pub fn knockback(mut self, kb: f32) -> Self {
        self.spec.knockback = kb;
        self
    }

    pub fn cost(mut self, cost: i32) -> Self {
        self.spec.cost = cost.max(0);
        self
    }

    pub fn build(self) -> CustomWeaponSpec {
        self.spec
    }

    pub fn register(self, host: &mut impl HostBridge) -> i32 {
        host.register_custom_weapon(&self.spec)
    }
}

/// Immediate-mode HUD builder collecting [`HudDrawCmd`]s for client mods with authentic
/// CS 1.6 VGUI2 / GoldSrc 0px sharp-corner bevel styling helpers.
#[derive(Debug, Default, Clone)]
pub struct HudCanvas {
    cmds: Vec<HudDrawCmd>,
}

impl HudCanvas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) {
        self.cmds.push(HudDrawCmd::Rect { x, y, w, h, rgba });
    }

    pub fn panel(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4], title: impl Into<String>) {
        self.cmds.push(HudDrawCmd::Panel {
            x,
            y,
            w,
            h,
            rgba,
            title: title.into(),
        });
    }

    /// Draws a classic Steam 2003 / CS 1.6 VGUI2 0px-corner olive box with crisp 1px raised bevel
    /// edges (`BORDER_BRIGHT` top/left, `BORDER_DARK` bottom/right) and optional header label.
    pub fn vgui_box(&mut self, x: f32, y: f32, w: f32, h: f32, title: &str) {
        let px = 0.0015;
        let py = 0.0025;
        // Dark outer shadow edge (right + bottom)
        self.rect(x, y + h - py, w, py, vgui::BORDER_DARK);
        self.rect(x + w - px, y, px, h, vgui::BORDER_DARK);
        // Bright highlight edge (top + left)
        self.rect(x, y, w, py, vgui::BORDER_BRIGHT);
        self.rect(x, y, px, h, vgui::BORDER_BRIGHT);
        // Olive VGUI2 window body
        self.panel(x + px, y + py, (w - 2.0 * px).max(0.0), (h - 2.0 * py).max(0.0), vgui::CONTROL_BG, title);
    }

    pub fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, fraction: f32, rgba: [u8; 4]) {
        self.cmds.push(HudDrawCmd::ProgressBar {
            x,
            y,
            w,
            h,
            fraction: fraction.clamp(0.0, 1.0),
            rgba,
        });
    }

    /// Draws a recessed 0px-corner VGUI2 meter background + fill bar.
    pub fn vgui_progress_bar(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fraction: f32,
        fill_rgba: [u8; 4],
    ) {
        self.rect(x, y, w, h, vgui::WINDOW_BG);
        self.bar(x, y, w, h, fraction, fill_rgba);
    }

    pub fn text(&mut self, x: f32, y: f32, size: f32, rgba: [u8; 4], text: impl Into<String>) {
        self.cmds.push(HudDrawCmd::Text {
            x,
            y,
            size,
            rgba,
            text: text.into(),
        });
    }

    /// Draws classic CS 1.6 additive HUD amber text (`#FFA000`).
    pub fn amber_text(&mut self, x: f32, y: f32, size: f32, text: impl Into<String>) {
        self.text(x, y, size, vgui::HUD_AMBER, text);
    }

    /// Replays all buffered draw commands directly into a [`HostBridge`] (e.g. `WasmHostBridge`).
    pub fn flush(&self, host: &mut impl HostBridge) {
        for cmd in &self.cmds {
            match cmd {
                HudDrawCmd::Rect { x, y, w, h, rgba } => {
                    host.hud_draw_rect(*x, *y, *w, *h, *rgba);
                }
                HudDrawCmd::Text {
                    x,
                    y,
                    size,
                    rgba,
                    text,
                } => {
                    host.hud_draw_text(*x, *y, *size, *rgba, text);
                }
                HudDrawCmd::ProgressBar {
                    x,
                    y,
                    w,
                    h,
                    fraction,
                    rgba,
                } => {
                    host.hud_draw_bar(*x, *y, *w, *h, *fraction, *rgba);
                }
                HudDrawCmd::Panel {
                    x,
                    y,
                    w,
                    h,
                    rgba,
                    title,
                } => {
                    host.hud_panel(*x, *y, *w, *h, *rgba, title);
                }
            }
        }
    }

    pub fn cmds(&self) -> &[HudDrawCmd] {
        &self.cmds
    }

    pub fn finish(self) -> Vec<HudDrawCmd> {
        self.cmds
    }
}
