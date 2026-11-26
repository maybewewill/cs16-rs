//! Phase 18 Stage 1B — Mandatory Full-Match Playability Walkthrough (`Tasks 7, 8, 9 & 10`).
//!
//! Exercises the real native `cs-client` + `LocalGame` + `ViewBuilder` + `GoldSrcMixer` + VGUI2
//! stack end-to-end on retail Steam `de_dust2` and `de_inferno` across all 7 live session steps:
//! 1. Main Menu -> `Options -> Video` (Cancel rollback + Apply) & `New Game -> Bots` (`bot_quota 9`).
//! 2. Join flow (`MOTD` -> `TeamMenu` -> `ClassMenu`) & clean spawn HUD (`$800`, 1:1 px, NO `[1..5]` bar).
//! 3. `B-4-2` keyboard + mouse VGUI2 buying (`BuyRifles`, `O-2`, `,`, `O-4`, weaponbox drop & sounds).
//! 4. `M4A1` & `USP` silencer persistence (3.1s+) + `AWP` scope (`40°/10°`) vs `AUG` zoom (`55°`).
//! 5. 1-to-1 dry audio (`snd_occlusion 0`, `room_type 0`), centered `300 ms` footsteps, silent walk,
//!    and `.mdl` `5004` weapon keyframe sounds (`boltpull`/`clipout`/`clipin`).
//! 6. ZBot freeze-time buys, `250 u/s` knife rollout, rifle switch, continuous steering, crimson blood
//!    (`r > 100, g < 30, b < 30`), `{blood` decals & killfeed on `de_dust2` and `de_inferno`.
//! 7. C4 plant (`BarTime(3)` -> `c4_beep` -> `%!MRAD_BOMBPL` -> CT bot defuse `%!MRAD_BOMBDEF` &
//!    next round) + local DeathCam (`80°` roll tilt) -> `Space`/`MOUSE1`/`MOUSE2` Spectator cycling.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_assets::scheme::{LocalizationTable, VguiColors};
use cs_assets::vgui_res::VguiCoordMode;
use cs_client::audio::{GoldSrcMixer, ListenerState};
use cs_client::input::button;
use cs_client::render::scene::ModelKind;
use cs_client::ui::hud_extras::WeaponBucketState;
use cs_client::ui::main_menu::{MainMenuRenderCtx, MainMenuUi};
use cs_client::ui::options::{OptionsRenderCtx, OptionsTab, OptionsUi, VideoApplyRequest};
use cs_client::ui::screens::{BuyClassTeamMenuUi, BuyMenuRenderCtx, VguiMenuKind};
use cs_client::ui::should_draw_dynamic_crosshair;
use cs_client::ui::vgui_theme::VguiPalette;
use cs_client::ui::vgui2::VguiRuntime;
use cs_client::view::{FrameView, MapFxWorld, ViewBuilder, ViewInputs};
use cs_clientcore::SoundCommand;
use cs_clientcore::sound::is_player_footstep_sound;
use cs_cvar::{CVarRegistry, EngineAction};
use cs_game::player::Team;
use cs_game::weapons::WeaponId;
use cs_local::{LocalGame, LocalOptions};
use cs_pm::collision::{Hull, WorldCollision};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_vfs::{Vfs, VfsConfig};
use egui::{Key, Pos2, Rect, Vec2};
use glam::Vec3;

fn discover_retail_vfs() -> Option<Vfs> {
    let explicit = std::env::var_os("CS16_GAMEDIR").map(PathBuf::from);
    Vfs::discover(
        explicit.as_deref(),
        VfsConfig {
            enable_hd_models: false,
            enable_downloads: false,
        },
    )
    .ok()
}

fn require_steam_map(map: &str) -> Option<Vfs> {
    let vfs = discover_retail_vfs()?;
    vfs.exists(&format!("maps/{map}.bsp")).then_some(vfs)
}

fn start_wired_local(
    vfs: &Vfs,
    map: &str,
    bots: u32,
) -> (LocalGame, ViewBuilder, Arc<WorldCollision>) {
    start_wired_local_cvars(vfs, map, bots, &[])
}

fn start_wired_local_cvars(
    vfs: &Vfs,
    map: &str,
    bots: u32,
    initial_cvars: &[(&str, &str)],
) -> (LocalGame, ViewBuilder, Arc<WorldCollision>) {
    let opts = LocalOptions {
        max_clients: 12,
        bots,
        player_name: "Walkthrough".to_string(),
        text_menus: false,
        initial_cvars: initial_cvars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        ..LocalOptions::default()
    };
    let mut game = LocalGame::start(vfs, map, &opts).expect("LocalGame::start");
    let bsp = Arc::new(BspMap::load(vfs, map).expect("load map BSP"));
    let coll = Arc::new(WorldCollision::from_bsp(&bsp));

    let mut view = ViewBuilder::new();
    view.set_map_bounds(Some(&coll));
    view.set_map_bsp(Some(Arc::clone(&bsp)));
    view.set_world_context(Some(Arc::clone(&coll)), Some(vfs.clone()));

    let conn = game.connection_mut();
    conn.frames_mut().set_remote_is_local(true);
    conn.set_world_collision(Some(Arc::clone(&coll)));
    if let Ok(mat) = vfs.read_bytes("sound/materials.txt") {
        conn.predictor_mut().set_materials(&mat);
    }
    conn.set_fx_world(Box::new(MapFxWorld::new(
        Some(Arc::clone(&coll)),
        Some(vfs.clone()),
        view.names(),
    )));
    (game, view, coll)
}

fn make_cmd(
    viewangles: Vec3,
    forwardmove: f32,
    sidemove: f32,
    buttons: u16,
    msec: u8,
) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        lerp_msec: 100,
        msec,
        viewangles,
        forwardmove,
        sidemove,
        upmove: 0.0,
        lightlevel: 0,
        buttons,
        impulse: 0,
        weaponselect: 0,
        impact_index: 0,
        impact_position: Vec3::ZERO,
    }
}

fn step_with_view(
    game: &mut LocalGame,
    view: &mut ViewBuilder,
    angles: Vec3,
    cmd: GoldSrcUserCmd,
) -> FrameView {
    game.connection_mut().queue_cmd(cmd);
    game.step();
    let inputs = ViewInputs {
        now: game.time(),
        frametime: 0.01,
        right_hand: true,
        ..ViewInputs::default()
    };
    let (fv, _) = view.build(game.connection_mut(), angles, &inputs);
    fv
}

