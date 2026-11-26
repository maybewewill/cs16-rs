//! The window shell: `winit` events, the console and menus, and one [`Session`] (a local game or
//! a connection) that does the rest through `cs-clientcore`. Each frame the shell sends the
//! input as a usercmd, steps the session, draws the scene and the HUD the session built, and
//! plays the sounds it queued.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cs_assets::bsp::BspMap;
use cs_assets::demo_pack::{DEMO_MAP_NAME, build_instant_demo_pack};
use cs_assets::scheme::{LocalizationTable, SkyboxTextures, VguiColors, load_menu_background};
use cs_assets::vgui_scripts::TitlesTable;
use cs_clientcore::hud::{DeathNotice, HudState, VguiMenuState};
use cs_clientcore::{ConnState, ConsoleCommand, SoundCommand, ViewAngleUpdate};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_local::LocalOptions;
use cs_mod_host::{ClientModHost, ClientModTransfer};
use cs_pm::collision::WorldCollision;
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;
use tracing::{error, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use crate::audio::{AudioEngine, ListenerState};
use crate::session::Session;
use cs_client::camera::Camera;
use cs_client::hudtext::{format_message_with_titles, format_saytext_with_titles};
use cs_client::input::{
    CmdClock, CmdParams, ConsoleRoute, InputActionState, KeyboardLookParams, MoveSpeeds,
    build_usercmd, route_unknown, step_inventory_cycle_mode,
};
use cs_client::render::bsp_renderer::BspRenderStats;
use cs_client::render::lighting::{GpuLightsUniform, TextureFilterMode};
use cs_client::render::mdl_renderer::{StudioInstance, viewmodel_transform_with_angles};
use cs_client::render::scene::ModelRegistry;
use cs_client::render::{RenderSettings, Renderer, SceneDynamicData};
use cs_client::ui::chat::ChatInputUi;
use cs_client::ui::console::{ConsoleRenderCtx, ConsoleUi};
use cs_client::ui::hud::{
    HudAnimState, HudChatEntry, HudExtrasContext, HudSprites, HudView, PickupHistoryEntry,
    PickupKind, compute_hud_visibility, draw_cs16_hud_with_runtime, draw_voice_speakers,
};
use cs_client::ui::hud_extras::{
    OverviewMapTex, WeaponBucketSound, WeaponBucketState, draw_overview_map,
};
use cs_client::ui::main_menu::{MainMenuRenderCtx, MainMenuUi};
use cs_client::ui::mod_hud::{draw_mod_approval_prompt, draw_mod_hud};
use cs_client::ui::options::{OptionsRenderCtx, OptionsUi, VideoApplyRequest};
use cs_client::ui::scoreboard::{ScoreRow, ScoreboardRenderCtx, show_scoreboard_vgui};
use cs_client::ui::screens::{
    BuyClassTeamMenuUi, BuyMenuRenderCtx, CommandMenuUi, DemoPlayerUi, DemoUiAction,
    DialogRenderCtx, DisconnectErrorDialog, LoadingDialogUi, MotdRenderCtx, MotdVguiUi,
    PlayerListDialogUi, ServerPasswordUi, SpecSidebarPlayer, SpectatorMode, SpectatorRenderCtx,
    SpectatorVguiUi, VguiMenuKind, collect_spec_sidebar_players,
};
use cs_client::ui::server_menu::{
    draw_styled_text_menu_vgui, draw_text_menu_vgui, menu_key_enabled,
    menu_styled_lines_with_titles,
};
use cs_client::ui::vgui_assets::VguiAssetBank;
use cs_client::ui::vgui_theme::{VguiPalette, apply_vgui2_theme, paint_vgui_bevel};
use cs_client::ui::vgui2::{VguiRuntime, install_scheme_fonts_into_egui};
use cs_client::ui::{
    CrosshairInputs, CrosshairState, FrameTelemetry, KeyDisplayState, draw_crosshair_vgui_dt,
    draw_ljstats, draw_net_graph_with_telemetry, draw_r_speeds, draw_showkeys, draw_showpos,
    draw_showspeed, draw_top_left_console_overlays, should_draw_dynamic_crosshair,
};
use cs_client::view::{FrameView, ViewInputs, ViewStats};
use cs_client::view_effects::{calc_view_idle, calc_viewmodel_angles};
use cs_pm::collision::Hull;

const MAX_FEED: usize = 5;
const APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon-64.png");

fn decode_app_icon_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png).ok()?;
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w, h, rgba.into_raw()))
}

fn load_window_icon() -> Option<winit::window::Icon> {
    let (w, h, rgba) = decode_app_icon_rgba()?;
    winit::window::Icon::from_rgba(rgba, w, h).ok()
}

pub struct LaunchConfig {
    pub gamedir: Option<PathBuf>,
    pub demo: bool,
    pub startup_map: Option<String>,
    pub open_console: bool,
    pub startup_exec: Vec<String>,
    pub auto_screenshot: Option<PathBuf>,
    pub startup_ui: Option<String>,
    pub headless: bool,
    pub width: u32,
    pub height: u32,
    pub wait_camera: u32,
    pub wait_seconds: Option<f32>,
    pub record_dir: Option<PathBuf>,
    pub record_frames: u32,
    pub record_fps: f32,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            gamedir: None,
            demo: false,
            startup_map: None,
            open_console: false,
            startup_exec: Vec::new(),
            auto_screenshot: None,
            startup_ui: None,
            headless: false,
            width: 1280,
            height: 720,
            wait_camera: 1,
            wait_seconds: None,
            record_dir: None,
            record_frames: 0,
            record_fps: 24.0,
        }
    }
}

struct WindowRuntime {
    window: Option<Arc<Window>>,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    bg_texture: Option<egui::TextureHandle>,
    brand_logo: Option<egui::TextureHandle>,
    hud_sprites: Option<HudSprites>,
    vgui_bank: VguiAssetBank,
    cursor_captured: bool,
}

/// The HUD logs the shell times itself (the state keeps no clock for them).
#[derive(Default)]
struct HudFeeds {
    deaths: Vec<(DeathNotice, f64)>,
    chat: Vec<(HudChatEntry, f64)>,
    notify: Vec<(String, f64)>,
    pickups: Vec<PickupHistoryEntry>,
    last_hud_resets: u32,
    center: Option<(String, f64)>,
    menu_since: Option<(f64, String)>,
    motd_shown: bool,
    motd_open: bool,
}

impl HudFeeds {
    fn expire(&mut self, now: f64, feed_life: f64, chat_life: f64, center_life: f64) {
        if feed_life <= 0.0 {
            self.deaths.clear();
        } else {
            self.deaths.retain(|(_, t)| now - t <= feed_life);
        }
        if chat_life <= 0.0 {
            self.chat.clear();
        } else {
            self.chat.retain(|(_, t)| now - t <= chat_life);
        }
        let notify_life = feed_life.max(0.5);
        self.notify.retain(|(_, t)| now - t < notify_life);
        self.pickups.retain(|e| now < e.end_time);
        if self
            .center
            .as_ref()
            .is_some_and(|(_, t)| now - t > center_life)
        {
            self.center = None;
        }
    }
}

pub struct Cs16App {
    launch: LaunchConfig,
    vfs: Option<Vfs>,
    available_maps: Vec<String>,
    cvars: CVarRegistry,
    palette: VguiPalette,
    loc: Option<LocalizationTable>,
    audio: AudioEngine,
    audio_precache_warmed: bool,
    runtime: Option<WindowRuntime>,

    session: Session,
    camera: Camera,
    input_state: InputActionState,
    pending_impulse: u8,
    cmd_clock: CmdClock,
    mouse_sens_scale: f32,
    view_stats: ViewStats,
    last_stats: BspRenderStats,
    feeds: HudFeeds,
    hud_anim: HudAnimState,
    crosshair_state: CrosshairState,
    crosshair_shots_fired: i32,
    crosshair_last_weapon: u8,
    crosshair_last_clip: i32,
    prev_mouse_delta: (f32, f32),
    pending_mouse_delta: (f64, f64),
    frame_view: FrameView,
    active_backend_cvar: String,
    mod_transfer: ClientModTransfer,
    mod_host: ClientModHost,

    menu_ui: MainMenuUi,
    console_ui: ConsoleUi,
    options_ui: OptionsUi,
    chat_ui: ChatInputUi,
    vgui_rt: VguiRuntime,
    buy_team_class_ui: BuyClassTeamMenuUi,
    motd_ui: MotdVguiUi,
    spectator_ui: SpectatorVguiUi,
    command_menu_ui: CommandMenuUi,
    loading_ui: LoadingDialogUi,
    player_list_ui: PlayerListDialogUi,
    demo_player_ui: DemoPlayerUi,
    password_ui: ServerPasswordUi,
    disconnect_err_ui: DisconnectErrorDialog,
    weapon_buckets: WeaponBucketState,
    titles: TitlesTable,
    overview_tex: Option<OverviewMapTex>,
    overview_map_name: String,
    scoreboard_toggled: bool,
    startup_ui_applied: bool,
    in_game_frames: u64,

    start_time: Instant,
    last_frame: Instant,
    fps_smooth: f32,
    frame_ms_smooth: f32,
    frame_count: u64,
    pending_screenshot: Option<PathBuf>,
    exit_after_screenshot: bool,
    recorded_frames: u32,
    headless_fixed_dt: Option<f32>,
    config_path: PathBuf,
    recording_demo: Option<String>,
    pending_rcon: Option<PendingRcon>,
    last_connect_addr: Option<String>,
    demo_playlist: Vec<String>,
    demo_playlist_idx: usize,
    last_demo: Option<String>,
    console_notify_lines: Vec<(String, f64)>,
    last_log_seen: usize,
    strafe_sidemove: f32,
    voice_player_scales: [f32; 32],
    lj_tracker: cs_pm::ljstats::JumpTracker,
    last_ljstats: Option<(f64, cs_pm::ljstats::JumpStats)>,
    setup_gamedir_input: String,
    setup_error: Option<String>,
}

struct PendingRcon {
    socket: std::net::UdpSocket,
    target: std::net::SocketAddr,
    client: cs_clientcore::RconClient,
    deadline: std::time::Instant,
}

impl Cs16App {
    pub fn new(mut launch: LaunchConfig) -> Self {
        let mut cvars = CVarRegistry::new_with_defaults();
        let config_path = PathBuf::from("config.cfg");

        if let Ok(cfg_text) = std::fs::read_to_string(&config_path) {
            let _ = cvars.execute_script(&cfg_text);
            cvars.clear_dirty();
        }

        let vfs_cfg = VfsConfig {
            enable_hd_models: cvars.get_bool("cl_hd_models"),
            enable_downloads: true,
        };

        let initial_setup_dir = launch
            .gamedir
            .as_ref()
            .map(|p| p.display().to_string())
            .or_else(|| std::env::var("CS16_GAMEDIR").ok())
            .unwrap_or_default();
        let (vfs, available_maps, palette, loc, setup_error) = if launch.demo {
            let t0 = Instant::now();
            let pack = build_instant_demo_pack();
            let vfs = match Vfs::discover(launch.gamedir.as_deref(), vfs_cfg) {
                Ok(mut real) => {
                    let added = pack.mount_as_fallback(&mut real);
                    cvars.info(format!(
                        "Mounted Steam assets + {} clean-room Instant Demo fallback files in {:.1} ms.",
                        added,
                        t0.elapsed().as_secs_f32() * 1000.0
                    ));
                    real
                }
                Err(_) => {
                    let v = pack.to_vfs();
                    cvars.info(format!(
                        "Mounted clean-room Instant Demo ({} files, {} KiB) in {:.1} ms — zero Steam files needed.",
                        pack.file_count(),
                        pack.total_bytes() / 1024,
                        t0.elapsed().as_secs_f32() * 1000.0
                    ));
                    v
                }
            };
            let maps = vfs.list_maps();
            let colors = VguiColors::load(&vfs);
            let lang = cvars.get_str("cl_language").to_string();
            let loc = LocalizationTable::load_language(&vfs, &lang);
            let has_connect = launch
                .startup_exec
                .iter()
                .any(|c| c.trim_start().to_ascii_lowercase().starts_with("connect "));
            if launch.startup_map.is_none() && !has_connect {
                launch.startup_map = Some(DEMO_MAP_NAME.to_string());
            }
            if cvars.get_i32("bot_quota") <= 0 && !has_connect {
                cvars.set("bot_quota", "9");
            }
            if launch.headless && launch.startup_ui.is_none() && !has_connect {
                launch.startup_ui = Some("hud".to_string());
            }
            (Some(vfs), maps, VguiPalette::from(colors), Some(loc), None)
        } else {
            match Vfs::discover(launch.gamedir.as_deref(), vfs_cfg) {
                Ok(vfs) => {
                    let maps = vfs.list_maps();
                    let colors = VguiColors::load(&vfs);
                    cvars.info(format!(
                        "Mounted GoldSrc assets from: {}",
                        vfs.root().display()
                    ));
                    cvars.info(format!(
                        "Indexed {} files ({} maps). Click New Game or type `map de_dust2`.",
                        vfs.file_count(),
                        maps.len()
                    ));
                    let lang = cvars.get_str("cl_language").to_string();
                    let loc = LocalizationTable::load_language(&vfs, &lang);
                    (Some(vfs), maps, VguiPalette::from(colors), Some(loc), None)
                }
                Err(err) => {
                    let msg = format!("{err}");
                    cvars.error(&msg);
                    warn!("{msg}");
                    (
                        None,
                        Vec::new(),
                        VguiPalette::from(VguiColors::default()),
                        None,
                        Some(msg),
                    )
                }
            }
        };

        let mut console_ui = ConsoleUi::default();
        if launch.open_console {
            console_ui.open();
        }

        let mut menu_ui = MainMenuUi::default();
        let mut options_ui = OptionsUi::default();
        let mut vgui_rt = VguiRuntime::new();
        let mut spectator_ui = SpectatorVguiUi::default();
        let mut command_menu_ui = CommandMenuUi::default();
        let mut titles = TitlesTable::default();
        if let Some(v) = &vfs {
            let lang = cvars.get_str("cl_language").to_string();
            menu_ui.load_resources(v, &lang);
            options_ui.load_resources(v);
            vgui_rt.load_scheme_fonts(v);
            spectator_ui.load_assets(v);
            command_menu_ui.load_from_vfs(v);
            titles = TitlesTable::load(v);
        }

        let mut audio = AudioEngine::new();
        if let Some(v) = &vfs {
            audio.load_sentences(v);
        }

        let now = Instant::now();
        let last_log_seen = cvars.log_lines().len();
        let exit_after_screenshot = launch.auto_screenshot.is_some();
        let active_backend_cvar = cvars.get_str("r_refdll").to_ascii_lowercase();
        Self {
            launch,
            vfs,
            available_maps,
            cvars,
            palette,
            loc,
            audio,
            audio_precache_warmed: false,
            runtime: None,
            session: Session::none(),
            camera: Camera::default(),
            input_state: InputActionState::default(),
            pending_impulse: 0,
            cmd_clock: CmdClock::default(),
            mouse_sens_scale: 1.0,
            view_stats: ViewStats::default(),
            last_stats: BspRenderStats {
                draw_calls: 0,
                triangles: 0,
            },
            feeds: HudFeeds::default(),
            hud_anim: HudAnimState::default(),
            crosshair_state: CrosshairState::default(),
            crosshair_shots_fired: 0,
            crosshair_last_weapon: 0,
            crosshair_last_clip: 0,
            prev_mouse_delta: (0.0, 0.0),
            pending_mouse_delta: (0.0, 0.0),
            frame_view: FrameView::default(),
            active_backend_cvar,
            mod_transfer: ClientModTransfer::default(),
            mod_host: ClientModHost::new(),
            menu_ui,
            console_ui,
            options_ui,
            chat_ui: ChatInputUi::default(),
            vgui_rt,
            buy_team_class_ui: BuyClassTeamMenuUi::default(),
            motd_ui: MotdVguiUi::default(),
            spectator_ui,
            command_menu_ui,
            loading_ui: LoadingDialogUi::default(),
            player_list_ui: PlayerListDialogUi::default(),
            demo_player_ui: DemoPlayerUi::default(),
            password_ui: ServerPasswordUi::default(),
            disconnect_err_ui: DisconnectErrorDialog::default(),
            weapon_buckets: WeaponBucketState::default(),
            titles,
            overview_tex: None,
            overview_map_name: String::new(),
            scoreboard_toggled: false,
            startup_ui_applied: false,
            in_game_frames: 0,
            start_time: now,
            last_frame: now,
            fps_smooth: 100.0,
            frame_ms_smooth: 10.0,
            frame_count: 0,
            pending_screenshot: None,
            exit_after_screenshot,
            recorded_frames: 0,
            headless_fixed_dt: None,
            config_path,
            recording_demo: None,
            pending_rcon: None,
            last_connect_addr: None,
            demo_playlist: Vec::new(),
            demo_playlist_idx: 0,
            last_demo: None,
            console_notify_lines: Vec::new(),
            last_log_seen,
            strafe_sidemove: 0.0,
            voice_player_scales: [1.0; 32],
            lj_tracker: cs_pm::ljstats::JumpTracker::default(),
            last_ljstats: None,
            setup_gamedir_input: initial_setup_dir,
            setup_error,
        }
    }

