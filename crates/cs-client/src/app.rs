//! The window shell: `winit` events, the console and menus, and one [`Session`] (a local game or
//! a connection) that does the rest through `cs-clientcore`. Each frame the shell sends the
//! input as a usercmd, steps the session, draws the scene and the HUD the session built, and
//! plays the sounds it queued.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cs_assets::bsp::BspMap;
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
use winit::dpi::LogicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::audio::{AudioEngine, ListenerState};
use crate::session::Session;
use cs_client::camera::Camera;
use cs_client::hudtext::{format_message_with_titles, format_saytext_with_titles};
use cs_client::input::{
    CmdClock, CmdParams, ConsoleRoute, InputActionState, MoveSpeeds, build_usercmd, route_unknown,
    step_inventory_cycle,
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
use cs_client::ui::hud_extras::{OverviewMapTex, WeaponBucketState, draw_overview_map};
use cs_pm::collision::Hull;
use cs_client::ui::main_menu::{MainMenuRenderCtx, MainMenuUi};
use cs_client::ui::mod_hud::{draw_mod_approval_prompt, draw_mod_hud};
use cs_client::ui::options::{OptionsRenderCtx, OptionsUi};
use cs_client::ui::scoreboard::{ScoreRow, ScoreboardRenderCtx, show_scoreboard_vgui};
use cs_client::ui::screens::{
    BuyClassTeamMenuUi, BuyMenuRenderCtx, CommandMenuUi, DemoPlayerUi, DemoUiAction,
    DialogRenderCtx, DisconnectErrorDialog, LoadingDialogUi, MotdRenderCtx, MotdVguiUi,
    PlayerListDialogUi, ServerPasswordUi, SpectatorMode, SpectatorRenderCtx, SpectatorVguiUi,
    VguiMenuKind,
};
use cs_client::ui::server_menu::{
    draw_styled_text_menu_vgui, draw_text_menu_vgui, menu_key_enabled,
    menu_styled_lines_with_titles,
};
use cs_client::ui::vgui2::{VguiRuntime, install_scheme_fonts_into_egui};
use cs_client::ui::vgui_assets::VguiAssetBank;
use cs_client::ui::vgui_theme::{VguiPalette, apply_vgui2_theme};
use cs_client::ui::{
    CrosshairInputs, CrosshairState, FrameTelemetry, draw_crosshair_vgui, draw_net_graph,
    should_draw_dynamic_crosshair,
};
use cs_client::view::{FrameView, ViewInputs, ViewStats};
use cs_client::view_effects::{calc_view_idle, calc_viewmodel_angles};

const MAX_FEED: usize = 5;

pub struct LaunchConfig {
    pub gamedir: Option<PathBuf>,
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
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            gamedir: None,
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
        }
    }
}

struct WindowRuntime {
    window: Option<Arc<Window>>,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    bg_texture: Option<egui::TextureHandle>,
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
        if self.center.as_ref().is_some_and(|(_, t)| now - t > center_life) {
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
    weapon_switch_timer: f32,
    last_weapon: Option<u8>,
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
    config_path: PathBuf,
    recording_demo: Option<String>,
    pending_rcon: Option<PendingRcon>,
}

struct PendingRcon {
    socket: std::net::UdpSocket,
    target: std::net::SocketAddr,
    client: cs_clientcore::RconClient,
    deadline: std::time::Instant,
}

impl Cs16App {
    pub fn new(launch: LaunchConfig) -> Self {
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

        let (vfs, available_maps, palette, loc) =
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
                    (Some(vfs), maps, VguiPalette::from(colors), Some(loc))
                }
                Err(err) => {
                    cvars.error(format!("{err}"));
                    warn!("{err}");
                    (None, Vec::new(), VguiPalette::from(VguiColors::default()), None)
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
        let exit_after_screenshot = launch.auto_screenshot.is_some();
        Self {
            launch,
            vfs,
            available_maps,
            cvars,
            palette,
            loc,
            audio,
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
            weapon_switch_timer: 0.0,
            last_weapon: None,
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
            config_path,
            recording_demo: None,
            pending_rcon: None,
        }
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
        self.last_weapon = None;
        self.weapon_switch_timer = 0.0;
        self.audio.stop_all();
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
        self.cvars.info(format!("Starting local game on \"{map_name}\"..."));
        self.end_session();
        let Some(vfs) = self.vfs.clone() else {
            self.cvars.error("Half-Life / CS 1.6 directory not mounted.");
            return;
        };
        let opts = LocalOptions {
            bots: self.cvars.get_i32("bot_quota").clamp(0, 31) as u32,
            player_name: self.cvars.get_str("name").to_string(),
            hostname: self.cvars.get_str("hostname").to_string(),
            // `_vgui_menus 1` asks server for VGUIMenu dialogs; `_vgui_menus 0` uses ShowMenu.
            text_menus: !self.cvars.get_bool("_vgui_menus"),
            ..LocalOptions::default()
        };
        match Session::start_local(&vfs, map_name, &opts) {
            Ok(session) => {
                self.session = session;
                for name in [
                    "bot_difficulty",
                    "mp_roundtime",
                    "mp_freezetime",
                    "mp_startmoney",
                    "sv_gravity",
                    "sv_airaccelerate",
                ] {
                    let val = self.cvars.get_str(name);
                    if !val.is_empty() {
                        let _ = self.session.server_console(&format!("{name} {val}"));
                    }
                }
                if let Err(err) = self.load_map(map_name) {
                    error!(map = map_name, %err, "Failed to load BSP map");
                    self.cvars.error(format!("Failed to load map \"{map_name}\": {err}"));
                    self.end_session();
                    return;
                }
                self.after_session_start();
            }
            Err(err) => {
                error!(map = map_name, %err, "Failed to start the local game");
                self.cvars.error(format!("Failed to start \"{map_name}\": {err}"));
            }
        }
    }