fn run_buy_menu_key(
    ui: &mut BuyClassTeamMenuUi,
    rt: &mut VguiRuntime,
    vfs: &Vfs,
    loc: &LocalizationTable,
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
        assert!(
            (egui_ui.ctx().pixels_per_point() - 1.0).abs() < 1e-4,
            "VGUI2/HUD must run in 1:1 physical pixel space"
        );
        out_cmd = ui.show(
            egui_ui.ctx(),
            rt,
            &BuyMenuRenderCtx {
                vfs: Some(vfs),
                loc: Some(loc),
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

fn run_buy_menu_click(
    ui: &mut BuyClassTeamMenuUi,
    rt: &mut VguiRuntime,
    vfs: &Vfs,
    loc: &LocalizationTable,
    pos: Pos2,
    is_ct: bool,
    money: i32,
) -> Option<String> {
    let ctx = egui::Context::default();
    let mut out_cmd = None;
    for (step_idx, pressed) in [None, None, Some(true), Some(false)]
        .into_iter()
        .enumerate()
    {
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
            time: Some(step_idx as f64 * 0.05),
            ..Default::default()
        };
        if step_idx >= 1 {
            raw.events.push(egui::Event::PointerMoved(pos));
        }
        if let Some(pressed) = pressed {
            raw.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut out = ctx.run_ui(raw, |egui_ui| {
            if let Some(cmd) = ui.show(
                egui_ui.ctx(),
                rt,
                &BuyMenuRenderCtx {
                    vfs: Some(vfs),
                    loc: Some(loc),
                    map_name: "de_dust2",
                    is_ct,
                    allowed_keys: 0x3FF,
                    can_cancel: true,
                    money,
                },
            ) {
                out_cmd = Some(cmd);
            }
        });
        out.textures_delta.clear();
        if out_cmd.is_some() {
            break;
        }
    }
    out_cmd
}

fn join_team_and_spawn(game: &mut LocalGame, view: &mut ViewBuilder, team: u8, class: u8) {
    game.run_frames(8);
    game.connection_mut()
        .execute_local(&format!("jointeam {team}"));
    game.run_frames(8);
    game.connection_mut()
        .execute_local(&format!("joinclass {class}"));
    for _ in 0..40 {
        let fv = step_with_view(
            game,
            view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
        if fv.alive {
            break;
        }
    }
}

fn aim_angles(from: Vec3, to: Vec3) -> Vec3 {
    let d = to - from;
    let horiz = (d.x * d.x + d.y * d.y).sqrt().max(1e-4);
    let yaw = d.y.atan2(d.x).to_degrees();
    let pitch = (-d.z).atan2(horiz).to_degrees().clamp(-89.0, 89.0);
    Vec3::new(pitch, yaw, 0.0)
}

fn stereo_rms(samples: &[f32]) -> (f32, f32) {
    let mut sum_l = 0.0f64;
    let mut sum_r = 0.0f64;
    let mut n = 0usize;
    for pair in samples.as_chunks::<2>().0 {
        sum_l += f64::from(pair[0]) * f64::from(pair[0]);
        sum_r += f64::from(pair[1]) * f64::from(pair[1]);
        n += 1;
    }
    if n == 0 {
        return (0.0, 0.0);
    }
    (
        (sum_l / n as f64).sqrt() as f32,
        (sum_r / n as f64).sqrt() as f32,
    )
}

// -----------------------------------------------------------------------------------------------
// Step 1 — Main Menu -> Options -> Video (Cancel rollback + Apply) & New Game -> Bots
// -----------------------------------------------------------------------------------------------

#[test]
fn step1_options_video_and_new_game_bots_tab() {
    let Some(vfs) = require_steam_map("de_dust2") else {
        eprintln!("skipping step1_options_video_and_new_game_bots_tab: Steam de_dust2 absent");
        return;
    };
    let loc = LocalizationTable::load_language(&vfs, "english");
    let pal = VguiPalette::from(VguiColors::default());
    let mut cvars = CVarRegistry::new_with_defaults();
    let mut rt = VguiRuntime::new();

    // 1. Options -> Video: verify live GPU adapter/backend label, Cancel rollback, and Apply request.
    let mut opts = OptionsUi::default();
    opts.open = true;
    opts.tab = OptionsTab::Video;
    opts.load_resources(&vfs);
    opts.set_gpu_info("NVIDIA GeForce RTX 4090", "Vulkan");

    let run_opts = |opts: &mut OptionsUi, cvars: &mut CVarRegistry, rt: &mut VguiRuntime| {
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
                    vfs: Some(&vfs),
                    loc: Some(&loc),
                    palette: pal,
                    steam_icon: None,
                },
            );
        });
        out.textures_delta.clear();
    };

    // Initial open captures snapshot.
    cvars.set("gl_vsync", "0");
    cvars.set("cl_crosshair_size", "auto");
    cvars.set("gl_texturemode", "GL_LINEAR_MIPMAP_LINEAR");
    run_opts(&mut opts, &mut cvars, &mut rt);
    assert_eq!(opts.gpu_backend, "Vulkan");

    // Mutate CVars then Cancel -> snapshot reverts cleanly.
    cvars.set("gl_vsync", "1");
    cvars.set("cl_crosshair_size", "small");
    cvars.set("gl_texturemode", "GL_NEAREST");
    opts.cancel_and_close(&mut cvars);
    assert!(!opts.open);
    assert!(!cvars.get_bool("gl_vsync"));
    assert_eq!(cvars.get_str("cl_crosshair_size"), "auto");
    assert_eq!(cvars.get_str("gl_texturemode"), "GL_LINEAR_MIPMAP_LINEAR");

    // Re-open with updated target display CVars -> Apply emits VideoApplyRequest + persists CVars.
    cvars.set("width", "1920");
    cvars.set("height", "1080");
    cvars.set("fullscreen", "1");
    cvars.set("r_refdll", "vulkan");
    cvars.set("gl_vsync", "1");
    cvars.set("cl_crosshair_size", "small");
    cvars.set("cl_crosshair_color", "50 250 50");
    cvars.set("gl_texturemode", "GL_LINEAR");
    opts.open = true;
    opts.tab = OptionsTab::Video;
    run_opts(&mut opts, &mut cvars, &mut rt);
    opts.apply_changes(&mut cvars);
    assert!(opts.save_requested);
    assert_eq!(
        opts.video_apply_requested,
        Some(VideoApplyRequest {
            width: 1920,
            height: 1080,
            fullscreen: true,
            backend: "vulkan".to_string(),
        })
    );
    assert!(cvars.get_bool("gl_vsync"));
    assert_eq!(cvars.get_str("gl_texturemode"), "GL_LINEAR");
    assert_eq!(cvars.get_str("cl_crosshair_size"), "small");

    // 2. Main Menu -> New Game -> Bots tab: configure 9 Normal bots & game rules -> click Start.
    cvars.set("bot_quota", "9");
    cvars.set("bot_difficulty", "1");
    cvars.set("mp_startmoney", "1200");
    cvars.set("mp_freezetime", "4");
    cvars.set("mp_roundtime", "2.5");
    cvars.set("mp_buytime", "0.5");

    let mut menu = MainMenuUi::default();
    menu.load_resources(&vfs, "english");
    menu.open_new_game_bots_tab();
    let maps = vec!["de_dust2".to_string(), "de_inferno".to_string()];

    // Start button sits at dlg_min + (283, 427, 72, 24) inside the 448x460 centered 1280x720 dialog.
    let dlg_x = ((1280.0 - 448.0) * 0.5_f32).floor();
    let dlg_y = ((720.0 - 460.0) * 0.5_f32).floor();
    let start_pos = Pos2::new(dlg_x + 283.0 + 36.0, dlg_y + 427.0 + 12.0);
    let ctx = egui::Context::default();
    let mut actions = Vec::new();
    // Pass 0 lays out the dialog, Pass 1 hovers Start, Pass 2 presses, Pass 3 releases -> click!
    for (step_idx, event) in [None, None, Some(true), Some(false)]
        .into_iter()
        .enumerate()
    {
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
            time: Some(step_idx as f64 * 0.05),
            ..Default::default()
        };
        if step_idx >= 1 {
            raw.events.push(egui::Event::PointerMoved(start_pos));
        }
        if let Some(pressed) = event {
            raw.events.push(egui::Event::PointerButton {
                pos: start_pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut out = ctx.run_ui(raw, |ui| {
            let cmds = menu.show(
                ui.ctx(),
                &mut rt,
                &mut cvars,
                &MainMenuRenderCtx {
                    vfs: Some(&vfs),
                    loc: Some(&loc),
                    available_maps: &maps,
                    in_game: false,
                    bg_texture: None,
                    steam_icon: None,
                    palette: pal,
                },
            );
            actions.extend(cmds.actions);
        });
        out.textures_delta.clear();
    }

    let launched_map = actions
        .iter()
        .find_map(|a| match a {
            EngineAction::LoadMap(m) => Some(m.as_str()),
            _ => None,
        })
        .expect("clicking Start in New Game must emit EngineAction::LoadMap");
    assert!(
        launched_map == "de_dust2" || launched_map == "de_inferno",
        "unexpected map picked: {launched_map}"
    );
    assert_eq!(cvars.get_i32("bot_quota"), 9);
    assert_eq!(cvars.get_i32("bot_difficulty"), 1);

    // Apply those CVars onto a fresh LocalGame on de_dust2 (just like New Game -> Start) and verify 9 ZBots.
    let cvar_pairs: Vec<(&str, &str)> = [
        "bot_quota",
        "bot_difficulty",
        "mp_startmoney",
        "mp_freezetime",
        "mp_roundtime",
        "mp_buytime",
    ]
    .iter()
    .map(|&k| (k, cvars.get_str(k)))
    .collect();
    let (mut game, _view, _coll) = start_wired_local_cvars(&vfs, "de_dust2", 9, &cvar_pairs);
    let _ = game.console("bot_join_after_player 0");
    for _ in 0..700 {
        game.step();
        let bots = game
            .server()
            .client_infos()
            .into_iter()
            .filter(|c| c.is_bot)
            .count();
        if bots >= 9 {
            break;
        }
    }

    assert_eq!(game.server().cvar_i32("bot_quota"), 9);
    assert_eq!(game.server().cvar_i32("bot_difficulty"), 1);
    assert_eq!(game.server().cvar_i32("mp_startmoney"), 1200);
    assert_eq!(game.server().cvar_i32("mp_freezetime"), 4);
    let active_bots = game
        .server()
        .client_infos()
        .into_iter()
        .filter(|c| c.is_bot)
        .count();
    assert_eq!(active_bots, 9, "New Game -> Bots must spawn 9 live ZBots");
}

// -----------------------------------------------------------------------------------------------
// Step 2 — Join Flow (`MOTD` -> `TeamMenu` -> `ClassMenu`) & Clean Spawn HUD (No `[1..5]` bar)
// -----------------------------------------------------------------------------------------------

#[test]
fn step2_join_flow_and_clean_spawn_hud() {
    let Some(vfs) = require_steam_map("de_dust2") else {
        eprintln!("skipping step2_join_flow_and_clean_spawn_hud: Steam de_dust2 absent");
        return;
    };
    let loc = LocalizationTable::load_language(&vfs, "english");
    let mut rt = VguiRuntime::new();
    let (mut game, mut view, _coll) = start_wired_local(&vfs, "de_dust2", 0);
    let _ = game.console("mp_startmoney 800");
    let _ = game.console("mp_freezetime 5");
    game.run_frames(15);

    // MOTD arrives on join, then VGUI TeamMenu -> select `2` (CT) via real TeamMenu.res.
    let mut menu = BuyClassTeamMenuUi::default();
    menu.open_kind(VguiMenuKind::Team, true);
    let team_cmd = run_buy_menu_key(&mut menu, &mut rt, &vfs, &loc, Key::Num2, true, 800)
        .expect("pressing 2 on TeamMenu.res must emit jointeam 2");
    assert_eq!(team_cmd, "jointeam 2");
    game.connection_mut().execute_local(&team_cmd);
    game.run_frames(10);

    // ClassMenu_CT.res -> press `1` -> joinclass 1.
    menu.open_kind(VguiMenuKind::ClassCt, true);
    let class_cmd = run_buy_menu_key(&mut menu, &mut rt, &vfs, &loc, Key::Num1, true, 800)
        .expect("pressing 1 on ClassMenu_CT.res must emit joinclass 1");
    assert_eq!(class_cmd, "joinclass 1");
    game.connection_mut().execute_local(&class_cmd);

    let mut spawned = false;
    for _ in 0..40 {
        let fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
        if fv.alive {
            spawned = true;
            break;
        }
    }
    assert!(
        spawned,
        "player must spawn cleanly after TeamMenu -> ClassMenu"
    );

    let hud = &game.connection().fx().hud;
    assert_eq!(hud.money, 800, "spawn money must be $800");
    let owned = game.connection().client_data().map_or(0, |d| d.weapons);
    assert_ne!(owned & (1 << 16), 0, "CT must spawn with USP (id 16)");
    assert_ne!(owned & (1 << 29), 0, "CT must spawn with Knife (id 29)");

    // WeaponBucketState MUST stay inactive (no top `[1..5]` DrawWList bar) on spawn, buy, pickup, and `Q` (lastinv).
    let mut buckets = WeaponBucketState::default();
    assert!(buckets.active_slot.is_none());
    game.connection_mut().execute_local("lastinv");
    game.run_frames(6);
    assert!(
        buckets.active_slot.is_none(),
        "Q (lastinv) must NEVER activate the top [1..5] weapon bar"
    );
    // `hud_fastswitch 1` on a single-weapon slot switches immediately without leaving DrawWList open.
    let owned_set: HashSet<u8> = (1u8..=30).filter(|&id| (owned & (1 << id)) != 0).collect();
    let (picked, _) = buckets.on_slot_press_mode(&game.connection().fx().hud, &owned_set, 3, 1);
    assert_eq!(picked.as_deref(), Some("weapon_knife"));
    assert!(
        buckets.active_slot.is_none(),
        "hud_fastswitch 1 single-weapon slot press must NOT show top [1..5] weapon bar"
    );
}

// -----------------------------------------------------------------------------------------------
// Step 3 — `B-4-2` Keyboard & Mouse VGUI2 Buying + Equipment/Ammo + Weaponbox Drop & Pickup Sounds
// -----------------------------------------------------------------------------------------------

#[test]
fn step3_b42_keyboard_and_mouse_vgui2_buying() {
    let Some(vfs) = require_steam_map("de_dust2") else {
        eprintln!("skipping step3_b42_keyboard_and_mouse_vgui2_buying: Steam de_dust2 absent");
        return;
    };
    let loc = LocalizationTable::load_language(&vfs, "english");
    let mut rt = VguiRuntime::new();

    // Part A: Terrorist B -> 4 -> 2 (AK-47), O -> 2 (vesthelm), `,` (primammo), O -> 4 (hegren), weaponbox drop.
    let (mut game_t, mut view_t, _coll) = start_wired_local_cvars(
        &vfs,
        "de_dust2",
        0,
        &[("mp_startmoney", "16000"), ("mp_freezetime", "5")],
    );
    join_team_and_spawn(&mut game_t, &mut view_t, 1, 1);
    assert_eq!(game_t.connection().fx().hud.money, 16000);
    let _ = game_t.connection_mut().fx_mut().sounds.drain();

    // Press B (`buy`) -> server opens VGUIMenu(Buy).
    game_t.connection_mut().execute_local("buy");
    game_t.run_frames(5);
    let mut buy_ui = BuyClassTeamMenuUi::default();
    buy_ui.open_kind(VguiMenuKind::Buy, false);
    assert!(buy_ui.is_open());

    // Press `4` (Rifles) -> navigates MainBuyMenu.res -> BuyRifles_TER.res and STAYS OPEN!
    let nav_cmd = run_buy_menu_key(&mut buy_ui, &mut rt, &vfs, &loc, Key::Num4, false, 16000);
    assert_eq!(
        nav_cmd, None,
        "pressing 4 must navigate submenu, not send command"
    );
    assert!(
        buy_ui
            .active_res
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case("resource/UI/BuyRifles_TER.res")),
        "expected BuyRifles_TER.res, got {:?}",
        buy_ui.active_res
    );
    // Repeated VGUIMenu(Buy) sync next frame must NOT clobber BuyRifles_TER.res.
    buy_ui.open_kind(VguiMenuKind::Buy, false);
    assert!(
        buy_ui
            .active_res
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case("resource/UI/BuyRifles_TER.res"))
    );

    // Press `2` (`&2 CV-47`) -> emits `ak47` and closes menu.
    let ak_cmd = run_buy_menu_key(&mut buy_ui, &mut rt, &vfs, &loc, Key::Num2, false, 16000)
        .expect("pressing 2 on BuyRifles_TER.res must buy AK-47");
    assert_eq!(ak_cmd.to_ascii_lowercase(), "ak47");
    assert!(!buy_ui.is_open());

    game_t.connection_mut().execute_local(&ak_cmd);
    game_t.run_frames(10);
    assert_eq!(
        game_t
            .connection()
            .fx()
            .hud
            .current_weapon
            .map(|w| w.weapon_id),
        Some(28),
        "AK-47 must auto-equip on purchase"
    );
    assert_eq!(game_t.connection().fx().hud.money, 13500);

    // O -> 2 (`vesthelm`), `,` (`primammo`), O -> 4 (`hegren`).
    buy_ui.open_kind_fresh(VguiMenuKind::BuyEquipment, false);
    let vest_cmd = run_buy_menu_key(&mut buy_ui, &mut rt, &vfs, &loc, Key::Num2, false, 13500)
        .expect("O -> 2 must buy vesthelm");
    assert_eq!(vest_cmd.to_ascii_lowercase(), "vesthelm");
    game_t.connection_mut().execute_local(&vest_cmd);
    game_t.connection_mut().execute_local("primammo");

    buy_ui.open_kind_fresh(VguiMenuKind::BuyEquipment, false);
    let he_cmd = run_buy_menu_key(&mut buy_ui, &mut rt, &vfs, &loc, Key::Num4, false, 12500)
        .expect("O -> 4 must buy hegren");
    assert_eq!(he_cmd.to_ascii_lowercase(), "hegren");
    game_t.connection_mut().execute_local(&he_cmd);
    game_t.run_frames(10);

    assert_eq!(game_t.connection().fx().hud.armor, 100);
    let owned_t = game_t.connection().client_data().map_or(0, |d| d.weapons);
    assert_ne!(owned_t & (1 << 4), 0, "HE Grenade (id 4) must be owned");

    // Verify pickup sounds (`items/gunpickup2.wav` and `items/9mmclip1.wav`).
    let drained_t: Vec<String> = game_t
        .connection_mut()
        .fx_mut()
        .sounds
        .drain()
        .into_iter()
        .filter_map(|c| match c {
            SoundCommand::Start(s) => s.name,
            _ => None,
        })
        .collect();
    assert!(
        drained_t
            .iter()
            .any(|n| n.to_ascii_lowercase().contains("gunpickup2")),
        "buying rifle must play items/gunpickup2.wav; got {drained_t:?}"
    );
    assert!(
        drained_t
            .iter()
            .any(|n| n.to_ascii_lowercase().contains("9mmclip1")),
        "buying primammo must play items/9mmclip1.wav; got {drained_t:?}"
    );

    // Buy a second primary (`galil`) -> drops AK-47 as a `weaponbox` (`models/w_ak47.mdl`).
    game_t.connection_mut().execute_local("galil");
    game_t.run_frames(10);
    let dropped_ak = game_t
        .server()
        .edicts()
        .iter_live()
        .any(|(_, e)| e.vars.model.to_ascii_lowercase().contains("w_ak47.mdl"));
    assert!(
        dropped_ak,
        "buying second primary must drop previous weapon as w_ak47.mdl weaponbox"
    );

    // Part B: CT mouse click inside BuyRifles_CT.res buys M4A1.
    let (mut game_ct, mut view_ct, _coll) = start_wired_local_cvars(
        &vfs,
        "de_dust2",
        0,
        &[("mp_startmoney", "16000"), ("mp_freezetime", "5")],
    );
    join_team_and_spawn(&mut game_ct, &mut view_ct, 2, 1);

    let mut buy_ct = BuyClassTeamMenuUi::default();
    buy_ct.open_kind_fresh(VguiMenuKind::BuyRifles, true);
    let res_path = "resource/UI/BuyRifles_CT.res";
    rt.load_res(&vfs, res_path);
    // BackgroundPanel centers a 640x480 proportional canvas at 4:3 inside 1280x720 (`vp = (160, 0, 960, 720)`).
    let nodes = rt.compute_layout_mode(
        res_path,
        [160.0, 0.0, 960.0, 720.0],
        VguiCoordMode::Proportional640,
        Some(&loc),
    );
    let m4_node = nodes
        .iter()
        .find(|n| {
            n.command.eq_ignore_ascii_case("m4a1") || n.field_name.eq_ignore_ascii_case("m4a1")
        })
        .expect("BuyRifles_CT.res must contain m4a1 button");
    let click_pos = Pos2::new(
        m4_node.rect[0] + m4_node.rect[2] * 0.5,
        m4_node.rect[1] + m4_node.rect[3] * 0.5,
    );
    let m4_cmd = run_buy_menu_click(&mut buy_ct, &mut rt, &vfs, &loc, click_pos, true, 16000)
        .expect("mouse click on M4A1 in BuyRifles_CT.res must emit m4a1");
    assert_eq!(m4_cmd.to_ascii_lowercase(), "m4a1");
    game_ct.connection_mut().execute_local(&m4_cmd);
    game_ct.run_frames(10);
    assert_eq!(
        game_ct
            .connection()
            .fx()
            .hud
            .current_weapon
            .map(|w| w.weapon_id),
        Some(22),
        "M4A1 must auto-equip after mouse-click purchase"
    );
    assert_eq!(game_ct.connection().fx().hud.money, 12900);
}

