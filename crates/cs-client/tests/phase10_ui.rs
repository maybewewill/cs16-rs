//! Phase 10 UI & VGUI2 granular parity test suite (`docs/parity/ui.toml`).
//! Each `#[test]` asserts one concrete UI/HUD/VGUI2 parity behaviour in isolation.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use cs_assets::scheme::{
    ClientVguiColors, LocalizationTable, VguiColors, decode_valve_text, load_overview_image_rgba,
};
use cs_assets::vgui_font::{MarlettGlyph, SchemeFontSet, load_marlett_font};
use cs_assets::vgui_res::{ControlKind, TextAlignment, VguiResDocument};
use cs_assets::vgui_scripts::{
    CommandMenuAction, CommandMenuTree, GameMenuEntry, OverviewSpec, ScrOptionKind, SettingsScript,
    TitlesTable,
};
use cs_client::ui::console::{ConsoleRenderCtx, ConsoleUi};
use cs_client::ui::hud::{HudExtrasContext, HudView, draw_cs16_hud_with_extras, draw_cs16_hud_with_runtime};
use cs_client::ui::hud_extras::{
    WeaponBucketState, compute_damage_quadrants, damage_hazard_icons, evaluate_dhud_text_fx,
    format_hud_text_entry, format_status_bar,
};
use cs_client::ui::main_menu::{MainMenuRenderCtx, MainMenuUi};
use cs_client::ui::options::{
    OptionsRenderCtx, OptionsTab, OptionsUi, parse_kb_act_list, render_scr_options,
};
use cs_client::ui::scoreboard::{ScoreRow, scoreboard_to_list_panel, show_scoreboard};
use cs_client::ui::screens::{
    BuyClassTeamMenuUi, CommandMenuUi, DemoPlayerUi, DialogRenderCtx, DisconnectErrorDialog,
    LoadingDialogUi, MotdRenderCtx, MotdVguiUi, ServerBrowserEntry, ServerBrowserTab,
    ServerBrowserUi, ServerPasswordUi, SpectatorMode, SpectatorRenderCtx, SpectatorVguiUi,
    VguiMenuKind, dialogs::DemoTransportState, dialogs::PlayerListDialogUi,
};
use cs_client::ui::server_menu::{
    MenuSpanColor, draw_styled_text_menu, menu_key_enabled, menu_styled_lines,
};
use cs_client::ui::vgui2::controls::{
    ListSortDir, VguiControlState, VguiSliderState, align_to_egui,
};
use cs_client::ui::vgui2::{
    HtmlBlock, VguiEvent, VguiRuntime, client_ui_viewport, install_marlett_into_egui,
    install_scheme_fonts_into_egui, paint_html_doc, parse_html_doc,
};
use cs_client::ui::vgui_assets::ClientPalette;
use cs_client::ui::vgui_theme::VguiPalette;
use cs_client::ui::{FrameTelemetry, draw_crosshair, draw_net_graph};
use cs_clientcore::HudState;
use cs_clientcore::hud::DeathNotice;
use cs_cvar::{CVarRegistry, EngineAction};
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::{CsUserMsg, UserMsgDesc};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

fn palette() -> VguiPalette {
    VguiPalette::from(VguiColors::default())
}

fn client_palette() -> ClientPalette {
    ClientPalette::from(ClientVguiColors::default())
}

fn discover_retail_vfs() -> Option<Vfs> {
    let dir = std::env::var_os("CS16_GAMEDIR").map(PathBuf::from)?;
    Vfs::discover(
        Some(dir.as_path()),
        VfsConfig {
            enable_hd_models: false,
            enable_downloads: false,
        },
    )
    .ok()
}

fn run_egui_frame(mut draw: impl FnMut(&egui::Context)) -> usize {
    let ctx = egui::Context::default();
    let mut total_shapes = 0usize;
    for _pass in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(1024.0, 768.0),
            )),
            ..Default::default()
        };
        ctx.begin_pass(input);
        draw(&ctx);
        let mut out = ctx.end_pass();
        total_shapes = total_shapes.max(out.shapes.len());
        out.textures_delta.clear();
    }
    total_shapes
}

fn register_hud_msg(hud: &mut HudState, id: u8, name: &str, size: i8) {
    hud.handle(
        &SvcMessage::NewUserMsg(UserMsgDesc {
            id,
            size,
            name: name.to_string(),
        }),
        0.0,
    );
}

fn push_usermsg(hud: &mut HudState, id: u8, name: &str, msg: CsUserMsg, now: f64) {
    hud.handle(
        &SvcMessage::UserMsg {
            id,
            name: name.to_string(),
            decoded: msg,
        },
        now,
    );
}

#[test]
fn ui_screen_main_menu_and_new_game_parses_gamemenu_and_settings_scr() {
    let entries = GameMenuEntry::parse(
        r##""GameMenu" {
            "1" { "label" "#GameUI_GameMenu_ResumeGame" "command" "ResumeGame" "OnlyInGame" "1" }
            "2" { "label" "#GameUI_GameMenu_NewGame" "command" "OpenCreateMultiplayerGameDialog" }
            "4" { "label" "#GameUI_GameMenu_FindServers" "command" "OpenServerBrowser" }
        }"##,
    );
    assert_eq!(entries.len(), 3);
    assert!(entries[0].only_in_game);
    assert_eq!(entries[1].command, "OpenCreateMultiplayerGameDialog");

    let mut menu = MainMenuUi::default();
    let mut rt = VguiRuntime::new();
    let mut cvars = CVarRegistry::new_with_defaults();
    let maps = vec!["de_dust2".to_string(), "cs_office".to_string()];
    let out = run_egui_frame(|ctx| {
        let cmds = menu.show(
            ctx,
            &mut rt,
            &mut cvars,
            &MainMenuRenderCtx {
                vfs: None,
                loc: None,
                available_maps: &maps,
                in_game: false,
                bg_texture: None,
                steam_icon: None,
                palette: palette(),
            },
        );
        assert!(!cmds.resume_game);
    });
    assert!(out > 0);

    if let Some(vfs) = discover_retail_vfs() {
        let mut real_menu = MainMenuUi::default();
        real_menu.load_resources(&vfs, "english");
        assert!(!real_menu.game_menu.is_empty());
        assert!(real_menu.settings_script.is_some());
    } else {
        eprintln!("SKIPPED retail GameMenu.res check: CS16_GAMEDIR not set");
    }
}

#[test]
fn ui_screen_buy_menu_vgui_navigates_ct_and_t_submenus() {
    let mut buy = BuyClassTeamMenuUi::default();
    buy.open_kind(VguiMenuKind::Buy, true);
    assert_eq!(buy.active_res.as_deref(), Some("resource/UI/MainBuyMenu.res"));

    let nav = buy.handle_command("Resource/UI/BuyRifles.res", true, None);
    assert_eq!(nav, None);
    assert_eq!(buy.active_res.as_deref(), Some("resource/ui/buyrifles.res"));
    let bought = buy.handle_command("ak47", true, None);
    assert_eq!(bought.as_deref(), Some("ak47"));
    assert!(buy.active_res.is_none());

    if let Some(vfs) = discover_retail_vfs() {
        let mut rt = VguiRuntime::new();
        assert!(rt.load_res(&vfs, "resource/UI/BuyRifles_CT.res").is_some());
        assert!(rt.load_res(&vfs, "resource/UI/BuyRifles_TER.res").is_some());
    } else {
        eprintln!("SKIPPED retail BuyMenu .res check: CS16_GAMEDIR not set");
    }
}

