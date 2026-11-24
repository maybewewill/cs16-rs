//! CS 1.6 `commandmenu.txt` (`+commandmenu` / `H`) cascading `ClientScheme.res` command menu.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_assets::vgui_scripts::{CommandMenuAction, CommandMenuItem, CommandMenuTree};
use cs_vfs::Vfs;
use egui::{Color32, Key, Pos2, Rect, Sense, Stroke, Vec2};

use crate::ui::vgui2::VguiTextDrawOpts;
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
        let sy = (screen.height() / 480.0).max(0.5);
        // `CCommandMenu::Show` (`100b2ed0`): background `WindowBG` / `0xc8000000`, `x = 0`,
        // `y = max(0, (sh - mh) / 2)`, row height `22 * sy`.
        let bg_col = scheme_color_or(
            &runtime.client_scheme,
            "WindowBG",
            Color32::from_rgba_unmultiplied(0, 0, 0, 200),
        );
        let bg_col = Color32::from_rgba_unmultiplied(bg_col.r(), bg_col.g(), bg_col.b(), 200);
        let border_col = Color32::from_rgba_unmultiplied(188, 112, 0, 120);
        let amber = scheme_color_or(
            &runtime.client_scheme,
            "ControlText",
            Color32::from_rgb(255, 176, 0),
        );
        let hover_bg = Color32::from_rgba_unmultiplied(188, 112, 0, 48);

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
        let row_h = (22.0 * sy).round();
        let pad_left = (8.0 * sy).round();
        let pad_right = (24.0 * sy).round();
        let min_col_w = (110.0 * sy).round();
        let client_fonts = runtime.client_scheme.fonts.clone();
        let mut col_x = screen.left();
        let mut base_y: Option<f32> = None;

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
            let mut max_txt_w = 0.0f32;
            let labels: Vec<String> = items
                .iter()
                .map(|(_, item)| {
                    let resolved = loc.map_or(item.label.as_str(), |l| l.resolve(&item.label));
                    let clean = if resolved.starts_with('#') {
                        item.label.trim_start_matches('#').to_string()
                    } else {
                        resolved.to_string()
                    };
                    let w = runtime
                        .font_atlas
                        .measure_text_scaled(
                            ctx,
                            vfs,
                            &client_fonts,
                            "client",
                            "DefaultSmall",
                            screen_h,
                            &clean,
                            false,
                        )
                        .x;
                    max_txt_w = max_txt_w.max(w);
                    clean
                })
                .collect();

            let col_w = (max_txt_w + pad_left + pad_right).round().max(min_col_w);
            let col_h = (items.len() as f32 * row_h + 2.0).round();
            let col_y = match base_y {
                None => screen.top() + ((screen.height() - col_h) * 0.5).max(0.0).round(),
                Some(y) => y.clamp(screen.top(), (screen.bottom() - col_h).max(screen.top())),
            };
            let panel_rect = Rect::from_min_size(Pos2::new(col_x, col_y), Vec2::new(col_w, col_h));
            let selected_sub = self.submenu_path.get(depth).copied();

            egui::Area::new(egui::Id::unique(("cs16_command_menu_col", depth)))
                .order(egui::Order::Foreground)
                .fixed_pos(panel_rect.min)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(panel_rect.size(), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(panel_rect, 0.0, bg_col);
                    p.rect_stroke(panel_rect, 0.0, Stroke::new(1.0, border_col), egui::StrokeKind::Inside);

                    for (row_idx, ((orig_idx, item), label)) in
                        items.iter().zip(labels.iter()).enumerate()
                    {
                        let ry = panel_rect.top() + 1.0 + row_idx as f32 * row_h;
                        let row_r = Rect::from_min_size(
                            Pos2::new(panel_rect.left() + 1.0, ry),
                            Vec2::new((col_w - 2.0).max(8.0), row_h),
                        );
                        let resp = ui.interact(
                            row_r,
                            ui.make_persistent_id(("cmdm", depth, row_idx)),
                            Sense::click(),
                        );
                        runtime.track_interactive(&format!("cmdm_{depth}_{row_idx}"), &resp);
                        let is_sel = selected_sub == Some(*orig_idx);
                        if resp.hovered() || is_sel {
                            ui.painter().rect_filled(row_r, 0.0, hover_bg);
                        }
                        let txt_r = Rect::from_min_max(
                            Pos2::new(row_r.left() + pad_left, row_r.top()),
                            Pos2::new(row_r.right() - (16.0 * sy).round(), row_r.bottom()),
                        );
                        runtime.font_atlas.paint_text(
                            ui.painter(),
                            ctx,
                            vfs,
                            &client_fonts,
                            "client",
                            "DefaultSmall",
                            screen_h,
                            txt_r,
                            label,
                            amber,
                            VguiTextDrawOpts {
                                align: TextAlignment::West,
                                process_hotkey: false,
                                proportional_scale: false,
                            },
                        );

                        match &item.action {
                            CommandMenuAction::Submenu(_) => {
                                let ax = row_r.right() - (8.0 * sy).round();
                                let ay = row_r.center().y;
                                let aw = (3.5 * sy).round().max(3.0);
                                let ah = (3.5 * sy).round().max(3.0);
                                ui.painter().add(egui::Shape::convex_polygon(
                                    vec![
                                        Pos2::new(ax - aw, ay - ah),
                                        Pos2::new(ax + aw * 0.4, ay),
                                        Pos2::new(ax - aw, ay + ah),
                                    ],
                                    amber,
                                    Stroke::NONE,
                                ));
                                if resp.hovered() || resp.clicked() {
                                    update_depth = Some((depth, *orig_idx));
                                    base_y = Some(row_r.top());
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

            col_x = panel_rect.right();
            if base_y.is_none() {
                base_y = Some(panel_rect.top());
            }
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