// -----------------------------------------------------------------------------------------------
// Step 4 — Silencers (`M4A1` & `USP` persistence 3.1s+) & Scopes (`AWP` 40/10 vs `AUG` 55)
// -----------------------------------------------------------------------------------------------

#[test]
fn step4_silencers_and_scopes() {
    let Some(vfs) = require_steam_map("de_dust2") else {
        eprintln!("skipping step4_silencers_and_scopes: Steam de_dust2 absent");
        return;
    };
    let (mut game, mut view, _coll) = start_wired_local_cvars(
        &vfs,
        "de_dust2",
        0,
        &[("mp_startmoney", "16000"), ("mp_freezetime", "0")],
    );
    join_team_and_spawn(&mut game, &mut view, 2, 1);

    // 1. M4A1 silencer attach (`MOUSE2`) + wait 3.1s -> silenced idle (seq 0), fire (1..=3), reload (4).
    game.connection_mut().execute_local("m4a1");
    for _ in 0..110 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
    }
    // Press MOUSE2 to attach silencer.
    for _ in 0..4 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, button::ATTACK2, 10),
        );
    }
    // Wait 3.1 seconds (310 ticks @ 10 ms) for attach animation (2.0s) to finish and idle.
    let mut last_fv = FrameView::default();
    for _ in 0..310 {
        last_fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
    }
    let vm = last_fv.viewmodel.as_ref().expect("M4A1 viewmodel visible");
    assert!(vm.path.to_ascii_lowercase().contains("v_m4a1.mdl"));
    assert_eq!(
        vm.sequence, 0,
        "silenced M4A1 idle MUST stay on sequence 0 (never snap to unsilenced seq 7); got {}",
        vm.sequence
    );

    // Fire 1 silenced M4A1 shot -> sequence in 1..=3 (silenced shoot), then reload -> seq 4.
    let mut saw_m4_sil_fire = false;
    for _ in 0..8 {
        let fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, button::ATTACK, 10),
        );
        if let Some(v) = &fv.viewmodel
            && (1..=3).contains(&v.sequence)
        {
            saw_m4_sil_fire = true;
        }
    }
    assert!(
        saw_m4_sil_fire,
        "silenced M4A1 must play silenced fire sequence (1..=3)"
    );

    let mut saw_m4_sil_reload = false;
    for i in 0..25 {
        let btn = if i < 4 { button::RELOAD } else { 0 };
        let fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, btn, 10),
        );
        if let Some(v) = &fv.viewmodel
            && v.sequence == 4
        {
            saw_m4_sil_reload = true;
        }
    }
    assert!(
        saw_m4_sil_reload,
        "silenced M4A1 reload must play silenced reload sequence 4"
    );

    // 2. USP silencer attach (`MOUSE2`) + wait 3.1s -> silenced idle (seq 0, never unsilenced seq 8).
    game.connection_mut().execute_local("weapon_usp");
    for _ in 0..120 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
    }
    for _ in 0..4 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, button::ATTACK2, 10),
        );
    }
    for _ in 0..310 {
        last_fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
    }
    let usp_vm = last_fv.viewmodel.as_ref().expect("USP viewmodel visible");
    assert!(usp_vm.path.to_ascii_lowercase().contains("v_usp.mdl"));
    assert_eq!(
        usp_vm.sequence, 0,
        "silenced USP idle MUST stay on sequence 0 (never unsilenced seq 8); got {}",
        usp_vm.sequence
    );

    // 3. AWP zoom: 90° -> 40° -> 10° with sniper scope active, viewmodel hidden & crosshair hidden.
    game.connection_mut().execute_local("awp");
    for _ in 0..150 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
    }
    // Zoom level 1 (40°)
    for i in 0..40 {
        let btn = if i < 3 { button::ATTACK2 } else { 0 };
        last_fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, btn, 10),
        );
    }
    assert!(
        (last_fv.fov - 40.0).abs() < 1.0,
        "AWP 1st zoom must be 40 deg, got {}",
        last_fv.fov
    );
    assert!(last_fv.fov <= 40.0, "AWP zoom 1 activates sniper_scope");
    assert!(last_fv.viewmodel.is_none(), "AWP zoom must hide viewmodel");
    assert!(
        !should_draw_dynamic_crosshair(last_fv.fov, 18),
        "AWP zoom must hide regular crosshair"
    );

    // Zoom level 2 (10°)
    for i in 0..40 {
        let btn = if i < 3 { button::ATTACK2 } else { 0 };
        last_fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, btn, 10),
        );
    }
    assert!(
        (last_fv.fov - 10.0).abs() < 1.0,
        "AWP 2nd zoom must be 10 deg, got {}",
        last_fv.fov
    );
    assert!(last_fv.viewmodel.is_none());
    assert!(!should_draw_dynamic_crosshair(last_fv.fov, 18));

    // 4. AUG zoom: 55° WITHOUT black sniper scope overlay, normal crosshair STILL visible!
    game.connection_mut().execute_local("aug");
    for _ in 0..120 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
    }
    for i in 0..40 {
        let btn = if i < 3 { button::ATTACK2 } else { 0 };
        last_fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, btn, 10),
        );
    }
    assert!(
        (last_fv.fov - 55.0).abs() < 1.0,
        "AUG zoom must be 55 deg, got {}",
        last_fv.fov
    );
    assert!(
        last_fv.fov > 40.0,
        "AUG 55 deg zoom must NOT activate black sniper_scope overlay"
    );
    assert!(
        should_draw_dynamic_crosshair(last_fv.fov, 8),
        "AUG 55 deg zoom MUST keep normal crosshair visible"
    );
}