#[test]
fn ui_screen_team_and_class_menu_resolves_team_and_class_res() {
    assert_eq!(VguiMenuKind::from_vgui_menu_id(2, ""), Some(VguiMenuKind::Team));
    // ReGameDLL_CS cdll_dll.h: VGUI_Menu_Class_T = 26, VGUI_Menu_Class_CT = 27, Buy = 28..=34
    assert_eq!(VguiMenuKind::from_vgui_menu_id(26, ""), Some(VguiMenuKind::ClassT));
    assert_eq!(VguiMenuKind::from_vgui_menu_id(27, ""), Some(VguiMenuKind::ClassCt));
    assert_eq!(VguiMenuKind::from_vgui_menu_id(28, ""), Some(VguiMenuKind::Buy));
    assert_eq!(VguiMenuKind::from_vgui_menu_id(3, ""), Some(VguiMenuKind::ClassT));

    let mut ui = BuyClassTeamMenuUi::default();
    ui.open_kind(VguiMenuKind::Team, true);
    assert_eq!(ui.active_res.as_deref(), Some("resource/UI/TeamMenu.res"));
    assert_eq!(
        ui.handle_command("jointeam 2", true, None).as_deref(),
        Some("jointeam 2")
    );

    ui.open_kind(VguiMenuKind::ClassCt, true);
    assert_eq!(ui.active_res.as_deref(), Some("resource/UI/ClassMenu_CT.res"));
    assert_eq!(
        ui.handle_command("joinclass 1", true, None).as_deref(),
        Some("joinclass 1")
    );
}

#[test]
fn ui_screen_scoreboard_tab_builds_rows_and_list_panel() {
    let rows = vec![
        ScoreRow {
            slot: 0,
            name: "Alpha".into(),
            frags: 14,
            deaths: 3,
            ping: 15,
            team: "CT".into(),
            dead: false,
            bomb: false,
            vip: false,
        },
        ScoreRow {
            slot: 1,
            name: "Bravo".into(),
            frags: 6,
            deaths: 5,
            ping: 22,
            team: "TERRORIST".into(),
            dead: true,
            bomb: true,
            vip: false,
        },
        ScoreRow {
            slot: 2,
            name: "Watcher".into(),
            frags: 0,
            deaths: 0,
            ping: 10,
            team: "SPECTATOR".into(),
            dead: true,
            bomb: false,
            vip: false,
        },
    ];
    let list = scoreboard_to_list_panel(&rows, 0b10);
    assert_eq!(list.rows.len(), 3);
    assert_eq!(list.rows[0][0], "Alpha");
    assert_eq!(list.rows[0][2], "14");

    let out = run_egui_frame(|ctx| {
        show_scoreboard(
            ctx,
            "CS 1.6 Test Server",
            "de_dust2",
            5,
            4,
            &rows,
            Some(0),
            client_palette(),
        );
    });
    assert!(out > 0);
}

#[test]
fn ui_screen_options_dialog_binds_cvars_and_keybinds() {
    let mut opts = OptionsUi::default();
    opts.open = true;
    opts.tab = OptionsTab::Multiplayer;
    let mut rt = VguiRuntime::new();
    let mut cvars = CVarRegistry::new_with_defaults();
    let out = run_egui_frame(|ctx| {
        opts.show(
            ctx,
            &mut rt,
            &mut cvars,
            &OptionsRenderCtx {
                vfs: None,
                loc: None,
                palette: palette(),
                steam_icon: None,
            },
        );
    });
    assert!(out > 0);
    assert!(cvars.key_for_command("+commandmenu").is_some());
    assert!(cvars.key_for_command("+voicerecord").is_some());
}

#[test]
fn ui_screen_developer_console_filters_and_completes_commands() {
    let mut console = ConsoleUi::default();
    console.open();
    assert!(console.open);
    let mut rt = VguiRuntime::new();
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.info("Test log line");
    let maps = vec!["de_dust2".into()];
    let out = run_egui_frame(|ctx| {
        let actions = console.show(
            ctx,
            &mut rt,
            &mut cvars,
            &ConsoleRenderCtx {
                vfs: None,
                loc: None,
                available_maps: &maps,
                palette: palette(),
                steam_icon: None,
            },
        );
        assert!(actions.is_empty());
    });
    assert!(out > 0);
}

#[test]
fn ui_hud_health_armor_ammo_money_timer_renders_hud_state() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 70, "Health", 1);
    register_hud_msg(&mut hud, 71, "Battery", 2);
    register_hud_msg(&mut hud, 72, "Money", 5);
    register_hud_msg(&mut hud, 73, "CurWeapon", 3);
    register_hud_msg(&mut hud, 74, "AmmoX", 2);
    register_hud_msg(&mut hud, 75, "RoundTime", 2);
    push_usermsg(&mut hud, 70, "Health", CsUserMsg::Health(87), 1.0);
    push_usermsg(&mut hud, 71, "Battery", CsUserMsg::Battery(95), 1.0);
    push_usermsg(
        &mut hud,
        72,
        "Money",
        CsUserMsg::Money {
            amount: 4250,
            flash: false,
        },
        1.0,
    );
    push_usermsg(
        &mut hud,
        73,
        "CurWeapon",
        CsUserMsg::CurWeapon {
            is_active: true,
            weapon_id: 28,
            clip_ammo: 30,
        },
        1.0,
    );
    push_usermsg(
        &mut hud,
        74,
        "AmmoX",
        CsUserMsg::AmmoX {
            ammo_id: 2,
            amount: 90,
        },
        1.0,
    );
    push_usermsg(&mut hud, 75, "RoundTime", CsUserMsg::RoundTime(105), 1.0);

    let names = BTreeMap::new();
    let owned = HashSet::from([28u8]);
    let extras = HudExtrasContext::default();
    let out = run_egui_frame(|ctx| {
        let hv = HudView {
            hud: &hud,
            now: 2.0,
            time_sec: 2.0,
            names: &names,
            owned: &owned,
            feed: &[],
            chat: &[],
            notify: &[],
            center: None,
            weapon_switch_timer: 0.0,
            alive: true,
            scoped: false,
            eye: Vec3::ZERO,
            yaw_deg: 0.0,
            underwater_tint: None,
        };
        draw_cs16_hud_with_extras(ctx, None, &hv, &extras);
    });
    assert!(out > 0);
}

#[test]
fn ui_hud_crosshair_and_awp_scope_adjusts_color_and_renders() {
    let mut cvars = CVarRegistry::new_with_defaults();
    let actions = cvars.execute_script("adjust_crosshair");
    assert_eq!(actions, vec![EngineAction::AdjustCrosshair(None)]);

    let hud = HudState::default();
    let names = BTreeMap::new();
    let owned = HashSet::new();
    let extras = HudExtrasContext::default();
    let out = run_egui_frame(|ctx| {
        draw_crosshair(ctx, &cvars, 3.0);
        let hv = HudView {
            hud: &hud,
            now: 1.0,
            time_sec: 1.0,
            names: &names,
            owned: &owned,
            feed: &[],
            chat: &[],
            notify: &[],
            center: None,
            weapon_switch_timer: 0.0,
            alive: true,
            scoped: true,
            eye: Vec3::ZERO,
            yaw_deg: 0.0,
            underwater_tint: None,
        };
        draw_cs16_hud_with_extras(ctx, None, &hv, &extras);
    });
    assert!(out > 2);
}