    fn mount_gamedir(&mut self, dir: &Path) -> Result<(), String> {
        let vfs_cfg = VfsConfig {
            enable_hd_models: self.cvars.get_bool("cl_hd_models"),
            enable_downloads: true,
        };
        let vfs = Vfs::discover(Some(dir), vfs_cfg).map_err(|e| format!("{e}"))?;
        let maps = vfs.list_maps();
        let colors = VguiColors::load(&vfs);
        let palette = VguiPalette::from(colors);
        let lang = self.cvars.get_str("cl_language").to_string();
        let loc = LocalizationTable::load_language(&vfs, &lang);

        self.menu_ui.load_resources(&vfs, &lang);
        self.options_ui.load_resources(&vfs);
        self.vgui_rt.invalidate_cache();
        self.vgui_rt.load_scheme_fonts(&vfs);
        self.spectator_ui.load_assets(&vfs);
        self.command_menu_ui.load_from_vfs(&vfs);
        self.titles = TitlesTable::load(&vfs);
        self.audio.load_sentences(&vfs);

        if let Some(rt) = &mut self.runtime {
            apply_vgui2_theme(&rt.egui_ctx, palette);
            install_scheme_fonts_into_egui(&rt.egui_ctx, &vfs, &self.vgui_rt.scheme_fonts);
            rt.bg_texture = load_menu_background(&vfs).ok().map(|img| {
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width as usize, img.height as usize],
                    &img.rgba,
                );
                rt.egui_ctx.load_texture(
                    "cs16_menu_background",
                    color_image,
                    egui::TextureOptions::LINEAR,
                )
            });
            rt.hud_sprites = Some(HudSprites::load(&rt.egui_ctx, &vfs));
            rt.vgui_bank = VguiAssetBank::load(&rt.egui_ctx, Some(&vfs));
        }

        self.cvars.info(format!(
            "Mounted GoldSrc assets from: {} ({} files, {} maps)",
            vfs.root().display(),
            vfs.file_count(),
            maps.len()
        ));
        self.vfs = Some(vfs);
        self.available_maps = maps;
        self.palette = palette;
        self.loc = Some(loc);
        self.setup_error = None;
        Ok(())
    }

    fn launch_instant_demo(&mut self) {
        let t0 = Instant::now();
        let pack = build_instant_demo_pack();
        let vfs = match self.vfs.take() {
            Some(mut existing) => {
                pack.mount_as_fallback(&mut existing);
                existing
            }
            None => pack.to_vfs(),
        };
        let maps = vfs.list_maps();
        let colors = VguiColors::load(&vfs);
        let palette = VguiPalette::from(colors);
        let lang = self.cvars.get_str("cl_language").to_string();
        let loc = LocalizationTable::load_language(&vfs, &lang);

        self.menu_ui.load_resources(&vfs, &lang);
        self.options_ui.load_resources(&vfs);
        self.vgui_rt.invalidate_cache();
        self.vgui_rt.load_scheme_fonts(&vfs);
        self.spectator_ui.load_assets(&vfs);
        self.command_menu_ui.load_from_vfs(&vfs);
        self.titles = TitlesTable::load(&vfs);
        self.audio.load_sentences(&vfs);

        if let Some(rt) = &mut self.runtime {
            apply_vgui2_theme(&rt.egui_ctx, palette);
            rt.hud_sprites = Some(HudSprites::load(&rt.egui_ctx, &vfs));
            rt.vgui_bank = VguiAssetBank::load(&rt.egui_ctx, Some(&vfs));
        }

        self.cvars.info(format!(
            "Instant Demo ready ({} files, {} KiB in {:.1} ms) — starting 5v5 on {DEMO_MAP_NAME}.",
            pack.file_count(),
            pack.total_bytes() / 1024,
            t0.elapsed().as_secs_f32() * 1000.0
        ));
        self.vfs = Some(vfs);
        self.available_maps = maps;
        self.palette = palette;
        self.loc = Some(loc);
        self.setup_error = None;
        if self.cvars.get_i32("bot_quota") <= 0 {
            self.cvars.set("bot_quota", "9");
        }
        self.start_local_map(DEMO_MAP_NAME);
    }

    fn is_in_game(&self) -> bool {
        self.session.in_game()
    }

    fn issue_rcon_command(&mut self, args: &[String]) {
        use std::net::ToSocketAddrs;
        if args.is_empty() {
            self.cvars.warn("Usage: rcon <command>");
            return;
        }
        let password = self.cvars.get_str("rcon_password").to_string();
        if password.is_empty() {
            self.cvars.warn("rcon_password is empty");
            return;
        }
        let raw_addr = self.cvars.get_str("rcon_address").trim().to_string();
        let target = if !raw_addr.is_empty() {
            let with_port = if raw_addr.contains(':') {
                raw_addr
            } else {
                let port = self.cvars.get_str("rcon_port").trim();
                let port = if port.is_empty() { "27015" } else { port };
                format!("{raw_addr}:{port}")
            };
            match with_port
                .to_socket_addrs()
                .ok()
                .and_then(|mut it| it.find(std::net::SocketAddr::is_ipv4))
            {
                Some(sa) => sa,
                None => {
                    self.cvars.warn(format!("rcon: cannot resolve {with_port}"));
                    return;
                }
            }
        } else if let Some(sa) = self.session.remote_udp_addr() {
            sa
        } else {
            self.cvars
                .warn("rcon: connect to a remote server or set rcon_address");
            return;
        };
        let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else {
            self.cvars.warn("rcon: failed to bind UDP socket");
            return;
        };
        let _ = socket.set_nonblocking(true);
        let mut client = cs_clientcore::RconClient::new();
        let pkt = client.start_command(&password, &args.join(" "));
        let _ = socket.send_to(&pkt, target);
        self.pending_rcon = Some(PendingRcon {
            socket,
            target,
            client,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(3),
        });
    }

    fn poll_rcon(&mut self) {
        let Some(mut pending) = self.pending_rcon.take() else {
            return;
        };
        let mut buf = [0u8; 4096];
        while let Ok((len, from)) = pending.socket.recv_from(&mut buf) {
            if from != pending.target {
                continue;
            }
            if let Some(reply_pkt) = pending.client.on_datagram(&buf[..len]) {
                let _ = pending.socket.send_to(&reply_pkt, pending.target);
            }
        }
        for line in pending.client.take_replies() {
            for part in line.lines() {
                self.cvars.info(part);
            }
        }
        if std::time::Instant::now() < pending.deadline {
            self.pending_rcon = Some(pending);
        }
    }

    fn any_modal_open(&self) -> bool {
        self.menu_ui.visible
            || self.console_ui.open
            || self.options_ui.open
            || self.chat_ui.open
            || self.motd_ui.open
            || self.buy_team_class_ui.active_res.is_some()
            || self.command_menu_ui.open
            || self.player_list_ui.open
            || self.demo_player_ui.open
            || self.password_ui.open
            || self.disconnect_err_ui.open
            || !self.mod_transfer.pending_approvals().is_empty()
    }

    fn wants_mouse_capture(&self) -> bool {
        self.is_in_game() && !self.any_modal_open()
    }

    fn sync_cursor_capture(&mut self) {
        let want = self.wants_mouse_capture();
        if let Some(rt) = &mut self.runtime
            && rt.cursor_captured != want
        {
            if let Some(win) = &rt.window {
                if want {
                    let grabbed = win
                        .set_cursor_grab(CursorGrabMode::Locked)
                        .or_else(|_| win.set_cursor_grab(CursorGrabMode::Confined));
                    if grabbed.is_ok() {
                        win.set_cursor_visible(false);
                        rt.cursor_captured = true;
                    }
                } else {
                    let _ = win.set_cursor_grab(CursorGrabMode::None);
                    win.set_cursor_visible(true);
                    rt.cursor_captured = false;
                    self.input_state.clear();
                    self.pending_mouse_delta = (0.0, 0.0);
                }
            } else {
                rt.cursor_captured = want;
            }
        }
    }

    /// Loads `map_name` into the renderer and gives the session its collision.
    fn load_map(&mut self, map_name: &str) -> Result<(), String> {
        let Some(vfs) = &self.vfs else {
            return Err("Half-Life / CS 1.6 directory not mounted.".into());
        };
        let t0 = Instant::now();
        let map = BspMap::load(vfs, map_name).map_err(|e| e.to_string())?;
        let sky = SkyboxTextures::load(vfs, &map.skyname);
        let collision = Arc::new(WorldCollision::from_bsp(&map));
        if let Some(rt) = &mut self.runtime {
            rt.renderer.load_map(&map, &sky);
            if let Some(win) = &rt.window {
                win.set_title(&format!(
                    "Counter-Strike 1.6 (Rust) - {} [{}]",
                    map.name, rt.renderer.adapter_name
                ));
            }
        }
        // The prediction walks the map's hulls and the effects trace against it; the sound
        // surfaces come with the materials file.
        if let Ok(txt) = vfs.read_bytes("sound/materials.txt") {
            self.session.connection_predictor_materials(&txt);
        }
        self.session.set_map(&map.name, Some(collision));
        self.session.set_map_bsp(Some(Arc::new(map.clone())));
        self.feeds = HudFeeds::default();
        self.audio.stop_all();
        self.audio_precache_warmed = false;
        if let Some(rt) = &mut self.runtime {
            rt.renderer.set_model_registry(ModelRegistry::default());
            rt.renderer.clear_model_caches();
        }
        self.cvars.info(format!(
            "Loaded {} in {:.0} ms ({} faces, {} batches, sky: {})",
            map.name,
            t0.elapsed().as_secs_f32() * 1000.0,
            map.faces.len(),
            map.batches.len(),
            sky.skyname
        ));
        Ok(())
    }

    fn start_local_map(&mut self, map_name: &str) {
        self.cvars
            .info(format!("Starting local game on \"{map_name}\"..."));
        self.end_session();
        let Some(vfs) = self.vfs.clone() else {
            self.cvars
                .error("Half-Life / CS 1.6 directory not mounted.");
            return;
        };
        const LOCAL_SERVER_CVARS: &[&str] = &[
            "bot_quota",
            "bot_difficulty",
            "bot_join_team",
            "bot_chatter",
            "bot_join_after_player",
            "bot_stop",
            "mp_roundtime",
            "mp_freezetime",
            "mp_startmoney",
            "mp_buytime",
            "mp_timelimit",
            "mp_friendlyfire",
            "mp_c4timer",
            "mp_forcecamera",
            "mp_forcechasecam",
            "sv_gravity",
            "sv_airaccelerate",
            "sv_cheats",
            "mp_infinite_ammo",
            "mp_infinite_grenades",
            "mp_mirrordamage",
            "violence_hblood",
            "violence_ablood",
            "violence_hgibs",
            "violence_agibs",
            "sv_showimpacts",
            "sv_grenade_trajectory",
            "sv_grenade_trajectory_time",
            "sv_showhitboxes",
            "mp_buy_anywhere",
            "mp_warmup_mode",
            "sv_damage_print",
            "mp_knife_round",
        ];
        let initial_cvars: Vec<(String, String)> = LOCAL_SERVER_CVARS
            .iter()
            .filter_map(|&name| {
                let val = self.cvars.get_str(name);
                (!val.is_empty()).then(|| (name.to_string(), val.to_string()))
            })
            .collect();
        let opts = LocalOptions {
            bots: self.cvars.get_i32("bot_quota").clamp(0, 31) as u32,
            player_name: self.cvars.get_str("name").to_string(),
            hostname: self.cvars.get_str("hostname").to_string(),
            // `_vgui_menus 1` asks server for VGUIMenu dialogs; `_vgui_menus 0` uses ShowMenu.
            text_menus: !self.cvars.get_bool("_vgui_menus"),
            initial_cvars,
            ..LocalOptions::default()
        };
        match Session::start_local(&vfs, map_name, &opts) {
            Ok(session) => {
                self.session = session;
                for &name in LOCAL_SERVER_CVARS {
                    let val = self.cvars.get_str(name);
                    if !val.is_empty() {
                        let _ = self.session.server_console(&format!("{name} {val}"));
                    }
                }
                let lserver_cfg = self.cvars.get_str("lservercfgfile").trim().to_string();
                if !lserver_cfg.is_empty()
                    && let Ok(cfg_text) = vfs.read_string_lossy(&lserver_cfg)
                {
                    for line in cfg_text.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() && !trimmed.starts_with("//") {
                            let _ = self.session.server_console(trimmed);
                        }
                    }
                }
                if let Err(err) = self.load_map(map_name) {
                    error!(map = map_name, %err, "Failed to load BSP map");
                    self.cvars
                        .error(format!("Failed to load map \"{map_name}\": {err}"));
                    self.end_session();
                    return;
                }
                self.after_session_start();
            }
            Err(err) => {
                error!(map = map_name, %err, "Failed to start the local game");
                self.cvars
                    .error(format!("Failed to start \"{map_name}\": {err}"));
            }
        }
    }

    fn connect_remote(&mut self, addr: &str) {
        self.end_session();
        let name = self.cvars.get_str("name").to_string();
        let vgui_menus = self.cvars.get_bool("_vgui_menus");
        self.last_connect_addr = Some(addr.to_string());
        match Session::connect_remote(self.vfs.as_ref(), addr, &name, vgui_menus) {
            Ok(session) => {
                self.cvars.info(format!("Connecting to {addr}..."));
                self.session = session;
                self.menu_ui.find_servers_open = false;
                self.after_session_start();
            }
            Err(err) => self
                .cvars
                .error(format!("Failed to connect to {addr}: {err}")),
        }
    }

    fn after_session_start(&mut self) {
        self.feeds = HudFeeds::default();
        self.view_stats = ViewStats::default();
        self.weapon_buckets = WeaponBucketState::default();
        self.buy_team_class_ui.close();
        self.motd_ui.open = false;
        self.command_menu_ui.set_open(false);
        self.disconnect_err_ui.open = false;
        self.overview_tex = None;
        self.overview_map_name.clear();
        self.scoreboard_toggled = false;
        self.mod_transfer = ClientModTransfer::default();
        self.mod_host = ClientModHost::new();
        self.vgui_rt.invalidate_cache();
        if let Some(v) = &self.vfs {
            self.vgui_rt.load_scheme_fonts(v);
        }
        self.strafe_sidemove = 0.0;
        self.audio_precache_warmed = false;
        self.sync_client_cvars_to_session(true);
        self.menu_ui.visible = false;
        self.console_ui.open = false;
        self.options_ui.open = false;
        self.chat_ui.open = false;
        self.sync_cursor_capture();
    }

    /// Syncs `FCVAR_USERINFO` and mirrored client prediction/network CVars to `Session`.
    fn sync_client_cvars_to_session(&mut self, full: bool) {
        if !self.session.is_active() {
            return;
        }
        if full {
            let vars: Vec<(String, String)> = self
                .cvars
                .userinfo_vars()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            for (k, v) in vars {
                self.session.set_client_cvar(&k, &v);
            }
            let _ = self.cvars.take_dirty_userinfo();
        } else {
            for (k, v) in self.cvars.take_dirty_userinfo() {
                self.session.set_client_cvar(&k, &v);
            }
        }
        if self.session.is_local() {
            for (k, v) in self.cvars.take_dirty_server() {
                let _ = self.session.server_console(&format!("{k} {v}"));
            }
        } else {
            let _ = self.cvars.take_dirty_server();
        }
        for name in [
            "cl_cmdrate",
            "cl_timeout",
            "cl_nopred",
            "cl_nosmooth",
            "cl_smoothtime",
            "ex_interp",
            "cl_allowdownload",
            "cl_allowupload",
            "cl_shadows",
            "cl_minmodels",
            "cl_min_ct",
            "cl_min_t",
            "fastsprites",
            "max_smokepuffs",
        ] {
            let desired = self.cvars.get_str(name).to_string();
            if full || self.session.client_cvar(name).as_deref() != Some(desired.as_str()) {
                self.session.set_client_cvar(name, &desired);
            }
        }
    }

    /// Drops the session and the map; the menu comes back.
    fn end_session(&mut self) {
        self.pending_rcon = None;
        if !self.session.is_active() {
            return;
        }
        self.session.close();
        if let Some(rt) = &mut self.runtime {
            rt.renderer.unload_map();
            rt.renderer.set_entity_scene(None);
            rt.renderer.set_model_registry(ModelRegistry::default());
            rt.renderer.clear_model_caches();
            if let Some(win) = &rt.window {
                win.set_title("Counter-Strike 1.6 (Rust)");
            }
        }
        self.audio.stop_all();
        self.audio_precache_warmed = false;
        self.feeds = HudFeeds::default();
        self.hud_anim = HudAnimState::default();
        self.crosshair_state = CrosshairState::default();
        self.crosshair_shots_fired = 0;
        self.crosshair_last_weapon = 0;
        self.crosshair_last_clip = 0;
        self.view_stats = ViewStats::default();
        self.menu_ui.visible = true;
        self.chat_ui.open = false;
        self.buy_team_class_ui.close();
        self.motd_ui.open = false;
        self.command_menu_ui.set_open(false);
        self.loading_ui.visible = false;
        self.overview_tex = None;
        self.overview_map_name.clear();
        self.scoreboard_toggled = false;
        self.input_state.clear();
        self.pending_mouse_delta = (0.0, 0.0);
        self.pending_impulse = 0;
        self.cmd_clock = CmdClock::default();
        self.weapon_buckets = WeaponBucketState::default();
        self.mod_transfer = ClientModTransfer::default();
        self.mod_host = ClientModHost::new();
        self.cvars.set_forward_unknown(false);
        self.sync_cursor_capture();
    }

    fn save_config(&mut self) {
        let data = self.cvars.serialize_config();
        match std::fs::write(&self.config_path, data) {
            Ok(()) => {
                self.cvars.clear_dirty();
                self.cvars
                    .info(format!("Saved settings to {}", self.config_path.display()));
            }
            Err(err) => {
                self.cvars.error(format!(
                    "Failed to write {}: {err}",
                    self.config_path.display()
                ));
            }
        }
    }

    fn exec_cfg_file(&mut self, filename: &str) -> Vec<EngineAction> {
        let local_path = Path::new(filename);
        if let Ok(content) = std::fs::read_to_string(local_path) {
            self.cvars.info(format!("Executing {filename} (local)..."));
            return self.cvars.execute_script(&content);
        }
        if let Some(vfs) = &self.vfs
            && let Ok(content) = vfs.read_string_lossy(filename)
        {
            self.cvars.info(format!("Executing {filename} (VFS)..."));
            return self.cvars.execute_script(&content);
        }
        self.cvars.warn(format!("couldn't exec {filename}"));
        Vec::new()
    }

    /// Plays what the client core queued: server/event sounds, ambient DSP, material bullet
    /// impacts, radio/VOX sentences, reloads, and predicted footsteps using the reused
    /// `self.frame_view` (flat indexed `entity_positions`, zero `HashMap` hashing).
    fn play_sounds_from_frame_view(&mut self, local_entity: Option<u16>, sess_now: f64) {
        let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
        let suit_vol = finite_or(self.cvars.get_f32("suitvolume"), 0.25).clamp(0.0, 1.0);
        let listener_eye = self.frame_view.eye;
        let coll = self.session.collision().cloned();
        let occ = |pos: Vec3| {
            coll.as_ref()
                .is_some_and(|c| c.trace_world(listener_eye, pos, Hull::Point).fraction < 0.99)
        };
        self.audio.update_listener_indexed(
            ListenerState {
                origin: self.frame_view.eye,
                right: self.frame_view.listener_right,
                local_entity,
                waterlevel: self.frame_view.waterlevel,
            },
            &self.frame_view.entity_positions,
        );

        let occ_ref = coll.as_ref().map(|_| &occ as &dyn Fn(Vec3) -> bool);
        for cmd in self.session.take_sounds() {
            if let SoundCommand::RoomType(rt) = &cmd {
                self.cvars.set("room_type", rt.to_string());
            }
            self.audio
                .handle_sound_command(self.vfs.as_ref(), &cmd, occ_ref);
        }
        for impact in &self.frame_view.impact_sounds {
            self.audio.handle_sound_command(
                self.vfs.as_ref(),
                &SoundCommand::Start(impact.clone()),
                occ_ref,
            );
        }
        for (sender, code, pitch) in &self.frame_view.radio_audio {
            self.audio.play_send_audio_with_radar(
                self.vfs.as_ref(),
                code,
                master,
                *pitch,
                *sender,
                sess_now,
                &mut self.hud_anim.radio_flashes,
            );
        }
        for (vol, kind) in &self.frame_view.reload_sounds {
            self.audio.play_reload_sound(self.vfs.as_ref(), *vol, *kind);
        }
        for s in self.session.take_predicted_sounds() {
            if s.volume.is_finite() {
                let start = cs_clientcore::SoundStart::by_name(
                    &s.sample,
                    local_entity.unwrap_or(1),
                    s.channel.clamp(0, 7) as u8,
                    self.frame_view.eye,
                    s.volume,
                    s.attenuation,
                    s.pitch.clamp(2, 255) as u8,
                );
                self.audio.handle_sound_command(
                    self.vfs.as_ref(),
                    &SoundCommand::Start(start),
                    None,
                );
            }
        }
        let _ = suit_vol;
    }

    /// Moves what the HUD state collects into the timed logs the HUD draws.
    fn collect_hud_feeds(&mut self, now: f64) {
        let loc = self.loc.as_ref();
        let titles = Some(&self.titles);
        let names = self.session.player_names();
        let mute_enemy = self.cvars.get_bool("cl_mute_enemy_team");
        let local_slot1 = self.session.local_slot().map(|s| s + 1);
        let feeds = &mut self.feeds;
        let _ = self.session.with_hud_mut(|hud| {
            let local_team = local_slot1
                .and_then(|s| hud.players.get(&s))
                .map(|p| p.team.as_str())
                .filter(|t| *t == "CT" || *t == "TERRORIST")
                .map(str::to_string);
            for d in hud.deaths.drain(..) {
                feeds.deaths.push((d, now));
            }
            for line in hud.chat.drain(..) {
                let team = hud.players.get(&line.sender).map(|p| p.team.clone());
                if mute_enemy
                    && let (Some(lt), Some(st)) = (&local_team, &team)
                    && (st == "CT" || st == "TERRORIST")
                    && st != lt
                {
                    continue;
                }
                let text = format_saytext_with_titles(&line.text, &line.params, loc, titles);
                if !text.is_empty() {
                    feeds.chat.push((HudChatEntry { text, team }, now));
                }
            }
            if hud.hud_resets != feeds.last_hud_resets {
                feeds.last_hud_resets = hud.hud_resets;
                feeds.pickups.clear();
            }
            for wid in hud.weapon_pickups.drain(..) {
                let empty = matches!(hud.clips.get(&wid), Some(&0))
                    && hud.weapons.get(&wid).is_some_and(|w| {
                        w.ammo1_type != 255
                            && hud.ammo.get(&w.ammo1_type).copied().unwrap_or(0) == 0
                    });
                if let Some(existing) = feeds.pickups.iter_mut().find(
                    |e| matches!(e.kind, PickupKind::Weapon { weapon_id, .. } if weapon_id == wid),
                ) {
                    existing.kind = PickupKind::Weapon {
                        weapon_id: wid,
                        empty,
                    };
                    existing.end_time = now + 5.0;
                } else {
                    feeds.pickups.push(PickupHistoryEntry {
                        kind: PickupKind::Weapon {
                            weapon_id: wid,
                            empty,
                        },
                        end_time: now + 5.0,
                    });
                }
            }
            for (ammo_id, amount) in hud.ammo_pickups.drain(..) {
                if let Some(existing) = feeds.pickups.iter_mut().find(
                    |e| matches!(e.kind, PickupKind::Ammo { ammo_id: id, .. } if id == ammo_id),
                ) {
                    existing.kind = PickupKind::Ammo { ammo_id, amount };
                    existing.end_time = now + 5.0;
                } else {
                    feeds.pickups.push(PickupHistoryEntry {
                        kind: PickupKind::Ammo { ammo_id, amount },
                        end_time: now + 5.0,
                    });
                }
            }
            for classname in hud.item_pickups.drain(..) {
                if let Some(existing) = feeds.pickups.iter_mut().find(
                    |e| matches!(&e.kind, PickupKind::Item { classname: c } if c == &classname),
                ) {
                    existing.end_time = now + 5.0;
                } else {
                    feeds.pickups.push(PickupHistoryEntry {
                        kind: PickupKind::Item { classname },
                        end_time: now + 5.0,
                    });
                }
            }
            if feeds.pickups.len() > 12 {
                let excess = feeds.pickups.len() - 12;
                feeds.pickups.drain(..excess);
            }
            let mut got_dest4 = false;
            for line in hud.text.drain(..) {
                let is_chat = line.dest == 3 || line.dest == 5;
                let text = if is_chat {
                    format_saytext_with_titles(&line.message, &line.params, loc, titles)
                } else {
                    format_message_with_titles(&line.message, &line.params, loc, titles)
                };
                if text.is_empty() {
                    continue;
                }
                match line.dest {
                    4 => {
                        feeds.center = Some((text, now));
                        got_dest4 = true;
                    }
                    3 | 5 => {
                        let team = line.params.first().and_then(|sender_name| {
                            names
                                .iter()
                                .find(|(_, n)| *n == sender_name)
                                .and_then(|(slot0, _)| slot0.checked_add(1))
                                .and_then(|slot1| hud.players.get(&slot1))
                                .map(|p| p.team.clone())
                        });
                        feeds.chat.push((HudChatEntry { text, team }, now));
                    }
                    1 => feeds.notify.push((text, now)),
                    _ => {}
                }
            }
            if let Some(text) = hud.center_print.take()
                && !got_dest4
            {
                let text = format_message_with_titles(&text, &[], loc, titles);
                feeds.center = Some((text, now));
            }
            if hud
                .menu
                .as_ref()
                .is_some_and(|m| !m.multipart && (m.keys == 0 || m.text.is_empty()))
            {
                hud.menu = None;
            }
            // A menu that came: remember when (once complete), so it expires by its display time.
            match (&hud.menu, &feeds.menu_since) {
                (Some(m), _) if m.multipart => feeds.menu_since = None,
                (Some(m), Some((_, t))) if *t == m.text => {}
                (Some(m), _) => feeds.menu_since = Some((now, m.text.clone())),
                (None, _) => feeds.menu_since = None,
            }
            if let Some(m) = &hud.menu
                && !m.multipart
                && m.display_time > 0
                && let Some((since, _)) = &feeds.menu_since
                && now - since > f64::from(m.display_time)
            {
                hud.menu = None;
            }
        });
        trim_feed(&mut self.feeds.deaths);
        trim_feed(&mut self.feeds.chat);
        trim_feed(&mut self.feeds.notify);
        let feed_life = f64::from(finite_or(self.cvars.get_f32("hud_deathnotice_time"), 6.0));
        let chat_life = f64::from(finite_or(self.cvars.get_f32("hud_saytext_time"), 5.0));
        let center_life = f64::from(finite_or(self.cvars.get_f32("scr_centertime"), 2.0).max(0.5));
        self.feeds.expire(now, feed_life, chat_life, center_life);

        // The MOTD pops up once, when it is whole.
        if !self.feeds.motd_shown
            && self
                .session
                .with_hud(|h| h.motd_complete && !h.motd.is_empty())
                .unwrap_or(false)
            && self.is_in_game()
        {
            self.feeds.motd_shown = true;
            self.feeds.motd_open = true;
            self.motd_ui.open = true;
        }
    }

    fn forward_to_server(&mut self, line: &str) {
        if self.session.in_game() {
            self.session.send_stringcmd(line);
        } else {
            self.cvars.warn(format!("Not connected: \"{line}\""));
        }
    }

    fn process_actions(
        &mut self,
        actions: Vec<EngineAction>,
        event_loop: Option<&ActiveEventLoop>,
    ) -> bool {
        let mut queue = actions;
        let mut guard = 0usize;
        while !queue.is_empty() && guard < 32 {
            guard += 1;
            let batch = std::mem::take(&mut queue);
            for action in batch {
                match action {
                    EngineAction::LoadMap(name) => self.start_local_map(&name),
                    EngineAction::Disconnect => {
                        self.end_session();
                        self.cvars.info("Disconnected to main menu.");
                    }
                    EngineAction::Quit => {
                        if self.cvars.is_dirty() {
                            self.save_config();
                        }
                        self.end_session();
                        if let Some(el) = event_loop {
                            el.exit();
                        }
                        return true;
                    }
                    EngineAction::ExecFile(file) => queue.extend(self.exec_cfg_file(&file)),
                    EngineAction::WriteConfig => self.save_config(),
                    EngineAction::ToggleConsole => {
                        self.console_ui.toggle();
                        self.sync_cursor_capture();
                    }
                    EngineAction::ToggleMenu => {
                        let has_text_menu = self
                            .session
                            .with_hud(|h| {
                                h.menu.as_ref().is_some_and(|m| {
                                    !m.multipart && m.keys != 0 && !m.text.is_empty()
                                })
                            })
                            .unwrap_or(false);
                        if self.console_ui.open {
                            self.console_ui.open = false;
                        } else if self.motd_ui.open {
                            self.motd_ui.open = false;
                            self.feeds.motd_open = false;
                        } else if self.buy_team_class_ui.is_open() {
                            self.buy_team_class_ui.close();
                            self.session.with_hud_mut(|h| h.vgui_menu = None);
                        } else if has_text_menu {
                            self.session.with_hud_mut(|h| h.menu = None);
                        } else if self.weapon_buckets.active_slot.is_some() {
                            self.weapon_buckets.close();
                        } else if self.command_menu_ui.open {
                            self.command_menu_ui.set_open(false);
                        } else if self.player_list_ui.open {
                            self.player_list_ui.open = false;
                        } else if self.demo_player_ui.open {
                            self.demo_player_ui.open = false;
                        } else if self.password_ui.open {
                            self.password_ui.open = false;
                        } else if self.disconnect_err_ui.open {
                            self.disconnect_err_ui.open = false;
                        } else if self.chat_ui.open {
                            self.chat_ui.open = false;
                        } else if self.options_ui.open {
                            self.options_ui.cancel_and_close(&mut self.cvars);
                        } else if self.menu_ui.new_game_open {
                            self.menu_ui.new_game_open = false;
                        } else if self.menu_ui.find_servers_open {
                            self.menu_ui.find_servers_open = false;
                        } else if self.session.is_active() {
                            self.menu_ui.visible = !self.menu_ui.visible;
                        }
                        self.sync_cursor_capture();
                    }
                    EngineAction::Connect(addr) => self.connect_remote(&addr),
                    EngineAction::Screenshot(custom_path) => {
                        let path = custom_path.map(PathBuf::from).unwrap_or_else(|| {
                            let map = self.session.loaded_map().unwrap_or("menu");
                            PathBuf::from(format!("screenshot_{map}_{}.png", self.frame_count))
                        });
                        self.cvars
                            .info(format!("Capturing screenshot to {}...", path.display()));
                        self.pending_screenshot = Some(path);
                    }
                    EngineAction::ActionInput { name, pressed } => {
                        if name == "commandmenu" {
                            self.command_menu_ui.set_open(pressed);
                            self.sync_cursor_capture();
                        }
                        self.input_state.apply_action_input(name, pressed);
                    }
                    EngineAction::GetPos => {
                        let p = self.camera.position;
                        self.cvars.info(format!(
                            "setpos {:.2} {:.2} {:.2}; // yaw {:.1} pitch {:.1}",
                            p.x, p.y, p.z, self.camera.yaw_deg, self.camera.pitch_deg
                        ));
                    }
                    EngineAction::Status => {
                        let state = self.session.state();
                        let map = self.session.loaded_map().unwrap_or("<none>").to_string();
                        self.cvars.info(format!(
                            "map: {map} | state: {state:?} | slots: {} | entities: {} ({} brush, {} studio, {} sprite, {} temp, {} beams) | fps: {:.1}",
                            self.session.max_clients(),
                            self.view_stats.entities,
                            self.view_stats.brush,
                            self.view_stats.studio,
                            self.view_stats.sprites,
                            self.view_stats.tempents,
                            self.view_stats.beams,
                            self.fps_smooth
                        ));
                    }
                    // Commands the server runs: the client sends them as `clc_stringcmd`.
                    EngineAction::OpenBuyMenu => self.forward_to_server("buy"),
                    EngineAction::OpenBuyEquipMenu => self.forward_to_server("buyequip"),
                    EngineAction::BuyItem(item) => self.forward_to_server(&item),
                    EngineAction::ChooseTeam => self.forward_to_server("chooseteam"),
                    EngineAction::GiveWeapon(w) => self.forward_to_server(&format!("give {w}")),
                    EngineAction::ToggleGod => self.forward_to_server("god"),
                    EngineAction::ToggleNoclip => self.forward_to_server("noclip"),
                    EngineAction::Suicide => self.forward_to_server("kill"),
                    EngineAction::DropWeapon => self.forward_to_server("drop"),
                    EngineAction::LastInv => self.forward_to_server("lastinv"),
                    EngineAction::InvNext => self.cycle_inventory(true),
                    EngineAction::InvPrev => self.cycle_inventory(false),
                    EngineAction::ToggleNightVision => self.forward_to_server("nightvision"),
                    EngineAction::OpenRadio(group) => {
                        self.forward_to_server(&format!("radio{group}"));
                    }
                    EngineAction::Say { team_only, text } => {
                        let cmd = if team_only { "say_team" } else { "say" };
                        self.forward_to_server(&format!("{cmd} \"{}\"", quote_chat(&text)));
                    }
                    EngineAction::OpenChat { team_only } => {
                        if self.is_in_game() {
                            self.chat_ui.open_chat(team_only);
                            self.sync_cursor_capture();
                        }
                    }
                    EngineAction::SelectSlot(slot) => self.select_slot(slot),
                    EngineAction::Impulse(n) => self.pending_impulse = n,
                    EngineAction::Forward(tokens) => {
                        if let Some(cmd) = tokens.first().map(String::as_str)
                            && cmd.eq_ignore_ascii_case("rcon")
                        {
                            self.issue_rcon_command(&tokens[1..]);
                            continue;
                        }
                        if let Some(cmd) = tokens.first().map(String::as_str)
                            && (cmd == "mod_approve" || cmd == "mod_deny")
                        {
                            if let Some(id) = tokens.get(1) {
                                let ok = if cmd == "mod_approve" {
                                    self.mod_transfer.approve(id)
                                } else {
                                    self.mod_transfer.deny(id)
                                };
                                if ok {
                                    self.sync_cursor_capture();
                                } else {
                                    self.cvars.warn(format!("unknown pending mod `{id}`"));
                                }
                            } else {
                                self.cvars.warn(format!("usage: {cmd} <mod_id>"));
                            }
                            continue;
                        }
                        if let Some(name) = tokens.first()
                            && self.session.has_client_cvar(name)
                        {
                            if let Some(val) = tokens.get(1) {
                                self.session.set_client_cvar(name, val);
                            } else if let Some(cur) = self.session.client_cvar(name) {
                                self.cvars.info(format!("\"{name}\" is \"{cur}\""));
                            }
                            continue;
                        }
                        match route_unknown(&tokens) {
                            ConsoleRoute::Server(line) => self.forward_to_server(&line),
                            ConsoleRoute::Local(line) => self.session.execute_local(&line),
                            ConsoleRoute::Denied => {
                                self.cvars.warn("that command stays on this machine");
                            }
                        }
                    }
                    EngineAction::SetPos(_) => {
                        self.cvars
                            .warn("setpos needs the server's cheats; use noclip");
                    }
                    // The server console of a local game.
                    EngineAction::RestartRound => self.server_console("sv_restart 1"),
                    EngineAction::BotAdd(team) => {
                        let line = team.map_or("bot_add".to_string(), |t| format!("bot_add_{t}"));
                        self.server_console(&line);
                    }
                    EngineAction::BotKick => self.server_console("bot_kick"),
                    EngineAction::Mp3Command(args) => self.audio.handle_mp3_command(&args),
                    EngineAction::CdCommand(args) => self.audio.handle_cd_command(&args),
                    EngineAction::PlaySound { sample, volume } => {
                        let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
                        self.audio
                            .play_2d(self.vfs.as_ref(), &sample, volume * master);
                    }
                    EngineAction::SpeakSentence(spec) => {
                        let suit =
                            finite_or(self.cvars.get_f32("suitvolume"), 0.25).clamp(0.0, 1.0);
                        self.audio.play_sentence(self.vfs.as_ref(), &spec, suit);
                    }
                    EngineAction::StopSound => self.audio.stop_all(),
                    EngineAction::AdjustCrosshair(color_idx) => {
                        const PRESETS: [&str; 5] = [
                            "50 250 50",
                            "250 50 50",
                            "50 50 250",
                            "250 250 50",
                            "50 250 250",
                        ];
                        let cur = self.cvars.get_str("cl_crosshair_color").to_string();
                        let next = match color_idx {
                            Some(i) => PRESETS[usize::from(i) % PRESETS.len()],
                            None => {
                                let idx = PRESETS
                                    .iter()
                                    .position(|p| *p == cur)
                                    .map_or(0, |i| (i + 1) % PRESETS.len());
                                PRESETS[idx]
                            }
                        };
                        self.cvars.set("cl_crosshair_color", next);
                    }
                    EngineAction::ToggleScores => {
                        self.scoreboard_toggled = !self.scoreboard_toggled;
                    }
                    EngineAction::OpenPlayerList => {
                        self.player_list_ui.open = true;
                        self.sync_cursor_capture();
                    }
                    EngineAction::OpenDemoPlayer(path) => {
                        if let Some(p) = path {
                            self.demo_player_ui.load_demo(&p);
                        }
                        self.demo_player_ui.open = true;
                        self.sync_cursor_capture();
                    }
                    EngineAction::SpecMode(mode) => {
                        let next = SpectatorMode::from_i32(mode);
                        self.spectator_ui.mode = next;
                        if self.is_in_game() {
                            self.session
                                .send_stringcmd(&format!("specmode {}", next as i32));
                        }
                    }
                    EngineAction::SpecMenu(visible) => {
                        self.spectator_ui.menu_visible = visible != 0;
                    }
                    EngineAction::RecordDemo(name) => {
                        if name.trim().is_empty() {
                            self.cvars.warn("usage: record <demoname>");
                        } else if !self.session.is_active() || self.session.is_demo() {
                            self.cvars
                                .warn("must be connected to a server to record a demo");
                        } else {
                            let clean = name.trim().trim_end_matches(".dem").to_string();
                            self.session.start_recording("cstrike");
                            self.recording_demo = Some(clean.clone());
                            self.cvars.info(format!("recording to {clean}.dem."));
                        }
                    }
                    EngineAction::StopDemo => {
                        if let Some(name) = self.recording_demo.take() {
                            match self.session.stop_recording() {
                                Ok(Some(bytes)) => {
                                    let file_name = format!("{name}.dem");
                                    let out_path = self
                                        .config_path
                                        .parent()
                                        .filter(|p| !p.as_os_str().is_empty())
                                        .unwrap_or_else(|| std::path::Path::new("."))
                                        .join(&file_name);
                                    match std::fs::write(&out_path, &bytes) {
                                        Ok(()) => self.cvars.info(format!(
                                            "Completed demo {file_name} ({} bytes)",
                                            bytes.len()
                                        )),
                                        Err(e) => self.cvars.warn(format!(
                                            "failed to write {}: {e}",
                                            out_path.display()
                                        )),
                                    }
                                }
                                Ok(None) => self.cvars.info("Not recording a demo."),
                                Err(e) => self.cvars.warn(format!("demo stop error: {e}")),
                            }
                        } else if self.session.is_demo() {
                            self.session.close();
                            self.demo_player_ui.state =
                                cs_client::ui::screens::DemoTransportState::Stopped;
                        } else {
                            self.cvars.info("Not recording a demo.");
                        }
                    }
                    EngineAction::PlayDemo(name) => {
                        self.start_demo_file(&name, false);
                    }
                    EngineAction::TimeDemo(name) => {
                        self.start_demo_file(&name, true);
                    }
                    EngineAction::RconCommand(_) | EngineAction::SetMaster(_) => {}
                    EngineAction::ForceCenterView => {
                        self.camera.pitch_deg = 0.0;
                    }
                    EngineAction::ConDump(opt_name) => {
                        let file_name = opt_name
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "condump000.txt".to_string());
                        let out_path = self
                            .config_path
                            .parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or_else(|| Path::new("."))
                            .join(&file_name);
                        let dump = self.cvars.format_console_dump();
                        match std::fs::write(&out_path, dump) {
                            Ok(()) => self
                                .cvars
                                .info(format!("console dumped to {}", out_path.display())),
                            Err(e) => self
                                .cvars
                                .warn(format!("condump failed ({}): {e}", out_path.display())),
                        }
                    }
                    EngineAction::Retry => {
                        if let Some(addr) = self.last_connect_addr.clone() {
                            self.connect_remote(&addr);
                        } else if self.session.is_local()
                            && let Some(m) = self.session.loaded_map().map(str::to_string)
                        {
                            self.start_local_map(&m);
                        } else {
                            self.cvars.warn("retry: no previous server address");
                        }
                    }
                    EngineAction::Reconnect => {
                        if self.session.is_local()
                            && let Some(m) = self.session.loaded_map().map(str::to_string)
                        {
                            self.start_local_map(&m);
                        } else if let Some(addr) = self.last_connect_addr.clone() {
                            self.connect_remote(&addr);
                        } else {
                            self.session.execute_local("reconnect");
                        }
                    }
                    EngineAction::RestartMap => {
                        if let Some(m) = self.session.loaded_map().map(str::to_string) {
                            self.start_local_map(&m);
                        } else {
                            self.cvars.warn("restart: no active map");
                        }
                    }
                    EngineAction::ShutdownServer => {
                        self.end_session();
                        self.cvars.info("Server shut down.");
                    }
                    EngineAction::FullUpdate => {
                        self.session.request_full_update();
                    }
                    EngineAction::DlFile(res) => {
                        self.forward_to_server(&format!("dlfile {res}"));
                    }
                    EngineAction::ShowBriefing => {
                        self.motd_ui.open = true;
                        self.feeds.motd_open = true;
                        self.sync_cursor_capture();
                    }
                    EngineAction::Demos { kind, args } => match kind.as_str() {
                        "lastdemo" => {
                            if let Some(name) = self.last_demo.clone() {
                                self.start_demo_file(&name, false);
                            } else {
                                self.cvars.info("No previous demo played.");
                            }
                        }
                        _ => {
                            if !args.is_empty() {
                                self.demo_playlist = args;
                                self.demo_playlist_idx = 0;
                            }
                            if self.demo_playlist.is_empty() {
                                self.cvars.info("No demos in demo playlist.");
                            } else {
                                let idx = self.demo_playlist_idx % self.demo_playlist.len();
                                let next = self.demo_playlist[idx].clone();
                                self.demo_playlist_idx = (idx + 1) % self.demo_playlist.len();
                                self.start_demo_file(&next, false);
                            }
                        }
                    },
                    EngineAction::DisplaySoundList => {
                        let list = self.audio.format_sound_list();
                        for line in list.lines() {
                            self.cvars.info(line);
                        }
                    }
                    EngineAction::MCache => {
                        let summary = self.session.precache_models_summary();
                        for line in summary.lines() {
                            self.cvars.info(line);
                        }
                    }
                    EngineAction::DeltaStats => {
                        let summary = self.session.delta_stats_summary();
                        for line in summary.lines() {
                            self.cvars.info(line);
                        }
                    }
                    EngineAction::DeltaClear => {
                        self.session.delta_clear();
                        self.cvars.info("Delta frame history cleared.");
                    }
                    EngineAction::DemoTimescale(scale) => {
                        if let Some(p) = self.session.demo_player_mut() {
                            let s = scale.clamp(0.1, 4.0);
                            p.set_speed(s);
                            self.demo_player_ui.speed = s;
                            self.cvars.info(format!("demo_timescale: {s:.2}x"));
                        } else {
                            self.cvars.warn("demo_timescale: no demo playing");
                        }
                    }
                    EngineAction::DemoPause => {
                        if let Some(p) = self.session.demo_player_mut() {
                            p.set_paused(true);
                            self.demo_player_ui.state =
                                cs_client::ui::screens::DemoTransportState::Paused;
                        } else {
                            self.cvars.warn("demo_pause: no demo playing");
                        }
                    }
                    EngineAction::DemoResume => {
                        if let Some(p) = self.session.demo_player_mut() {
                            p.set_paused(false);
                            self.demo_player_ui.state =
                                cs_client::ui::screens::DemoTransportState::Playing;
                        } else {
                            self.cvars.warn("demo_resume: no demo playing");
                        }
                    }
                    EngineAction::DemoTogglePause => {
                        if let Some(p) = self.session.demo_player_mut() {
                            let next = !p.is_paused();
                            p.set_paused(next);
                            self.demo_player_ui.state = if next {
                                cs_client::ui::screens::DemoTransportState::Paused
                            } else {
                                cs_client::ui::screens::DemoTransportState::Playing
                            };
                        } else {
                            self.cvars.warn("demo_togglepause: no demo playing");
                        }
                    }
                    EngineAction::DemoGoto(secs) => {
                        if let Some(p) = self.session.demo_player_mut() {
                            let target = secs.clamp(0.0, p.total_duration());
                            p.seek(target);
                            self.demo_player_ui.current_secs = p.current_time();
                        } else {
                            self.cvars.warn("demo_goto: no demo playing");
                        }
                    }
                    EngineAction::VoiceScalePlayer { slot, scale } => {
                        if usize::from(slot) < self.voice_player_scales.len() {
                            let s = scale.clamp(0.0, 2.0);
                            self.voice_player_scales[usize::from(slot)] = s;
                            self.cvars
                                .info(format!("voice_scale_player[{slot}] = {s:.2}"));
                        }
                    }
                    EngineAction::ServerCmd(tokens) => {
                        let line = tokens.join(" ");
                        if self.session.is_local() {
                            self.server_console(&line);
                        } else {
                            match tokens.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                                Some("maps") => {
                                    let pat = tokens.get(1).map(|s| s.to_ascii_lowercase());
                                    let mut count = 0usize;
                                    let matches: Vec<String> = self
                                        .available_maps
                                        .iter()
                                        .filter(|m| {
                                            pat.as_ref().is_none_or(|p| {
                                                let p = p.trim_matches('*');
                                                p.is_empty() || m.to_ascii_lowercase().contains(p)
                                            })
                                        })
                                        .cloned()
                                        .collect();
                                    for m in matches {
                                        count += 1;
                                        self.cvars.info(format!("  {m}.bsp"));
                                    }
                                    self.cvars.info(format!("{count} maps"));
                                }
                                Some("version") => {
                                    self.cvars.info(
                                        "Protocol version 48 / cs16-rs Engine (Build 10210 compat)",
                                    );
                                }
                                _ => self.server_console(&line),
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn start_demo_file(&mut self, raw_name: &str, timedemo: bool) {
        let trimmed = raw_name.trim();
        if trimmed.is_empty() {
            self.cvars.warn("usage: playdemo <filename>");
            return;
        }
        let fname = if trimmed.to_ascii_lowercase().ends_with(".dem") {
            trimmed.to_string()
        } else {
            format!("{trimmed}.dem")
        };
        let bytes = std::fs::read(&fname).or_else(|_| {
            self.vfs
                .as_ref()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no vfs"))
                .and_then(|v| {
                    v.read_bytes(&fname).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string())
                    })
                })
        });
        match bytes {
            Ok(data) => match Session::start_demo(self.vfs.as_ref(), &data) {
                Ok(mut sess) => {
                    if timedemo {
                        if let Some(report) = sess
                            .demo_player_mut()
                            .map(|p| p.run_timedemo_all(1.0 / 60.0))
                        {
                            self.cvars.info(format!(
                                "timedemo {}: {} frames, {:.3}s, {:.1} fps",
                                fname, report.frames, report.elapsed_secs, report.avg_fps
                            ));
                        }
                    } else if let Some(p) = sess.demo_player() {
                        self.last_demo = Some(fname.clone());
                        self.demo_player_ui.load_demo(&fname);
                        self.demo_player_ui.duration_secs = p.total_duration();
                        self.demo_player_ui.total_ticks =
                            (p.total_duration() * 100.0).ceil().max(1.0) as u32;
                    }
                    self.session.close();
                    self.session = sess;
                    self.audio_precache_warmed = false;
                    self.menu_ui.visible = false;
                    self.sync_cursor_capture();
                }
                Err(e) => self
                    .cvars
                    .warn(format!("Failed to start demo {fname}: {e}")),
            },
            Err(e) => self.cvars.warn(format!("Couldn't open {fname}: {e}")),
        }
    }

    fn apply_demo_ui_actions(&mut self, actions: Vec<DemoUiAction>) {
        if let Some(player) = self.session.demo_player() {
            self.demo_player_ui.current_secs = player.current_time();
            self.demo_player_ui.duration_secs = player.total_duration();
            self.demo_player_ui.tick = (player.current_time() * 100.0).round().max(0.0) as u32;
        }
        for act in actions {
            match act {
                DemoUiAction::Load(name) => self.start_demo_file(&name, false),
                DemoUiAction::Stop => {
                    if self.session.is_demo() {
                        self.session.close();
                    }
                }
                DemoUiAction::TogglePause => {
                    if let Some(p) = self.session.demo_player_mut() {
                        let next = !p.is_paused();
                        p.set_paused(next);
                    }
                }
                DemoUiAction::StepBack => {
                    if let Some(p) = self.session.demo_player_mut() {
                        let target = (p.current_time() - 0.5).max(0.0);
                        p.seek(target);
                    }
                }
                DemoUiAction::StepForward => {
                    if let Some(p) = self.session.demo_player_mut() {
                        let _ = p.step_frame();
                    }
                }
                DemoUiAction::SetSpeed(spd) => {
                    if let Some(p) = self.session.demo_player_mut() {
                        p.set_speed(spd);
                    }
                }
                DemoUiAction::Seek(secs) => {
                    if let Some(p) = self.session.demo_player_mut() {
                        p.seek(secs);
                    }
                }
                DemoUiAction::JumpStart => {
                    if let Some(p) = self.session.demo_player_mut() {
                        p.seek(0.0);
                    }
                }
                DemoUiAction::JumpEnd => {
                    if let Some(p) = self.session.demo_player_mut() {
                        let end = p.total_duration();
                        p.seek(end);
                    }
                }
            }
        }
    }

    fn server_console(&mut self, line: &str) {
        match self.session.server_console(line) {
            Some(out) if !out.is_empty() => self.cvars.info(out.trim_end().to_string()),
            Some(_) => {}
            None => self.cvars.warn(format!(
                "\"{line}\" needs a local game (the server is elsewhere)"
            )),
        }
    }

    fn owned_weapon_ids(hud: &HudState, owned_bits: u32) -> HashSet<u8> {
        hud.weapons
            .values()
            .map(|w| w.weapon_id)
            .filter(|id| *id < 32 && owned_bits & (1 << id) != 0)
            .collect()
    }

    /// `slot1`..`slot10`: answers an open server text/VGUI menu first, then selects weapon buckets.
    fn select_slot(&mut self, slot: u8) {
        if self.buy_team_class_ui.is_open() {
            return;
        }
        let digit = if slot == 10 { 0 } else { slot };
        let text_keys = self
            .session
            .with_hud(|h| {
                h.menu
                    .as_ref()
                    .filter(|m| !m.multipart && m.keys != 0 && !m.text.is_empty())
                    .map(|m| m.keys)
            })
            .flatten();
        if let Some(keys) = text_keys
            && menu_key_enabled(keys, digit)
        {
            self.session.send_stringcmd(&format!("menuselect {slot}"));
            self.session.with_hud_mut(|h| h.menu = None);
            return;
        }
        // Never touch `h.vgui_menu` when `_vgui_menus 1` is active: the VGUI2 Buy/Team/Class
        // screen owns digit hotkeys (`B-4-2`) exclusively. Only use the text fallback when
        // `_vgui_menus 0`.
        if !self.cvars.get_bool("_vgui_menus") {
            let vgui_target = self
                .session
                .with_hud(|h| {
                    h.vgui_menu
                        .as_ref()
                        .filter(|v| !v.multipart && v.keys != 0 && menu_key_enabled(v.keys, digit))
                        .map(|v| v.menu_id)
                })
                .flatten();
            if let Some(menu_id) = vgui_target {
                let cmd = vgui_menu_select_command(menu_id, slot);
                self.session.send_stringcmd(&cmd);
                self.session.with_hud_mut(|h| h.vgui_menu = None);
                return;
            }
        }
        if slot == 10 {
            return;
        }
        let owned_bits = self.session.owned_weapons();
        let fastswitch_mode = self.cvars.get_i32("hud_fastswitch");
        let buckets = &mut self.weapon_buckets;
        let (pick, snd) = self
            .session
            .with_hud(|hud| {
                let owned = Self::owned_weapon_ids(hud, owned_bits);
                buckets.on_slot_press_mode(hud, &owned, slot, fastswitch_mode)
            })
            .unwrap_or((None, None));
        if let Some(sound) = snd {
            self.audio
                .play_ui_sound(self.vfs.as_ref(), sound.vfs_path());
        }
        if let Some(name) = pick {
            self.session.send_stringcmd(&name);
        }
    }

    /// `invnext` / `invprev`: cycles owned weapons across buckets/positions (`hud_fastswitch` 0/1/2).
    fn cycle_inventory(&mut self, forward: bool) {
        if self.buy_team_class_ui.is_open() {
            return;
        }
        let owned_bits = self.session.owned_weapons();
        let fastswitch_mode = self.cvars.get_i32("hud_fastswitch");
        let buckets = &mut self.weapon_buckets;
        let (pick, snd) = self
            .session
            .with_hud(|hud| {
                let owned = Self::owned_weapon_ids(hud, owned_bits);
                step_inventory_cycle_mode(buckets, hud, &owned, forward, fastswitch_mode)
            })
            .unwrap_or((None, None));
        if let Some(sound) = snd {
            self.audio
                .play_ui_sound(self.vfs.as_ref(), sound.vfs_path());
        }
        if let Some(name) = pick {
            self.session.send_stringcmd(&name);
        }
    }

    /// Queues this frame's usercmd(s). At default `fps_max 100` (`fps_override 0`), emits 1
    /// command per frame; when `fps_override 1` runs above `105 FPS` with `cl_cmd_100hz 1`,
    /// emits commands on a clean `10 ms` (`100 Hz`) cadence.
    fn queue_usercmd(&mut self, dt: f64) {
        if !self.session.in_game() {
            self.input_state.consume_impulses();
            return;
        }
        let fps_max = finite_or(self.cvars.get_f32("fps_max"), 100.0);
        let fps_override = self.cvars.get_bool("fps_override");
        let cl_cmd_100hz = self.cvars.get_bool("cl_cmd_100hz");
        let use_grid = should_use_100hz_cmd_grid(
            fps_max,
            fps_override,
            cl_cmd_100hz,
            self.session.is_active(),
            self.session.max_clients(),
        );
        let ticks = self.cmd_clock.grid_ticks_100hz(dt, use_grid);
        if ticks.is_empty() {
            return;
        }
        let capture = self.wants_mouse_capture();
        let angles = Vec3::new(self.camera.pitch_deg, self.camera.yaw_deg, 0.0);
        let speeds = MoveSpeeds {
            forward: self.cvars.get_f32("cl_forwardspeed"),
            back: self.cvars.get_f32("cl_backspeed"),
            side: self.cvars.get_f32("cl_sidespeed"),
            up: self.cvars.get_f32("cl_upspeed"),
            walk_key: self.cvars.get_f32("cl_movespeedkey"),
        };
        let interp = self.session.interp();
        for (idx, msec) in ticks.into_iter().enumerate() {
            let held = self.input_state.held(capture);
            let lerp_dt = if use_grid { 0.01 } else { dt as f32 };
            let params = CmdParams {
                msec,
                lerp_msec: self.cmd_clock.lerp_msec(interp, lerp_dt),
                impulse: if idx == 0 {
                    std::mem::take(&mut self.pending_impulse)
                } else {
                    0
                },
            };
            let mut cmd = build_usercmd(&held, angles, params, &speeds);
            cmd.sidemove += if idx == 0 {
                std::mem::take(&mut self.strafe_sidemove)
            } else {
                0.0
            };
            self.session.queue_cmd(cmd);
            if idx == 0 {
                self.input_state.consume_impulses();
            }
        }
    }

    fn handle_bound_key(
        &mut self,
        key_name: &str,
        pressed: bool,
        event_loop: Option<&ActiveEventLoop>,
    ) -> bool {
        let actions = self.cvars.handle_key_bind(key_name, pressed);
        if actions.is_empty() {
            return false;
        }
        if pressed
            && key_name == "mouse1"
            && self.weapon_buckets.active_slot.is_some()
            && actions.iter().any(|a| {
                matches!(
                    a,
                    EngineAction::ActionInput { name, pressed: true } if name == "attack"
                )
            })
        {
            let buckets = &mut self.weapon_buckets;
            if let Some(w_name) = self
                .session
                .with_hud(|h| buckets.confirm_selection(h))
                .flatten()
            {
                self.audio
                    .play_ui_sound(self.vfs.as_ref(), WeaponBucketSound::Select.vfs_path());
                self.session.send_stringcmd(&w_name);
                let filtered: Vec<EngineAction> = actions
                    .into_iter()
                    .filter(|a| {
                        !matches!(
                            a,
                            EngineAction::ActionInput { name, pressed: true } if name == "attack"
                        )
                    })
                    .collect();
                if !filtered.is_empty() {
                    let _ = self.process_actions(filtered, event_loop);
                }
                return true;
            }
        }
        let _ = self.process_actions(actions, event_loop);
        true
    }

    fn handle_key_event(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        let pressed = event.state == ElementState::Pressed;
        let PhysicalKey::Code(code) = event.physical_key else {
            return false;
        };

        if code == KeyCode::Backquote && pressed && !event.repeat {
            self.console_ui.toggle();
            self.sync_cursor_capture();
            return true;
        }

        if code == KeyCode::Escape && pressed && !event.repeat {
            let _ = self.process_actions(vec![EngineAction::ToggleMenu], Some(event_loop));
            return true;
        }

        // Allow releasing TAB even while the scoreboard is visible, but block TAB scoreboard
        // while any modal menu (Console/Chat/MainMenu/Options/Buy/MOTD/Dialogs) is open.
        if code == KeyCode::Tab && !self.any_modal_open() && self.is_in_game() {
            self.input_state.apply_action_input("showscores", pressed);
            return true;
        }

        if !self.wants_mouse_capture() {
            return false;
        }

        // Classic text `ShowMenu` (radio `Z`/`X`/`C`, `_vgui_menus 0` buy/team, AMXX/Python mods)
        // takes the digit keys its mask enables without switching weapons.
        // VGUI2 Buy/Team/Class (`_vgui_menus 1`) is NEVER intercepted here — `BuyClassTeamMenuUi`
        // handles `1..9`, `0`, `A`, `R`, `Tab`, `Up`, `Down`, `Enter`, `Escape` exclusively.
        if pressed
            && !event.repeat
            && let Some(digit) = digit_of(code)
        {
            let text_keys = self
                .session
                .with_hud(|h| {
                    h.menu
                        .as_ref()
                        .filter(|m| !m.multipart && m.keys != 0 && !m.text.is_empty())
                        .map(|m| m.keys)
                })
                .flatten();
            if let Some(keys) = text_keys
                && menu_key_enabled(keys, digit)
            {
                let n = if digit == 0 { 10 } else { digit };
                self.session.send_stringcmd(&format!("menuselect {n}"));
                self.session.with_hud_mut(|h| h.menu = None);
                return true;
            }
            if !self.cvars.get_bool("_vgui_menus") {
                let vgui_target = self
                    .session
                    .with_hud(|h| {
                        h.vgui_menu
                            .as_ref()
                            .filter(|v| {
                                !v.multipart && v.keys != 0 && menu_key_enabled(v.keys, digit)
                            })
                            .map(|v| v.menu_id)
                    })
                    .flatten();
                if let Some(menu_id) = vgui_target {
                    let n = if digit == 0 { 10 } else { digit };
                    let cmd = vgui_menu_select_command(menu_id, n);
                    self.session.send_stringcmd(&cmd);
                    self.session.with_hud_mut(|h| h.vgui_menu = None);
                    return true;
                }
            }
        }

        if let Some(key_name) = physical_key_to_goldsrc(code) {
            if event.repeat {
                return true;
            }
            if self.handle_bound_key(key_name, pressed, Some(event_loop)) {
                return true;
            }
        }
        false
    }

    /// The scoreboard rows from the HUD state.
    fn score_rows(&self, names: &BTreeMap<u8, String>) -> (Vec<ScoreRow>, i16, i16) {
        self.session
            .with_hud(|hud| build_score_rows(hud, names))
            .unwrap_or_default()
    }

    fn apply_view_angle_updates(&mut self) {
        for upd in self.session.take_view_angle_updates() {
            match upd {
                ViewAngleUpdate::Set(angles) => {
                    if angles.x.is_finite() {
                        self.camera.pitch_deg = angles.x.clamp(-89.0, 89.0);
                    }
                    if angles.y.is_finite() {
                        self.camera.yaw_deg = angles.y.rem_euclid(360.0);
                    }
                }
                ViewAngleUpdate::AddYaw(yaw) => {
                    if yaw.is_finite() {
                        self.camera.yaw_deg = (self.camera.yaw_deg + yaw).rem_euclid(360.0);
                    }
                }
            }
        }
    }

    /// Accumulates raw OS mouse counts for low-latency same-frame sampling before
    /// `queue_usercmd` and camera matrix construction (matching GoldSrc `IN_MouseMove`).
    fn accumulate_raw_mouse_delta(&mut self, dx: f64, dy: f64) {
        if dx.is_finite() {
            self.pending_mouse_delta.0 += dx;
        }
        if dy.is_finite() {
            self.pending_mouse_delta.1 += dy;
        }
    }

    /// Applies the frame's accumulated raw mouse delta to `self.camera` (and `+strafe`),
    /// honoring `m_filter`, `m_customaccel*`, `zoom_sensitivity_ratio`, `m_yaw`/`m_pitch`,
    /// and `cl_pitchup`/`cl_pitchdown`.
    fn sample_accumulated_mouse_delta(&mut self) {
        let (raw_x64, raw_y64) = std::mem::take(&mut self.pending_mouse_delta);
        let raw_dx = raw_x64 as f32;
        let raw_dy = raw_y64 as f32;
        let m_filter = self.cvars.get_bool("m_filter");
        let (fx, fy) = if m_filter {
            let (px, py) = self.prev_mouse_delta;
            ((raw_dx + px) * 0.5, (raw_dy + py) * 0.5)
        } else {
            (raw_dx, raw_dy)
        };
        self.prev_mouse_delta = (raw_dx, raw_dy);
        if fx == 0.0 && fy == 0.0 {
            return;
        }
        self.apply_processed_mouse_delta(fx, fy);
    }

    /// Flushes any raw mouse motion that arrived after `queue_usercmd` immediately before
    /// camera matrix construction without double-applying `m_filter` on idle frames.
    fn flush_late_mouse_delta(&mut self) {
        if self.pending_mouse_delta.0 == 0.0 && self.pending_mouse_delta.1 == 0.0 {
            return;
        }
        let (raw_x64, raw_y64) = std::mem::take(&mut self.pending_mouse_delta);
        let raw_dx = raw_x64 as f32;
        let raw_dy = raw_y64 as f32;
        self.prev_mouse_delta.0 += raw_dx;
        self.prev_mouse_delta.1 += raw_dy;
        self.apply_processed_mouse_delta(raw_dx, raw_dy);
    }

    fn apply_processed_mouse_delta(&mut self, fx: f32, fy: f32) {
        let m_yaw = self.cvars.get_f32("m_yaw");
        let m_pitch = self.cvars.get_f32("m_pitch");
        let (adj_x, adj_y, sens) = compute_mouse_move_params(
            fx,
            fy,
            finite_or(self.cvars.get_f32("sensitivity"), 3.0).max(0.0),
            self.mouse_sens_scale,
            self.cvars.get_i32("m_customaccel"),
            finite_or(self.cvars.get_f32("m_customaccel_scale"), 0.04).max(0.0),
            finite_or(self.cvars.get_f32("m_customaccel_max"), 0.0).max(0.0),
            finite_or(self.cvars.get_f32("m_customaccel_exponent"), 1.0),
            m_yaw,
            m_pitch,
        );
        let pitch_up = self.cvars.get_f32("cl_pitchup");
        let pitch_down = self.cvars.get_f32("cl_pitchdown");
        let strafe_mode =
            self.input_state.active_actions.contains("strafe") || self.cvars.get_bool("lookstrafe");
        let yaw_delta_x = if strafe_mode {
            let m_side = finite_or(self.cvars.get_f32("m_side"), 0.8);
            self.strafe_sidemove += adj_x * sens * m_side;
            0.0
        } else {
            adj_x
        };
        self.camera.apply_mouse_delta_clamped(
            yaw_delta_x,
            adj_y,
            sens,
            m_yaw,
            m_pitch,
            pitch_up,
            pitch_down,
        );
    }

    fn render_and_step(&mut self, event_loop: Option<&ActiveEventLoop>) -> bool {
        let now = Instant::now();
        let raw_dt = (now - self.last_frame).as_secs_f32();
        let dt = if self.last_connect_addr.is_none() {
            self.headless_fixed_dt
                .unwrap_or_else(|| raw_dt.clamp(0.0001, 0.1))
        } else {
            raw_dt.clamp(0.0001, 0.1)
        };
        self.last_frame = now;

        let inst_fps = 1.0 / raw_dt.max(0.0001);
        self.fps_smooth = self.fps_smooth * 0.9 + inst_fps * 0.1;
        self.frame_ms_smooth = self.frame_ms_smooth * 0.9 + (raw_dt * 1000.0) * 0.1;
        self.poll_rcon();

        let wait_actions = self.cvars.tick_wait_queue();
        if !wait_actions.is_empty() && self.process_actions(wait_actions, event_loop) {
            return true;
        }
        self.sync_client_cvars_to_session(false);

        // 1. Apply pending server-forced view angles, keyboard turn/look, and latest raw mouse
        //    `dx/dy` immediately before `queue_usercmd` so this frame's motion hits same-frame!
        self.cvars.set_forward_unknown(self.session.is_active());
        self.apply_view_angle_updates();
        if self.wants_mouse_capture() {
            let klook_params = KeyboardLookParams {
                dt,
                yawspeed: finite_or(self.cvars.get_f32("cl_yawspeed"), 210.0),
                pitchspeed: finite_or(self.cvars.get_f32("cl_pitchspeed"), 225.0),
                anglespeedkey: finite_or(self.cvars.get_f32("cl_anglespeedkey"), 0.67),
                pitch_up: finite_or(self.cvars.get_f32("cl_pitchup"), 89.0),
                pitch_down: finite_or(self.cvars.get_f32("cl_pitchdown"), 89.0),
            };
            self.input_state
                .apply_keyboard_angles(&mut self.camera, &klook_params);
            self.sample_accumulated_mouse_delta();
        }
        self.queue_usercmd(f64::from(dt));
        let timescale = finite_or(self.cvars.get_f32("host_timescale"), 1.0).clamp(0.05, 10.0);
        self.session.advance(f64::from(dt * timescale));
        self.apply_view_angle_updates();

        for um in self.session.drain_unknown_usermsgs() {
            if !self.mod_transfer.ingest_usermsg(
                &um.name,
                &um.payload,
                &mut self.mod_host,
                self.vfs.as_mut(),
            ) {
                self.mod_host.on_usermsg(&um.name, &um.payload);
            }
        }
        for err in self.mod_transfer.take_errors() {
            self.cvars.warn(format!("Mod download error: {err}"));
        }
        for cmd in self.mod_transfer.take_outgoing_commands() {
            self.session.send_stringcmd(&cmd);
        }
        for cmd in self.mod_host.take_execs() {
            self.session.send_stringcmd(&cmd);
        }

        // The connection ended (kicked, timed out, rejected, the server shut down).
        if let Some(state) = self.session.state() {
            if state.is_terminal() {
                let reason = match &state {
                    ConnState::Rejected(why) => Some(("Connection Rejected", why.clone())),
                    ConnState::Disconnected(why) => Some(("Disconnected", why.clone())),
                    _ => None,
                };
                self.end_session();
                if let Some((title, why)) = reason {
                    self.cvars.error(format!("Disconnected: {why}"));
                    if why.to_ascii_lowercase().contains("bad password")
                        || why.to_ascii_lowercase().contains("invalid password")
                    {
                        self.password_ui.open = true;
                    } else {
                        self.disconnect_err_ui.trigger(title, why);
                    }
                }
            } else if !matches!(state, ConnState::InGame) {
                self.loading_ui.visible = true;
                let (msg, prog) = match state {
                    ConnState::AwaitChallenge => (
                        "Establishing network connection to server...".to_string(),
                        0.2,
                    ),
                    ConnState::AwaitConnection => ("Connecting to server...".to_string(), 0.45),
                    ConnState::Signon => {
                        if let Some((pending, cur)) = self.session.download_progress()
                            && pending > 0
                        {
                            let file = cur.unwrap_or_else(|| "resources".to_string());
                            (format!("Downloading {file} ({pending} left)..."), 0.6)
                        } else {
                            ("Downloading resources and precaching...".to_string(), 0.8)
                        }
                    }
                    _ => ("Loading...".to_string(), 0.1),
                };
                self.loading_ui.status_text = msg;
                self.loading_ui.progress = prog;
            } else {
                self.loading_ui.visible = false;
            }
        } else {
            self.loading_ui.visible = false;
        }

        // The server named its map: load it before the first frame is predicted.
        if let Some(map) = self.session.server_map()
            && self.session.loaded_map() != Some(map.as_str())
            && let Err(err) = self.load_map(&map)
        {
            self.cvars
                .error(format!("Failed to load map \"{map}\": {err}"));
            self.end_session();
        }

        // Text the server stuffed: dispatch only an explicit safe allowlist (never re-parse
        // unquoted server arguments through `CVarRegistry::execute_script`).
        for cmd in self.session.take_shell_commands() {
            let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
            match classify_shell_command(&cmd) {
                Some(ShellStuffAction::CloseMenu) => {
                    self.buy_team_class_ui.close();
                    self.session.with_hud_mut(|h| {
                        h.menu = None;
                        h.vgui_menu = None;
                    });
                }
                Some(ShellStuffAction::PlaySound(sample)) => {
                    self.audio.play_2d(self.vfs.as_ref(), &sample, master);
                }
                Some(ShellStuffAction::SpeakSentence(spec)) => {
                    let suit = finite_or(self.cvars.get_f32("suitvolume"), 0.25).clamp(0.0, 1.0);
                    self.audio.play_sentence(self.vfs.as_ref(), &spec, suit);
                }
                Some(ShellStuffAction::Mp3(args)) => self.audio.handle_mp3_command(&args),
                Some(ShellStuffAction::Cd(args)) => self.audio.handle_cd_command(&args),
                Some(ShellStuffAction::StopSound) => self.audio.stop_all(),
                Some(ShellStuffAction::Echo(text)) => self.cvars.info(text),
                None => {}
            }
        }
        self.cvars.set_forward_unknown(self.session.is_active());
        for line in self.session.take_console_lines() {
            self.cvars.info(line);
        }

        let time_sec = self.start_time.elapsed().as_secs_f32();
        let total_logs = self.cvars.log_lines().len();
        if total_logs > self.last_log_seen {
            let new_lines: Vec<String> = self
                .cvars
                .log_lines()
                .iter()
                .skip(self.last_log_seen)
                .map(|l| l.text.clone())
                .collect();
            for line in new_lines {
                for part in line.split(['\n', '\r']) {
                    let clean = part.trim();
                    if !clean.is_empty() {
                        self.console_notify_lines
                            .push((clean.to_string(), f64::from(time_sec)));
                    }
                }
            }
            if self.console_notify_lines.len() > 16 {
                let excess = self.console_notify_lines.len() - 16;
                self.console_notify_lines.drain(..excess);
            }
            self.last_log_seen = total_logs;
        } else if total_logs < self.last_log_seen {
            self.last_log_seen = total_logs;
        }
        let in_game = self.is_in_game();
        let show_scores = (self.input_state.active_actions.contains("showscores")
            || self.scoreboard_toggled)
            && in_game
            && !self.menu_ui.visible;
        let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
        let suit_vol = finite_or(self.cvars.get_f32("suitvolume"), 0.25).clamp(0.0, 1.0);
        let room_off = self.cvars.get_bool("room_off");
        let occ_factor = finite_or(self.cvars.get_f32("s_occfactor"), 0.0).clamp(0.0, 0.9);
        let lerping = self.cvars.get_bool("s_lerping");
        let hisound = self.cvars.get_bool("hisound");
        let mp3_vol = finite_or(self.cvars.get_f32("mp3volume"), 0.8).clamp(0.0, 1.0);
        let bgm_enabled = self.cvars.get_bool("bgmvolume");
        let mp3_fade = finite_or(self.cvars.get_f32("mp3fadetime"), 2.0).clamp(0.0, 30.0);
        self.audio.set_params(
            master,
            suit_vol,
            room_off,
            occ_factor,
            lerping,
            hisound,
            mp3_vol,
            bgm_enabled,
            mp3_fade,
        );
        self.audio
            .set_snd_occlusion(self.cvars.get_bool("snd_occlusion"));
        let want_menu_music = !self.session.is_active()
            && self.launch.auto_screenshot.is_none()
            && self.launch.record_dir.is_none();
        self.audio.sync_menu_music(
            self.vfs.as_ref(),
            want_menu_music,
            mp3_vol,
            bgm_enabled,
            mp3_fade,
            dt,
        );

        let default_fov = finite_or(self.cvars.get_f32("default_fov"), 90.0).clamp(60.0, 120.0);

        // 2. The frame of the session (reusing `self.frame_view` via `build_view_into`).
        let mut has_view = false;
        if in_game {
            if !self.audio_precache_warmed {
                self.audio_precache_warmed = true;
                if let Some(vfs) = self.vfs.as_ref() {
                    let audio = &mut self.audio;
                    let warmed = self
                        .session
                        .with_precache(|precache| audio.warmup_precache(vfs, precache))
                        .unwrap_or(0);
                    if warmed > 0 {
                        info!(warmed, "Pre-decoded signon precache WAVs into audio cache");
                    }
                }
            }
            self.in_game_frames += 1;
            if let Some(ui_mode) = self.launch.startup_ui.clone() {
                let m = ui_mode.to_ascii_lowercase();
                if matches!(
                    m.as_str(),
                    "hud" | "buy" | "buy:rifles" | "scoreboard" | "commandmenu" | "chat"
                ) {
                    if self.in_game_frames == 1 && m == "buy:rifles" {
                        let _ = self.session.server_console("mp_startmoney 16000");
                    } else if self.in_game_frames == 2 {
                        self.session.send_stringcmd("jointeam 2");
                    } else if self.in_game_frames == 6 {
                        self.session.send_stringcmd("joinclass 1");
                    } else if self.in_game_frames == 14 {
                        self.motd_ui.open = false;
                        self.feeds.motd_open = false;
                        self.buy_team_class_ui.close();
                        self.session.with_hud_mut(|h| {
                            h.vgui_menu = None;
                            h.menu = None;
                        });
                        match m.as_str() {
                            "buy" => {
                                self.cvars.set("_vgui_menus", "1");
                                self.feeds.center = None;
                                self.feeds.notify.clear();
                                self.buy_team_class_ui.open_kind(VguiMenuKind::Buy, true);
                            }
                            "buy:rifles" => {
                                self.cvars.set("_vgui_menus", "1");
                                self.feeds.center = None;
                                self.feeds.notify.clear();
                                self.buy_team_class_ui
                                    .open_kind_fresh(VguiMenuKind::BuyRifles, true);
                                self.buy_team_class_ui.focus_index = 2;
                                self.buy_team_class_ui.hovered_class = Some("m4a1".to_string());
                            }
                            "scoreboard" => self.scoreboard_toggled = true,
                            "commandmenu" => self.command_menu_ui.set_open(true),
                            "chat" => self.chat_ui.open_chat(false),
                            _ => {}
                        }
                        if m != "hud" {
                            self.startup_ui_applied = true;
                        }
                        self.sync_cursor_capture();
                    } else if m == "hud" && self.in_game_frames == 90 {
                        self.startup_ui_applied = true;
                    }
                } else if matches!(
                    m.as_str(),
                    "ak47" | "m4a1:silenced" | "awp:scope" | "aug:zoom" | "blood:bot"
                ) {
                    let is_t = m == "ak47";
                    if self.in_game_frames == 1 {
                        let _ = self.session.server_console("mp_startmoney 16000");
                        let _ = self.session.server_console("mp_freezetime 0");
                        if m == "blood:bot" {
                            let _ = self.session.server_console("bot_stop 1");
                            let _ = self.session.server_console("bot_add_t");
                        }
                    } else if self.in_game_frames == 2 {
                        self.session
                            .send_stringcmd(if is_t { "jointeam 1" } else { "jointeam 2" });
                    } else if self.in_game_frames == 6 {
                        self.session.send_stringcmd("joinclass 1");
                    } else if self.in_game_frames == 12 {
                        self.motd_ui.open = false;
                        self.feeds.motd_open = false;
                        self.buy_team_class_ui.close();
                        self.session.with_hud_mut(|h| {
                            h.vgui_menu = None;
                            h.menu = None;
                        });
                        match m.as_str() {
                            "ak47" => self.session.send_stringcmd("ak47"),
                            "m4a1:silenced" | "blood:bot" => self.session.send_stringcmd("m4a1"),
                            "awp:scope" => self.session.send_stringcmd("awp"),
                            "aug:zoom" => self.session.send_stringcmd("aug"),
                            _ => {}
                        }
                        self.sync_cursor_capture();
                    } else if m == "ak47" && self.in_game_frames == 65 {
                        self.startup_ui_applied = true;
                    } else if m == "m4a1:silenced" {
                        if self.in_game_frames == 75 {
                            self.input_state.apply_action_input("attack2", true);
                        } else if self.in_game_frames == 80 {
                            self.input_state.apply_action_input("attack2", false);
                        } else if self.in_game_frames == 250 {
                            self.startup_ui_applied = true;
                        }
                    } else if m == "awp:scope" {
                        if self.in_game_frames == 125 {
                            self.input_state.apply_action_input("attack2", true);
                        } else if self.in_game_frames == 130 {
                            self.input_state.apply_action_input("attack2", false);
                        } else if self.in_game_frames == 145 {
                            self.startup_ui_applied = true;
                        }
                    } else if m == "aug:zoom" {
                        if self.in_game_frames == 75 {
                            self.input_state.apply_action_input("attack2", true);
                        } else if self.in_game_frames == 80 {
                            self.input_state.apply_action_input("attack2", false);
                        } else if self.in_game_frames == 96 {
                            self.startup_ui_applied = true;
                        }
                    } else if m == "blood:bot" {
                        if self.in_game_frames == 68 {
                            let mut chosen_yaw = self.camera.yaw_deg;
                            let mut dir = {
                                let fwd = self.camera.forward();
                                let flat = Vec3::new(fwd.x, fwd.y, 0.0).normalize_or_zero();
                                if flat.length_squared() > 0.01 {
                                    flat
                                } else {
                                    Vec3::X
                                }
                            };
                            let coll_opt = self.session.collision().cloned();
                            let _ = self.session.with_local_game_mut(|local| {
                                let local_pos = {
                                    let srv = local.server();
                                    let local_id = srv.edicts().client(0);
                                    srv.edicts()
                                        .get(local_id)
                                        .map(|e| e.vars.origin)
                                        .unwrap_or(Vec3::ZERO)
                                };
                                if let Some(coll) = coll_opt.as_deref() {
                                    for deg in (0..360).step_by(15) {
                                        let r = (deg as f32).to_radians();
                                        let d = Vec3::new(r.cos(), r.sin(), 0.0);
                                        let tr = coll.trace_world(
                                            local_pos + Vec3::new(0.0, 0.0, 18.0),
                                            local_pos + d * 140.0 + Vec3::new(0.0, 0.0, 18.0),
                                            cs_pm::collision::Hull::Standing,
                                        );
                                        if tr.fraction >= 0.99 {
                                            dir = d;
                                            chosen_yaw = deg as f32;
                                            break;
                                        }
                                    }
                                }
                                let target_pos = local_pos + dir * 92.0;
                                let max_c = local.server().config().max_clients;
                                let mut bot_slot = None;
                                for slot in 1..max_c {
                                    let id = local.server().edicts().client(slot);
                                    if local
                                        .server()
                                        .edicts()
                                        .get(id)
                                        .is_some_and(|e| e.vars.health > 0.0)
                                    {
                                        bot_slot = Some(slot);
                                        break;
                                    }
                                }
                                if let Some(slot) = bot_slot {
                                    if let Some(p) = local
                                        .cs_game_mut()
                                        .state_mut()
                                        .players
                                        .get_mut(usize::from(slot))
                                        .and_then(|o| o.as_mut())
                                    {
                                        p.kevlar = Default::default();
                                        p.armor = 0.0;
                                    }
                                    local.server_mut().set_client_origin(slot, target_pos);
                                }
                            });
                            self.camera.yaw_deg = chosen_yaw;
                            self.camera.pitch_deg = 6.0;
                        } else if self.in_game_frames == 76 {
                            self.input_state.apply_action_input("attack", true);
                        } else if self.in_game_frames == 81 {
                            self.input_state.apply_action_input("attack", false);
                            self.startup_ui_applied = true;
                        }
                    }
                } else if m == "class" {
                    if self.in_game_frames == 2 {
                        self.session.send_stringcmd("jointeam 2");
                    } else if self.in_game_frames == 10 {
                        self.motd_ui.open = false;
                        self.feeds.motd_open = false;
                        self.buy_team_class_ui.close();
                        self.cvars.set("_vgui_menus", "1");
                        self.session.with_hud_mut(|h| {
                            h.vgui_menu = None;
                            h.menu = None;
                        });
                        self.buy_team_class_ui
                            .open_kind(VguiMenuKind::ClassCt, true);
                        self.startup_ui_applied = true;
                        self.sync_cursor_capture();
                    }
                } else if m == "team" && self.in_game_frames == 10 {
                    self.motd_ui.open = false;
                    self.feeds.motd_open = false;
                    self.cvars.set("_vgui_menus", "1");
                    self.session.with_hud_mut(|h| {
                        h.vgui_menu = None;
                        h.menu = None;
                    });
                    self.buy_team_class_ui.open_kind(VguiMenuKind::Team, true);
                    self.startup_ui_applied = true;
                    self.sync_cursor_capture();
                } else if m == "motd" && self.in_game_frames == 10 {
                    self.buy_team_class_ui.close();
                    self.session.with_hud_mut(|h| {
                        h.vgui_menu = None;
                        h.menu = None;
                    });
                    self.motd_ui.open = true;
                    self.feeds.motd_open = true;
                    self.startup_ui_applied = true;
                    self.sync_cursor_capture();
                } else if matches!(m.as_str(), "spectator" | "spectator:menu") {
                    if self.in_game_frames == 2 {
                        self.session.send_stringcmd("jointeam 6");
                    } else if self.in_game_frames == 14 {
                        self.motd_ui.open = false;
                        self.feeds.motd_open = false;
                        self.buy_team_class_ui.close();
                        self.session.with_hud_mut(|h| {
                            h.vgui_menu = None;
                            h.menu = None;
                        });
                        self.spectator_ui.menu_visible = m == "spectator:menu";
                        self.startup_ui_applied = true;
                        self.sync_cursor_capture();
                    }
                } else if m == "none" && self.in_game_frames == 8 {
                    self.motd_ui.open = false;
                    self.feeds.motd_open = false;
                    self.buy_team_class_ui.close();
                    self.session.with_hud_mut(|h| {
                        h.vgui_menu = None;
                        h.menu = None;
                    });
                    self.startup_ui_applied = true;
                    self.sync_cursor_capture();
                }
            }
            let inputs = ViewInputs {
                now: self.session.now(),
                frametime: f64::from(dt),
                right_hand: self.cvars.get_bool("cl_righthand"),
                draw_viewmodel: self.cvars.get_bool("r_drawviewmodel"),
                default_fov,
                viewmodel_fov: finite_or(self.cvars.get_f32("viewmodel_fov"), 90.0)
                    .clamp(50.0, 120.0),
                cl_bob: finite_or(self.cvars.get_f32("cl_bob"), 0.01),
                cl_bobcycle: finite_or(self.cvars.get_f32("cl_bobcycle"), 0.8),
                cl_bobup: finite_or(self.cvars.get_f32("cl_bobup"), 0.5),
                cl_rollangle: finite_or(self.cvars.get_f32("cl_rollangle"), 2.0),
                cl_rollspeed: finite_or(self.cvars.get_f32("cl_rollspeed"), 200.0),
                cl_viewmodel_lag: finite_or(self.cvars.get_f32("cl_viewmodel_lag"), 1.0),
                cl_weather: self.cvars.get_bool("cl_weather"),
                sv_cheats: self.cvars.get_bool("sv_cheats"),
                sv_showimpacts: self.cvars.get_i32("sv_showimpacts"),
                sv_showhitboxes: self.cvars.get_bool("sv_showhitboxes"),
                spec_xray: self.cvars.get_bool("spec_xray"),
            };
            let angles = Vec3::new(
                finite_or(self.camera.pitch_deg, 0.0),
                finite_or(self.camera.yaw_deg, 0.0),
                0.0,
            );
            if let Some(registry) =
                self.session
                    .build_view_into(&mut self.frame_view, angles, &inputs)
            {
                if let (Some(reg), Some(rt)) = (registry, &mut self.runtime) {
                    rt.renderer.set_model_registry(reg);
                }
                has_view = true;
            }
            self.collect_hud_feeds(inputs.now);
            let hud_now = self.session.now();
            let eye = if has_view {
                self.frame_view.eye
            } else {
                self.camera.position
            };
            let yaw_deg = self.camera.yaw_deg;
            self.session
                .with_hud(|h| self.hud_anim.step_with_pose(dt, h, hud_now, eye, yaw_deg));
        }
        self.weapon_buckets.step(dt);

        if has_view {
            self.view_stats = self.frame_view.stats;
            if self.frame_view.eye.is_finite() {
                self.camera.position = self.frame_view.eye;
            }
            let local_entity = self.session.local_slot().map(|s| u16::from(s) + 1);
            let sess_now_snd = self.session.now();
            self.play_sounds_from_frame_view(local_entity, sess_now_snd);
            let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
            for path in &self.frame_view.anim_sounds {
                self.audio.play_2d(self.vfs.as_ref(), path, 0.8 * master);
            }
        }
        let voice_enable =
            self.cvars.get_bool("voice_enable") && self.cvars.get_bool("voice_modenable");
        let voice_scale = finite_or(self.cvars.get_f32("voice_scale"), 1.0).clamp(0.0, 4.0);
        let voice_loopback = self.cvars.get_bool("voice_loopback");
        let voice_avg = finite_or(self.cvars.get_f32("voice_avggain"), 0.5);
        let voice_max = finite_or(self.cvars.get_f32("voice_maxgain"), 5.0);
        let recording = in_game
            && !self.menu_ui.visible
            && self.input_state.active_actions.contains("voicerecord");
        self.audio
            .set_voice_steam_compat(self.cvars.get_bool("voice_steam_compat"));
        self.audio
            .set_voice_recording(recording, self.cvars.get_bool("voice_forcemicrecord"));
        let sess_now_voice = self.session.now();
        for ve in self.session.take_voice_events() {
            self.audio.push_voice_event(ve, sess_now_voice);
        }
        let mut muted_mask = 0u32;
        if self.cvars.get_bool("cl_mute_enemy_team")
            && let Some(local_slot1) = self.session.local_slot().map(|s| s + 1)
        {
            self.session.with_hud(|h| {
                let local_team = h
                    .players
                    .get(&local_slot1)
                    .map(|p| p.team.as_str())
                    .filter(|t| *t == "CT" || *t == "TERRORIST");
                if let Some(lt) = local_team {
                    for (&slot1, pinfo) in &h.players {
                        if (1..=32).contains(&slot1)
                            && (pinfo.team == "CT" || pinfo.team == "TERRORIST")
                            && pinfo.team != lt
                        {
                            muted_mask |= 1u32 << (slot1 - 1);
                        }
                    }
                }
            });
        }
        for outgoing_voice in self.audio.step_voice_with_filters(
            sess_now_voice,
            voice_enable,
            voice_scale,
            voice_loopback,
            voice_avg,
            voice_max,
            muted_mask,
            &self.voice_player_scales,
        ) {
            self.session.send_voice_data(&outgoing_voice);
        }
        let voice_speakers = self.audio.active_speakers(sess_now_voice);
        let voice_local_talking = self.audio.is_local_transmitting(sess_now_voice);
        let paused = self.any_modal_open();
        let names = self.session.player_names();
        let (score_rows, ct_score, t_score) = self.score_rows(&names);
        let owned: HashSet<u8> = {
            let bits = self.session.owned_weapons();
            (0u8..32).filter(|id| bits & (1 << id) != 0).collect()
        };

        let sess_now = self.session.now();
        let deaths: Vec<DeathNotice> = self.feeds.deaths.iter().map(|(d, _)| d.clone()).collect();
        let death_times: Vec<f64> = self.feeds.deaths.iter().map(|(_, t)| *t).collect();
        let chat_entries: Vec<HudChatEntry> =
            self.feeds.chat.iter().map(|(c, _)| c.clone()).collect();
        let chat_times: Vec<f64> = self.feeds.chat.iter().map(|(_, t)| *t).collect();
        let chat: Vec<String> = chat_entries.iter().map(|c| c.text.clone()).collect();
        let pickups = self.feeds.pickups.clone();
        let notify: Vec<String> = self.feeds.notify.iter().map(|(c, _)| c.clone()).collect();
        let center = self.feeds.center.as_ref().map(|(c, _)| c.clone());
        let styled_menu_now = self
            .session
            .with_hud(|h| {
                h.menu
                    .as_ref()
                    .filter(|m| !m.multipart && m.keys != 0 && !m.text.is_empty())
                    .map(|m| {
                        menu_styled_lines_with_titles(
                            &m.text,
                            self.loc.as_ref(),
                            Some(&self.titles),
                        )
                    })
            })
            .flatten();
        let use_vgui_menus = self.cvars.get_bool("_vgui_menus");
        let active_vgui_menu = self
            .session
            .with_hud(|h| {
                h.vgui_menu
                    .as_ref()
                    .filter(|v| !v.multipart && v.keys != 0)
                    .cloned()
            })
            .flatten();
        let fallback_vgui_lines = if !use_vgui_menus && styled_menu_now.is_none() {
            active_vgui_menu.as_ref().map(vgui_menu_fallback_lines)
        } else {
            None
        };
        if use_vgui_menus && let Some(ref vm) = active_vgui_menu {
            if let Some(kind) = VguiMenuKind::from_vgui_menu_id(vm.menu_id, &vm.name) {
                let local_ct = self
                    .session
                    .local_slot()
                    .and_then(|s| {
                        self.session
                            .with_hud(|h| h.players.get(&(s + 1)).is_some_and(|p| p.team == "CT"))
                    })
                    .unwrap_or(false);
                if kind.is_buy() && !self.buy_team_class_ui.is_open() {
                    self.feeds.center = None;
                    self.feeds.notify.retain(|(s, _)| !s.contains("BUY key"));
                }
                self.buy_team_class_ui.open_kind(kind, local_ct);
            }
        } else if !use_vgui_menus {
            self.buy_team_class_ui.close();
        }
        let motd_raw: Option<String> = self
            .motd_ui
            .open
            .then(|| self.session.with_hud(|h| h.motd.clone()))
            .flatten();

        let local_speed = self.session.local_speed();
        let map_name = self.session.loaded_map().unwrap_or("").to_string();
        if !map_name.is_empty() && self.overview_map_name != map_name {
            self.overview_map_name = map_name.clone();
            if let (Some(rt), Some(vfs)) = (&self.runtime, &self.vfs) {
                self.overview_tex = OverviewMapTex::load(&rt.egui_ctx, vfs, &map_name);
            }
        }
        let (local_is_ct, local_has_team) = self
            .session
            .local_slot()
            .and_then(|s| {
                self.session
                    .with_hud(|h| {
                        h.players.get(&(s + 1)).map(|p| {
                            (
                                p.team == "CT",
                                !p.team.is_empty() && !p.team.eq_ignore_ascii_case("UNASSIGNED"),
                            )
                        })
                    })
                    .flatten()
            })
            .unwrap_or((true, false));
        let spec_players: Vec<(u8, String)> =
            names.iter().map(|(&k, v)| (k + 1, v.clone())).collect();
        let (round_secs_left, spec_hp) = self
            .session
            .with_hud(|h| {
                (
                    h.round_seconds_left(sess_now).max(0.0) as u32,
                    h.spec_health,
                )
            })
            .unwrap_or((0, None));
        let hostname = self
            .session
            .with_hud(|h| h.server_name.clone())
            .unwrap_or_default();
        // Sniper scope overlay applies at <= 40 deg (AWP/Scout/auto-sniper), keeping AUG/SG552 (55 deg) unmasked.
        let scoped = has_view
            && self.frame_view.fov.is_finite()
            && self.frame_view.fov > 0.0
            && self.frame_view.fov <= 40.0;
        let alive = has_view && self.frame_view.alive;
        let observer_mode = if has_view {
            self.frame_view.observer_mode
        } else {
            0
        };

        let mod_hud_cmds = if in_game {
            let (hp, ap, money) = self
                .session
                .with_hud(|h| (i32::from(h.health), i32::from(h.armor), h.money))
                .unwrap_or((100, 0, 0));
            let dt_ms = (dt * 1000.0).round() as i32;
            self.mod_host.render_hud(dt_ms, hp, ap, money)
        } else {
            Vec::new()
        };
        let pending_mod_approvals = self.mod_transfer.pending_approvals();

        // 3. egui: HUD, menus, console.
        let mut pending_actions = Vec::new();
        let mut should_resume = false;
        let mut sent_chat: Option<(bool, String)> = None;
        let mut motd_close = false;
        let mut vgui_cmd_out: Option<String> = None;
        let mut vgui_closed = false;
        let mut cmdmenu_exec: Option<String> = None;
        let mut cancel_loading = false;

        let Some(rt) = &mut self.runtime else {
            return false;
        };
        rt.egui_ctx.set_zoom_factor(1.0);
        let raw_input = if let (Some(state), Some(win)) = (&mut rt.egui_state, &rt.window) {
            let mut input = state.take_egui_input(win);
            let phys = win.inner_size();
            let event_scale = (win.scale_factor() as f32).max(1e-3);
            normalize_raw_input_physical_pixels(
                &mut input,
                phys.width.max(1),
                phys.height.max(1),
                event_scale,
            );
            input
        } else {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(self.launch.width as f32, self.launch.height as f32),
                )),
                time: Some(self.start_time.elapsed().as_secs_f64()),
                predicted_dt: dt,
                viewports: std::iter::once((
                    egui::ViewportId::ROOT,
                    egui::ViewportInfo {
                        native_pixels_per_point: Some(1.0),
                        ..Default::default()
                    },
                ))
                .collect(),
                ..Default::default()
            }
        };
        rt.egui_ctx.begin_pass(raw_input);

        let (hud_health, hud_hidden, hud_timer_vis, hud_nvg, hud_money) = self
            .session
            .with_hud(|h| {
                (
                    i32::from(h.health),
                    h.hidden,
                    h.timer_visible,
                    h.night_vision,
                    h.money,
                )
            })
            .unwrap_or((0, 0, false, false, 0));
        let hud_vis = compute_hud_visibility(alive, hud_health, hud_hidden, hud_timer_vis);
        if has_view {
            let v = &self.frame_view;
            let (wid, wflags, on_ground, ducking, ch_spd, clip) =
                self.session.local_crosshair_state();
            if !paused && hud_vis.crosshair && !scoped && should_draw_dynamic_crosshair(v.fov, wid)
            {
                if wid != self.crosshair_last_weapon {
                    self.crosshair_shots_fired = 0;
                    self.crosshair_last_weapon = wid;
                    self.crosshair_last_clip = clip;
                } else if clip >= 0 {
                    if clip < self.crosshair_last_clip {
                        self.crosshair_shots_fired += self.crosshair_last_clip - clip;
                        if self.crosshair_shots_fired > 600 {
                            self.crosshair_shots_fired = 1;
                        }
                    }
                    self.crosshair_last_clip = clip;
                }
                let ch_inputs = CrosshairInputs {
                    weapon_id: wid,
                    weapon_flags: wflags,
                    on_ground,
                    ducking,
                    speed_2d: ch_spd,
                    shots_fired: self.crosshair_shots_fired,
                    night_vision: hud_nvg,
                    dynamic: self.cvars.get_bool("cl_dynamiccrosshair"),
                    hide_hud: hud_hidden,
                };
                draw_crosshair_vgui_dt(
                    &rt.egui_ctx,
                    &self.cvars,
                    &mut self.crosshair_state,
                    &ch_inputs,
                    dt,
                );
            }
            let mut extras = HudExtrasContext {
                titles: Some(&self.titles),
                loc: self.loc.as_ref(),
                overview: self.overview_tex.as_ref(),
                buckets: Some(&self.weapon_buckets),
                anim: Some(&self.hud_anim),
                chat_entries: Some(&chat_entries),
                death_times: Some(&death_times),
                chat_times: Some(&chat_times),
                chat_input_open: self.chat_ui.open,
                pickup_history: &pickups,
                observer_mode,
                ..Default::default()
            };
            extras.apply_cvars(&self.cvars);
            let hud_state = self.session.with_hud(|h| {
                let view = HudView {
                    hud: h,
                    now: sess_now,
                    time_sec,
                    names: &names,
                    owned: &owned,
                    feed: &deaths,
                    chat: &chat,
                    notify: &notify,
                    center: center.as_deref(),
                    weapon_switch_timer: 0.0,
                    alive,
                    scoped,
                    eye: v.eye,
                    yaw_deg: self.camera.yaw_deg,
                    underwater_tint: v.underwater_tint,
                };
                draw_cs16_hud_with_runtime(
                    &rt.egui_ctx,
                    rt.hud_sprites.as_ref(),
                    &view,
                    &extras,
                    &mut self.vgui_rt,
                    self.vfs.as_ref(),
                );
                if !alive
                    && matches!(
                        self.spectator_ui.mode,
                        SpectatorMode::MapOverviewFree | SpectatorMode::MapOverviewChase
                    )
                    && let Some(ov) = &self.overview_tex
                {
                    let screen = rt.egui_ctx.content_rect();
                    let ov_rect = screen.shrink2(egui::vec2(32.0, 72.0));
                    let ov_painter = rt.egui_ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Background,
                        egui::Id::unique("cs16_spectator_overview"),
                    ));
                    draw_overview_map(
                        &ov_painter,
                        ov_rect,
                        ov,
                        h,
                        v.eye,
                        self.camera.yaw_deg,
                        self.cvars.get_bool("spec_drawcone"),
                        self.cvars.get_bool("spec_drawnames"),
                        &names,
                    );
                }
            });
            let _ = hud_state;
            draw_voice_speakers(&rt.egui_ctx, &names, &voice_speakers, voice_local_talking);
            draw_mod_hud(&rt.egui_ctx, &mod_hud_cmds);
            if let Some(styled) = &styled_menu_now {
                let scale = (rt.egui_ctx.content_rect().height() / 768.0).clamp(0.85, 1.45);
                draw_styled_text_menu_vgui(
                    &rt.egui_ctx,
                    &mut self.vgui_rt,
                    self.vfs.as_ref(),
                    styled,
                    scale,
                );
            } else if let Some(lines) = &fallback_vgui_lines {
                let scale = (rt.egui_ctx.content_rect().height() / 768.0).clamp(0.85, 1.45);
                draw_text_menu_vgui(
                    &rt.egui_ctx,
                    &mut self.vgui_rt,
                    self.vfs.as_ref(),
                    lines,
                    scale,
                );
            }
            let modal_menu_open = self.motd_ui.open
                || self.buy_team_class_ui.active_res.is_some()
                || styled_menu_now.is_some()
                || fallback_vgui_lines.is_some();
            if observer_mode != 0 {
                self.spectator_ui.mode = SpectatorMode::from_i32(observer_mode);
            }
            let forced_spectator = self.launch.startup_ui.as_deref().is_some_and(|s| {
                s.eq_ignore_ascii_case("spectator") || s.eq_ignore_ascii_case("spectator:menu")
            }) && self.startup_ui_applied;
            let show_spec_gui = (!alive && observer_mode != 0) || forced_spectator;
            let show_spec_bars =
                self.cvars.get_bool("spec_hud_bars") && (self.session.is_demo() || show_spec_gui);
            if in_game && !modal_menu_open && (show_spec_gui || show_spec_bars) {
                let player_weapons = &v.player_weapons;
                let observed_target = v.observed_target;
                let sidebar_players: Vec<SpecSidebarPlayer> = if show_spec_bars {
                    self.session
                        .with_hud(|h| {
                            collect_spec_sidebar_players(h, &names, player_weapons, observed_target)
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let spec_ctx = SpectatorRenderCtx {
                    vfs: self.vfs.as_ref(),
                    loc: self.loc.as_ref(),
                    ct_score,
                    t_score,
                    round_timer_secs: round_secs_left,
                    players: &spec_players,
                    spec_health: spec_hp,
                    sidebar_players: &sidebar_players,
                    hud_sprites: rt.hud_sprites.as_ref(),
                    suppress_letterbox: !show_spec_gui,
                };
                let spec_actions = self.spectator_ui.show(
                    &rt.egui_ctx,
                    &mut self.vgui_rt,
                    &mut self.cvars,
                    &spec_ctx,
                );
                pending_actions.extend(spec_actions);
            }
            draw_top_left_console_overlays(
                &rt.egui_ctx,
                &self.cvars,
                self.fps_smooth,
                &self.console_notify_lines,
                f64::from(time_sec),
                self.console_ui.open,
            );
            let telemetry = FrameTelemetry {
                fps: self.fps_smooth,
                frame_ms: self.frame_ms_smooth,
                map_name: &map_name,
                pos: self.camera.position.to_array(),
                yaw_deg: self.camera.yaw_deg,
                pitch_deg: self.camera.pitch_deg,
                speed: local_speed,
                draw_calls: self.last_stats.draw_calls,
                triangles: self.last_stats.triangles,
            };
            draw_showpos(&rt.egui_ctx, &self.cvars, &telemetry);
            draw_r_speeds(&rt.egui_ctx, &self.cvars, &telemetry);
            if in_game {
                draw_showspeed(&rt.egui_ctx, &self.cvars, local_speed);
                let held = self.input_state.held(!paused);
                let keys_disp = KeyDisplayState {
                    forward: held.actions.is_some_and(|a| a.contains("forward")),
                    back: held.actions.is_some_and(|a| a.contains("back")),
                    left: held.actions.is_some_and(|a| a.contains("moveleft")),
                    right: held.actions.is_some_and(|a| a.contains("moveright")),
                    jump: held.actions.is_some_and(|a| a.contains("jump"))
                        || held.impulse_actions.is_some_and(|a| a.contains("jump")),
                    duck: held.actions.is_some_and(|a| a.contains("duck"))
                        || held.impulse_actions.is_some_and(|a| a.contains("duck")),
                };
                draw_showkeys(&rt.egui_ctx, &self.cvars, &keys_disp);
                if self.cvars.get_bool("cl_ljstats") {
                    if let Some(s) = self.session.local_lj_sample() {
                        if s.teleported {
                            self.lj_tracker.invalidate();
                        } else {
                            let min_cvar = self.cvars.get_f32("cl_ljstats_min");
                            let min_distance = if min_cvar > 0.0 { min_cvar } else { 215.0 };
                            let sidemove = match (keys_disp.left, keys_disp.right) {
                                (true, false) => -400.0,
                                (false, true) => 400.0,
                                _ => 0.0,
                            };
                            let forwardmove = match (keys_disp.forward, keys_disp.back) {
                                (true, false) => 400.0,
                                (false, true) => -400.0,
                                _ => 0.0,
                            };
                            if let Some(stats) =
                                self.lj_tracker.observe(cs_pm::ljstats::JumpFrameInput {
                                    dt,
                                    origin: s.origin,
                                    velocity: s.velocity,
                                    on_ground: s.on_ground,
                                    ducking: s.ducking,
                                    waterlevel: s.waterlevel,
                                    on_ladder: false,
                                    noclip: false,
                                    jump_pressed: keys_disp.jump,
                                    sidemove,
                                    forwardmove,
                                    gravity: s.gravity,
                                    airaccelerate: s.airaccelerate,
                                    maxspeed: s.maxspeed,
                                    is_highjump_edge: false,
                                    min_distance,
                                })
                            {
                                self.cvars.info(stats.format_summary());
                                self.last_ljstats = Some((f64::from(time_sec), stats));
                            }
                        }
                    }
                    let active_stats = self
                        .last_ljstats
                        .as_ref()
                        .filter(|(t, _)| f64::from(time_sec) - *t <= 5.0)
                        .map(|(_, s)| s);
                    draw_ljstats(&rt.egui_ctx, &self.cvars, active_stats);
                } else {
                    self.lj_tracker.invalidate();
                }
            }
            if self
                .session
                .with_net_graph(|ng| {
                    draw_net_graph_with_telemetry(
                        &rt.egui_ctx,
                        &self.cvars,
                        &telemetry,
                        Some(ng),
                        self.palette,
                    );
                })
                .is_none()
            {
                draw_net_graph_with_telemetry(
                    &rt.egui_ctx,
                    &self.cvars,
                    &telemetry,
                    None,
                    self.palette,
                );
            }
            if show_scores {
                let sb_ctx = ScoreboardRenderCtx {
                    vfs: self.vfs.as_ref(),
                    loc: self.loc.as_ref(),
                    hostname: &hostname,
                    map_name: &map_name,
                    ct_score,
                    t_score,
                    players: &score_rows,
                    local_slot: self.session.local_slot(),
                    short_headers: self.cvars.get_bool("scoreboard_shortheaders"),
                    fallback_palette: rt.vgui_bank.client_palette,
                };
                show_scoreboard_vgui(&rt.egui_ctx, &mut self.vgui_rt, &sb_ctx);
            }
            if self.chat_ui.open {
                let mut con_ext = HudExtrasContext::default();
                con_ext.apply_cvars(&self.cvars);
                sent_chat = self.chat_ui.show(
                    &rt.egui_ctx,
                    &mut self.vgui_rt,
                    self.vfs.as_ref(),
                    con_ext.con_color,
                );
            }
            if let Some(raw_motd) = &motd_raw {
                let motd_ctx = MotdRenderCtx {
                    vfs: self.vfs.as_ref(),
                    loc: self.loc.as_ref(),
                    hostname: &hostname,
                    raw_motd,
                };
                if self
                    .motd_ui
                    .show(&rt.egui_ctx, &mut self.vgui_rt, &motd_ctx)
                {
                    motd_close = true;
                }
            }
            if !self.motd_ui.open && !motd_close && self.buy_team_class_ui.is_open() {
                let can_cancel = if let Some(ref vm) = active_vgui_menu {
                    (vm.keys & (1u16 << 9)) != 0
                } else {
                    let res = self
                        .buy_team_class_ui
                        .active_res
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if res.contains("teammenu") {
                        local_has_team
                    } else if res.contains("classmenu") {
                        alive
                    } else {
                        true
                    }
                };
                let buy_ctx = BuyMenuRenderCtx {
                    vfs: self.vfs.as_ref(),
                    loc: self.loc.as_ref(),
                    map_name: &map_name,
                    is_ct: local_is_ct,
                    allowed_keys: active_vgui_menu.as_ref().map_or(0, |v| v.keys),
                    can_cancel,
                    money: hud_money,
                };
                let was_open = self.buy_team_class_ui.is_open();
                vgui_cmd_out =
                    self.buy_team_class_ui
                        .show(&rt.egui_ctx, &mut self.vgui_rt, &buy_ctx);
                if was_open && !self.buy_team_class_ui.is_open() {
                    vgui_closed = true;
                }
            }
            cmdmenu_exec = self.command_menu_ui.show(
                &rt.egui_ctx,
                &mut self.vgui_rt,
                self.vfs.as_ref(),
                self.loc.as_ref(),
                &map_name,
            );
            if self.command_menu_ui.take_pending_briefing() {
                self.motd_ui.open = true;
                self.feeds.motd_open = true;
            }
        }

        let steam_icon = rt.vgui_bank.steam_icon.as_ref();
        let menu_cmds = self.menu_ui.show(
            &rt.egui_ctx,
            &mut self.vgui_rt,
            &mut self.cvars,
            &MainMenuRenderCtx {
                vfs: self.vfs.as_ref(),
                loc: self.loc.as_ref(),
                available_maps: &self.available_maps,
                in_game: self.session.is_active(),
                bg_texture: rt.bg_texture.as_ref(),
                steam_icon,
                palette: self.palette,
            },
        );
        pending_actions.extend(menu_cmds.actions);
        if menu_cmds.open_options {
            self.options_ui.open = true;
        }
        if menu_cmds.open_player_list {
            self.player_list_ui.open = true;
        }
        if menu_cmds.open_demo_player {
            self.demo_player_ui.open = true;
        }
        if menu_cmds.resume_game {
            should_resume = true;
        }
        self.options_ui
            .set_gpu_info(&rt.renderer.adapter_name, &rt.renderer.backend_name);
        self.options_ui.show(
            &rt.egui_ctx,
            &mut self.vgui_rt,
            &mut self.cvars,
            &OptionsRenderCtx {
                vfs: self.vfs.as_ref(),
                loc: self.loc.as_ref(),
                palette: self.palette,
                steam_icon,
            },
        );
        let dlg_ctx = DialogRenderCtx {
            vfs: self.vfs.as_ref(),
            loc: self.loc.as_ref(),
            palette: self.palette,
            steam_icon,
        };
        if self
            .loading_ui
            .show(&rt.egui_ctx, &mut self.vgui_rt, &dlg_ctx)
        {
            cancel_loading = true;
        }
        let vban_cmd = self.player_list_ui.show(
            &rt.egui_ctx,
            &mut self.vgui_rt,
            &names,
            self.session.local_slot(),
            &dlg_ctx,
        );
        let demo_actions = self
            .demo_player_ui
            .show(&rt.egui_ctx, &mut self.vgui_rt, &dlg_ctx);
        let pw_submit = self
            .password_ui
            .show(&rt.egui_ctx, &mut self.vgui_rt, &dlg_ctx);
        self.disconnect_err_ui
            .show(&rt.egui_ctx, &mut self.vgui_rt, &dlg_ctx);
        let mod_decisions = draw_mod_approval_prompt(&rt.egui_ctx, &pending_mod_approvals);
        let mut mount_path_requested: Option<PathBuf> = None;
        let mut launch_demo_from_setup = false;
        let mut open_console_from_setup = false;
        let mut quit_from_setup = false;
        if self.vfs.is_none() && !self.session.is_active() {
            let pal = self.palette;
            let logo_tex = rt.brand_logo.as_ref();
            let resp = egui::Window::new("cs16-rs — Mount Half-Life Assets")
                .id(egui::Id::unique("cs16_pre_vfs_setup"))
                .title_bar(false)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    egui::Frame::NONE
                        .fill(pal.control_bg)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .inner_margin(egui::Margin::same(12))
                        .stroke(egui::Stroke::new(1.0, pal.border_bright)),
                )
                .show(&rt.egui_ctx, |ui| {
                    ui.set_min_width(440.0);
                    ui.horizontal(|ui| {
                        if let Some(tex) = logo_tex {
                            ui.image(egui::load::SizedTexture::new(
                                tex.id(),
                                egui::vec2(48.0, 48.0),
                            ));
                        }
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("cs16-rs — Half-Life / CS 1.6 Asset Setup")
                                    .color(pal.bright_control_text)
                                    .strong()
                                    .size(15.0),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Assets are mounted read-only from your local Steam Half-Life install.",
                                )
                                .color(pal.base_text)
                                .size(12.0),
                            );
                        });
                    });
                    ui.add_space(8.0);
                    egui::Frame::NONE
                        .fill(pal.window_bg)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .inner_margin(egui::Margin::same(8))
                        .stroke(egui::Stroke::new(1.0, pal.border_dark))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Enter path to your Half-Life folder (containing cstrike/ and valve/):",
                                )
                                .color(pal.bright_text)
                                .size(12.0),
                            );
                            ui.add_space(4.0);
                            let edit = egui::TextEdit::singleline(&mut self.setup_gamedir_input)
                                .desired_width(410.0)
                                .hint_text(r"C:\Program Files (x86)\Steam\steamapps\common\Half-Life");
                            let edit_resp = ui.add(edit);
                            if edit_resp.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                && !self.setup_gamedir_input.trim().is_empty()
                            {
                                mount_path_requested =
                                    Some(PathBuf::from(self.setup_gamedir_input.trim()));
                            }
                            if let Some(ref err) = self.setup_error {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(err)
                                        .color(egui::Color32::from_rgb(255, 160, 0))
                                        .size(11.5),
                                );
                            }
                        });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let demo_btn = egui::Button::new(
                            egui::RichText::new("Play Instant Demo (No Steam Files)")
                                .color(pal.bright_control_text)
                                .strong(),
                        )
                        .fill(pal.control_bg)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .stroke(egui::Stroke::new(1.0, pal.bright_control_text));
                        if ui.add(demo_btn).clicked() {
                            launch_demo_from_setup = true;
                        }
                        let mount_btn = egui::Button::new(
                            egui::RichText::new("Mount Folder").color(pal.bright_control_text),
                        )
                        .fill(pal.control_bg)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .stroke(egui::Stroke::new(1.0, pal.border_bright));
                        if ui.add(mount_btn).clicked()
                            && !self.setup_gamedir_input.trim().is_empty()
                        {
                            mount_path_requested =
                                Some(PathBuf::from(self.setup_gamedir_input.trim()));
                        }
                        let con_btn = egui::Button::new(
                            egui::RichText::new("Console").color(pal.base_text),
                        )
                        .fill(pal.control_bg)
                        .corner_radius(egui::CornerRadius::ZERO)
                        .stroke(egui::Stroke::new(1.0, pal.border_bright));
                        if ui.add(con_btn).clicked() {
                            open_console_from_setup = true;
                        }
                        let quit_btn =
                            egui::Button::new(egui::RichText::new("Quit").color(pal.base_text))
                                .fill(pal.control_bg)
                                .corner_radius(egui::CornerRadius::ZERO)
                                .stroke(egui::Stroke::new(1.0, pal.border_bright));
                        if ui.add(quit_btn).clicked() {
                            quit_from_setup = true;
                        }
                    });
                });
            if let Some(inner) = resp {
                paint_vgui_bevel(
                    &rt.egui_ctx.layer_painter(inner.response.layer_id),
                    inner.response.rect,
                    true,
                    pal,
                );
            }
        }
        pending_actions.extend(self.console_ui.show(
            &rt.egui_ctx,
            &mut self.vgui_rt,
            &mut self.cvars,
            &ConsoleRenderCtx {
                vfs: self.vfs.as_ref(),
                loc: self.loc.as_ref(),
                available_maps: &self.available_maps,
                palette: self.palette,
                steam_icon,
            },
        ));

        let full_output = rt.egui_ctx.end_pass();
        if let (Some(state), Some(win)) = (&mut rt.egui_state, &rt.window) {
            state.handle_platform_output(win, full_output.platform_output);
        }
        let paint_jobs = rt
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for ui_snd in self.vgui_rt.take_ui_sounds() {
            self.audio
                .play_ui_sound(self.vfs.as_ref(), ui_snd.vfs_path());
        }

        // 4. The 3D frame: flush any late mouse motion before camera matrix build and sync
        //    swapchain latency controls (`gl_vsync` / `gl_lowlatency`).
        if self.wants_mouse_capture() {
            self.flush_late_mouse_delta();
        }
        let gl_vsync = self.cvars.get_bool("gl_vsync");
        let gl_lowlatency = self.cvars.get_bool("gl_lowlatency");
        let scene_for_frame = has_view.then(|| self.frame_view.take_scene());
        let Some(rt) = &mut self.runtime else {
            if let Some(prev_scene) = scene_for_frame {
                self.frame_view.restore_scene(prev_scene);
            }
            return false;
        };
        rt.renderer.sync_swapchain_latency(gl_vsync, gl_lowlatency);
        rt.renderer.set_entity_scene(scene_for_frame);

        let mut render_cam = self.camera.clone();
        let mut fov = default_fov;
        let mut viewmodel_inst: Option<(&str, glam::Mat4, i32, i32, f32, bool)> = None;
        let default_lightstyles = [1.0f32; 64];
        let mut fog = None;
        let mut sensitivity_scale = 1.0;
        let (
            beams_slice,
            points_slice,
            decals_slice,
            custom_decals_opt,
            lightstyles_ref,
            lights,
            elights_slice,
        ) = if has_view {
            let v = &self.frame_view;
            let punch_x = finite_or(v.punch.x, 0.0);
            let punch_y = finite_or(v.punch.y, 0.0);
            let punch_z = finite_or(v.punch.z, 0.0);
            let v_idlescale = finite_or(self.cvars.get_f32("v_idlescale"), 0.0);
            let idle = calc_view_idle(time_sec, v_idlescale);
            render_cam.position.z += finite_or(v.bob, 0.0) + finite_or(v.shake_z, 0.0);
            render_cam.pitch_deg = (finite_or(render_cam.pitch_deg, 0.0)
                + punch_x
                + finite_or(v.shake_angles.x, 0.0)
                + idle.pitch)
                .clamp(-89.0, 89.0);
            render_cam.yaw_deg = (finite_or(render_cam.yaw_deg, 0.0)
                + punch_y
                + finite_or(v.shake_angles.y, 0.0)
                + idle.yaw)
                .rem_euclid(360.0);
            let death_roll = if !alive
                && observer_mode == 0
                && local_has_team
                && in_game
                && finite_or(v.roll_deg, 0.0).abs() < 1.0
            {
                80.0
            } else {
                0.0
            };
            render_cam.roll_deg = (punch_z
                + finite_or(v.roll_deg, 0.0)
                + death_roll
                + finite_or(v.shake_angles.z, 0.0)
                + idle.roll)
                .clamp(-90.0, 90.0);
            let base_fov = finite_or(v.fov, default_fov).clamp(10.0, 120.0);
            fov = (base_fov + finite_or(v.underwater_fov_delta, 0.0)).clamp(10.0, 120.0);
            let zoom_ratio = finite_or(self.cvars.get_f32("zoom_sensitivity_ratio"), 1.2).max(0.0);
            sensitivity_scale = compute_zoom_sens_scale(base_fov, default_fov, zoom_ratio);
            if let Some(vm) = &v.viewmodel {
                let eye = if v.eye.is_finite() { v.eye } else { Vec3::ZERO };
                let b = finite_or(v.bob, 0.0);
                let fov_z = if (fov - 90.0).abs() < 0.5 || (fov - 110.0).abs() < 0.5 {
                    1.0
                } else if (fov - 100.0).abs() < 0.5 {
                    2.0
                } else if (fov - 80.0).abs() < 0.5 {
                    0.5
                } else {
                    0.0
                };
                // `client.dll` `0x1006b52f..1006b7a0`: project `bob * 0.4` along full 3D view forward.
                let (view_fwd, _, _) = cs_client::render::scene::angle_vectors(Vec3::new(
                    render_cam.pitch_deg,
                    render_cam.yaw_deg,
                    render_cam.roll_deg,
                ));
                let vm_eye = eye
                    + Vec3::new(0.0, 0.0, b - 1.0 + fov_z)
                    + view_fwd * (b * 0.4)
                    + v.viewmodel_lag;
                let (vm_yaw, vm_pitch, vm_roll) = calc_viewmodel_angles(
                    render_cam.yaw_deg,
                    render_cam.pitch_deg,
                    render_cam.roll_deg,
                    b,
                    idle,
                );
                let xf = viewmodel_transform_with_angles(
                    vm_eye,
                    vm_yaw,
                    vm_pitch,
                    vm_roll,
                    Vec3::ZERO,
                    vm.right_hand,
                );
                viewmodel_inst = Some((
                    vm.path.as_str(),
                    xf,
                    vm.sequence,
                    vm.body,
                    vm.animtime,
                    vm.right_hand,
                ));
            }
            fog = if self.cvars.get_bool("gl_fog") {
                v.fog
            } else {
                None
            };
            (
                v.beams.as_slice(),
                v.points.as_slice(),
                v.decals.as_slice(),
                Some(&v.custom_decals),
                &v.lightstyles,
                v.lights,
                v.elights.as_slice(),
            )
        } else {
            (
                &[][..],
                &[][..],
                &[][..],
                None,
                &default_lightstyles,
                GpuLightsUniform::default(),
                &[][..],
            )
        };
        self.mouse_sens_scale = sensitivity_scale;

        let debug_mode =
            if self.cvars.get_bool("r_lightmap") || self.cvars.get_bool("r_lightmap_only") {
                1.0
            } else if self.cvars.get_bool("r_fullbright") {
                2.0
            } else {
                0.0
            };
        let texture_mode = TextureFilterMode::parse(self.cvars.get_str("gl_texturemode"));
        let anisotropy = self.cvars.get_i32("gl_ansio").clamp(1, 16) as u16;
        let msaa_samples = if self.cvars.get_i32("gl_msaa") >= 4 {
            4
        } else {
            1
        };
        let viewmodel_fov = finite_or(self.cvars.get_f32("viewmodel_fov"), 90.0).clamp(50.0, 120.0);
        let settings = RenderSettings {
            gamma: finite_or(self.cvars.get_f32("gamma"), 2.5).clamp(1.8, 3.0),
            texgamma: finite_or(self.cvars.get_f32("texgamma"), 2.0).clamp(1.8, 3.0),
            lightgamma: finite_or(self.cvars.get_f32("lightgamma"), 2.5).clamp(1.8, 3.0),
            brightness: finite_or(self.cvars.get_f32("brightness"), 0.0).clamp(0.0, 3.0),
            gl_overbright: self.cvars.get_bool("gl_overbright"),
            direct: finite_or(self.cvars.get_f32("direct"), 0.9).clamp(0.75, 1.0),
            gl_picmip: self.cvars.get_i32("gl_picmip").clamp(0, 4),
            gl_max_size: self.cvars.get_i32("gl_max_size").clamp(64, 4096) as u32,
            gl_round_down: self.cvars.get_i32("gl_round_down").clamp(0, 3),
            fov_deg: fov,
            viewmodel_fov,
            aspect_mode: self.cvars.get_str("vid_aspect_mode"),
            texture_mode,
            anisotropy,
            msaa_samples,
            software_mode: self.cvars.get_bool("r_software_mode"),
            r_vis: self.cvars.get_bool("r_vis"),
            r_decals: self.cvars.get_i32("r_decals").max(0) as usize,
            wave_height: 0.0,
            debug_mode,
            time_sec,
            fog,
        };
        let vm_ref =
            viewmodel_inst.map(
                |(path, xf, seq, body, started, right_hand)| StudioInstance {
                    model_path: path,
                    transform: xf,
                    sequence: seq,
                    frame: 0.0,
                    animtime: started,
                    framerate: 1.0,
                    now: sess_now as f32,
                    body,
                    light_rgb: Vec3::splat(0.95),
                    flip_winding: right_hand,
                    attached_model: None,
                    ..StudioInstance::default()
                },
            );
        let scene_data = SceneDynamicData {
            vfs: self.vfs.as_ref(),
            viewmodel: vm_ref,
            beams: beams_slice,
            points: points_slice,
            decals: decals_slice,
            custom_decals: custom_decals_opt,
            lightstyles: lightstyles_ref,
            lights,
            elights: elights_slice,
        };

        self.frame_count += 1;
        let min_in_game_frames = self
            .launch
            .wait_seconds
            .map(|s| ((s * 60.0).ceil() as u64).max(24))
            .unwrap_or(24);
        let waiting_for_game = self.launch.startup_map.is_some()
            || !self.launch.startup_exec.is_empty()
            || self.session.is_active();
        let ready_for_capture = if waiting_for_game {
            (self.in_game_frames >= min_in_game_frames
                && (self.startup_ui_applied || self.launch.startup_ui.is_none()))
                || self.frame_count >= 600
        } else {
            self.frame_count >= 6
        };
        let auto_capture_now = self.launch.auto_screenshot.is_some()
            && if waiting_for_game {
                ready_for_capture
            } else {
                self.frame_count == 6
            };
        let record_target = if self.launch.record_frames == 0 {
            72
        } else {
            self.launch.record_frames
        };
        let record_this_frame = self.launch.record_dir.is_some()
            && ready_for_capture
            && self.recorded_frames < record_target;
        let capture_path = if auto_capture_now {
            self.launch.auto_screenshot.take()
        } else if record_this_frame {
            self.launch
                .record_dir
                .as_ref()
                .map(|dir| dir.join(format!("frame_{:04}.png", self.recorded_frames)))
        } else {
            self.pending_screenshot.take()
        };

        let render_result = rt.renderer.render_frame(
            &render_cam,
            &settings,
            &scene_data,
            full_output.textures_delta,
            &paint_jobs,
            full_output.pixels_per_point,
            capture_path.as_deref(),
        );
        if let Some(prev_scene) = rt.renderer.take_entity_scene() {
            self.frame_view.restore_scene(prev_scene);
        }
        if let Some(stats) = render_result {
            self.last_stats = stats;
            if record_this_frame {
                self.recorded_frames += 1;
                if self.recorded_frames >= record_target {
                    if let Some(el) = event_loop {
                        el.exit();
                    }
                    return true;
                }
            }
            if auto_capture_now && self.exit_after_screenshot {
                if let Some(el) = event_loop {
                    el.exit();
                }
                return true;
            }
        }

        // 5. Post-frame actions.
        if launch_demo_from_setup {
            self.launch_instant_demo();
        }
        if let Some(dir) = mount_path_requested
            && let Err(err) = self.mount_gamedir(&dir)
        {
            self.setup_error = Some(err.clone());
            self.cvars.error(err);
        }
        if open_console_from_setup {
            self.console_ui.open();
        }
        if quit_from_setup {
            pending_actions.push(EngineAction::Quit);
        }
        self.apply_demo_ui_actions(demo_actions);
        for (mod_id, allow) in mod_decisions {
            if allow {
                self.mod_transfer.approve(&mod_id);
            } else {
                self.mod_transfer.deny(&mod_id);
            }
        }
        for cmd in self.mod_transfer.take_outgoing_commands() {
            self.session.send_stringcmd(&cmd);
        }
        if let Some((team_only, text)) = sent_chat {
            let cmd = if team_only { "say_team" } else { "say" };
            self.forward_to_server(&format!("{cmd} \"{}\"", quote_chat(&text)));
        }
        if motd_close {
            self.feeds.motd_open = false;
            self.motd_ui.open = false;
        }
        if let Some(cmd) = vgui_cmd_out {
            self.session.send_stringcmd(&cmd);
            self.session.with_hud_mut(|h| h.vgui_menu = None);
        } else if vgui_closed {
            self.session.with_hud_mut(|h| h.vgui_menu = None);
        }
        if let Some(cmd) = cmdmenu_exec {
            pending_actions.extend(self.cvars.execute_script(&cmd));
        }
        if let Some(vban) = vban_cmd {
            self.forward_to_server(&vban);
        }
        if let Some((addr, pw)) = pw_submit {
            self.cvars.set("password", &pw);
            self.connect_remote(&addr);
        }
        if cancel_loading {
            self.end_session();
        }
        if should_resume {
            self.menu_ui.visible = false;
            self.options_ui.open = false;
            self.console_ui.open = false;
            self.chat_ui.open = false;
        }
        if self.options_ui.hd_models_changed {
            self.options_ui.hd_models_changed = false;
            let enabled = self.cvars.get_bool("cl_hd_models");
            if let Some(vfs) = &mut self.vfs {
                vfs.set_hd_models(enabled);
                self.available_maps = vfs.list_maps();
                if let Some(rt) = &mut self.runtime {
                    rt.renderer.clear_model_caches();
                }
                self.cvars.info(format!(
                    "cl_hd_models = {} (VFS & model cache refreshed)",
                    i32::from(enabled)
                ));
            }
        }
        if let Some(req) = self.options_ui.video_apply_requested.take() {
            self.apply_video_request(req);
        }
        if self.options_ui.save_requested {
            self.options_ui.save_requested = false;
            self.save_config();
        }
        if !pending_actions.is_empty() && self.process_actions(pending_actions, event_loop) {
            return true;
        }
        self.sync_cursor_capture();

        if !self.launch.headless {
            let fps_max = finite_or(self.cvars.get_f32("fps_max"), 100.0);
            let fps_override = self.cvars.get_bool("fps_override");
            if let Some(fps) = effective_fps_limit(
                fps_max,
                fps_override,
                self.session.is_active(),
                self.session.max_clients(),
            ) {
                let timescale =
                    finite_or(self.cvars.get_f32("host_timescale"), 1.0).clamp(0.05, 10.0);
                let target = Duration::from_secs_f64(f64::from(timescale) / (f64::from(fps) + 0.5));
                let elapsed = now.elapsed();
                if let Some(rem) = target.checked_sub(elapsed)
                    && rem > Duration::from_millis(1)
                {
                    std::thread::sleep(rem - Duration::from_millis(1));
                }
            }
        }

        if let Some(rt) = &self.runtime
            && let Some(win) = &rt.window
        {
            win.request_redraw();
        }
        false
    }

    fn apply_video_request(&mut self, req: VideoApplyRequest) {
        let mut rebuild_backend = false;
        let desired_backend = req.backend.trim().to_ascii_lowercase();
        if desired_backend != self.active_backend_cvar {
            rebuild_backend = true;
        }
        if let Some(rt) = &mut self.runtime
            && let Some(win) = &rt.window
        {
            if req.fullscreen {
                win.set_fullscreen(Some(Fullscreen::Borderless(None)));
            } else {
                win.set_fullscreen(None);
                let _ = win.request_inner_size(PhysicalSize::new(req.width, req.height));
            }
            let sz = win.inner_size();
            let rw = if req.fullscreen {
                sz.width.max(1)
            } else {
                req.width.max(1)
            };
            let rh = if req.fullscreen {
                sz.height.max(1)
            } else {
                req.height.max(1)
            };
            rt.renderer.resize(rw, rh);
            rt.renderer.sync_swapchain_latency(
                self.cvars.get_bool("gl_vsync"),
                self.cvars.get_bool("gl_lowlatency"),
            );
        }
        if rebuild_backend {
            let win_opt = self.runtime.as_ref().and_then(|rt| rt.window.clone());
            if let Some(win) = win_opt {
                match create_window_renderer(Arc::clone(&win), &desired_backend) {
                    Ok(mut new_renderer) => {
                        new_renderer.sync_swapchain_latency(
                            self.cvars.get_bool("gl_vsync"),
                            self.cvars.get_bool("gl_lowlatency"),
                        );
                        let active_map = self.session.loaded_map().map(str::to_string);
                        let (egui_ctx, bg_texture, brand_logo, hud_sprites, vgui_bank) =
                            self.init_egui_context();
                        let egui_state = egui_winit::State::new(
                            egui_ctx.clone(),
                            egui::ViewportId::ROOT,
                            &win,
                            Some(1.0),
                            None,
                            None,
                        );
                        let captured = self.runtime.as_ref().is_some_and(|r| r.cursor_captured);
                        if let Some(ref map_name) = active_map
                            && let Some(vfs) = &self.vfs
                            && let Ok(bsp) = BspMap::load(vfs, map_name)
                        {
                            let sky = SkyboxTextures::load(vfs, &bsp.skyname);
                            new_renderer.load_map(&bsp, &sky);
                        }
                        self.cvars.info(format!(
                            "Switched GPU renderer backend to {} ({})",
                            new_renderer.backend_name, new_renderer.adapter_name
                        ));
                        self.active_backend_cvar = desired_backend;
                        self.runtime = Some(WindowRuntime {
                            window: Some(win),
                            renderer: new_renderer,
                            egui_ctx,
                            egui_state: Some(egui_state),
                            bg_texture,
                            brand_logo,
                            hud_sprites,
                            vgui_bank,
                            cursor_captured: captured,
                        });
                    }
                    Err(err) => {
                        self.cvars.warn(format!(
                            "GPU backend `{desired_backend}` unavailable ({err}); keeping current backend."
                        ));
                    }
                }
            }
        }
    }

    fn init_egui_context(
        &self,
    ) -> (
        egui::Context,
        Option<egui::TextureHandle>,
        Option<egui::TextureHandle>,
        Option<HudSprites>,
        VguiAssetBank,
    ) {
        let egui_ctx = egui::Context::default();
        egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        egui_ctx.set_zoom_factor(1.0);
        egui_ctx.set_pixels_per_point(1.0);
        apply_vgui2_theme(&egui_ctx, self.palette);
        if let Some(vfs) = &self.vfs {
            install_scheme_fonts_into_egui(&egui_ctx, vfs, &self.vgui_rt.scheme_fonts);
        }
        let bg_texture = self.vfs.as_ref().and_then(|vfs| {
            load_menu_background(vfs).ok().map(|img| {
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width as usize, img.height as usize],
                    &img.rgba,
                );
                egui_ctx.load_texture(
                    "cs16_menu_background",
                    color_image,
                    egui::TextureOptions::LINEAR,
                )
            })
        });
        let brand_logo =
            cs_client::ui::vgui_assets::load_brand_logo_texture(&egui_ctx, "cs16_brand_logo");
        let hud_sprites = self
            .vfs
            .as_ref()
            .map(|vfs| HudSprites::load(&egui_ctx, vfs));
        let vgui_bank = VguiAssetBank::load(&egui_ctx, self.vfs.as_ref());
        (egui_ctx, bg_texture, brand_logo, hud_sprites, vgui_bank)
    }

    fn apply_startup_commands(&mut self, event_loop: Option<&ActiveEventLoop>) -> bool {
        if let Some(ui_mode) = self.launch.startup_ui.as_deref() {
            match ui_mode.to_ascii_lowercase().as_str() {
                "buy:rifles" => {
                    self.cvars.set("bot_quota", "0");
                    self.cvars.set("mp_startmoney", "16000");
                }
                "ak47" | "m4a1:silenced" | "awp:scope" | "aug:zoom" => {
                    self.cvars.set("bot_quota", "0");
                    self.cvars.set("mp_startmoney", "16000");
                    self.cvars.set("mp_freezetime", "0");
                }
                "blood:bot" => {
                    self.cvars.set("bot_quota", "0");
                    self.cvars.set("mp_startmoney", "16000");
                    self.cvars.set("mp_freezetime", "0");
                    self.cvars.set("bot_stop", "1");
                }
                _ => {}
            }
        }
        let startup_cmds = std::mem::take(&mut self.launch.startup_exec);
        for cmd in startup_cmds {
            let actions = self.cvars.execute_script(&cmd);
            if self.process_actions(actions, event_loop) {
                return true;
            }
        }
        if let Some(map) = self.launch.startup_map.clone() {
            self.start_local_map(&map);
        }
        if let Some(ui_mode) = self.launch.startup_ui.take() {
            let lower = ui_mode.to_ascii_lowercase();
            if lower == "options" {
                self.options_ui.open = true;
            } else if let Some(tab_name) = lower.strip_prefix("options:") {
                if let Some(tab) = cs_client::ui::options::OptionsTab::from_name(tab_name) {
                    self.options_ui.tab = tab;
                }
                self.options_ui.open = true;
            } else {
                match lower.as_str() {
                    "newgame" => self.menu_ui.new_game_open = true,
                    "newgame:bots" => self.menu_ui.open_new_game_bots_tab(),
                    "servers" => {
                        self.menu_ui.find_servers_open = true;
                        self.menu_ui.server_browser.open = true;
                    }
                    "console" => self.console_ui.open(),
                    "playerlist" => self.player_list_ui.open = true,
                    "demo" => self.demo_player_ui.open = true,
                    _ => {
                        self.launch.startup_ui = Some(ui_mode);
                    }
                }
            }
            self.sync_cursor_capture();
        }
        false
    }

    /// Run headless offscreen without creating any visible OS window (for `cargo xtask visual-diff`
    /// and headless frame-sequence recording).
    pub fn run_headless(mut self) -> Result<(), String> {
        if let Some(dir) = &self.launch.record_dir {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("create record dir {}: {e}", dir.display()))?;
            let fps = self.launch.record_fps.clamp(1.0, 120.0);
            self.headless_fixed_dt = Some(1.0 / fps);
        }
        let renderer = Renderer::new_headless(self.launch.width, self.launch.height)?;
        if !self
            .cvars
            .get_str("r_refdll")
            .eq_ignore_ascii_case(&renderer.backend_name)
        {
            self.cvars.set("r_refdll", "auto");
        }
        self.active_backend_cvar = self.cvars.get_str("r_refdll").to_ascii_lowercase();
        let (egui_ctx, bg_texture, brand_logo, hud_sprites, vgui_bank) = self.init_egui_context();
        info!(gpu = %renderer.adapter_name, "Counter-Strike 1.6 (Rust) headless runtime ready");
        self.runtime = Some(WindowRuntime {
            window: None,
            renderer,
            egui_ctx,
            egui_state: None,
            bg_texture,
            brand_logo,
            hud_sprites,
            vgui_bank,
            cursor_captured: false,
        });
        if self.apply_startup_commands(None) {
            self.end_session();
            return Ok(());
        }
        let timeout_secs = if self.launch.record_dir.is_some() {
            90
        } else {
            30
        };
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        while Instant::now() < deadline {
            if self.render_and_step(None) {
                self.end_session();
                return Ok(());
            }
            if self.launch.record_dir.is_some() && self.is_in_game() {
                if self.last_connect_addr.is_some() {
                    let fps = self.launch.record_fps.clamp(1.0, 120.0);
                    std::thread::sleep(Duration::from_secs_f32(1.0 / fps));
                }
            } else {
                std::thread::sleep(Duration::from_millis(16));
            }
        }
        let pending = self.launch.auto_screenshot.take();
        let rec_dir = self.launch.record_dir.take();
        let rec_done = self.recorded_frames;
        let rec_target = if self.launch.record_frames == 0 {
            72
        } else {
            self.launch.record_frames
        };
        self.end_session();
        if let Some(path) = pending {
            return Err(format!(
                "headless run timed out after {timeout_secs}s before saving {} (in_game_frames={}, frames={})",
                path.display(),
                self.in_game_frames,
                self.frame_count
            ));
        }
        if let Some(dir) = rec_dir
            && rec_done < rec_target
        {
            return Err(format!(
                "headless recording timed out after {timeout_secs}s in {} ({rec_done}/{rec_target} frames)",
                dir.display()
            ));
        }
        Ok(())
    }
}