// -----------------------------------------------------------------------------------------------
// Step 5 — 1-to-1 Dry Audio, Centered 300ms Footsteps, Silent +speed Walk & .mdl 5004 Events
// -----------------------------------------------------------------------------------------------

#[test]
fn step5_audio_1to1_footsteps_and_5004_events() {
    let Some(vfs) = require_steam_map("de_dust2") else {
        eprintln!("skipping step5_audio_1to1_footsteps_and_5004_events: Steam de_dust2 absent");
        return;
    };
    // Verify GoldSrcMixer defaults: snd_occlusion == false & room_type == 0 dry.
    let mut mixer = GoldSrcMixer::new();
    assert!(
        !mixer.snd_occlusion(),
        "snd_occlusion must default to false"
    );
    assert_eq!(mixer.room_type(), 0, "room_type must default to 0 (dry)");

    let (mut game, mut view, _coll) = start_wired_local_cvars(
        &vfs,
        "de_dust2",
        0,
        &[("mp_startmoney", "16000"), ("mp_freezetime", "0")],
    );
    join_team_and_spawn(&mut game, &mut view, 2, 1);

    // Clear initial spawn sounds.
    let _ = game.connection_mut().fx_mut().sounds.drain();
    let _ = game.connection_mut().predictor_mut().take_sounds();

    let local_ent = game
        .connection()
        .player_slot()
        .map(|s| u16::from(s) + 1)
        .unwrap_or(1);

    // Pick the longest unobstructed run direction from CT spawn so we sustain > 150 u/s for > 1.2s.
    let start_pos = game
        .connection()
        .client_data()
        .map_or(Vec3::ZERO, |d| d.origin);
    let mut best_yaw = 90.0f32;
    let mut best_dist = 0.0f32;
    for deg in (0..360).step_by(15) {
        let rad = (deg as f32).to_radians();
        let dir = Vec3::new(rad.cos(), rad.sin(), 0.0);
        let tr = _coll.trace_world(
            start_pos + Vec3::new(0.0, 0.0, 4.0),
            start_pos + dir * 700.0 + Vec3::new(0.0, 0.0, 4.0),
            Hull::Standing,
        );
        let dist = tr.fraction * 700.0;
        if dist > best_dist {
            best_dist = dist;
            best_yaw = deg as f32;
        }
    }
    let run_angles = Vec3::new(0.0, best_yaw, 0.0);

    // Run at full speed (> 150 u/s) for 1.8s (180 ticks).
    let mut server_local_steps = 0usize;
    let mut pred_step_times = Vec::new();
    let mut pred_step_samples = Vec::new();
    for _ in 0..180 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            run_angles,
            make_cmd(run_angles, 250.0, 0.0, button::FORWARD, 10),
        );
        for sc in game.connection_mut().fx_mut().sounds.drain() {
            if let SoundCommand::Start(s) = sc
                && s.entity == local_ent
                && s.name.as_deref().is_some_and(is_player_footstep_sound)
            {
                server_local_steps += 1;
            }
        }
        for ps in game.connection_mut().predictor_mut().take_sounds() {
            if is_player_footstep_sound(&ps.sample) {
                pred_step_times.push(game.time());
                pred_step_samples.push(ps.sample);
            }
        }
    }
    assert_eq!(
        server_local_steps, 0,
        "local player must receive ZERO duplicate server footstep packets"
    );
    assert!(
        pred_step_times.len() >= 3,
        "running > 150 u/s for 1.6s must emit >= 3 predicted footsteps; got {:?}",
        pred_step_samples
    );
    for w in pred_step_times.windows(2) {
        let dt = w[1] - w[0];
        assert!(
            (0.24..=0.40).contains(&dt),
            "predicted footstep spacing must be ~300 ms, got {:.3}s",
            dt
        );
    }

    // Verify GoldSrcMixer renders local entity footstep purely centered (`left == right`) even with
    // an off-axis origin.
    let step_path = format!(
        "sound/{}",
        pred_step_samples[0].trim_start_matches("sound/")
    );
    mixer.update_listener_and_entities(
        ListenerState {
            origin: Vec3::ZERO,
            right: Vec3::NEG_Y,
            local_entity: Some(local_ent),
            waterlevel: 0,
        },
        &HashMap::new(),
        None,
    );
    let step_start = cs_clientcore::SoundStart::by_name(
        &step_path,
        local_ent,
        4,
        Vec3::new(0.0, 400.0, 0.0), // off-axis to prove local_entity ignores spatial pan
        1.0,
        0.8,
        100,
    );
    mixer.handle_sound_command(Some(&vfs), &SoundCommand::Start(step_start), None);
    let mut pcm = vec![0.0f32; 4096];
    mixer.mix_frames(&mut pcm);
    let (l_rms, r_rms) = stereo_rms(&pcm);
    assert!(
        l_rms > 1e-4,
        "real Steam footstep WAV must render non-silent PCM"
    );
    assert!(
        (l_rms - r_rms).abs() < 1e-4,
        "local footstep must be purely centered left==right ({l_rms} vs {r_rms})"
    );

    // Stop, let velocity decay to 0, then walk with `+speed` (<= 150 u/s) for 1.2s -> 0 footsteps!
    for _ in 0..40 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            run_angles,
            make_cmd(run_angles, 0.0, 0.0, 0, 10),
        );
    }
    let _ = game.connection_mut().predictor_mut().take_sounds();
    let walk_angles = Vec3::new(0.0, (best_yaw + 180.0) % 360.0, 0.0);
    let mut walk_steps = 0usize;
    for _ in 0..120 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            walk_angles,
            make_cmd(walk_angles, 110.0, 0.0, button::FORWARD | button::RUN, 10),
        );
        for ps in game.connection_mut().predictor_mut().take_sounds() {
            if is_player_footstep_sound(&ps.sample) {
                walk_steps += 1;
            }
        }
    }
    assert_eq!(
        walk_steps, 0,
        "+speed walking (<= 150 u/s) must emit ZERO footsteps"
    );

    // Verify `.mdl` `5004` weapon keyframe sounds (`boltpull`/`clipout`/`clipin`) fire on draw & reload!
    let mut anim_sounds = Vec::new();
    let _ = game.console("mp_buy_anywhere 1");
    game.connection_mut().execute_local("m4a1");
    game.connection_mut().execute_local("primammo");
    for _ in 0..110 {
        let fv = step_with_view(
            &mut game,
            &mut view,
            walk_angles,
            make_cmd(walk_angles, 0.0, 0.0, 0, 10),
        );
        anim_sounds.extend(fv.anim_sounds);
    }
    // Fire 2 shots then reload to trigger clipout + clipin + boltpull 5004 events.
    for i in 0..340 {
        let btn = if i < 6 {
            button::ATTACK
        } else if (12..16).contains(&i) {
            button::RELOAD
        } else {
            0
        };
        let fv = step_with_view(
            &mut game,
            &mut view,
            walk_angles,
            make_cmd(walk_angles, 0.0, 0.0, btn, 10),
        );
        anim_sounds.extend(fv.anim_sounds);
    }
    let lower_anim: Vec<String> = anim_sounds.iter().map(|s| s.to_ascii_lowercase()).collect();
    assert!(
        lower_anim.iter().any(|s| s.contains("clipout")),
        "M4A1 reload must fire 5004 clipout event; got {lower_anim:?}"
    );
    assert!(
        lower_anim.iter().any(|s| s.contains("clipin")),
        "M4A1 reload must fire 5004 clipin event; got {lower_anim:?}"
    );
    assert!(
        lower_anim.iter().any(|s| s.contains("boltpull")),
        "M4A1 draw/reload must fire 5004 boltpull event; got {lower_anim:?}"
    );
}