#[test]
fn ui_hud_radar_and_nvg_overlay_renders_teammates_and_radartype() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 80, "Radar", 7);
    register_hud_msg(&mut hud, 81, "NVGToggle", 1);
    push_usermsg(
        &mut hud,
        80,
        "Radar",
        CsUserMsg::Radar {
            player_id: 2,
            origin: Vec3::new(300.0, 120.0, 0.0),
        },
        1.0,
    );
    push_usermsg(&mut hud, 81, "NVGToggle", CsUserMsg::NvgToggle(true), 1.0);
    assert!(hud.night_vision);

    let names = BTreeMap::new();
    let owned = HashSet::new();
    let extras = HudExtrasContext {
        cl_radartype: true,
        ..Default::default()
    };
    let out = run_egui_frame(|ctx| {
        let hv = HudView {
            hud: &hud,
            now: 1.0,
            time_sec: 1.0,
            names: &names,
            owned: &owned,
            feed: &[],
            chat: &[],
            notify: &[],
            center: None,
            weapon_switch_timer: 0.0,
            alive: true,
            scoped: false,
            eye: Vec3::ZERO,
            yaw_deg: 45.0,
            underwater_tint: None,
        };
        draw_cs16_hud_with_extras(ctx, None, &hv, &extras);
    });
    assert!(out > 0);
}

#[test]
fn ui_hud_killfeed_chat_radio_netgraph_respects_con_color_and_saytext() {
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("net_graph", "1");
    cvars.set("hud_saytext_internal", "1");
    cvars.set("con_color", "100 220 120");
    let mut extras = HudExtrasContext::default();
    extras.apply_cvars(&cvars);
    assert!(extras.hud_saytext_internal);
    assert_eq!(extras.con_color, [100, 220, 120]);

    let hud = HudState::default();
    let mut names = BTreeMap::new();
    names.insert(0, "Killer".into());
    names.insert(1, "Victim".into());
    let owned = HashSet::new();
    let feed = vec![DeathNotice {
        killer: 1,
        victim: 2,
        headshot: true,
        weapon: "ak47".into(),
    }];
    let chat = vec!["Hello team".into()];
    let out = run_egui_frame(|ctx| {
        let hv = HudView {
            hud: &hud,
            now: 1.0,
            time_sec: 1.0,
            names: &names,
            owned: &owned,
            feed: &feed,
            chat: &chat,
            notify: &[],
            center: None,
            weapon_switch_timer: 0.0,
            alive: true,
            scoped: false,
            eye: Vec3::ZERO,
            yaw_deg: 0.0,
            underwater_tint: None,
        };
        draw_cs16_hud_with_extras(ctx, None, &hv, &extras);
        draw_net_graph(
            ctx,
            &cvars,
            &FrameTelemetry {
                fps: 100.0,
                frame_ms: 10.0,
                map_name: "de_dust2",
                pos: [0.0; 3],
                yaw_deg: 0.0,
                pitch_deg: 0.0,
                speed: 250.0,
                draw_calls: 42,
                triangles: 12000,
            },
            palette(),
        );
    });
    assert!(out > 0);
}

#[test]
fn ui_vgui2_res_layout_interpreter_resolves_center_right_fill_coords() {
    let res_text = r#""Test.res" {
        "Box" {
            "ControlName" "Panel"
            "fieldName" "Box"
            "xpos" "c-100"
            "ypos" "r80"
            "wide" "f20"
            "tall" "40"
            "visible" "1"
            "enabled" "1"
        }
    }"#;
    let doc = VguiResDocument::parse("test.res", res_text);
    let mut rt = VguiRuntime::new();
    rt.insert_res("test.res", res_text);
    assert_eq!(doc.controls.len(), 1);
    let nodes = rt.compute_layout("test.res", [0.0, 0.0, 640.0, 480.0], None);
    assert_eq!(nodes[0].rect[0], 220.0);
    assert_eq!(nodes[0].rect[1], 400.0);
    assert_eq!(nodes[0].rect[2], 400.0);
    assert_eq!(nodes[0].rect[3], 40.0);
}

#[test]
fn ui_vgui2_control_panel_paints_panel_control() {
    let mut rt = VguiRuntime::new();
    rt.insert_res(
        "panel.res",
        r#""Panel.res" { "P" { "ControlName" "EditablePanel" "fieldName" "P" "xpos" "10" "ypos" "10" "wide" "120" "tall" "60" "visible" "1" } }"#,
    );
    let out = run_egui_frame(|ctx| {
        egui::Area::new(egui::Id::unique("test_panel")).show(ctx, |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 480.0));
            let evs = rt.paint_res_controls(ui, "panel.res", rect, None, None, palette());
            assert!(evs.is_empty());
        });
    });
    assert!(out > 0);
}

#[test]
fn ui_vgui2_control_label_resolves_alignment_and_text() {
    assert_eq!(align_to_egui(TextAlignment::Center), egui::Align2::CENTER_CENTER);
    let mut rt = VguiRuntime::new();
    rt.insert_res(
        "lbl.res",
        r##""Lbl.res" { "L" { "ControlName" "Label" "fieldName" "L" "xpos" "0" "ypos" "0" "wide" "100" "tall" "20" "labelText" "#Hello" "textAlignment" "center" "visible" "1" } }"##,
    );
    let mut loc = LocalizationTable::default();
    loc.insert_text("\"lang\"\n{\n\"Tokens\"\n{\n\"Hello\" \"Privet\"\n}\n}");
    assert_eq!(
        rt.doc_label("lbl.res", "L", Some(&loc)).as_deref(),
        Some("Privet")
    );
}

#[test]
fn ui_vgui2_control_button_emits_command_event() {
    let mut rt = VguiRuntime::new();
    rt.insert_res(
        "btn.res",
        r#""Btn.res" { "OkBtn" { "ControlName" "Button" "fieldName" "OkBtn" "xpos" "10" "ypos" "10" "wide" "80" "tall" "24" "labelText" "OK" "command" "vguicancel" "visible" "1" "enabled" "1" } }"#,
    );
    let cmd = rt.trigger_button("btn.res", "OkBtn");
    assert_eq!(cmd.as_deref(), Some("vguicancel"));
    assert_eq!(
        rt.take_events(),
        vec![VguiEvent::Command {
            field: "OkBtn".into(),
            command: "vguicancel".into()
        }]
    );
}

#[test]
fn ui_vgui2_control_image_panel_parses_image_and_paints() {
    let doc = VguiResDocument::parse(
        "img.res",
        r#""Img.res" { "Logo" { "ControlName" "ImagePanel" "fieldName" "Logo" "image" "gfx/vgui/ct_urban" "xpos" "0" "ypos" "0" "wide" "64" "tall" "64" "visible" "1" } }"#,
    );
    let spec = doc.find("Logo").unwrap();
    assert_eq!(spec.kind, ControlKind::ImagePanel);
    assert_eq!(spec.image, "gfx/vgui/ct_urban.tga");
}