impl ApplicationHandler for Cs16App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.runtime.is_some() {
            return;
        }

        let mut attrs = Window::default_attributes()
            .with_title("Counter-Strike 1.6 (Rust)")
            .with_inner_size(PhysicalSize::new(self.launch.width, self.launch.height));
        if self.cvars.get_bool("fullscreen") {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        if let Some(icon) = load_window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));

        let desired_backend = self.cvars.get_str("r_refdll").to_ascii_lowercase();
        let mut renderer = match create_window_renderer(Arc::clone(&window), &desired_backend) {
            Ok(r) => {
                self.active_backend_cvar = desired_backend;
                r
            }
            Err(err) => {
                warn!(backend = %desired_backend, %err, "Requested GPU backend unavailable; falling back to default");
                self.cvars.set("r_refdll", "auto");
                self.active_backend_cvar = "auto".to_string();
                Renderer::new(Arc::clone(&window))
            }
        };
        renderer.sync_swapchain_latency(
            self.cvars.get_bool("gl_vsync"),
            self.cvars.get_bool("gl_lowlatency"),
        );
        let (egui_ctx, bg_texture, brand_logo, hud_sprites, vgui_bank) = self.init_egui_context();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(1.0),
            None,
            None,
        );

        info!(gpu = %renderer.adapter_name, "Counter-Strike 1.6 (Rust) window ready");

        self.runtime = Some(WindowRuntime {
            window: Some(window),
            renderer,
            egui_ctx,
            egui_state: Some(egui_state),
            bg_texture,
            brand_logo,
            hud_sprites,
            vgui_bank,
            cursor_captured: false,
        });

        let _ = self.apply_startup_commands(Some(event_loop));
        if let Some(rt) = &self.runtime
            && let Some(win) = &rt.window
        {
            win.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match &event {
            WindowEvent::CloseRequested => {
                if self.cvars.is_dirty() {
                    self.save_config();
                }
                self.end_session();
                event_loop.exit();
                return;
            }
            WindowEvent::Focused(false) => {
                if let Some(rt) = &mut self.runtime
                    && rt.cursor_captured
                    && let Some(win) = &rt.window
                {
                    let _ = win.set_cursor_grab(CursorGrabMode::None);
                    win.set_cursor_visible(true);
                    rt.cursor_captured = false;
                }
                self.input_state.clear();
                self.pending_mouse_delta = (0.0, 0.0);
            }
            WindowEvent::Resized(new_size) => {
                if let Some(rt) = &mut self.runtime {
                    rt.renderer.resize(new_size.width, new_size.height);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.wants_mouse_capture() {
                    let pressed = *state == ElementState::Pressed;
                    if let Some(key_name) = mouse_button_to_goldsrc(*button) {
                        let _ = self.handle_bound_key(key_name, pressed, Some(event_loop));
                    }
                    return;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.wants_mouse_capture() {
                    let notches = match delta {
                        MouseScrollDelta::LineDelta(_, y) => {
                            self.input_state.accumulate_wheel(*y, true)
                        }
                        MouseScrollDelta::PixelDelta(pos) => {
                            self.input_state.accumulate_wheel(pos.y as f32, false)
                        }
                    };
                    let count = notches.unsigned_abs().min(16);
                    let key_name = if notches >= 0 {
                        "mwheelup"
                    } else {
                        "mwheeldown"
                    };
                    for _ in 0..count {
                        let _ = self.handle_bound_key(key_name, true, Some(event_loop));
                        let _ = self.handle_bound_key(key_name, false, Some(event_loop));
                    }
                    return;
                }
            }
            WindowEvent::KeyboardInput { event: key_ev, .. } => {
                if self.handle_key_event(key_ev, event_loop) {
                    return;
                }
            }
            WindowEvent::RedrawRequested => {
                let _ = self.render_and_step(Some(event_loop));
                return;
            }
            _ => {}
        }

        if !self.wants_mouse_capture()
            && let Some(rt) = &mut self.runtime
            && let (Some(state), Some(win)) = (&mut rt.egui_state, &rt.window)
        {
            let _ = state.on_window_event(win, &event);
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && self.wants_mouse_capture()
        {
            self.accumulate_raw_mouse_delta(dx, dy);
        }
    }
}

/// A chat line inside quotes: a `"` would end the quoted argument, so it becomes `'`.
fn quote_chat(text: &str) -> String {
    text.replace('"', "'")
}

/// Keeps the newest [`MAX_FEED`] entries.
fn trim_feed<T>(v: &mut Vec<T>) {
    let extra = v.len().saturating_sub(MAX_FEED);
    v.drain(..extra);
}

fn digit_of(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Digit0 => 0,
        KeyCode::Digit1 => 1,
        KeyCode::Digit2 => 2,
        KeyCode::Digit3 => 3,
        KeyCode::Digit4 => 4,
        KeyCode::Digit5 => 5,
        KeyCode::Digit6 => 6,
        KeyCode::Digit7 => 7,
        KeyCode::Digit8 => 8,
        KeyCode::Digit9 => 9,
        _ => return None,
    })
}