    fn connect_remote(&mut self, addr: &str) {
        self.end_session();
        let name = self.cvars.get_str("name").to_string();
        let vgui_menus = self.cvars.get_bool("_vgui_menus");
        match Session::connect_remote(self.vfs.as_ref(), addr, &name, vgui_menus) {
            Ok(session) => {
                self.cvars.info(format!("Connecting to {addr}..."));
                self.session = session;
                self.menu_ui.find_servers_open = false;
                self.after_session_start();
            }
            Err(err) => self.cvars.error(format!("Failed to connect to {addr}: {err}")),
        }
    }

    fn after_session_start(&mut self) {
        self.feeds = HudFeeds::default();
        self.view_stats = ViewStats::default();
        self.last_weapon = None;
        self.weapon_switch_timer = 0.0;
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
        let vgui_flag = if self.cvars.get_bool("_vgui_menus") { "1" } else { "0" };
        self.session.set_client_cvar("_vgui_menus", vgui_flag);
        self.menu_ui.visible = false;
        self.console_ui.open = false;
        self.options_ui.open = false;
        self.chat_ui.open = false;
        self.sync_cursor_capture();
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
        self.pending_impulse = 0;
        self.cmd_clock = CmdClock::default();
        self.last_weapon = None;
        self.weapon_switch_timer = 0.0;
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
    /// impacts, radio/VOX sentences, reloads, and predicted footsteps.
    fn play_sounds(&mut self, view: &FrameView, local_entity: Option<u16>, sess_now: f64) {
        let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
        let suit_vol = finite_or(self.cvars.get_f32("suitvolume"), 0.25).clamp(0.0, 1.0);
        let listener_eye = view.eye;
        let coll = self.session.collision().cloned();
        let occ = |pos: Vec3| {
            coll.as_ref()
                .is_some_and(|c| c.trace_world(listener_eye, pos, Hull::Point).fraction < 0.99)
        };
        self.audio.update_listener(
            ListenerState {
                origin: view.eye,
                right: view.listener_right,
                local_entity,
                waterlevel: view.waterlevel,
            },
            &view.entity_positions,
            coll.as_ref().map(|_| &occ as &dyn Fn(Vec3) -> bool),
        );

        let occ_ref = coll.as_ref().map(|_| &occ as &dyn Fn(Vec3) -> bool);
        for cmd in self.session.take_sounds() {
            if let SoundCommand::RoomType(rt) = &cmd {
                self.cvars.set("room_type", rt.to_string());
            }
            self.audio.handle_sound_command(self.vfs.as_ref(), &cmd, occ_ref);
        }
        for impact in &view.impact_sounds {
            self.audio.handle_sound_command(
                self.vfs.as_ref(),
                &SoundCommand::Start(impact.clone()),
                occ_ref,
            );
        }
        for (sender, code, pitch) in &view.radio_audio {
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
        for (vol, kind) in &view.reload_sounds {
            self.audio.play_reload_sound(self.vfs.as_ref(), *vol, *kind);
        }
        for s in self.session.take_predicted_sounds() {
            if s.volume.is_finite() {
                let start = cs_clientcore::SoundStart::by_name(
                    &s.sample,
                    local_entity.unwrap_or(1),
                    s.channel.clamp(0, 7) as u8,
                    view.eye,
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
        let feeds = &mut self.feeds;
        let weapon = self
            .session
            .with_hud_mut(|hud| {
                for d in hud.deaths.drain(..) {
                    feeds.deaths.push((d, now));
                }
                for line in hud.chat.drain(..) {
                    let text = format_saytext_with_titles(&line.text, &line.params, loc, titles);
                    if !text.is_empty() {
                        let team = hud.players.get(&line.sender).map(|p| p.team.clone());
                        feeds.chat.push((HudChatEntry { text, team }, now));
                    }
                }
                for wid in hud.weapon_pickups.drain(..) {
                    let empty = matches!(hud.clips.get(&wid), Some(&0))
                        && hud.weapons.get(&wid).is_some_and(|w| {
                            w.ammo1_type != 255 && hud.ammo.get(&w.ammo1_type).copied().unwrap_or(0) == 0
                        });
                    feeds.pickups.push(PickupHistoryEntry {
                        kind: PickupKind::Weapon { weapon_id: wid, empty },
                        end_time: now + 5.0,
                    });
                }
                for (ammo_id, amount) in hud.ammo_pickups.drain(..) {
                    feeds.pickups.push(PickupHistoryEntry {
                        kind: PickupKind::Ammo { ammo_id, amount },
                        end_time: now + 5.0,
                    });
                }
                for classname in hud.item_pickups.drain(..) {
                    feeds.pickups.push(PickupHistoryEntry {
                        kind: PickupKind::Item { classname },
                        end_time: now + 5.0,
                    });
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
                hud.current_weapon.map(|w| w.weapon_id)
            })
            .flatten();
        trim_feed(&mut self.feeds.deaths);
        trim_feed(&mut self.feeds.chat);
        trim_feed(&mut self.feeds.notify);
        let feed_life = f64::from(finite_or(self.cvars.get_f32("hud_deathnotice_time"), 6.0));
        let chat_life = f64::from(finite_or(self.cvars.get_f32("hud_saytext_time"), 5.0));
        let center_life = f64::from(finite_or(self.cvars.get_f32("scr_centertime"), 2.0).max(0.5));
        self.feeds.expire(now, feed_life, chat_life, center_life);

        if weapon != self.last_weapon {
            if self.last_weapon.is_some() {
                self.weapon_switch_timer = 1.8;
            }
            self.last_weapon = weapon;
        }
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
                        if self.console_ui.open {
                            self.console_ui.open = false;
                        } else if self.motd_ui.open {
                            self.motd_ui.open = false;
                            self.feeds.motd_open = false;
                        } else if self.buy_team_class_ui.active_res.is_some() {
                            self.buy_team_class_ui.close();
                            self.session.with_hud_mut(|h| h.vgui_menu = None);
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
                            self.options_ui.open = false;
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
                        self.cvars.warn("setpos needs the server's cheats; use noclip");
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
                        let suit = finite_or(self.cvars.get_f32("suitvolume"), 0.25).clamp(0.0, 1.0);
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
                            self.session.send_stringcmd(&format!("specmode {}", next as i32));
                        }
                    }
                    EngineAction::SpecMenu(visible) => {
                        self.spectator_ui.menu_visible = visible != 0;
                    }
                    EngineAction::RecordDemo(name) => {
                        if name.trim().is_empty() {
                            self.cvars.warn("usage: record <demoname>");
                        } else if !self.session.is_active() || self.session.is_demo() {
                            self.cvars.warn("must be connected to a server to record a demo");
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
                    v.read_bytes(&fname)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))
                })
        });
        match bytes {
            Ok(data) => match Session::start_demo(self.vfs.as_ref(), &data) {
                Ok(mut sess) => {
                    if timedemo {
                        if let Some(report) = sess.demo_player_mut().map(|p| p.run_timedemo_all(1.0 / 60.0)) {
                            self.cvars.info(format!(
                                "timedemo {}: {} frames, {:.3}s, {:.1} fps",
                                fname, report.frames, report.elapsed_secs, report.avg_fps
                            ));
                        }
                    } else if let Some(p) = sess.demo_player() {
                        self.demo_player_ui.load_demo(&fname);
                        self.demo_player_ui.duration_secs = p.total_duration();
                        self.demo_player_ui.total_ticks =
                            (p.total_duration() * 100.0).ceil().max(1.0) as u32;
                    }
                    self.session.close();
                    self.session = sess;
                    self.menu_ui.visible = false;
                    self.sync_cursor_capture();
                }
                Err(e) => self.cvars.warn(format!("Failed to start demo {fname}: {e}")),
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
            None => self
                .cvars
                .warn(format!("\"{line}\" needs a local game (the server is elsewhere)")),
        }
    }

    fn owned_weapon_ids(hud: &HudState, owned_bits: u32) -> HashSet<u8> {
        hud.weapons
            .values()
            .map(|w| w.weapon_id)
            .filter(|id| *id < 32 && owned_bits & (1 << id) != 0)
            .collect()
    }

    /// `slot1`..`slot5`: selects or opens the weapon bucket selector (`hud_fastswitch`).
    fn select_slot(&mut self, slot: u8) {
        let owned_bits = self.session.owned_weapons();
        let fastswitch = self.cvars.get_bool("hud_fastswitch");
        let buckets = &mut self.weapon_buckets;
        let pick = self
            .session
            .with_hud(|hud| {
                let owned = Self::owned_weapon_ids(hud, owned_bits);
                buckets.on_slot_press(hud, &owned, slot, fastswitch)
            })
            .flatten();
        if let Some(name) = pick {
            self.session.send_stringcmd(&name);
        }
    }

    /// `invnext` / `invprev`: cycles owned weapons across buckets/positions (`hud_fastswitch`).
    fn cycle_inventory(&mut self, forward: bool) {
        let owned_bits = self.session.owned_weapons();
        let fastswitch = self.cvars.get_bool("hud_fastswitch");
        let buckets = &mut self.weapon_buckets;
        let pick = self
            .session
            .with_hud(|hud| {
                let owned = Self::owned_weapon_ids(hud, owned_bits);
                step_inventory_cycle(buckets, hud, &owned, forward, fastswitch)
            })
            .flatten();
        if let Some(name) = pick {
            self.session.send_stringcmd(&name);
        }
    }

    /// Queues this frame's usercmd. A cmd is `msec` of the frame, view angles from the mouse.
    fn queue_usercmd(&mut self, dt: f64) {
        if !self.session.in_game() {
            self.input_state.consume_impulses();
            return;
        }
        let capture = self.wants_mouse_capture();
        let held = self.input_state.held(capture);
        let angles = Vec3::new(self.camera.pitch_deg, self.camera.yaw_deg, 0.0);
        let params = CmdParams {
            msec: self.cmd_clock.msec(dt),
            lerp_msec: self.cmd_clock.lerp_msec(self.session.interp(), dt as f32),
            impulse: std::mem::take(&mut self.pending_impulse),
        };
        let speeds = MoveSpeeds {
            forward: self.cvars.get_f32("cl_forwardspeed"),
            back: self.cvars.get_f32("cl_backspeed"),
            side: self.cvars.get_f32("cl_sidespeed"),
            up: self.cvars.get_f32("cl_upspeed"),
            walk_key: self.cvars.get_f32("cl_movespeedkey"),
        };
        self.session
            .queue_cmd(build_usercmd(&held, angles, params, &speeds));
        self.input_state.consume_impulses();
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
            && !self.cvars.get_bool("hud_fastswitch")
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

        // Allow releasing TAB even while the scoreboard is visible
        if code == KeyCode::Tab
            && !self.console_ui.open
            && !self.chat_ui.open
            && !self.menu_ui.visible
            && self.is_in_game()
        {
            self.input_state.apply_action_input("showscores", pressed);
            return true;
        }

        if !self.wants_mouse_capture() {
            return false;
        }

        // A menu of the server takes the digit keys its mask enables (once complete).
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
                let n = if digit == 0 { 10 } else { digit };
                let cmd = vgui_menu_select_command(menu_id, n);
                self.session.send_stringcmd(&cmd);
                self.session.with_hud_mut(|h| h.vgui_menu = None);
                return true;
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

    fn render_and_step(&mut self, event_loop: Option<&ActiveEventLoop>) -> bool {
        let now = Instant::now();
        let raw_dt = (now - self.last_frame).as_secs_f32();
        let dt = raw_dt.clamp(0.0001, 0.1);
        self.last_frame = now;

        let inst_fps = 1.0 / raw_dt.max(0.0001);
        self.fps_smooth = self.fps_smooth * 0.9 + inst_fps * 0.1;
        self.frame_ms_smooth = self.frame_ms_smooth * 0.9 + (raw_dt * 1000.0) * 0.1;
        self.poll_rcon();

        if self.session.is_active() {
            let desired_name = self.cvars.get_str("name").to_string();
            if self.session.client_cvar("name").as_deref() != Some(desired_name.as_str()) {
                self.session.set_client_cvar("name", &desired_name);
            }
        }

        // 1. Apply any pending server-forced view angles, send usercmd, then step the session.
        self.cvars.set_forward_unknown(self.session.is_active());
        self.apply_view_angle_updates();
        self.queue_usercmd(f64::from(dt));
        let timescale = finite_or(self.cvars.get_f32("host_timescale"), 1.0).clamp(0.05, 10.0);
        self.session.advance(f64::from(dt * timescale));
        self.apply_view_angle_updates();

        for um in self.session.drain_unknown_usermsgs() {
            if !self
                .mod_transfer
                .ingest_usermsg(&um.name, &um.payload, &mut self.mod_host, self.vfs.as_mut())
            {
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
            self.cvars.error(format!("Failed to load map \"{map}\": {err}"));
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
        let in_game = self.is_in_game();
        let show_scores = (self.input_state.active_actions.contains("showscores")
            || self.scoreboard_toggled)
            && in_game
            && !self.menu_ui.visible;
        let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
        let suit_vol = finite_or(self.cvars.get_f32("suitvolume"), 0.25).clamp(0.0, 1.0);
        let room_off = self.cvars.get_bool("room_off");
        let occ_factor = finite_or(self.cvars.get_f32("s_occfactor"), 0.25).clamp(0.0, 0.9);
        let mp3_vol = finite_or(self.cvars.get_f32("mp3volume"), 0.8).clamp(0.0, 1.0);
        let bgm_enabled = self.cvars.get_bool("bgmvolume");
        let mp3_fade = finite_or(self.cvars.get_f32("mp3fadetime"), 2.0).clamp(0.0, 30.0);
        self.audio.set_params(
            master,
            suit_vol,
            room_off,
            occ_factor,
            mp3_vol,
            bgm_enabled,
            mp3_fade,
        );
        let want_menu_music = !self.session.is_active() && self.launch.auto_screenshot.is_none();
        self.audio.sync_menu_music(
            self.vfs.as_ref(),
            want_menu_music,
            mp3_vol,
            bgm_enabled,
            mp3_fade,
            dt,
        );

        let default_fov = finite_or(self.cvars.get_f32("default_fov"), 90.0)
            .clamp(60.0, 120.0);

        // 2. The frame of the session.
        let mut view: Option<FrameView> = None;
        if in_game {
            self.in_game_frames += 1;
            if let Some(ui_mode) = self.launch.startup_ui.clone() {
                let m = ui_mode.to_ascii_lowercase();
                if matches!(m.as_str(), "hud" | "buy" | "scoreboard" | "commandmenu" | "chat") {
                    if self.in_game_frames == 2 {
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
                            "scoreboard" => self.scoreboard_toggled = true,
                            "commandmenu" => self.command_menu_ui.set_open(true),
                            "chat" => self.chat_ui.open_chat(false),
                            _ => {}
                        }
                        self.startup_ui_applied = true;
                        self.sync_cursor_capture();
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
                        self.buy_team_class_ui.open_kind(VguiMenuKind::ClassCt, true);
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
            };
            let angles = Vec3::new(
                finite_or(self.camera.pitch_deg, 0.0),
                finite_or(self.camera.yaw_deg, 0.0),
                0.0,
            );
            if let Some((frame, registry)) = self.session.build_view(angles, &inputs) {
                if let (Some(reg), Some(rt)) = (registry, &mut self.runtime) {
                    rt.renderer.set_model_registry(reg);
                }
                view = Some(frame);
            }
            self.collect_hud_feeds(inputs.now);
            let hud_now = self.session.now();
            let eye = view.as_ref().map_or(self.camera.position, |v| v.eye);
            let yaw_deg = self.camera.yaw_deg;
            self.session.with_hud(|h| self.hud_anim.step_with_pose(dt, h, hud_now, eye, yaw_deg));
        }
        self.weapon_switch_timer = (self.weapon_switch_timer - dt).max(0.0);
        self.weapon_buckets.step(dt);

        if let Some(v) = &view {
            self.view_stats = v.stats;
            if v.eye.is_finite() {
                self.camera.position = v.eye;
            }
            let local_entity = self.session.local_slot().map(|s| u16::from(s) + 1);
            let sess_now_snd = self.session.now();
            self.play_sounds(v, local_entity, sess_now_snd);
            let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
            for path in &v.anim_sounds {
                self.audio.play_2d(self.vfs.as_ref(), path, 0.8 * master);
            }
        }
        let voice_enable = self.cvars.get_bool("voice_enable") && self.cvars.get_bool("voice_modenable");
        let voice_scale = finite_or(self.cvars.get_f32("voice_scale"), 1.0).clamp(0.0, 4.0);
        let voice_loopback = self.cvars.get_bool("voice_loopback");
        let voice_avg = finite_or(self.cvars.get_f32("voice_avggain"), 0.5);
        let voice_max = finite_or(self.cvars.get_f32("voice_maxgain"), 5.0);
        let recording = in_game
            && !self.menu_ui.visible
            && self.input_state.active_actions.contains("voicerecord");
        self.audio
            .set_voice_recording(recording, self.cvars.get_bool("voice_forcemicrecord"));
        let sess_now_voice = self.session.now();
        for ve in self.session.take_voice_events() {
            self.audio.push_voice_event(ve, sess_now_voice);
        }
        for outgoing_voice in self.audio.step_voice(
            sess_now_voice,
            voice_enable,
            voice_scale,
            voice_loopback,
            voice_avg,
            voice_max,
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
        let chat_entries: Vec<HudChatEntry> = self.feeds.chat.iter().map(|(c, _)| c.clone()).collect();
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
                    .map(|m| menu_styled_lines_with_titles(&m.text, self.loc.as_ref(), Some(&self.titles)))
            })
            .flatten();
        let use_vgui_menus = self.cvars.get_bool("_vgui_menus");
        let active_vgui_menu = self
            .session
            .with_hud(|h| h.vgui_menu.as_ref().filter(|v| !v.multipart && v.keys != 0).cloned())
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
                    .and_then(|s| self.session.with_hud(|h| h.players.get(&(s + 1)).is_some_and(|p| p.team == "CT")))
                    .unwrap_or(false);
                if kind == VguiMenuKind::Buy && self.buy_team_class_ui.active_res.is_none() {
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
                self.session.with_hud(|h| {
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
        let spec_players: Vec<(u8, String)> = names.iter().map(|(&k, v)| (k + 1, v.clone())).collect();
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
        let scoped = view
            .as_ref()
            .is_some_and(|v| v.fov.is_finite() && v.fov > 0.0 && v.fov <= 40.0);
        let alive = view.as_ref().is_some_and(|v| v.alive);
        let observer_mode = view.as_ref().map_or(0, |v| v.observer_mode);

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
        let mut cmdmenu_exec: Option<String> = None;
        let mut cancel_loading = false;

        let Some(rt) = &mut self.runtime else {
            return false;
        };
        let raw_input = if let (Some(state), Some(win)) = (&mut rt.egui_state, &rt.window) {
            state.take_egui_input(win)
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

        let (hud_health, hud_hidden, hud_timer_vis, hud_nvg) = self
            .session
            .with_hud(|h| (i32::from(h.health), h.hidden, h.timer_visible, h.night_vision))
            .unwrap_or((0, 0, false, false));
        let hud_vis = compute_hud_visibility(alive, hud_health, hud_hidden, hud_timer_vis);
        if let Some(v) = &view {
            let (wid, wflags, on_ground, ducking, ch_spd, clip) = self.session.local_crosshair_state();
            if !paused && hud_vis.crosshair && !scoped && should_draw_dynamic_crosshair(v.fov, wid) {
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
                draw_crosshair_vgui(&rt.egui_ctx, &self.cvars, &mut self.crosshair_state, &ch_inputs);
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
                    weapon_switch_timer: self.weapon_switch_timer,
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
            draw_voice_speakers(
                &rt.egui_ctx,
                &names,
                &voice_speakers,
                voice_local_talking,
            );
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
            let forced_spectator = self
                .launch
                .startup_ui
                .as_deref()
                .is_some_and(|s| {
                    s.eq_ignore_ascii_case("spectator") || s.eq_ignore_ascii_case("spectator:menu")
                })
                && self.startup_ui_applied;
            if in_game && !modal_menu_open && ((!alive && observer_mode != 0) || forced_spectator) {
                let spec_ctx = SpectatorRenderCtx {
                    vfs: self.vfs.as_ref(),
                    loc: self.loc.as_ref(),
                    ct_score,
                    t_score,
                    round_timer_secs: round_secs_left,
                    players: &spec_players,
                    spec_health: spec_hp,
                };
                let spec_actions = self.spectator_ui.show(
                    &rt.egui_ctx,
                    &mut self.vgui_rt,
                    &mut self.cvars,
                    &spec_ctx,
                );
                pending_actions.extend(spec_actions);
            }
            draw_net_graph(
                &rt.egui_ctx,
                &self.cvars,
                &FrameTelemetry {
                    fps: self.fps_smooth,
                    frame_ms: self.frame_ms_smooth,
                    map_name: &map_name,
                    pos: self.camera.position.to_array(),
                    yaw_deg: self.camera.yaw_deg,
                    pitch_deg: self.camera.pitch_deg,
                    speed: local_speed,
                    draw_calls: self.last_stats.draw_calls,
                    triangles: self.last_stats.triangles,
                },
                self.palette,
            );
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
                if self.motd_ui.show(&rt.egui_ctx, &mut self.vgui_rt, &motd_ctx) {
                    motd_close = true;
                }
            }
            if !self.motd_ui.open && !motd_close && self.buy_team_class_ui.active_res.is_some() {
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
                };
                vgui_cmd_out = self
                    .buy_team_class_ui
                    .show(&rt.egui_ctx, &mut self.vgui_rt, &buy_ctx);
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
        let demo_actions = self.demo_player_ui.show(&rt.egui_ctx, &mut self.vgui_rt, &dlg_ctx);
        let pw_submit = self
            .password_ui
            .show(&rt.egui_ctx, &mut self.vgui_rt, &dlg_ctx);
        self.disconnect_err_ui
            .show(&rt.egui_ctx, &mut self.vgui_rt, &dlg_ctx);
        let mod_decisions = draw_mod_approval_prompt(&rt.egui_ctx, &pending_mod_approvals);
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
            self.audio.play_ui_sound(self.vfs.as_ref(), ui_snd.vfs_path());
        }

        // 4. The 3D frame.
        let mut render_cam = self.camera.clone();
        let mut fov = default_fov;
        let mut viewmodel_inst: Option<(String, glam::Mat4, i32, i32, f32, bool)> = None;
        let mut beams = Vec::new();
        let mut points = Vec::new();
        let mut decals = Vec::new();
        let mut custom_decals = std::collections::HashMap::new();
        let mut lightstyles = [1.0f32; 64];
        let mut lights = GpuLightsUniform::default();
        let mut elights = Vec::new();
        let mut fog = None;
        let mut scene = None;
        let mut sensitivity_scale = 1.0;
        if let Some(v) = view {
            let punch_x = finite_or(v.punch.x, 0.0);
            let punch_y = finite_or(v.punch.y, 0.0);
            let punch_z = finite_or(v.punch.z, 0.0);
            let v_idlescale = finite_or(self.cvars.get_f32("v_idlescale"), 0.0);
            let idle = calc_view_idle(time_sec, v_idlescale);
            render_cam.position.z += finite_or(v.bob, 0.0) + finite_or(v.shake_z, 0.0);
            render_cam.pitch_deg =
                (finite_or(render_cam.pitch_deg, 0.0) + punch_x + finite_or(v.shake_angles.x, 0.0) + idle.pitch)
                    .clamp(-89.0, 89.0);
            render_cam.yaw_deg =
                (finite_or(render_cam.yaw_deg, 0.0) + punch_y + finite_or(v.shake_angles.y, 0.0) + idle.yaw)
                    .rem_euclid(360.0);
            render_cam.roll_deg =
                (punch_z + finite_or(v.roll_deg, 0.0) + finite_or(v.shake_angles.z, 0.0) + idle.roll)
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
                    vm.path.clone(),
                    xf,
                    vm.sequence,
                    vm.body,
                    vm.animtime,
                    vm.right_hand,
                ));
            }
            beams = v.beams;
            points = v.points;
            decals = v.decals;
            custom_decals = v.custom_decals;
            lightstyles = v.lightstyles;
            lights = v.lights;
            elights = v.elights;
            fog = v.fog;
            scene = Some(v.scene);
        }
        rt.renderer.set_entity_scene(scene);
        self.mouse_sens_scale = sensitivity_scale;

        let debug_mode = if self.cvars.get_bool("r_lightmap") || self.cvars.get_bool("r_lightmap_only")
        {
            1.0
        } else if self.cvars.get_bool("r_fullbright") {
            2.0
        } else {
            0.0
        };
        let texture_mode = TextureFilterMode::parse(self.cvars.get_str("gl_texturemode"));
        let anisotropy = self.cvars.get_i32("gl_ansio").clamp(1, 16) as u16;
        let msaa_samples = if self.cvars.get_i32("gl_msaa") >= 4 { 4 } else { 1 };
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
        let vm_ref = viewmodel_inst.as_ref().map(|(path, xf, seq, body, started, right_hand)| {
            StudioInstance {
                model_path: path.as_str(),
                transform: *xf,
                sequence: *seq,
                frame: 0.0,
                animtime: *started,
                framerate: 1.0,
                now: sess_now as f32,
                body: *body,
                light_rgb: Vec3::splat(0.95),
                flip_winding: *right_hand,
                attached_model: None,
                ..StudioInstance::default()
            }
        });
        let scene_data = SceneDynamicData {
            vfs: self.vfs.as_ref(),
            viewmodel: vm_ref,
            beams: &beams,
            points: &points,
            decals: &decals,
            custom_decals: Some(&custom_decals),
            lightstyles: &lightstyles,
            lights,
            elights: &elights,
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
        let auto_capture_now = self.launch.auto_screenshot.is_some()
            && if waiting_for_game {
                (self.in_game_frames >= min_in_game_frames
                    && (self.startup_ui_applied || self.launch.startup_ui.is_none()))
                    || self.frame_count >= 600
            } else {
                self.frame_count == 6
            };
        let capture_path = if auto_capture_now {
            self.launch.auto_screenshot.take()
        } else {
            self.pending_screenshot.take()
        };

        if let Some(stats) = rt.renderer.render_frame(
            &render_cam,
            &settings,
            &scene_data,
            full_output.textures_delta,
            &paint_jobs,
            full_output.pixels_per_point,
            capture_path.as_deref(),
        ) {
            self.last_stats = stats;
            if auto_capture_now && self.exit_after_screenshot {
                if let Some(el) = event_loop {
                    el.exit();
                }
                return true;
            }
        }

        // 5. Post-frame actions.
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
        if self.options_ui.save_requested {
            self.options_ui.save_requested = false;
            self.save_config();
        }
        if !pending_actions.is_empty()
            && self.process_actions(pending_actions, event_loop)
        {
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
                let timescale = finite_or(self.cvars.get_f32("host_timescale"), 1.0).clamp(0.05, 10.0);
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

    fn init_egui_context(&self) -> (egui::Context, Option<egui::TextureHandle>, Option<HudSprites>, VguiAssetBank) {
        let egui_ctx = egui::Context::default();
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
        let hud_sprites = self
            .vfs
            .as_ref()
            .map(|vfs| HudSprites::load(&egui_ctx, vfs));
        let vgui_bank = VguiAssetBank::load(&egui_ctx, self.vfs.as_ref());
        (egui_ctx, bg_texture, hud_sprites, vgui_bank)
    }

    fn apply_startup_commands(&mut self, event_loop: Option<&ActiveEventLoop>) -> bool {
        if let Some(map) = self.launch.startup_map.clone() {
            self.start_local_map(&map);
        }
        let startup_cmds = std::mem::take(&mut self.launch.startup_exec);
        for cmd in startup_cmds {
            let actions = self.cvars.execute_script(&cmd);
            if self.process_actions(actions, event_loop) {
                return true;
            }
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

    /// Run headless offscreen without creating any visible OS window (for `cargo xtask visual-diff`).
    pub fn run_headless(mut self) -> Result<(), String> {
        let renderer = Renderer::new_headless(self.launch.width, self.launch.height)?;
        let (egui_ctx, bg_texture, hud_sprites, vgui_bank) = self.init_egui_context();
        info!(gpu = %renderer.adapter_name, "Counter-Strike 1.6 (Rust) headless runtime ready");
        self.runtime = Some(WindowRuntime {
            window: None,
            renderer,
            egui_ctx,
            egui_state: None,
            bg_texture,
            hud_sprites,
            vgui_bank,
            cursor_captured: false,
        });
        if self.apply_startup_commands(None) {
            self.end_session();
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.render_and_step(None) {
                self.end_session();
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        let pending = self.launch.auto_screenshot.take();
        self.end_session();
        if let Some(path) = pending {
            return Err(format!(
                "headless run timed out after 30s before saving {} (in_game_frames={}, frames={})",
                path.display(),
                self.in_game_frames,
                self.frame_count
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

        let attrs = Window::default_attributes()
            .with_title("Counter-Strike 1.6 (Rust)")
            .with_inner_size(LogicalSize::new(
                f64::from(self.launch.width),
                f64::from(self.launch.height),
            ));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));

        let renderer = Renderer::new(Arc::clone(&window));
        let (egui_ctx, bg_texture, hud_sprites, vgui_bank) = self.init_egui_context();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
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
                    let key_name = if notches >= 0 { "mwheelup" } else { "mwheeldown" };
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
            let raw_dx = dx as f32;
            let raw_dy = dy as f32;
            let (fx, fy) = if self.cvars.get_bool("m_filter") {
                let (px, py) = self.prev_mouse_delta;
                ((raw_dx + px) * 0.5, (raw_dy + py) * 0.5)
            } else {
                (raw_dx, raw_dy)
            };
            self.prev_mouse_delta = (raw_dx, raw_dy);

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
            self.camera.apply_mouse_delta_clamped(
                adj_x,
                adj_y,
                sens,
                m_yaw,
                m_pitch,
                pitch_up,
                pitch_down,
            );
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

fn physical_key_to_goldsrc(code: KeyCode) -> Option<&'static str> {
    Some(match code {
        KeyCode::KeyW => "w",
        KeyCode::KeyA => "a",
        KeyCode::KeyS => "s",
        KeyCode::KeyD => "d",
        KeyCode::KeyE => "e",
        KeyCode::KeyR => "r",
        KeyCode::KeyG => "g",
        KeyCode::KeyB => "b",
        KeyCode::KeyQ => "q",
        KeyCode::KeyM => "m",
        KeyCode::KeyY => "y",
        KeyCode::KeyU => "u",
        KeyCode::KeyN => "n",
        KeyCode::KeyZ => "z",
        KeyCode::KeyX => "x",
        KeyCode::KeyC => "c",
        KeyCode::KeyT => "t",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Space => "space",
        KeyCode::ControlLeft | KeyCode::ControlRight => "ctrl",
        KeyCode::ShiftLeft | KeyCode::ShiftRight => "shift",
        KeyCode::AltLeft | KeyCode::AltRight => "alt",
        KeyCode::Tab => "tab",
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
        KeyCode::F5 => "f5",
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
fn effective_fps_limit(fps_max: f32, fps_override: bool, active: bool, max_clients: u8) -> Option<f32> {
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

/// Converts 1-based `HudState.players` entity keys to 0-based `ScoreRow.slot` and `names` keys.
fn build_score_rows(hud: &HudState, names: &BTreeMap<u8, String>) -> (Vec<ScoreRow>, i16, i16) {
    let rows = hud
        .players
        .iter()
        .filter(|(_, p)| matches!(p.team.as_str(), "CT" | "TERRORIST" | "SPECTATOR" | "UNASSIGNED"))
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
        "mp3" if matches!(cmd.args.first().map(String::as_str), Some("play" | "loop" | "stop")) => {
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
        for (slot1, team) in [(1u8, "CT"), (2, "TERRORIST"), (3, "SPECTATOR"), (4, "UNASSIGNED")] {
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
        assert_eq!(classify_shell_command(&slot10), Some(ShellStuffAction::CloseMenu));
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
        let (dx, dy, sens) = compute_mouse_move_params(
            10.0, 0.0, 2.0, 0.5, 1, 1.0, 5.0, 5.0, 0.022, 0.022,
        );
        assert_eq!((dx, dy), (10.0, 0.0));
        assert!((sens - 2.5).abs() < 1e-5);

        // m_customaccel == 2 applies extra m_yaw / m_pitch scaling.
        let (dx2, dy2, sens2) = compute_mouse_move_params(
            100.0, -50.0, 2.0, 1.0, 2, 0.5, 0.0, 1.0, 0.022, 0.03,
        );
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
            DeathNotice { killer: 1, victim: 2, headshot: false, weapon: "ak47".into() },
            1.0,
        ));
        feeds.chat.push((
            HudChatEntry { text: "hi".into(), team: None },
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
        assert_eq!(mouse_button_to_goldsrc(MouseButton::Forward), Some("mouse5"));
        assert_eq!(mouse_button_to_goldsrc(MouseButton::Other(9)), None);
    }
}