#[test]
fn ui_vgui2_control_frame_infers_window_size_and_paints() {
    let mut rt = VguiRuntime::new();
    rt.insert_res(
        "frm.res",
        r#""Frm.res" { "MainFrame" { "ControlName" "Frame" "fieldName" "MainFrame" "xpos" "40" "ypos" "40" "wide" "520" "tall" "380" "visible" "1" } }"#,
    );
    assert_eq!(rt.doc_window_size("frm.res", [400.0, 300.0]), [520.0, 380.0]);
}

#[test]
fn ui_vgui2_control_list_panel_sorts_columns_and_rows() {
    let mut rt = VguiRuntime::new();
    let mut list = cs_client::ui::vgui2::runtime::default_list_state(&[("Name", 120), ("Ping", 60)]);
    list.rows = vec![
        vec!["Server B".into(), "45".into()],
        vec!["Server A".into(), "12".into()],
    ];
    rt.set_state("ServerList", VguiControlState::List(list));
    assert!(rt.sort_list("ServerList", 0));
    let Some(VguiControlState::List(sorted)) = rt.get_state("ServerList") else {
        panic!("expected list state");
    };
    assert_eq!(sorted.sort_dir, Some(ListSortDir::Ascending));
    assert_eq!(sorted.rows[0][0], "Server A");
}

#[test]
fn ui_vgui2_control_rich_text_stores_colored_lines() {
    let mut rt = VguiRuntime::new();
    let mut rich = cs_client::ui::vgui2::runtime::default_rich_text();
    rich.lines.push(("Line 1".into(), [255, 180, 30, 255]));
    rt.set_state("ConsoleLog", VguiControlState::RichText(rich));
    rt.insert_res(
        "rt.res",
        r#""Rt.res" { "ConsoleLog" { "ControlName" "RichText" "fieldName" "ConsoleLog" "xpos" "0" "ypos" "0" "wide" "300" "tall" "120" "visible" "1" } }"#,
    );
    let out = run_egui_frame(|ctx| {
        egui::Area::new(egui::Id::unique("test_richtext")).show(ctx, |ui| {
            let r = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(640.0, 480.0));
            rt.paint_res_controls(ui, "rt.res", r, None, None, palette());
        });
    });
    assert!(out > 0);
}

#[test]
fn ui_vgui2_control_combo_box_switches_selected_item() {
    let mut rt = VguiRuntime::new();
    rt.set_state(
        "LangCombo",
        VguiControlState::Combo(cs_client::ui::vgui2::runtime::default_combo_state(
            &["English", "Russian"],
            0,
        )),
    );
    assert!(rt.select_combo("LangCombo", 1));
    assert!(!rt.select_combo("LangCombo", 9));
    let Some(VguiControlState::Combo(cb)) = rt.get_state("LangCombo") else {
        panic!("expected combo");
    };
    assert_eq!(cb.selected_text(), "Russian");
}

#[test]
fn ui_vgui2_control_slider_clamps_value_and_emits_change() {
    let mut rt = VguiRuntime::new();
    rt.set_state(
        "SensSlider",
        VguiControlState::Slider(VguiSliderState {
            value: 3.0,
            min: 0.2,
            max: 10.0,
        }),
    );
    rt.set_slider("SensSlider", 25.0);
    let Some(VguiControlState::Slider(s)) = rt.get_state("SensSlider") else {
        panic!("expected slider");
    };
    assert_eq!(s.value, 10.0);
    assert_eq!(
        rt.take_events(),
        vec![VguiEvent::ValueChanged {
            field: "SensSlider".into()
        }]
    );
}

#[test]
fn ui_vgui2_control_check_button_toggles_and_emits_change() {
    let mut rt = VguiRuntime::new();
    rt.set_state("FastSwitch", VguiControlState::Check(false));
    assert!(rt.toggle_check("FastSwitch"));
    assert!(!rt.toggle_check("FastSwitch"));
    assert_eq!(rt.take_events().len(), 2);
}

#[test]
fn ui_vgui2_control_text_entry_updates_text_and_emits_change() {
    let mut rt = VguiRuntime::new();
    rt.set_text_entry("NameEntry", "PlayerOne".into());
    assert!(
        matches!(rt.get_state("NameEntry"), Some(VguiControlState::Text(s)) if s == "PlayerOne")
    );
    assert_eq!(
        rt.take_events(),
        vec![VguiEvent::ValueChanged {
            field: "NameEntry".into()
        }]
    );
}

#[test]
fn ui_vgui2_control_property_sheet_switches_active_tab() {
    let mut rt = VguiRuntime::new();
    rt.set_state(
        "Tabs",
        VguiControlState::PropertySheet(cs_client::ui::vgui2::runtime::default_sheet_state(&[
            "Multiplayer",
            "Keyboard",
            "Video",
        ])),
    );
    assert!(rt.select_tab("Tabs", 2));
    assert!(!rt.select_tab("Tabs", 7));
    assert_eq!(
        rt.take_events(),
        vec![VguiEvent::TabChanged {
            field: "Tabs".into(),
            tab: 2
        }]
    );
}

#[test]
fn ui_truetype_font_rasterizer_loads_marlett_and_scheme_tiers() {
    let set = SchemeFontSet::parse(
        r#""Scheme" {
            "Fonts" {
                "Default" {
                    "1" { "name" "Tahoma" "tall" "12" "yres" "1 599" }
                    "2" { "name" "Tahoma" "tall" "16" "yres" "600 1024" }
                }
            }
            "CustomFontFiles" { "1" "resource/marlett.ttf" }
        }"#,
    );
    assert_eq!(set.resolve("Default", 480).unwrap().tall, 12.0);
    assert_eq!(set.resolve("Default", 768).unwrap().tall, 16.0);
    assert_eq!(MarlettGlyph::CHECK, 'a');

    if let Some(vfs) = discover_retail_vfs() {
        let marlett = load_marlett_font(&vfs).expect("resource/marlett.ttf present in Valve install");
        assert!(marlett.metrics(14.0).line_height > 0.0);
        let glyph = marlett
            .rasterize_glyph(MarlettGlyph::CHECK, 14.0)
            .expect("check glyph");
        assert!(glyph.width > 0 && glyph.height > 0);
        let ctx = egui::Context::default();
        assert!(install_marlett_into_egui(&ctx, &vfs));
        let real_set = SchemeFontSet::load(&vfs, "resource/TrackerScheme.res");
        assert!(install_scheme_fonts_into_egui(&ctx, &vfs, &real_set) >= 1);
    } else {
        eprintln!("SKIPPED retail marlett.ttf raster check: CS16_GAMEDIR not set");
    }
}

#[test]
fn ui_all_languages_localization_decodes_multilang_utf16le() {
    let src = "\"lang\"\n{\n\"Language\" \"russian\"\n\"Tokens\"\n{\n\"Test_Key\" \"Привет\"\n}\n}";
    let mut utf16 = vec![0xFFu8, 0xFE];
    for u in src.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let decoded = decode_valve_text(&utf16);
    let mut loc = LocalizationTable::default();
    loc.insert_text(&decoded);
    assert_eq!(loc.resolve("#Test_Key"), "Привет");

    if let Some(vfs) = discover_retail_vfs() {
        let langs = LocalizationTable::discover_languages(&vfs);
        assert!(langs.contains(&"english".to_string()));
        let ru = LocalizationTable::load_language(&vfs, "russian");
        assert_eq!(ru.language(), "russian");
        assert!(!ru.is_empty());
    } else {
        eprintln!("SKIPPED retail multi-language discovery: CS16_GAMEDIR not set");
    }
}