fn mouse_button_to_goldsrc(button: MouseButton) -> Option<&'static str> {
    match button {
        MouseButton::Left => Some("mouse1"),
        MouseButton::Right => Some("mouse2"),
        MouseButton::Middle => Some("mouse3"),
        MouseButton::Back => Some("mouse4"),
        MouseButton::Forward => Some("mouse5"),
        MouseButton::Other(_) => None,
    }
}

/// Map `r_refdll` (`auto`, `vulkan`, `dx12`, `metal`, `gl`) to `wgpu::Backends` and construct a
/// surface-backed [`Renderer`] without touching `render/mod.rs`.
fn create_window_renderer(window: Arc<Window>, backend_cvar: &str) -> Result<Renderer, String> {
    let backends = match backend_cvar.trim().to_ascii_lowercase().as_str() {
        "vulkan" | "vk" => wgpu::Backends::VULKAN,
        "dx12" | "d3d12" | "direct3d12" => wgpu::Backends::DX12,
        "metal" => wgpu::Backends::METAL,
        "gl" | "opengl" | "gles" => wgpu::Backends::GL,
        _ => wgpu::Backends::PRIMARY | wgpu::Backends::GL,
    };
    let size = window.inner_size();
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    let instance = wgpu::Instance::new(desc);
    let surface = instance
        .create_surface(window)
        .map_err(|e| format!("create wgpu surface: {e}"))?;
    pollster::block_on(Renderer::new_from_surface(
        &instance,
        surface,
        size.width.max(1),
        size.height.max(1),
    ))
}

