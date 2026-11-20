//! 1-to-1 Counter-Strike 1.6 VGUI2 Buy Menu (`Resource/UI/MainBuyMenu.res`, `BuyRifles_*.res`,
//! `cstrike/classes/*.res`, `cstrike/gfx/vgui/*.tga`) plus classic old-style text buy menu (`_vgui_menus 0`).

use cs_assets::bsp::SpawnTeam;
use cs_vfs::Vfs;
use egui::{Color32, FontId, Key, Pos2, Stroke};

use crate::ui::vgui_assets::{
    VguiAssetBank, VirtualCanvas, cs16_vgui_button, paint_cs16_background_panel,
    render_class_res_panel,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuyCategory {
    #[default]
    Root,
    Pistols,
    Shotguns,
    Smgs,
    Rifles,
    MachineGuns,
    Equipment,
}

#[derive(Default)]
pub struct BuyMenuUi {
    pub open: bool,
    pub category: BuyCategory,
    hovered_res: Option<String>,
}

struct BuyItem {
    hotkey: u8,
    cmd: &'static str,
    label: &'static str,
    price: i32,
    res_name: &'static str,
}

impl BuyMenuUi {
    pub fn open_menu(&mut self) {
        self.open = true;
        self.category = BuyCategory::Root;
        self.hovered_res = None;
    }

    pub fn open_category(&mut self, cat: BuyCategory) {
        self.open = true;
        self.category = cat;
        self.hovered_res = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.category = BuyCategory::Root;
        self.hovered_res = None;
    }

    /// Render the authentic CS 1.6 Buy Menu and return an optional console item name to purchase.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        team: SpawnTeam,
        money: i32,
        vgui_style: bool,
        vfs: Option<&Vfs>,
        bank: &mut VguiAssetBank,
    ) -> Option<String> {
        if !self.open {
            return None;
        }

        // Authentic CS 1.6 keyboard hotkeys (`1`..`8`, `0`, `F1` autobuy)
        let (pressed_digit, f1_pressed) = ctx.input(|i| {
            let keys = [
                (Key::Num0, 0u8),
                (Key::Num1, 1),
                (Key::Num2, 2),
                (Key::Num3, 3),
                (Key::Num4, 4),
                (Key::Num5, 5),
                (Key::Num6, 6),
                (Key::Num7, 7),
                (Key::Num8, 8),
            ];
            let mut d_hit = None;
            for (k, d) in keys {
                if i.key_pressed(k) {
                    d_hit = Some(d);
                    break;
                }
            }
            (d_hit, i.key_pressed(Key::F1))
        });

        if f1_pressed {
            self.close();
            return Some(autobuy_item(team, money).to_string());
        }

        if let Some(d) = pressed_digit {
            if d == 0 {
                self.close();
                return None;
            }
            match self.category {
                BuyCategory::Root => match d {
                    1 => {
                        self.category = BuyCategory::Pistols;
                        self.hovered_res = None;
                    }
                    2 => {
                        self.category = BuyCategory::Shotguns;
                        self.hovered_res = None;
                    }
                    3 => {
                        self.category = BuyCategory::Smgs;
                        self.hovered_res = None;
                    }
                    4 => {
                        self.category = BuyCategory::Rifles;
                        self.hovered_res = None;
                    }
                    5 => {
                        self.category = BuyCategory::MachineGuns;
                        self.hovered_res = None;
                    }
                    6 => {
                        self.close();
                        return Some("primammo".into());
                    }
                    7 => {
                        self.close();
                        return Some("secammo".into());
                    }
                    8 => {
                        self.category = BuyCategory::Equipment;
                        self.hovered_res = None;
                    }
                    _ => {}
                },
                cat => {
                    let items = category_items(cat, team);
                    if let Some(item) = items.iter().find(|it| it.hotkey == d)
                        && money >= item.price
                    {
                        let cmd = item.cmd.to_string();
                        self.close();
                        return Some(cmd);
                    }
                }
            }
        }

        // Classic Old-Style Text Buy Menu (`_vgui_menus 0`)
        if !vgui_style {
            return self.show_old_style_text_menu(ctx, team, money, bank);
        }

        // Authentic 1-to-1 CS 1.6 VGUI2 Buy Menu (`BackgroundPanel.res` + `MainBuyMenu.res` + `classes/*.res`)
        let screen = ctx.content_rect();
        let vc = VirtualCanvas::from_screen(screen);
        let cp = bank.client_palette;
        let mut bought: Option<String> = None;

        egui::Area::new(egui::Id::unique("cs16_vgui_buy_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                let title = match self.category {
                    BuyCategory::Root => "Shop by Category",
                    BuyCategory::Pistols => "Handguns",
                    BuyCategory::Shotguns => "Shotguns",
                    BuyCategory::Smgs => "Sub-Machine Guns",
                    BuyCategory::Rifles => "Rifles",
                    BuyCategory::MachineGuns => "Machine Guns",
                    BuyCategory::Equipment => "Equipment",
                };
                paint_cs16_background_panel(ui.painter(), vc, title, bank);

                match self.category {
                    BuyCategory::Root => {
                        // Exact `MainBuyMenu.res` header label at (84, 87)
                        ui.painter().text(
                            vc.pt(84.0, 92.0),
                            egui::Align2::LEFT_CENTER,
                            "Select Category",
                            FontId::proportional((10.5 * vc.scale).clamp(11.0, 18.0)),
                            cp.title_text,
                        );

                        // Exact `MainBuyMenu.res` vertical divider at x=236, y=116..400
                        ui.painter().line_segment(
                            [vc.pt(236.0, 116.0), vc.pt(236.0, 400.0)],
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 140)),
                        );

                        struct RootRow {
                            hk: &'static str,
                            label: &'static str,
                            cat: Option<BuyCategory>,
                            cmd: Option<&'static str>,
                            enabled: bool,
                            preview_res: &'static str,
                            y: f32,
                        }
                        let rows = [
                            RootRow { hk: "1", label: "Pistols", cat: Some(BuyCategory::Pistols), cmd: None, enabled: true, preview_res: "deserteagle", y: 116.0 },
                            RootRow { hk: "2", label: "Shotguns", cat: Some(BuyCategory::Shotguns), cmd: None, enabled: true, preview_res: "m3", y: 148.0 },
                            RootRow { hk: "3", label: "SMG", cat: Some(BuyCategory::Smgs), cmd: None, enabled: true, preview_res: "mp5", y: 180.0 },
                            RootRow { hk: "4", label: "Rifles", cat: Some(BuyCategory::Rifles), cmd: None, enabled: true, preview_res: if team == SpawnTeam::Terrorist { "ak47" } else { "m4a1" }, y: 212.0 },
                            RootRow { hk: "5", label: "Machine Guns", cat: Some(BuyCategory::MachineGuns), cmd: None, enabled: true, preview_res: "m249", y: 244.0 },
                            RootRow { hk: "6", label: "Primary Ammo", cat: None, cmd: Some("primammo"), enabled: money >= 60, preview_res: "ak47", y: 276.0 },
                            RootRow { hk: "7", label: "Secondary Ammo", cat: None, cmd: Some("secammo"), enabled: money >= 60, preview_res: "deserteagle", y: 308.0 },
                            RootRow { hk: "8", label: "Equipment", cat: Some(BuyCategory::Equipment), cmd: None, enabled: true, preview_res: "kevlar_helmet", y: 340.0 },
                        ];

                        for row in &rows {
                            let rect = vc.rect(76.0, row.y, 148.0, 20.0);
                            let (clicked, hovered) = cs16_vgui_button(
                                ui,
                                rect,
                                &format!("buy_root_{}", row.hk),
                                Some(row.hk),
                                row.label,
                                row.enabled,
                                false,
                                vc.scale,
                                cp,
                            );
                            if hovered {
                                self.hovered_res = Some(row.preview_res.to_string());
                            }
                            if clicked {
                                if let Some(cat) = row.cat {
                                    self.category = cat;
                                    self.hovered_res = None;
                                } else if let Some(cmd) = row.cmd {
                                    bought = Some(cmd.to_string());
                                }
                            }
                        }

                        // Exact `MainBuyMenu.res` right-side Autobuy + Rebuy buttons at (249, 116) & (249, 148)
                        let auto_rect = vc.rect(249.0, 116.0, 168.0, 20.0);
                        let (auto_clicked, _) = cs16_vgui_button(
                            ui,
                            auto_rect,
                            "buy_autobuy",
                            None,
                            "Autobuy (F1)",
                            true,
                            false,
                            vc.scale,
                            cp,
                        );
                        if auto_clicked {
                            bought = Some(autobuy_item(team, money).to_string());
                        }

                        let rebuy_rect = vc.rect(249.0, 148.0, 168.0, 20.0);
                        let (rebuy_clicked, _) = cs16_vgui_button(
                            ui,
                            rebuy_rect,
                            "buy_rebuy",
                            None,
                            "Rebuy Equipment",
                            money >= 650,
                            false,
                            vc.scale,
                            cp,
                        );
                        if rebuy_clicked {
                            bought = Some("vesthelm".to_string());
                        }

                        // Preview box below Autobuy/Rebuy when hovering a category
                        if let Some(ref res_name) = self.hovered_res.clone() {
                            render_class_res_panel(
                                ctx,
                                ui.painter(),
                                vc,
                                249.0,
                                186.0,
                                res_name,
                                vfs,
                                bank,
                            );
                        } else {
                            // Show player cash & CS 1.6 hint cleanly on right panel
                            let info_rect = vc.rect(249.0, 190.0, 320.0, 110.0);
                            ui.painter().text(
                                info_rect.left_top(),
                                egui::Align2::LEFT_TOP,
                                format!("Available Cash:  ${money}"),
                                FontId::proportional((11.5 * vc.scale).clamp(12.0, 19.0)),
                                cp.title_text,
                            );
                            ui.painter().text(
                                info_rect.left_top() + egui::vec2(0.0, 26.0 * vc.scale),
                                egui::Align2::LEFT_TOP,
                                "Select a category from the left or press\n1-8 to purchase weapons & equipment.",
                                FontId::proportional((9.5 * vc.scale).clamp(11.0, 16.0)),
                                cp.label_dim_text,
                            );
                        }
                    }
                    cat => {
                        let items = category_items(cat, team);
                        // Default preview to the iconic weapon in this category until hovered
                        let default_res = default_preview_for_category(cat, team);
                        let active_res = self
                            .hovered_res
                            .clone()
                            .unwrap_or_else(|| default_res.to_string());

                        for (idx, item) in items.iter().enumerate() {
                            let y = 116.0 + (idx as f32) * 32.0;
                            let rect = vc.rect(76.0, y, 148.0, 20.0);
                            let affordable = money >= item.price;
                            let is_previewed = active_res.eq_ignore_ascii_case(item.res_name);
                            let hk_str = item.hotkey.to_string();
                            let (clicked, hovered) = cs16_vgui_button(
                                ui,
                                rect,
                                &format!("buy_item_{}", item.cmd),
                                Some(&hk_str),
                                item.label,
                                affordable,
                                is_previewed,
                                vc.scale,
                                cp,
                            );
                            if hovered {
                                self.hovered_res = Some(item.res_name.to_string());
                            }
                            if clicked {
                                bought = Some(item.cmd.to_string());
                            }
                        }

                        // Exact `ItemInfo` preview panel at (244, 116) from `BuyRifles_CT.res`!
                        render_class_res_panel(
                            ctx,
                            ui.painter(),
                            vc,
                            244.0,
                            116.0,
                            &active_res,
                            vfs,
                            bank,
                        );
                    }
                }

                // Exact `CancelButton` at (76, 380, 148, 20) from `MainBuyMenu.res` & `BuyRifles_CT.res`
                let cancel_rect = vc.rect(76.0, 380.0, 148.0, 20.0);
                let (cancel_clicked, _) = cs16_vgui_button(
                    ui,
                    cancel_rect,
                    "buy_cancel",
                    Some("0"),
                    "Cancel",
                    true,
                    false,
                    vc.scale,
                    cp,
                );
                if cancel_clicked {
                    self.close();
                }
            });

        if bought.is_some() {
            self.close();
        }
        bought
    }

    fn show_old_style_text_menu(
        &mut self,
        ctx: &egui::Context,
        team: SpawnTeam,
        money: i32,
        bank: &VguiAssetBank,
    ) -> Option<String> {
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("cs16_oldstyle_buy"),
        ));
        let cp = bank.client_palette;
        let x = 24.0;
        let mut y = screen.center().y - 110.0;
        let font = FontId::proportional(15.0);

        let header = match self.category {
            BuyCategory::Root => "Buy Item",
            BuyCategory::Pistols => "Buy Handgun",
            BuyCategory::Shotguns => "Buy Shotgun",
            BuyCategory::Smgs => "Buy Sub-Machine Gun",
            BuyCategory::Rifles => "Buy Rifle",
            BuyCategory::MachineGuns => "Buy Machine Gun",
            BuyCategory::Equipment => "Buy Equipment",
        };
        painter.text(Pos2::new(x, y), egui::Align2::LEFT_TOP, header, font.clone(), cp.title_text);
        y += 24.0;

        match self.category {
            BuyCategory::Root => {
                for line in [
                    "1. Handguns",
                    "2. Shotguns",
                    "3. Sub-Machine Guns",
                    "4. Rifles",
                    "5. Machine Guns",
                    "6. Primary weapon ammo",
                    "7. Secondary weapon ammo",
                    "8. Equipment",
                    "",
                    "0. Exit",
                ] {
                    painter.text(Pos2::new(x, y), egui::Align2::LEFT_TOP, line, font.clone(), Color32::WHITE);
                    y += 18.0;
                }
            }
            cat => {
                for item in category_items(cat, team) {
                    let col = if money >= item.price {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(130, 90, 40)
                    };
                    painter.text(
                        Pos2::new(x, y),
                        egui::Align2::LEFT_TOP,
                        format!("{}. {}  (${})", item.hotkey, item.label, item.price),
                        font.clone(),
                        col,
                    );
                    y += 18.0;
                }
                y += 12.0;
                painter.text(Pos2::new(x, y), egui::Align2::LEFT_TOP, "0. Exit", font, Color32::WHITE);
            }
        }
        None
    }
}