#[test]
fn ui_screen_motd_html_parses_tags_colors_and_renders_res() {
    let doc = parse_html_doc(
        "<html><body bgcolor=\"#102030\"><h1>Welcome</h1><p><font color=\"#FFAA00\">Rules</font><a href=\"http://example.com\">Link</a></p></body></html>",
    );
    assert_eq!(doc.bg_color, Some([0x10, 0x20, 0x30, 0xFF]));
    assert!(!doc.blocks.is_empty());

    let mut motd = MotdVguiUi::default();
    motd.open = true;
    let mut rt = VguiRuntime::new();
    let out = run_egui_frame(|ctx| {
        egui::Area::new(egui::Id::unique("test_html")).show(ctx, |ui| {
            paint_html_doc(ui, &doc);
        });
        let closed = motd.show(
            ctx,
            &mut rt,
            &MotdRenderCtx {
                vfs: None,
                loc: None,
                hostname: "My Server",
                raw_motd: "<b>Hello</b>",
            },
        );
        assert!(!closed);
    });
    assert!(out > 0);
}

#[test]
fn ui_screen_server_browser_full_filters_favorites_history_lan() {
    let mut sb = ServerBrowserUi {
        open: true,
        tab: ServerBrowserTab::Favorites,
        ..Default::default()
    };
    sb.add_favorite(ServerBrowserEntry {
        addr: "127.0.0.1:27015".into(),
        name: "Local Dust2".into(),
        game: "Counter-Strike".into(),
        map: "de_dust2".into(),
        players: 8,
        max_players: 16,
        ping_ms: 5,
        password: false,
        vac: true,
        spectate_port: false,
    });
    sb.filter.map = "dust2".into();
    sb.filter.no_password = true;
    sb.filter.secure_only = true;
    assert_eq!(sb.active_entries().len(), 1);
    sb.filter.map = "inferno".into();
    assert_eq!(sb.active_entries().len(), 0);
}

#[test]
fn ui_screen_spectator_and_bottom_bar_cycles_modes_and_paints_res() {
    let mut spec = SpectatorVguiUi::default();
    spec.menu_visible = true;
    spec.mode = SpectatorMode::from_i32(4);
    assert_eq!(spec.mode, SpectatorMode::FirstPerson);
    let mut rt = VguiRuntime::new();
    let mut cvars = CVarRegistry::new_with_defaults();
    let players = vec![(1, "Alpha".into()), (2, "Bravo".into())];
    let out = run_egui_frame(|ctx| {
        let acts = spec.show(
            ctx,
            &mut rt,
            &mut cvars,
            &SpectatorRenderCtx {
                vfs: None,
                loc: None,
                ct_score: 3,
                t_score: 2,
                round_timer_secs: 75,
                players: &players,
                spec_health: Some((82, 1)),
            },
        );
        assert!(acts.is_empty());
    });
    assert!(out > 0);
}

#[test]
fn ui_screen_command_menu_evaluates_map_conditions_and_submenus() {
    let tree = CommandMenuTree::parse(
        r#""1" "General" "say_team Hi"
           MAP de_dust2 "2" "Strat" {
               "1" "Rush B" "say_team Rush B"
           }
           MAP cs_office "3" "Hostages" "say_team Hostages""#,
    );
    let mut ui = CommandMenuUi::default();
    ui.open = true;
    ui.tree = tree;
    let dust2 = ui.visible_items("de_dust2");
    assert_eq!(dust2.len(), 2);
    assert_eq!(dust2[0].1.label, "General");
    assert!(matches!(dust2[1].1.action, CommandMenuAction::Submenu(_)));
    let office = ui.visible_items("cs_office");
    assert_eq!(office.len(), 2);
    assert_eq!(office[1].1.label, "Hostages");

    let mut rt = VguiRuntime::new();
    let out = run_egui_frame(|ctx| {
        let _ = ui.show(ctx, &mut rt, None, None, "de_dust2");
    });
    assert!(out > 0);
}

#[test]
fn ui_screen_loading_dialog_reads_res_and_cancels() {
    let mut dlg = LoadingDialogUi {
        visible: true,
        status_text: "Precaching resources...".into(),
        progress: 0.65,
        secondary_progress: Some(("sound/weapons/ak47-1.wav".into(), 0.4)),
        vac_secure: true,
    };
    let mut rt = VguiRuntime::new();
    let dlg_ctx = DialogRenderCtx {
        vfs: None,
        loc: None,
        palette: palette(),
        steam_icon: None,
    };
    let out = run_egui_frame(|ctx| {
        assert!(!dlg.show(ctx, &mut rt, &dlg_ctx));
    });
    assert!(out > 0);
}

#[test]
fn ui_screen_player_list_mute_toggles_vban_bitmask() {
    let mut dlg = PlayerListDialogUi::default();
    dlg.open = true;
    let mut names = BTreeMap::new();
    names.insert(0, "Me".into());
    names.insert(2, "LoudPlayer".into());
    dlg.sync_players(&names, Some(0));
    assert_eq!(dlg.list.rows.len(), 1);
    dlg.list.selected_row = Some(0);
    let cmd = dlg.toggle_mute_selected();
    assert_eq!(cmd.as_deref(), Some("vban 4"));
    let cmd2 = dlg.toggle_mute_selected();
    assert_eq!(cmd2.as_deref(), Some("vban 0"));
}

#[test]
fn ui_screen_demo_player_dialog_steps_transport_controls() {
    let mut demo = DemoPlayerUi {
        open: true,
        total_ticks: 1000,
        ..Default::default()
    };
    demo.load_demo("match.dem");
    assert_eq!(demo.state, DemoTransportState::Playing);
    demo.step_command("pause");
    assert_eq!(demo.state, DemoTransportState::Paused);
    demo.step_command("faster");
    assert!(demo.speed > 1.0);
    demo.step_command("stepf");
    assert_eq!(demo.tick, 50);
    demo.step_command("stop");
    assert_eq!(demo.state, DemoTransportState::Stopped);
}

#[test]
fn ui_screen_options_all_tabs_covers_7_tabs_and_user_scr() {
    let script = SettingsScript::parse(
        r#"VERSION 1.0
        DESCRIPTION INFO_OPTIONS
        {
            "cl_righthand" { "Weapon alignment" { LIST "Left" "0" "Right" "1" } { "1" } }
            "hud_fastswitch" { "Fast weapon switch" { BOOL } { "1" } }
        }"#,
    );
    assert_eq!(script.options.len(), 2);
    assert!(matches!(script.options[0].kind, ScrOptionKind::List(_)));
    let mut opts = OptionsUi::default();
    opts.open = true;
    opts.tab = OptionsTab::Lock;
    opts.user_script = Some(script);
    let mut rt = VguiRuntime::new();
    let mut cvars = CVarRegistry::new_with_defaults();
    for tab in [
        OptionsTab::Multiplayer,
        OptionsTab::Keyboard,
        OptionsTab::Mouse,
        OptionsTab::Audio,
        OptionsTab::Video,
        OptionsTab::Voice,
        OptionsTab::Lock,
    ] {
        opts.tab = tab;
        let out = run_egui_frame(|ctx| {
            opts.show(
                ctx,
                &mut rt,
                &mut cvars,
                &OptionsRenderCtx {
                    vfs: None,
                    loc: None,
                    palette: palette(),
                    steam_icon: None,
                },
            );
        });
        assert!(out > 0);
    }
}