// -----------------------------------------------------------------------------------------------
// Step 6 — ZBot Combat & Crimson Blood on Real Steam `de_dust2` and `de_inferno`
// -----------------------------------------------------------------------------------------------

#[test]
fn step6_zbot_combat_and_crimson_blood_dust2_and_inferno() {
    for map in ["de_dust2", "de_inferno"] {
        let Some(vfs) = require_steam_map(map) else {
            eprintln!("skipping {map}: Steam map absent");
            continue;
        };
        let (mut game, mut view, coll) = start_wired_local_cvars(
            &vfs,
            map,
            0,
            &[
                ("mp_startmoney", "16000"),
                ("mp_freezetime", "2"),
                ("bot_join_after_player", "0"),
            ],
        );
        let _ = game.console("bot_quota 6");
        join_team_and_spawn(&mut game, &mut view, 2, 1);

        // Give bots 2.5s to join, buy rifles in freeze time, and start rolling out.
        let mut saw_bot_bought_rifle = false;
        let mut saw_bot_knife_rollout = false;
        let mut saw_smooth_non_cardinal_velocity = false;

        for _ in 0..380 {
            let _ = step_with_view(
                &mut game,
                &mut view,
                Vec3::ZERO,
                make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
            );
            let gs = game.cs_game().state();
            for (slot, maybe_p) in gs
                .players
                .iter()
                .enumerate()
                .take(game.server().config().max_clients as usize)
                .skip(1)
            {
                let Some(p) = maybe_p.as_ref() else {
                    continue;
                };
                if !p.alive {
                    continue;
                }
                let spent_on_loadout = p.money < 14_500;
                let active = p.active_item();
                let holding_primary = matches!(
                    active,
                    Some(
                        WeaponId::Ak47
                            | WeaponId::M4a1
                            | WeaponId::Galil
                            | WeaponId::Famas
                            | WeaponId::Aug
                            | WeaponId::Sg552
                            | WeaponId::Awp
                            | WeaponId::Scout
                            | WeaponId::Mp5n
                            | WeaponId::P90
                            | WeaponId::M3
                            | WeaponId::Xm1014
                            | WeaponId::M249
                            | WeaponId::Ump45
                            | WeaponId::Mac10
                            | WeaponId::Tmp
                            | WeaponId::G3sg1
                            | WeaponId::Sg550
                    )
                );
                if holding_primary {
                    saw_bot_bought_rifle = true;
                }
                if spent_on_loadout && active == Some(WeaponId::Knife) {
                    saw_bot_knife_rollout = true;
                }
                let id = game.server().edicts().client(slot as u8);
                if let Some(e) = game.server().edicts().get(id) {
                    let v = e.vars.velocity;
                    let spd = (v.x * v.x + v.y * v.y).sqrt();
                    if spd > 120.0 {
                        let ang = v.y.atan2(v.x).to_degrees().rem_euclid(45.0);
                        if ang > 4.0 && ang < 41.0 {
                            saw_smooth_non_cardinal_velocity = true;
                        }
                    }
                }
            }
        }
        assert!(
            saw_bot_bought_rifle,
            "{map}: ZBots must buy primary rifles during freeze time"
        );
        assert!(
            saw_bot_knife_rollout,
            "{map}: ZBots with primary rifles must pull out Knife for spawn rollout"
        );
        assert!(
            saw_smooth_non_cardinal_velocity,
            "{map}: ZBots must steer with smooth continuous non-8-way velocity vectors"
        );

        // Freeze bots, buy M4A1, position a Terrorist ZBot cleanly in front of the player, shoot & kill it!
        let _ = game.console("bot_stop 1");
        let _ = game.console("mp_buy_anywhere 1");
        game.connection_mut().execute_local("m4a1");
        for _ in 0..95 {
            let _ = step_with_view(
                &mut game,
                &mut view,
                Vec3::ZERO,
                make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
            );
        }

        let local_id = game.server().edicts().client(0);
        let local_pos = game
            .server()
            .edicts()
            .get(local_id)
            .map(|e| e.vars.origin)
            .unwrap_or(Vec3::ZERO);

        // Find a clear line 110 units away from player along the horizontal plane.
        let mut aim_dir = Vec3::X;
        for deg in (0..360).step_by(15) {
            let r = (deg as f32).to_radians();
            let d = Vec3::new(r.cos(), r.sin(), 0.0);
            let tr = coll.trace_world(
                local_pos + Vec3::new(0.0, 0.0, 18.0),
                local_pos + d * 140.0 + Vec3::new(0.0, 0.0, 18.0),
                Hull::Standing,
            );
            if tr.fraction >= 0.99 {
                aim_dir = d;
                break;
            }
        }
        let target_pos = local_pos + aim_dir * 100.0;
        let t_slot = (1..game.server().config().max_clients)
            .find(|&s| {
                game.cs_game().state().players[usize::from(s)]
                    .as_ref()
                    .is_some_and(|p| p.alive && p.team == Team::Terrorist)
            })
            .expect("alive Terrorist bot for combat blood verification");
        // Clear kevlar on target bot (authentic CS 1.6 TraceAttack suppresses chest blood on kevlar)
        // so chest rifle hits emit crimson TE_BLOODSPRITE + TraceBleed `{blood` decals.
        if let Some(p) = game.cs_game_mut().state_mut().players[usize::from(t_slot)].as_mut() {
            p.kevlar = cs_game::player::ArmorType::None;
            p.armor = 0.0;
        }
        assert!(game.server_mut().set_client_origin(t_slot, target_pos));

        let eye = local_pos + Vec3::new(0.0, 0.0, 28.0);
        let angles = aim_angles(eye, target_pos + Vec3::new(0.0, 0.0, 14.0));
        // Let bot position replicate to client before firing.
        for _ in 0..6 {
            let _ = step_with_view(
                &mut game,
                &mut view,
                angles,
                make_cmd(angles, 0.0, 0.0, 0, 10),
            );
        }

        let initial_deaths = game.connection().fx().hud.deaths.len();
        let mut saw_crimson_blood = false;
        let mut saw_blood_decal = false;
        for _ in 0..80 {
            let fv = step_with_view(
                &mut game,
                &mut view,
                angles,
                make_cmd(angles, 0.0, 0.0, button::ATTACK, 10),
            );
            let models = view.models();
            for ent in &fv.scene.entities {
                if let Some(m) = models.get(ent.model)
                    && m.kind == ModelKind::Sprite
                    && m.name.to_ascii_lowercase().contains("blood")
                {
                    let [r, g, b] = ent.rendercolor;
                    assert!(
                        r > 100 && g < 30 && b < 30,
                        "{map}: blood sprite {} MUST be crimson red, got RGB({r},{g},{b})",
                        m.name
                    );
                    saw_crimson_blood = true;
                }
            }
            if fv
                .decals
                .iter()
                .any(|d| d.decal_name.to_ascii_lowercase().contains("blood"))
            {
                saw_blood_decal = true;
            }
            if game.connection().fx().hud.deaths.len() > initial_deaths && saw_crimson_blood {
                break;
            }
        }
        assert!(
            saw_crimson_blood,
            "{map}: shooting a ZBot must render dark crimson red bloodspray.spr (r>100,g<30,b<30)"
        );
        assert!(
            saw_blood_decal,
            "{map}: shooting a ZBot must spawn {{blood decals via TraceBleed"
        );
        assert!(
            game.connection().fx().hud.deaths.len() > initial_deaths,
            "{map}: killing a ZBot must emit a DeathMsg killfeed entry"
        );
    }
}