/// Lock `egui::RawInput` to an exact `1.0` `pixels_per_point` physical pixel grid (`phys_w x phys_h`)
/// regardless of Windows/macOS DPI scaling (`100%`, `125%`, `150%`, `200%`).
///
/// `egui_winit::State::on_window_event` divides `PhysicalPosition` cursor/touch/scroll deltas by
/// `window.scale_factor()`; multiplying those events back by `event_scale` restores 1:1 physical
/// coordinates so `ctx.content_rect()` at `1280x720` is always exactly `1280x720` and clicks hit
/// exact pixel positions.
fn normalize_raw_input_physical_pixels(
    raw_input: &mut egui::RawInput,
    phys_w: u32,
    phys_h: u32,
    event_scale: f32,
) {
    let scale = if event_scale.is_finite() && event_scale > 1e-3 {
        event_scale
    } else {
        1.0
    };
    raw_input.screen_rect = Some(egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(phys_w.max(1) as f32, phys_h.max(1) as f32),
    ));
    let vp = raw_input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default();
    vp.native_pixels_per_point = Some(1.0);
    if let Some(ref mut r) = vp.inner_rect {
        *r = egui::Rect::from_min_max(
            egui::Pos2::new(r.min.x * scale, r.min.y * scale),
            egui::Pos2::new(r.max.x * scale, r.max.y * scale),
        );
    }
    if let Some(ref mut r) = vp.outer_rect {
        *r = egui::Rect::from_min_max(
            egui::Pos2::new(r.min.x * scale, r.min.y * scale),
            egui::Pos2::new(r.max.x * scale, r.max.y * scale),
        );
    }
    if (scale - 1.0).abs() > 1e-4 {
        for ev in &mut raw_input.events {
            match ev {
                egui::Event::PointerMoved(pos) => {
                    pos.x *= scale;
                    pos.y *= scale;
                }
                egui::Event::PointerButton { pos, .. } => {
                    pos.x *= scale;
                    pos.y *= scale;
                }
                egui::Event::Touch { pos, .. } => {
                    pos.x *= scale;
                    pos.y *= scale;
                }
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta,
                    ..
                } => {
                    delta.x *= scale;
                    delta.y *= scale;
                }
                _ => {}
            }
        }
    }
}

