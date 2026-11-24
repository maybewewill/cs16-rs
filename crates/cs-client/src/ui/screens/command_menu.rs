//! CS 1.6 `commandmenu.txt` (`+commandmenu` / `H`) cascading `ClientScheme.res` command menu.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_assets::vgui_scripts::{CommandMenuAction, CommandMenuItem, CommandMenuTree};
use cs_vfs::Vfs;
use egui::{Color32, Key, Pos2, Stroke, Vec2};

use crate::ui::vgui2::runtime::{VguiRuntime, scheme_color_or};

#[derive(Default)]
pub struct CommandMenuUi {
    pub open: bool,
    pub tree: CommandMenuTree,
    submenu_path: Vec<usize>,
    pub pending_briefing: bool,
}

impl CommandMenuUi {
    pub fn load_from_vfs(&mut self, vfs: &Vfs) {
        if let Some(tree) = CommandMenuTree::load(vfs, "commandmenu.txt") {
            self.tree = tree;
        }
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        if !open {
            self.submenu_path.clear();
        }
    }

    pub fn take_pending_briefing(&mut self) -> bool {
        std::mem::take(&mut self.pending_briefing)
    }

    pub fn visible_items<'a>(&'a self, map_name: &str) -> Vec<(usize, &'a CommandMenuItem)> {
        self.visible_at_depth(&[], map_name)
    }

    fn visible_at_depth<'a>(
        &'a self,
        path: &[usize],
        map_name: &str,
    ) -> Vec<(usize, &'a CommandMenuItem)> {
        let mut slice: &[CommandMenuItem] = &self.tree.items;
        for &idx in path {
            let Some(item) = slice.get(idx) else {
                break;
            };
            match &item.action {
                CommandMenuAction::Submenu(children) => slice = children,
                CommandMenuAction::Command(_) => break,
            }
        }
        let clean_map = map_name.strip_suffix(".bsp").unwrap_or(map_name);
        slice
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.map_filter
                    .as_ref()
                    .is_none_or(|m| m.eq_ignore_ascii_case(clean_map))
            })
            .collect()
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        vfs: Option<&Vfs>,
        loc: Option<&LocalizationTable>,
        map_name: &str,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        let screen = ctx.content_rect();
        let screen_h = screen.height().round().max(480.0) as u32;
        let bg_col = scheme_color_or(
            &runtime.client_scheme,
            "Menu/BgColor",
            Color32::from_rgba_unmultiplied(0, 0, 0, 227),
        );
        let border_col = scheme_color_or(
            &runtime.client_scheme,
            "BorderBright",
            Color32::from_rgba_unmultiplied(188, 112, 0, 140),
        );

        let mut out_cmd: Option<String> = None;
        let mut update_depth: Option<(usize, usize)> = None;
        let mut close_now = ctx.input(|i| i.key_pressed(Key::Num0));

        // Keyboard 1..9 activates the active (deepest open) column item.
        let digit = ctx.input(|i| {
            [
                (Key::Num1, b'1'),
                (Key::Num2, b'2'),
                (Key::Num3, b'3'),
                (Key::Num4, b'4'),
                (Key::Num5, b'5'),
                (Key::Num6, b'6'),
                (Key::Num7, b'7'),
                (Key::Num8, b'8'),
                (Key::Num9, b'9'),
            ]
            .into_iter()
            .find_map(|(k, d)| i.key_pressed(k).then_some(char::from(d)))
        });
        if let Some(d_ch) = digit {
            let active_items = self.visible_at_depth(&self.submenu_path, map_name);
            if let Some((idx, it)) = active_items
                .into_iter()
                .find(|(_, it)| it.key.starts_with(d_ch))
            {
                match &it.action {
                    CommandMenuAction::Submenu(_) => {
                        let depth = self.submenu_path.len();
                        update_depth = Some((depth, idx));
                    }
                    CommandMenuAction::Command(cmd) => {
                        if cmd == "!MAPBRIEFING" {
                            self.pending_briefing = true;
                        } else {
                            out_cmd = Some(cmd.clone());
                        }
                        close_now = true;
                    }
                }
            }
        }

        let depth_count = self.submenu_path.len() + 1;
        let col_w = 210.0f32;
        let mut base_y = (screen.center().y - 120.0).max(24.0);

        for depth in 0..depth_count {
            let prefix = &self.submenu_path[..depth];
            let items: Vec<(usize, CommandMenuItem)> = self
                .visible_at_depth(prefix, map_name)
                .into_iter()
                .map(|(i, it)| (i, it.clone()))
                .collect();
            if items.is_empty() {
                break;
            }
            let pos = Pos2::new(18.0 + depth as f32 * (col_w + 4.0), base_y);
            let selected_sub = self.submenu_path.get(depth).copied();

            egui::Area::new(egui::Id::unique(("cs16_command_menu_col", depth)))
                .order(egui::Order::Foreground)
                .fixed_pos(pos)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(bg_col)
                        .stroke(Stroke::new(1.0, border_col))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            for (row_idx, (orig_idx, item)) in items.iter().enumerate() {
                                let label = loc.map_or(item.label.as_str(), |l| l.resolve(&item.label));
                                let clean = if label.starts_with('#') { &item.label } else { label };
                                let suffix = match &item.action {
                                    CommandMenuAction::Submenu(_) => "  >",
                                    CommandMenuAction::Command(_) => "",
                                };
                                let text = format!("{}. {clean}{suffix}", item.key);
                                let (rect, _) = ui.allocate_exact_size(Vec2::new(col_w, 22.0), egui::Sense::hover());
                                let is_sel = selected_sub == Some(*orig_idx);
                                let resp = runtime.draw_client_button(
                                    ui,
                                    ctx,
                                    vfs,
                                    &format!("cmdm_{depth}_{row_idx}"),
                                    rect,
                                    &text,
                                    false,
                                    true,
                                    is_sel,
                                    "DefaultSmall",
                                    TextAlignment::West,
                                    screen_h,
                                );
                                match &item.action {
                                    CommandMenuAction::Submenu(_) => {
                                        if resp.hovered() || resp.clicked() {
                                            update_depth = Some((depth, *orig_idx));
                                            base_y = rect.top();
                                        }
                                    }
                                    CommandMenuAction::Command(cmd) => {
                                        if resp.clicked() {
                                            if cmd == "!MAPBRIEFING" {
                                                self.pending_briefing = true;
                                            } else {
                                                out_cmd = Some(cmd.clone());
                                            }
                                            close_now = true;
                                        }
                                    }
                                }
                            }
                        });
                });
        }

        if let Some((depth, idx)) = update_depth {
            self.submenu_path.truncate(depth);
            self.submenu_path.push(idx);
        }
        if close_now {
            self.set_open(false);
        }
        out_cmd
    }
}