// -----------------------------------------------------------------------------------------------
// Step 7 — C4 Plant/Defuse (`BarTime` + `c4_beep` + Radio) & DeathCam (`80°` Roll) / Spectator
// -----------------------------------------------------------------------------------------------

#[test]
fn step7_c4_plant_defuse_and_deathcam_spectator() {
    let Some(vfs) = require_steam_map("de_dust2") else {
        eprintln!("skipping step7_c4_plant_defuse_and_deathcam_spectator: Steam de_dust2 absent");
        return;
    };
    let (mut game, mut view, _coll) = start_wired_local_cvars(
        &vfs,
        "de_dust2",
        0,
        &[
            ("mp_freezetime", "0"),
            ("mp_c4timer", "45"),
            ("mp_forcecamera", "0"),
            ("mp_forcechasecam", "0"),
            ("mp_limitteams", "0"),
            ("mp_autoteambalance", "0"),
            ("bot_join_after_player", "0"),
            ("bot_stop", "1"),
        ],
    );
    join_team_and_spawn(&mut game, &mut view, 1, 1);
    let _ = game.console("bot_add_ct");
    let _ = game.console("bot_add_ct");
    game.run_frames(20);
    let _ = game.console("sv_restart 1");
    game.run_frames(240);

    // Local Terrorist (slot 0) is the sole Terrorist, so GiveC4 automatically issues C4 to slot 0!
    assert!(
        game.cs_game().state().players[0]
            .as_ref()
            .is_some_and(|p| p.has_c4),
        "sole Terrorist slot 0 must receive C4 at round start"
    );
    let site_pos = game
        .server()
        .edicts()
        .iter_live()
        .find_map(|(_, e)| {
            (e.vars.classname == "func_bomb_target").then(|| (e.vars.absmin + e.vars.absmax) * 0.5)
        })
        .expect("de_dust2 must have func_bomb_target");
    let ct_slot = (1..game.server().config().max_clients)
        .find(|&s| {
            game.cs_game().state().players[usize::from(s)]
                .as_ref()
                .is_some_and(|p| p.team == Team::Ct)
        })
        .expect("CT bot slot");
    assert!(game.server_mut().set_client_origin(0, site_pos));
    game.connection_mut().execute_local("weapon_c4");
    for _ in 0..35 {
        let _ = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, 0, 10),
        );
    }

    // Hold +attack to plant C4 (~3s) and wait for post-plant c4_beep ticks (~1.5s):
    let mut saw_plant_bartime = false;
    let mut saw_bombpl = false;
    let mut saw_c4_beep = false;
    for _ in 0..580 {
        let btn = if saw_bombpl { 0 } else { button::ATTACK };
        let fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, btn, 10),
        );
        if let Some(bar) = game.connection().fx().hud.bar
            && bar.duration >= 3
        {
            saw_plant_bartime = true;
        }
        if fv
            .radio_audio
            .iter()
            .any(|(_, c, _)| c.to_ascii_uppercase().contains("BOMBPL"))
        {
            saw_bombpl = true;
        }
        for sc in game.connection_mut().fx_mut().sounds.drain() {
            if let SoundCommand::Start(s) = sc
                && s.name
                    .as_deref()
                    .is_some_and(|n| n.to_ascii_lowercase().contains("c4_beep"))
            {
                saw_c4_beep = true;
            }
        }
        if saw_bombpl && saw_c4_beep {
            break;
        }
    }
    assert!(
        saw_plant_bartime,
        "planting C4 must display BarTime(3) progress bar"
    );
    assert!(saw_bombpl, "planting C4 must announce %!MRAD_BOMBPL");
    assert!(saw_c4_beep, "planted C4 must emit c4_beep sounds");

    // Bring CT bot with defuse kit near the planted C4 and unpause bots -> CT kills local T or defuses!
    // First test local player death cam (80° roll) & Spectator cycling while CT rotates to defuse!
    if let Some(ct) = game.cs_game_mut().state_mut().players[usize::from(ct_slot)].as_mut() {
        ct.has_defuser = true;
    }
    assert!(
        game.server_mut()
            .set_client_origin(ct_slot, site_pos + Vec3::new(90.0, 0.0, 0.0))
    );
    let _ = game.console("bot_stop 0");

    // Trigger local player death via `kill` (or CT combat) to verify classic 80° death roll tilt + Spectator cycling!
    game.connection_mut().execute_local("kill");
    let mut saw_death_roll_80 = false;
    let mut observed_modes = HashSet::new();
    let mut saw_bombdef = false;

    for i in 0..900 {
        // After 2.8s of DeathCam, press Space (`IN_JUMP`) to cycle spectator modes and MOUSE1/MOUSE2 to cycle targets.
        let btn = if i == 300 || i == 330 || i == 360 {
            button::JUMP
        } else if i == 315 {
            button::ATTACK
        } else if i == 345 {
            button::ATTACK2
        } else {
            0
        };
        let fv = step_with_view(
            &mut game,
            &mut view,
            Vec3::ZERO,
            make_cmd(Vec3::ZERO, 0.0, 0.0, btn, 10),
        );
        if !fv.alive && fv.roll_deg.abs() >= 75.0 {
            saw_death_roll_80 = true;
        }
        if !fv.alive && fv.observer_mode > 0 {
            observed_modes.insert(fv.observer_mode);
        }
        if fv
            .radio_audio
            .iter()
            .any(|(_, c, _)| c.to_ascii_uppercase().contains("BOMBDEF"))
        {
            saw_bombdef = true;
        }
        if saw_death_roll_80 && observed_modes.len() >= 2 && saw_bombdef {
            break;
        }
    }
    assert!(
        saw_death_roll_80,
        "local player death must apply classic 80° GoldSrc death camera roll tilt"
    );
    assert!(
        observed_modes.len() >= 2,
        "pressing Space after DeathCam must cycle Spectator modes; saw {observed_modes:?}"
    );
    assert!(
        saw_bombdef || game.cs_game().state().rules.bomb_defused,
        "CT bot must rotate to planted C4 and defuse it with %!MRAD_BOMBDEF"
    );
}
