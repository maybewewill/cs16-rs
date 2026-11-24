//! CS 1.6 `commandmenu.txt` (`+commandmenu` / `H`) nested popup command menu.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_scripts::{CommandMenuAction, CommandMenuItem, CommandMenuTree};
use cs_vfs::Vfs;
use egui::{CornerRadius, Pos2, RichText, Stroke, Vec2};

use crate::ui::vgui_theme::VguiPalette;

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

    pub fn visible_items<'a>(
        &'a self,
        map_name: &str,
    ) -> Vec<(usize, &'a CommandMenuItem)> {
        let mut slice: &[CommandMenuItem] = &self.tree.items;
        for &idx in &self.submenu_path {
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
        loc: Option<&LocalizationTable>,
        map_name: &str,
        palette: VguiPalette,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        let items: Vec<(usize, CommandMenuItem)> = self
            .visible_items(map_name)
            .into_iter()
            .map(|(i, it)| (i, it.clone()))
            .collect();
        if items.is_empty() {
            return None;
        }
        let mut out_cmd = None;
        let mut next_sub: Option<usize> = None;
        let mut go_back = false;

        egui::Area::new(egui::Id::unique("cs16_command_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(18.0, ctx.content_rect().center().y - 120.0))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgba_unmultiplied(12, 16, 12, 232))
                    .stroke(Stroke::new(1.0, palette.border_bright))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        if !self.submenu_path.is_empty()
                            && ui
                                .add(egui::Button::new(".. Back").corner_radius(CornerRadius::ZERO))
                                .clicked()
                        {
                            go_back = true;
                        }
                        for (idx, item) in &items {
                            let label = loc.map_or(item.label.as_str(), |l| l.resolve(&item.label));
                            let suffix = match &item.action {
                                CommandMenuAction::Submenu(_) => " >",
                                CommandMenuAction::Command(_) => "",
                            };
                            let text = format!("{}. {label}{suffix}", item.key);
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(text).color(palette.base_text))
                                        .min_size(Vec2::new(220.0, 22.0))
                                        .corner_radius(CornerRadius::ZERO),
                                )
                                .clicked()
                            {
                                match &item.action {
                                    CommandMenuAction::Submenu(_) => next_sub = Some(*idx),
                                    CommandMenuAction::Command(cmd) => {
                                        if cmd == "!MAPBRIEFING" {
                                            self.pending_briefing = true;
                                        } else {
                                            out_cmd = Some(cmd.clone());
                                        }
                                        self.set_open(false);
                                    }
                                }
                            }
                        }
                    });
            });

        if go_back {
            self.submenu_path.pop();
        } else if let Some(s) = next_sub {
            self.submenu_path.push(s);
        }
        out_cmd
    }
}