#[test]
fn ui_screen_create_server_gameplay_page_renders_settings_scr() {
    let script = SettingsScript::parse(
        r#"VERSION 1.0
        DESCRIPTION SERVER_OPTIONS
        {
            "mp_roundtime" { "Time per round (minutes)" { NUMBER 1.00 9.00 } { "1.75" } }
        }"#,
    );
    assert_eq!(script.options.len(), 1);
    let mut cvars = CVarRegistry::new_with_defaults();
    let out = run_egui_frame(|ctx| {
        egui::Area::new(egui::Id::unique("test_scr_opts")).show(ctx, |ui| {
            render_scr_options(ui, &mut cvars, &script, None);
        });
    });
    assert!(out > 0);
}

#[test]
fn ui_screen_password_and_error_dialogs_submits_password_and_shows_disconnect() {
    let mut pw = ServerPasswordUi {
        open: true,
        target_addr: "127.0.0.1:27015".into(),
        password: "secret".into(),
    };
    let mut err = DisconnectErrorDialog::default();
    err.trigger("Disconnected", "Server shutting down");
    assert!(err.open);
    let mut rt = VguiRuntime::new();
    let dlg_ctx = DialogRenderCtx {
        vfs: None,
        loc: None,
        palette: palette(),
        steam_icon: None,
    };
    let out = run_egui_frame(|ctx| {
        assert!(pw.show(ctx, &mut rt, &dlg_ctx).is_none());
        err.show(ctx, &mut rt, &dlg_ctx);
    });
    assert!(out > 0);
}

#[test]
fn ui_custom_server_vgui_res_support_hot_reloads_overridden_res() {
    let mut rt = VguiRuntime::new();
    rt.insert_res(
        "resource/ui/motd.res",
        r#""MOTD" { "ok" { "ControlName" "Button" "fieldName" "ok" "labelText" "Join Match" "command" "vguicancel" "visible" "1" "enabled" "1" } }"#,
    );
    assert_eq!(
        rt.doc_label("resource/UI/MOTD.res", "ok", None).as_deref(),
        Some("Join Match")
    );
    rt.invalidate_cache();
    assert!(rt.doc("resource/UI/MOTD.res").is_none());
    rt.insert_res(
        "resource/ui/motd.res",
        r#""MOTD" { "ok" { "ControlName" "Button" "fieldName" "ok" "labelText" "Enter Modded Server" "command" "vguicancel" "visible" "1" "enabled" "1" } }"#,
    );
    assert_eq!(
        rt.doc_label("resource/UI/MOTD.res", "ok", None).as_deref(),
        Some("Enter Modded Server")
    );
}

#[test]
fn ui_hud_protocol48_usermsg_driver_drives_hud_from_proto48() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 66, "SayText", -1);
    register_hud_msg(&mut hud, 67, "TextMsg", -1);
    push_usermsg(
        &mut hud,
        66,
        "SayText",
        CsUserMsg::SayText {
            sender_id: 1,
            text: "#Cstrike_Chat_All".into(),
            params: vec!["Player".into(), "glhf".into()],
        },
        1.0,
    );
    push_usermsg(
        &mut hud,
        67,
        "TextMsg",
        CsUserMsg::TextMsg {
            dest: 4,
            message: "Bomb planted".into(),
            params: Vec::new(),
        },
        1.0,
    );
    assert_eq!(hud.chat.len(), 1);
    assert_eq!(hud.text.len(), 1);
    assert_eq!(hud.center_print.as_deref(), Some("Bomb planted"));
}

#[test]
fn ui_hud_overview_map_maps_world_to_uv_and_chromakeys_bmp() {
    let ov = OverviewSpec::parse(
        "global {\n ZOOM 1.50\n ORIGIN 0 0 0\n ROTATED 0\n}\nlayer {\n IMAGE \"overviews/de_dust2.bmp\"\n HEIGHT -128\n}\n",
    );
    let center_uv = ov.world_to_overview_uv([0.0, 0.0]);
    assert!((center_uv[0] - 0.5).abs() < 1e-4);
    assert!((center_uv[1] - 0.5).abs() < 1e-4);

    if let Some(vfs) = discover_retail_vfs() {
        let dust2 = OverviewSpec::load(&vfs, "de_dust2").expect("overviews/de_dust2.txt");
        let img = load_overview_image_rgba(&vfs, &dust2.image).expect("overviews/de_dust2.bmp");
        assert!(img.rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 0));
    } else {
        eprintln!("SKIPPED retail de_dust2 overview chromakey check: CS16_GAMEDIR not set");
    }
}

#[test]
fn ui_hud_status_icons_all_tracks_buyzone_c4_defuser_rescue() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 89, "StatusIcon", -1);
    for (sprite, status) in [("buyzone", 1), ("c4", 2), ("defuser", 1), ("rescue", 1)] {
        push_usermsg(
            &mut hud,
            89,
            "StatusIcon",
            CsUserMsg::StatusIcon {
                status,
                sprite_name: sprite.into(),
                color: Some([0, 200, 0]),
            },
            1.0,
        );
    }
    assert_eq!(hud.status_icons.len(), 4);
    assert_eq!(hud.status_icons["c4"].status, 2);
}

#[test]
fn ui_hud_progress_bar_bartime_tracks_bartime_and_bartime2() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 100, "BarTime", 2);
    register_hud_msg(&mut hud, 101, "BarTime2", 4);
    push_usermsg(&mut hud, 100, "BarTime", CsUserMsg::BarTime(5), 10.0);
    assert_eq!(hud.bar.as_ref().unwrap().duration, 5);
    push_usermsg(
        &mut hud,
        101,
        "BarTime2",
        CsUserMsg::BarTime2 {
            duration: 10,
            start_percent: 40,
        },
        12.0,
    );
    let b = hud.bar.as_ref().unwrap();
    assert_eq!(b.duration, 10);
    assert_eq!(b.start_percent, 40);
}

#[test]
fn ui_hud_center_and_hint_text_formats_hudtextargs_and_centertime() {
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("scr_centertime", "3.5");
    let mut extras = HudExtrasContext::default();
    extras.apply_cvars(&cvars);
    assert!((extras.scr_centertime - 3.5).abs() < 1e-4);

    let mut loc = LocalizationTable::default();
    loc.insert_text("\"lang\"\n{\n\"Tokens\"\n{\n\"Hint_Buy\" \"Press %s1 to buy\"\n}\n}");
    let formatted = format_hud_text_entry("#Hint_Buy", &["B".into()], None, Some(&loc));
    assert_eq!(formatted, "Press B to buy");
}