fn physical_key_to_goldsrc(code: KeyCode) -> Option<&'static str> {
    Some(match code {
        KeyCode::KeyA => "a",
        KeyCode::KeyB => "b",
        KeyCode::KeyC => "c",
        KeyCode::KeyD => "d",
        KeyCode::KeyE => "e",
        KeyCode::KeyF => "f",
        KeyCode::KeyG => "g",
        KeyCode::KeyH => "h",
        KeyCode::KeyI => "i",
        KeyCode::KeyJ => "j",
        KeyCode::KeyK => "k",
        KeyCode::KeyL => "l",
        KeyCode::KeyM => "m",
        KeyCode::KeyN => "n",
        KeyCode::KeyO => "o",
        KeyCode::KeyP => "p",
        KeyCode::KeyQ => "q",
        KeyCode::KeyR => "r",
        KeyCode::KeyS => "s",
        KeyCode::KeyT => "t",
        KeyCode::KeyU => "u",
        KeyCode::KeyV => "v",
        KeyCode::KeyW => "w",
        KeyCode::KeyX => "x",
        KeyCode::KeyY => "y",
        KeyCode::KeyZ => "z",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::Slash => "/",
        KeyCode::Backslash => "\\",
        KeyCode::Space => "space",
        KeyCode::Enter | KeyCode::NumpadEnter => "enter",
        KeyCode::Backspace => "backspace",
        KeyCode::ControlLeft | KeyCode::ControlRight => "ctrl",
        KeyCode::ShiftLeft | KeyCode::ShiftRight => "shift",
        KeyCode::AltLeft | KeyCode::AltRight => "alt",
        KeyCode::Tab => "tab",
        KeyCode::ArrowUp => "uparrow",
        KeyCode::ArrowDown => "downarrow",
        KeyCode::ArrowLeft => "leftarrow",
        KeyCode::ArrowRight => "rightarrow",
        KeyCode::Insert => "ins",
        KeyCode::Delete => "del",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::PageUp => "pgup",
        KeyCode::PageDown => "pgdn",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::F1 => "f1",
        KeyCode::F2 => "f2",
        KeyCode::F3 => "f3",
        KeyCode::F4 => "f4",
        KeyCode::F5 => "f5",
        KeyCode::F6 => "f6",
        KeyCode::F7 => "f7",
        KeyCode::F8 => "f8",
        KeyCode::F9 => "f9",
        KeyCode::F10 => "f10",
        KeyCode::F11 => "f11",
        KeyCode::F12 => "f12",
        _ => return None,
    })
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