fn autobuy_item(team: SpawnTeam, money: i32) -> &'static str {
    if money >= 3100 && team == SpawnTeam::CounterTerrorist {
        "m4a1"
    } else if money >= 2500 && team == SpawnTeam::Terrorist {
        "ak47"
    } else if money >= 1500 {
        "mp5"
    } else if money >= 1000 {
        "vesthelm"
    } else if money >= 650 {
        "deagle"
    } else {
        "hegren"
    }
}

fn default_preview_for_category(cat: BuyCategory, team: SpawnTeam) -> &'static str {
    match cat {
        BuyCategory::Pistols => "deserteagle",
        BuyCategory::Shotguns => "m3",
        BuyCategory::Smgs => "mp5",
        BuyCategory::Rifles => match team {
            SpawnTeam::CounterTerrorist => "m4a1",
            SpawnTeam::Terrorist => "ak47",
        },
        BuyCategory::MachineGuns => "m249",
        BuyCategory::Equipment => "kevlar_helmet",
        BuyCategory::Root => "ak47",
    }
}

fn category_items(cat: BuyCategory, team: SpawnTeam) -> Vec<BuyItem> {
    match cat {
        BuyCategory::Root => Vec::new(),
        BuyCategory::Pistols => {
            let mut list = vec![
                BuyItem { hotkey: 1, cmd: "glock", label: "9x19mm Sidearm", price: 400, res_name: "glock18" },
                BuyItem { hotkey: 2, cmd: "usp", label: "KM .45 Tactical", price: 500, res_name: "usp45" },
                BuyItem { hotkey: 3, cmd: "p228", label: "228 Compact", price: 600, res_name: "p228" },
                BuyItem { hotkey: 4, cmd: "deagle", label: "Night Hawk .50C", price: 650, res_name: "deserteagle" },
            ];
            match team {
                SpawnTeam::CounterTerrorist => list.push(BuyItem {
                    hotkey: 5,
                    cmd: "fiveseven",
                    label: "ES Five-Seven",
                    price: 750,
                    res_name: "fiveseven",
                }),
                SpawnTeam::Terrorist => list.push(BuyItem {
                    hotkey: 5,
                    cmd: "elite",
                    label: ".40 Dual Elites",
                    price: 800,
                    res_name: "elites",
                }),
            }
            list
        }
        BuyCategory::Shotguns => vec![
            BuyItem { hotkey: 1, cmd: "m3", label: "Leone 12 Gauge Super", price: 1700, res_name: "m3" },
            BuyItem { hotkey: 2, cmd: "xm1014", label: "Leone YG1265 Auto", price: 3000, res_name: "xm1014" },
        ],
        BuyCategory::Smgs => {
            let first = match team {
                SpawnTeam::CounterTerrorist => BuyItem {
                    hotkey: 1,
                    cmd: "tmp",
                    label: "Schmidt Machine Pistol",
                    price: 1250,
                    res_name: "tmp",
                },
                SpawnTeam::Terrorist => BuyItem {
                    hotkey: 1,
                    cmd: "mac10",
                    label: "Ingram MAC-10",
                    price: 1400,
                    res_name: "mac10",
                },
            };
            vec![
                first,
                BuyItem { hotkey: 2, cmd: "mp5", label: "KM Sub-Machine Gun", price: 1500, res_name: "mp5" },
                BuyItem { hotkey: 3, cmd: "ump45", label: "KM UMP45", price: 1700, res_name: "ump45" },
                BuyItem { hotkey: 4, cmd: "p90", label: "ES C90", price: 2350, res_name: "p90" },
            ]
        }
        BuyCategory::Rifles => match team {
            SpawnTeam::CounterTerrorist => vec![
                BuyItem { hotkey: 1, cmd: "famas", label: "Clarion 5.56", price: 2250, res_name: "famas" },
                BuyItem { hotkey: 2, cmd: "scout", label: "Schmidt Scout", price: 2750, res_name: "scout" },
                BuyItem { hotkey: 3, cmd: "m4a1", label: "Maverick M4A1 Carbine", price: 3100, res_name: "m4a1" },
                BuyItem { hotkey: 4, cmd: "aug", label: "Bullpup", price: 3500, res_name: "aug" },
                BuyItem { hotkey: 5, cmd: "sg550", label: "Krieg 550 Commando", price: 4200, res_name: "sg550" },
                BuyItem { hotkey: 6, cmd: "awp", label: "Magnum Sniper Rifle", price: 4750, res_name: "awp" },
            ],
            SpawnTeam::Terrorist => vec![
                BuyItem { hotkey: 1, cmd: "galil", label: "IDF Defender", price: 2000, res_name: "galil" },
                BuyItem { hotkey: 2, cmd: "ak47", label: "CV-47", price: 2500, res_name: "ak47" },
                BuyItem { hotkey: 3, cmd: "scout", label: "Schmidt Scout", price: 2750, res_name: "scout" },
                BuyItem { hotkey: 4, cmd: "sg552", label: "Krieg 552", price: 3500, res_name: "sg552" },
                BuyItem { hotkey: 5, cmd: "g3sg1", label: "D3/AU-1", price: 5000, res_name: "g3sg1" },
                BuyItem { hotkey: 6, cmd: "awp", label: "Magnum Sniper Rifle", price: 4750, res_name: "awp" },
            ],
        },
        BuyCategory::MachineGuns => vec![BuyItem {
            hotkey: 1,
            cmd: "m249",
            label: "M249",
            price: 5750,
            res_name: "m249",
        }],
        BuyCategory::Equipment => {
            let mut list = vec![
                BuyItem { hotkey: 1, cmd: "vest", label: "Kevlar", price: 650, res_name: "kevlar" },
                BuyItem { hotkey: 2, cmd: "vesthelm", label: "Kevlar + Helmet", price: 1000, res_name: "kevlar_helmet" },
                BuyItem { hotkey: 3, cmd: "flash", label: "Flashbang", price: 200, res_name: "flashbang" },
                BuyItem { hotkey: 4, cmd: "hegren", label: "HE Grenade", price: 300, res_name: "hegrenade" },
                BuyItem { hotkey: 5, cmd: "sgren", label: "Smoke Grenade", price: 300, res_name: "smokegrenade" },
            ];
            if team == SpawnTeam::CounterTerrorist {
                list.push(BuyItem { hotkey: 6, cmd: "defuser", label: "Defusal Kit", price: 200, res_name: "defuser" });
                list.push(BuyItem { hotkey: 7, cmd: "nvgs", label: "NightVision", price: 1250, res_name: "nightvision" });
            } else {
                list.push(BuyItem { hotkey: 6, cmd: "nvgs", label: "NightVision", price: 1250, res_name: "nightvision" });
            }
            list
        }
    }
}