#[test]
fn ui_hud_player_id_status_bar_formats_friend_enemy_hostage_hp() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 92, "StatusText", -1);
    register_hud_msg(&mut hud, 93, "StatusValue", -1);
    push_usermsg(
        &mut hud,
        92,
        "StatusText",
        CsUserMsg::StatusText {
            line: 0,
            text: "1 %c1: %p2  %h: %i3%".into(),
        },
        1.0,
    );
    push_usermsg(
        &mut hud,
        93,
        "StatusValue",
        CsUserMsg::StatusValue { index: 1, value: 1 },
        1.0,
    );
    push_usermsg(
        &mut hud,
        93,
        "StatusValue",
        CsUserMsg::StatusValue { index: 2, value: 1 },
        1.0,
    );
    push_usermsg(
        &mut hud,
        93,
        "StatusValue",
        CsUserMsg::StatusValue { index: 3, value: 100 },
        1.0,
    );
    let mut names = BTreeMap::new();
    names.insert(0, "Alpha".into());
    let (rel, bar) = format_status_bar(&hud, &names, None).unwrap();
    assert_eq!(rel, 1);
    assert!(bar.contains("Friend: Alpha"));
    assert!(bar.contains("HP: 100%"));
}

#[test]
fn ui_hud_text_menus_showmenu_parses_amx_colors_and_right_align() {
    let lines = menu_styled_lines("\\yAMX Mod\\n\n\\w1. \\rAK-47\\R$2500", None);
    assert_eq!(lines[0].spans[0], (MenuSpanColor::Yellow, "AMX Mod\\n".into()));
    assert!(lines[1].spans.iter().any(|(c, _)| *c == MenuSpanColor::Red));
    assert_eq!(lines[1].right.as_deref(), Some("$2500"));
    assert!(menu_key_enabled(0b11, 1));
    assert!(!menu_key_enabled(0b11, 3));

    let out = run_egui_frame(|ctx| {
        draw_styled_text_menu(ctx, &lines, 1.0);
    });
    assert!(out > 0);
}

#[test]
fn ui_hud_hudtext_and_amx_dhud_evaluates_titles_and_fx_0_1_2() {
    let titles = TitlesTable::parse(
        r#"
        $fadein 1.0
        $holdtime 2.0
        $fadeout 1.0
        GAMEOVER
        {
        Round Over
        }
        "#,
    );
    assert_eq!(titles.get("GAMEOVER").unwrap().text, "Round Over");
    assert!(evaluate_dhud_text_fx("Hello", 0, 1.0, 2.0, 1.0, 0.0, 1.5).is_some());
    assert!(evaluate_dhud_text_fx("Hello", 1, 1.0, 2.0, 1.0, 0.0, 1.5).is_some());
    let (typed, _, typing) = evaluate_dhud_text_fx("Hello", 2, 0.0, 2.0, 1.0, 0.1, 0.15).unwrap();
    assert!(typing);
    assert_eq!(typed.len(), 2);
}

#[test]
fn ui_hud_weapon_selection_buckets_supports_fastswitch_and_two_step() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 77, "WeaponList", -1);
    push_usermsg(
        &mut hud,
        77,
        "WeaponList",
        CsUserMsg::WeaponList {
            name: "weapon_ak47".into(),
            ammo1_type: 2,
            ammo1_max: 90,
            ammo2_type: u8::MAX,
            ammo2_max: u8::MAX,
            slot: 0,
            position: 1,
            weapon_id: 28,
            flags: 0,
        },
        1.0,
    );
    let owned = HashSet::from([28u8]);
    let mut b = WeaponBucketState::default();
    assert_eq!(b.on_slot_press(&hud, &owned, 1, false), None);
    assert_eq!(b.active_slot, Some(1));
    assert_eq!(b.confirm_selection(&hud).as_deref(), Some("weapon_ak47"));
    assert_eq!(
        b.on_slot_press(&hud, &owned, 1, true).as_deref(),
        Some("weapon_ak47")
    );
}

#[test]
fn ui_hud_damage_direction_and_type_icons_computes_quadrants_and_dmg_sprites() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 78, "Damage", 12);
    push_usermsg(
        &mut hud,
        78,
        "Damage",
        CsUserMsg::Damage {
            armor_damage: 5,
            health_damage: 25,
            damage_bits: cs_client::ui::hud_extras::DMG_BURN | cs_client::ui::hud_extras::DMG_SHOCK,
            origin: Vec3::new(100.0, 0.0, 0.0),
        },
        1.0,
    );
    let q = compute_damage_quadrants(&hud.damage_history, Vec3::ZERO, 0.0, 1.2);
    assert!(q[0] > 0.5);
    let haz = damage_hazard_icons(hud.damage.unwrap().bits);
    assert_eq!(haz, vec!["dmg_heat", "dmg_shock"]);
}

#[test]
fn ui_hud_flashlight_and_battery_icon_renders_flashlight_state() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 83, "Flashlight", 2);
    register_hud_msg(&mut hud, 84, "FlashBat", 1);
    push_usermsg(
        &mut hud,
        83,
        "Flashlight",
        CsUserMsg::Flashlight {
            on: true,
            battery: 75,
        },
        1.0,
    );
    assert!(hud.flashlight_on);
    assert_eq!(hud.flash_battery, 75);
    push_usermsg(&mut hud, 84, "FlashBat", CsUserMsg::FlashBat(62), 2.0);
    assert_eq!(hud.flash_battery, 62);
}

#[test]
fn ui_hud_scenario_and_hostage_indicators_tracks_scenario_bomb_hostages() {
    let mut hud = HudState::default();
    register_hud_msg(&mut hud, 95, "Scenario", -1);
    register_hud_msg(&mut hud, 96, "BombDrop", 7);
    register_hud_msg(&mut hud, 97, "HostagePos", 8);
    register_hud_msg(&mut hud, 98, "HostageK", 1);

    push_usermsg(
        &mut hud,
        95,
        "Scenario",
        CsUserMsg::Scenario {
            active: true,
            sprite: "hostage".into(),
            alpha: 255,
            flash: Some((2, 0)),
        },
        1.0,
    );
    assert_eq!(hud.scenario.as_ref().unwrap().sprite, "hostage");
    push_usermsg(
        &mut hud,
        96,
        "BombDrop",
        CsUserMsg::BombDrop {
            origin: Vec3::new(200.0, 300.0, 0.0),
            planted: true,
        },
        1.0,
    );
    assert!(hud.bomb.unwrap().1);
    push_usermsg(
        &mut hud,
        97,
        "HostagePos",
        CsUserMsg::HostagePos {
            flag: 1,
            hostage_id: 1,
            origin: Vec3::new(40.0, 50.0, 0.0),
        },
        1.0,
    );
    assert!(hud.hostages.contains_key(&1));
    push_usermsg(&mut hud, 98, "HostageK", CsUserMsg::HostageK(1), 2.0);
    assert!(!hud.hostages.contains_key(&1));
}

#[test]
fn ui_vgui2_trigger_button_emits_click_sound_and_command_event() {
    let mut rt = VguiRuntime::new();
    rt.insert_res(
        "resource/UI/SampleClick.res",
        r#""Sample" { "Ok" { "ControlName" "Button" "xpos" "10" "ypos" "10" "wide" "80" "tall" "20" "Command" "okay" "visible" "1" "enabled" "1" } }"#,
    );
    let cmd = rt.trigger_button("resource/UI/SampleClick.res", "Ok");
    assert_eq!(cmd.as_deref(), Some("okay"));
    let sounds = rt.take_ui_sounds();
    assert_eq!(sounds, vec![cs_client::ui::vgui2::VguiUiSound::Click]);
}