/// GoldSrc `IN_MouseMove` zoom sensitivity multiplier (`client.dll` `0x10036130..10036270`):
/// returns `(fov / default_fov) * zoom_ratio` whenever zoomed (`fov != default_fov`), unclamped.
fn compute_zoom_sens_scale(fov: f32, default_fov: f32, zoom_ratio: f32) -> f32 {
    if default_fov > 1.0 && (fov - default_fov).abs() > 1e-4 {
        (fov / default_fov) * zoom_ratio.max(0.0)
    } else {
        1.0
    }
}

/// GoldSrc `IN_MouseMove` custom acceleration & zoom sensitivity (`client.dll` `0x10036130..10036270`).
#[allow(clippy::too_many_arguments)]
fn compute_mouse_move_params(
    fx: f32,
    fy: f32,
    base_sens: f32,
    zoom_sens_scale: f32,
    m_customaccel: i32,
    accel_scale: f32,
    accel_max: f32,
    accel_exp: f32,
    m_yaw: f32,
    m_pitch: f32,
) -> (f32, f32, f32) {
    let (mut adj_x, mut adj_y) = (fx, fy);
    let mut sens = base_sens.max(0.0);
    if m_customaccel != 0 {
        if m_customaccel == 2 {
            adj_x *= m_yaw;
            adj_y *= m_pitch;
        }
        let mag = (adj_x * adj_x + adj_y * adj_y).sqrt();
        let accel = if mag > 0.0 {
            finite_or(mag.powf(accel_exp) * accel_scale, 0.0)
        } else {
            0.0
        };
        sens += accel;
        if accel_max > 1e-4 {
            sens = sens.min(accel_max);
        }
    }
    (adj_x, adj_y, sens * zoom_sens_scale)
}