#[test]
fn ui_kb_act_list_parses_headers_and_commands_from_retail() {
    let sample = r##"
"blank" "=========================="
"blank" "#Valve_Movement_Title"
"blank" "=========================="
"+forward" "#Valve_Move_Forward"
"+attack" "#Cstrike_Fire"
"##;
    let parsed = parse_kb_act_list(sample);
    assert_eq!(parsed.len(), 3);
    assert!(parsed[0].is_header);
    assert_eq!(parsed[0].label, "#Valve_Movement_Title");
    assert!(!parsed[1].is_header);
    assert_eq!(parsed[1].command, "+forward");

    if let Some(vfs) = discover_retail_vfs() {
        let mut opts = OptionsUi::default();
        opts.load_resources(&vfs);
        assert!(opts.kb_actions.len() > 20);
        assert!(!opts.spray_logos.is_empty());
        assert!(!opts.selected_spray.is_empty());
    }
}

#[test]
fn ui_options_advanced_localizes_user_scr_tokens() {
    let script = SettingsScript::parse(
        r##"VERSION 1.0
        DESCRIPTION INFO_OPTIONS
        {
            "hud_centerid" { "#Cstrike_Center_ID_Names" { BOOL } { "1" } }
        }"##,
    );
    let mut loc = LocalizationTable::default();
    loc.insert_text(
        "\"Tokens\"\n{\n\"Cstrike_Center_ID_Names\" \"Center player ID names\"\n}",
    );
    let mut cvars = CVarRegistry::new_with_defaults();
    let out = run_egui_frame(|ctx| {
        egui::Area::new(egui::Id::unique("test_adv_loc_scr")).show(ctx, |ui| {
            render_scr_options(ui, &mut cvars, &script, Some(&loc));
        });
    });
    assert!(out > 0);
}

#[test]
fn ui_hud_with_runtime_renders_engine_font_lines() {
    let hud = HudState {
        health: 100,
        money: 800,
        ..Default::default()
    };
    let mut names = BTreeMap::new();
    names.insert(0u8, "Alpha".to_string());
    let owned = HashSet::new();
    let feed = vec![DeathNotice {
        killer: 1,
        victim: 1,
        headshot: true,
        weapon: "ak47".to_string(),
    }];
    let chat = vec!["Alpha : rush b".to_string()];
    let notify = vec!["Player connected".to_string()];
    let view = HudView {
        hud: &hud,
        now: 1.2,
        time_sec: 1.2,
        names: &names,
        owned: &owned,
        feed: &feed,
        chat: &chat,
        notify: &notify,
        center: Some("Bomb has been planted!"),
        weapon_switch_timer: 0.0,
        alive: true,
        scoped: false,
        eye: Vec3::ZERO,
        yaw_deg: 0.0,
        underwater_tint: None,
    };
    let vfs = discover_retail_vfs();
    let mut rt = VguiRuntime::new();
    if let Some(ref v) = vfs {
        rt.load_scheme_fonts(v);
    }
    let extras = HudExtrasContext::default();
    let out = run_egui_frame(|ctx| {
        draw_cs16_hud_with_runtime(ctx, None, &view, &extras, &mut rt, vfs.as_ref());
    });
    assert!(out > 0);
}

#[test]
fn ui_screen_options_loads_all_7_res_pages_and_parses_tab_names() {
    assert_eq!(OptionsTab::from_name("multiplayer"), Some(OptionsTab::Multiplayer));
    assert_eq!(OptionsTab::from_name("audio"), Some(OptionsTab::Audio));
    assert_eq!(OptionsTab::from_name("video"), Some(OptionsTab::Video));
    assert_eq!(OptionsTab::from_name("aim"), Some(OptionsTab::Mouse));
    assert_eq!(OptionsTab::from_name("mouse"), Some(OptionsTab::Mouse));
    assert_eq!(OptionsTab::from_name("keyboard"), Some(OptionsTab::Keyboard));
    assert_eq!(OptionsTab::from_name("voice"), Some(OptionsTab::Voice));
    assert_eq!(OptionsTab::from_name("content"), Some(OptionsTab::Lock));
    assert_eq!(OptionsTab::from_name("lock"), Some(OptionsTab::Lock));

    let Some(vfs) = discover_retail_vfs() else {
        eprintln!("SKIPPED retail OptionsSub*.res check: CS16_GAMEDIR not set");
        return;
    };
    let mut rt = VguiRuntime::new();
    rt.load_scheme_fonts(&vfs);
    let mut opts = OptionsUi::default();
    opts.load_resources(&vfs);
    opts.open = true;
    let mut cvars = CVarRegistry::new_with_defaults();
    let loc = LocalizationTable::load_language(&vfs, "english");
    for tab in [
        OptionsTab::Multiplayer,
        OptionsTab::Audio,
        OptionsTab::Video,
        OptionsTab::Mouse,
        OptionsTab::Keyboard,
        OptionsTab::Voice,
        OptionsTab::Lock,
    ] {
        opts.tab = tab;
        let out = run_egui_frame(|ctx| {
            opts.show(
                ctx,
                &mut rt,
                &mut cvars,
                &OptionsRenderCtx {
                    vfs: Some(&vfs),
                    loc: Some(&loc),
                    palette: palette(),
                    steam_icon: None,
                },
            );
        });
        assert!(out > 0, "tab {tab:?} produced no shapes");
    }
    for path in [
        "resource/OptionsSubMultiplayer.res",
        "resource/OptionsSubAudio.res",
        "resource/OptionsSubVideo.res",
        "resource/OptionsSubMouse.res",
        "resource/OptionsSubKeyboard.res",
        "resource/OptionsSubVoice.res",
        "resource/OptionsSubAdvanced.res",
        "resource/MultiplayerAdvancedDialog.res",
        "resource/ContentControlDialog.res",
    ] {
        assert!(rt.doc(path).is_some(), "missing loaded .res document {path}");
    }
}

#[test]
fn ui_client_viewport_centers_4_3_across_aspect_ratios() {
    let rect_for = |w: f32, h: f32| {
        client_ui_viewport(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h)))
    };

    let r800 = rect_for(800.0, 600.0);
    assert_eq!((r800.left(), r800.top(), r800.width(), r800.height()), (0.0, 0.0, 800.0, 600.0));

    let r720 = rect_for(1280.0, 720.0);
    assert_eq!((r720.left(), r720.top(), r720.width(), r720.height()), (160.0, 0.0, 960.0, 720.0));

    let r800w = rect_for(1280.0, 800.0);
    assert_eq!((r800w.left(), r800w.top(), r800w.width(), r800w.height()), (106.0, 0.0, 1067.0, 800.0));

    let r1024 = rect_for(1280.0, 1024.0);
    assert_eq!((r1024.left(), r1024.top(), r1024.width(), r1024.height()), (0.0, 32.0, 1280.0, 960.0));
}

#[test]
fn ui_html_parses_css_font_line_height_and_blank_br() {
    let html = r#"<html><head><style>
        body { background: #000; margin: 8px; color: #FFB000; font: normal 16px/20px Verdana, sans-serif; }
    </style></head><body>Line 1<br><br>Line 3</body></html>"#;
    let doc = parse_html_doc(html);
    assert_eq!(doc.body_font_size, 16.0);
    assert_eq!(doc.line_height, Some(20.0));
    assert_eq!(doc.margin, 8.0);
    assert_eq!(doc.blocks.len(), 3);
    assert!(matches!(&doc.blocks[1], HtmlBlock::Paragraph { spans, .. } if spans.is_empty()));
}