/// GoldSrc frame rate cap (`hw.dll` `FUN_101d43d0`, `1 < maxclients`): enforces `fps_max >= 20.0`
/// in multiplayer (`max_clients > 1`).
fn effective_fps_limit(
    fps_max: f32,
    fps_override: bool,
    active: bool,
    max_clients: u8,
) -> Option<f32> {
    if fps_max <= 0.0 {
        return None;
    }
    if !active {
        return Some(31.0);
    }
    let min_fps = if max_clients > 1 { 20.0 } else { 0.5 };
    let f = fps_max.max(min_fps);
    Some(if fps_override { f } else { f.min(100.0) })
}

/// Returns `true` when `cl_cmd_100hz` should decouple `usercmd` emission onto a clean `10 ms`
/// (`100 Hz`) cadence: requires `fps_override 1` running above `105 FPS` (or unlimited).
fn should_use_100hz_cmd_grid(
    fps_max: f32,
    fps_override: bool,
    cl_cmd_100hz: bool,
    active: bool,
    max_clients: u8,
) -> bool {
    if !fps_override || !cl_cmd_100hz {
        return false;
    }
    effective_fps_limit(fps_max, fps_override, active, max_clients).is_none_or(|f| f > 105.0)
}

/// Converts 1-based `HudState.players` entity keys to 0-based `ScoreRow.slot` and `names` keys.
fn build_score_rows(hud: &HudState, names: &BTreeMap<u8, String>) -> (Vec<ScoreRow>, i16, i16) {
    let rows = hud
        .players
        .iter()
        .filter(|(_, p)| {
            matches!(
                p.team.as_str(),
                "CT" | "TERRORIST" | "SPECTATOR" | "UNASSIGNED"
            )
        })
        .filter_map(|(slot, p)| {
            let slot0 = slot.checked_sub(1)?;
            Some(ScoreRow {
                slot: slot0,
                name: names
                    .get(&slot0)
                    .cloned()
                    .unwrap_or_else(|| format!("player {slot}")),
                frags: p.frags,
                deaths: p.deaths,
                ping: 0,
                team: p.team.clone(),
                dead: p.attrib & 1 != 0,
                bomb: p.attrib & 2 != 0,
                vip: p.attrib & 4 != 0,
            })
        })
        .collect();
    (
        rows,
        hud.team_scores.get("CT").copied().unwrap_or(0),
        hud.team_scores.get("TERRORIST").copied().unwrap_or(0),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellStuffAction {
    CloseMenu,
    PlaySound(String),
    SpeakSentence(String),
    Mp3(Vec<String>),
    Cd(Vec<String>),
    StopSound,
    Echo(String),
}

/// Allowlisted `svc_stufftext` commands handled by the shell without re-parsing through `execute_script`.
fn classify_shell_command(cmd: &ConsoleCommand) -> Option<ShellStuffAction> {
    match cmd.name.to_ascii_lowercase().as_str() {
        "slot10" => Some(ShellStuffAction::CloseMenu),
        "stopsound" => Some(ShellStuffAction::StopSound),
        "echo" => Some(ShellStuffAction::Echo(cmd.args.join(" "))),
        "mp3"
            if matches!(
                cmd.args.first().map(String::as_str),
                Some("play" | "loop" | "stop")
            ) =>
        {
            Some(ShellStuffAction::Mp3(cmd.args.clone()))
        }
        "cd" if matches!(
            cmd.args.first().map(String::as_str),
            Some("play" | "loop" | "stop" | "off")
        ) =>
        {
            Some(ShellStuffAction::Cd(cmd.args.clone()))
        }
        "spk" | "play" => {
            if cmd.args.len() != 1 {
                return None;
            }
            let sample = cmd.args[0].trim();
            if sample.is_empty()
                || sample.starts_with(['/', '\\'])
                || sample.contains("..")
                || sample.contains([';', '\n', '\r', '\0'])
            {
                return None;
            }
            if cmd.name.eq_ignore_ascii_case("spk")
                && !sample.to_ascii_lowercase().ends_with(".wav")
            {
                Some(ShellStuffAction::SpeakSentence(sample.to_string()))
            } else {
                Some(ShellStuffAction::PlaySound(sample.to_string()))
            }
        }
        _ => None,
    }
}

fn vgui_menu_fallback_lines(menu: &VguiMenuState) -> Vec<String> {
    match menu.menu_id {
        2 => vec![
            "Select Team".into(),
            String::new(),
            "1. Terrorist Force".into(),
            "2. Counter-Terrorist Force".into(),
            String::new(),
            "5. Auto-Assign".into(),
            "6. Spectator".into(),
        ],
        3 | 26 => vec![
            "Select Class".into(),
            String::new(),
            "1 - 4. Choose Appearance".into(),
            "5. Auto-Select".into(),
        ],
        4..=10 => vec![
            "Buy Menu".into(),
            String::new(),
            "Press 1-8 to select, 0 to exit".into(),
        ],
        id => vec![
            format!("Server Menu #{id}"),
            String::new(),
            "Press number key to select, 0 to exit".into(),
        ],
    }
}

fn vgui_menu_select_command(menu_id: u8, n: u8) -> String {
    match menu_id {
        2 => format!("jointeam {n}"),
        3 | 26 => format!("joinclass {n}"),
        _ => format!("menuselect {n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_clientcore::hud::PlayerScore;

    #[test]
    fn score_rows_align_one_based_players_with_zero_based_slots_and_names() {
        let mut hud = HudState::default();
        hud.players.insert(
            1,
            PlayerScore {
                team: "CT".to_string(),
                frags: 5,
                deaths: 1,
                attrib: 0,
                ..Default::default()
            },
        );
        let mut names = BTreeMap::new();
        names.insert(0, "Alpha".to_string());
        let (rows, _, _) = build_score_rows(&hud, &names);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].slot, 0);
        assert_eq!(rows[0].name, "Alpha");
    }

    #[test]
    fn score_rows_include_spectators_and_unassigned() {
        let mut hud = HudState::default();
        for (slot1, team) in [
            (1u8, "CT"),
            (2, "TERRORIST"),
            (3, "SPECTATOR"),
            (4, "UNASSIGNED"),
        ] {
            hud.players.insert(
                slot1,
                PlayerScore {
                    team: team.to_string(),
                    frags: i16::from(slot1),
                    deaths: 0,
                    attrib: 0,
                    ..Default::default()
                },
            );
        }
        let names = BTreeMap::new();
        let (rows, _, _) = build_score_rows(&hud, &names);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().any(|r| r.slot == 2 && r.team == "SPECTATOR"));
        assert!(rows.iter().any(|r| r.slot == 3 && r.team == "UNASSIGNED"));
    }

    #[test]
    fn shell_stufftext_allowlist_blocks_command_injection() {
        let injected = ConsoleCommand {
            name: "spk".into(),
            args: vec!["x; bind w quit; kill".into()],
        };
        assert_eq!(classify_shell_command(&injected), None);

        let evil_path = ConsoleCommand {
            name: "spk".into(),
            args: vec!["../secret".into()],
        };
        assert_eq!(classify_shell_command(&evil_path), None);

        let good_spk = ConsoleCommand {
            name: "spk".into(),
            args: vec!["radio/ctwin.wav".into()],
        };
        assert_eq!(
            classify_shell_command(&good_spk),
            Some(ShellStuffAction::PlaySound("radio/ctwin.wav".into()))
        );

        let slot10 = ConsoleCommand {
            name: "slot10".into(),
            args: Vec::new(),
        };
        assert_eq!(
            classify_shell_command(&slot10),
            Some(ShellStuffAction::CloseMenu)
        );
    }

    #[test]
    fn vgui_menu_fallback_renders_and_maps_selection_commands() {
        let m = VguiMenuState {
            menu_id: 2,
            keys: 0b110011,
            time: 0,
            multipart: false,
            name: String::new(),
        };
        let lines = vgui_menu_fallback_lines(&m);
        assert!(lines[0].contains("Team"));
        assert_eq!(vgui_menu_select_command(2, 2), "jointeam 2");
        assert_eq!(vgui_menu_select_command(26, 1), "joinclass 1");
        assert_eq!(vgui_menu_select_command(4, 3), "menuselect 3");
    }

    #[test]
    fn mouse_accel_zoom_sens_and_multiplayer_fps_floor_match_build_10210() {
        // Zoom scale has no 0.5 deadzone and no [0.05, 2.0] clamp.
        let low_zoom = compute_zoom_sens_scale(10.0, 90.0, 0.1);
        assert!((low_zoom - (10.0 / 90.0) * 0.1).abs() < 1e-5);
        let high_zoom = compute_zoom_sens_scale(45.0, 90.0, 5.0);
        assert!((high_zoom - 2.5).abs() < 1e-5);

        // Total (sensitivity + accel) is capped by m_customaccel_max (> 1e-4) BEFORE zoom scaling.
        let (dx, dy, sens) =
            compute_mouse_move_params(10.0, 0.0, 2.0, 0.5, 1, 1.0, 5.0, 5.0, 0.022, 0.022);
        assert_eq!((dx, dy), (10.0, 0.0));
        assert!((sens - 2.5).abs() < 1e-5);

        // m_customaccel == 2 applies extra m_yaw / m_pitch scaling.
        let (dx2, dy2, sens2) =
            compute_mouse_move_params(100.0, -50.0, 2.0, 1.0, 2, 0.5, 0.0, 1.0, 0.022, 0.03);
        assert!((dx2 - 2.2).abs() < 1e-5);
        assert!((dy2 - (-1.5)).abs() < 1e-5);
        let expected_mag = (dx2 * dx2 + dy2 * dy2).sqrt();
        assert!((sens2 - (2.0 + expected_mag * 0.5)).abs() < 1e-5);

        // Multiplayer enforces fps_max >= 20.0.
        assert_eq!(effective_fps_limit(10.0, false, true, 16), Some(20.0));
        assert_eq!(effective_fps_limit(10.0, false, true, 1), Some(10.0));
        assert_eq!(effective_fps_limit(150.0, false, true, 16), Some(100.0));
        assert_eq!(effective_fps_limit(150.0, true, true, 16), Some(150.0));
    }

    #[test]
    fn hud_feeds_expire_respects_zero_deathnotice_and_saytext() {
        let mut feeds = HudFeeds::default();
        feeds.deaths.push((
            DeathNotice {
                killer: 1,
                victim: 2,
                headshot: false,
                weapon: "ak47".into(),
            },
            1.0,
        ));
        feeds.chat.push((
            HudChatEntry {
                text: "hi".into(),
                team: None,
            },
            1.0,
        ));
        feeds.expire(1.1, 0.0, 0.0, 2.0);
        assert!(feeds.deaths.is_empty());
        assert!(feeds.chat.is_empty());
    }

    #[test]
    fn mouse_buttons_map_to_goldsrc_mouse1_through_mouse5() {
        assert_eq!(mouse_button_to_goldsrc(MouseButton::Left), Some("mouse1"));
        assert_eq!(mouse_button_to_goldsrc(MouseButton::Right), Some("mouse2"));
        assert_eq!(mouse_button_to_goldsrc(MouseButton::Middle), Some("mouse3"));
        assert_eq!(mouse_button_to_goldsrc(MouseButton::Back), Some("mouse4"));
        assert_eq!(
            mouse_button_to_goldsrc(MouseButton::Forward),
            Some("mouse5")
        );
        assert_eq!(mouse_button_to_goldsrc(MouseButton::Other(9)), None);
    }

    #[test]
    fn high_hz_cmd_grid_gating_respects_fps_override_and_cvar() {
        assert!(!should_use_100hz_cmd_grid(100.0, false, true, true, 16));
        assert!(!should_use_100hz_cmd_grid(240.0, false, true, true, 16));
        assert!(!should_use_100hz_cmd_grid(100.0, true, true, true, 16));
        assert!(!should_use_100hz_cmd_grid(240.0, true, false, true, 16));
        assert!(should_use_100hz_cmd_grid(144.0, true, true, true, 16));
        assert!(should_use_100hz_cmd_grid(240.0, true, true, true, 16));
        assert!(should_use_100hz_cmd_grid(0.0, true, true, true, 16));
    }

    #[test]
    fn raw_mouse_accumulation_samples_same_frame_and_supports_m_filter() {
        let mut app = Cs16App::new(LaunchConfig::default());
        app.cvars.set("sensitivity", "2.0");
        app.cvars.set("m_yaw", "0.022");
        app.cvars.set("m_pitch", "0.022");
        app.cvars.set("m_filter", "0");
        app.camera.yaw_deg = 100.0;
        app.camera.pitch_deg = 0.0;

        // Multiple high-polling-rate USB packets accumulate cleanly into one frame sample.
        app.accumulate_raw_mouse_delta(3.0, -1.0);
        app.accumulate_raw_mouse_delta(7.0, 5.0);
        app.sample_accumulated_mouse_delta();
        let expected_yaw = 100.0 - 10.0 * 2.0 * 0.022;
        let expected_pitch = 4.0 * 2.0 * 0.022;
        assert!((app.camera.yaw_deg - expected_yaw).abs() < 1e-4);
        assert!((app.camera.pitch_deg - expected_pitch).abs() < 1e-4);
        assert_eq!(app.pending_mouse_delta, (0.0, 0.0));

        // Late mouse motion arriving before camera matrix build flushes on the same frame.
        app.accumulate_raw_mouse_delta(5.0, 0.0);
        app.flush_late_mouse_delta();
        assert!((app.camera.yaw_deg - (expected_yaw - 5.0 * 2.0 * 0.022)).abs() < 1e-4);

        // `m_filter 1` averages current frame delta with previous frame delta.
        app.cvars.set("m_filter", "1");
        app.prev_mouse_delta = (0.0, 0.0);
        app.camera.yaw_deg = 100.0;
        app.accumulate_raw_mouse_delta(10.0, 0.0);
        app.sample_accumulated_mouse_delta();
        assert!((app.camera.yaw_deg - (100.0 - 5.0 * 2.0 * 0.022)).abs() < 1e-4);
        // Idle frame decays the second half of `m_filter`.
        app.sample_accumulated_mouse_delta();
        assert!((app.camera.yaw_deg - (100.0 - 10.0 * 2.0 * 0.022)).abs() < 1e-4);
    }

    #[test]
    fn embedded_window_icon_decodes_to_64x64_rgba() {
        let (w, h, rgba) = decode_app_icon_rgba().expect("decode icon-64.png");
        assert_eq!((w, h), (64, 64));
        assert_eq!(rgba.len(), 64 * 64 * 4);
        assert_eq!(rgba[3], 0, "top-left corner must be transparent");
        assert!(
            rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 255),
            "window icon must contain opaque foreground pixels"
        );
    }

    #[test]
    fn hidpi_physical_pixel_normalization_locks_1_to_1_across_all_scales() {
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            let mut raw = egui::RawInput::default();
            // Simulate egui_winit dividing PhysicalPosition(640, 360) by OS scale_factor.
            raw.events.push(egui::Event::PointerMoved(egui::Pos2::new(
                640.0 / scale,
                360.0 / scale,
            )));
            raw.events.push(egui::Event::PointerButton {
                pos: egui::Pos2::new(300.0 / scale, 200.0 / scale),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            normalize_raw_input_physical_pixels(&mut raw, 1280, 720, scale);
            assert_eq!(
                raw.screen_rect,
                Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0)
                ))
            );
            assert_eq!(
                raw.viewports[&egui::ViewportId::ROOT].native_pixels_per_point,
                Some(1.0)
            );
            match raw.events[0] {
                egui::Event::PointerMoved(p) => {
                    assert!((p.x - 640.0).abs() < 1e-2 && (p.y - 360.0).abs() < 1e-2);
                }
                _ => panic!("expected PointerMoved"),
            }
            match raw.events[1] {
                egui::Event::PointerButton { pos, .. } => {
                    assert!((pos.x - 300.0).abs() < 1e-2 && (pos.y - 200.0).abs() < 1e-2);
                }
                _ => panic!("expected PointerButton"),
            }
        }
    }

    #[test]
    fn crosshair_dt_decay_is_fps_independent_and_aug_keeps_crosshair_while_awp_scopes() {
        let inputs = CrosshairInputs {
            weapon_id: 28, // AK-47
            on_ground: true,
            dynamic: true,
            shots_fired: 3,
            ..CrosshairInputs::default()
        };
        let sim = |fps: usize| -> f32 {
            let mut st = CrosshairState::default();
            let dt = 1.0 / fps as f32;
            // Fire pulse on first frame, then decay for 0.5s.
            st.compute_geometry_dt(&inputs, "auto", 1280, dt);
            for _ in 0..(fps / 2) {
                st.compute_geometry_dt(&inputs, "auto", 1280, dt);
            }
            st.crosshair_dist
        };
        let d60 = sim(60);
        let d100 = sim(100);
        let d240 = sim(240);
        assert!(
            (d60 - d100).abs() < 0.05 && (d240 - d100).abs() < 0.05,
            "dt decay must match across FPS: 60={d60}, 100={d100}, 240={d240}"
        );

        // AUG (8) and SG552 (27) zoomed to fov=55 keep normal crosshair & no black sniper scope;
        // AWP (18), Scout (3), SG550 (13), G3SG1 (24) hide regular crosshair.
        assert!(should_draw_dynamic_crosshair(55.0, 8));
        assert!(should_draw_dynamic_crosshair(55.0, 27));
        assert!(!should_draw_dynamic_crosshair(40.0, 18));
        assert!(!should_draw_dynamic_crosshair(90.0, 18));
        assert!(!should_draw_dynamic_crosshair(40.0, 3));
    }

    #[test]
    fn vgui_buy_menu_blocks_select_slot_and_maps_all_goldsrc_keys() {
        let mut app = Cs16App::new(LaunchConfig::default());
        app.buy_team_class_ui.open_kind(VguiMenuKind::Buy, true);
        assert!(app.buy_team_class_ui.is_open());
        assert!(app.any_modal_open());
        // Calling select_slot while VGUI Buy menu is open MUST NOT close or clobber it.
        app.select_slot(4);
        assert!(app.buy_team_class_ui.is_open());

        assert_eq!(physical_key_to_goldsrc(KeyCode::KeyO), Some("o"));
        assert_eq!(physical_key_to_goldsrc(KeyCode::KeyF), Some("f"));
        assert_eq!(physical_key_to_goldsrc(KeyCode::KeyK), Some("k"));
        assert_eq!(physical_key_to_goldsrc(KeyCode::KeyH), Some("h"));
        assert_eq!(physical_key_to_goldsrc(KeyCode::F1), Some("f1"));
        assert_eq!(physical_key_to_goldsrc(KeyCode::F2), Some("f2"));
        assert_eq!(physical_key_to_goldsrc(KeyCode::ArrowUp), Some("uparrow"));
    }
}
