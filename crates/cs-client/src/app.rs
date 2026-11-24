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
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::audio::AudioEngine;
use crate::session::Session;
use cs_client::camera::Camera;
use cs_client::hudtext::{format_message, strip_html};
use cs_client::input::{
    CmdClock, CmdParams, ConsoleRoute, Held, MoveSpeeds, build_usercmd, route_unknown, weapon_for_slot,
};
use cs_client::render::bsp_renderer::BspRenderStats;
use cs_client::render::lighting::{GpuLightsUniform, TextureFilterMode};
use cs_client::render::mdl_renderer::{StudioInstance, viewmodel_transform};
use cs_client::render::scene::ModelRegistry;
use cs_client::render::{RenderSettings, Renderer, SceneDynamicData};
use cs_client::ui::chat::ChatInputUi;
use cs_client::ui::console::ConsoleUi;
use cs_client::ui::hud::{HudSprites, HudView, draw_cs16_hud};
use cs_client::ui::main_menu::MainMenuUi;
use cs_client::ui::mod_hud::{draw_mod_approval_prompt, draw_mod_hud};
use cs_client::ui::options::OptionsUi;
use cs_client::ui::scoreboard::{ScoreRow, show_scoreboard};
use cs_client::ui::server_menu::{draw_text_menu, menu_key_enabled, menu_lines};
use cs_client::ui::vgui_assets::VguiAssetBank;
use cs_client::ui::vgui_theme::{VguiPalette, apply_vgui2_theme};
use cs_client::ui::{FrameTelemetry, draw_crosshair, draw_net_graph};
use cs_client::view::{FrameView, ViewInputs, ViewStats};

/// Seconds a kill, a chat line, a notification and the centre text stay on screen.
const FEED_LIFE: f64 = 6.0;
const CHAT_LIFE: f64 = 8.0;
const CENTER_LIFE: f64 = 3.0;
const MAX_FEED: usize = 5;

pub struct LaunchConfig {
    pub gamedir: Option<PathBuf>,
    pub startup_map: Option<String>,
    pub open_console: bool,
    pub startup_exec: Vec<String>,
    pub auto_screenshot: Option<PathBuf>,
    pub startup_ui: Option<String>,
}

struct WindowRuntime {
    window: Arc<Window>,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    bg_texture: Option<egui::TextureHandle>,
    hud_sprites: Option<HudSprites>,
    vgui_bank: VguiAssetBank,
    cursor_captured: bool,
}

/// The HUD logs the shell times itself (the state keeps no clock for them).
#[derive(Default)]
struct HudFeeds {
    deaths: Vec<(DeathNotice, f64)>,
    chat: Vec<(String, f64)>,
    notify: Vec<(String, f64)>,
    center: Option<(String, f64)>,
    menu_since: Option<(f64, String)>,
    motd_shown: bool,
    motd_open: bool,
}

impl HudFeeds {
    fn expire(&mut self, now: f64) {
        self.deaths.retain(|(_, t)| now - t < FEED_LIFE);
        self.chat.retain(|(_, t)| now - t < CHAT_LIFE);
        self.notify.retain(|(_, t)| now - t < FEED_LIFE);
        if self.center.as_ref().is_some_and(|(_, t)| now - t > CENTER_LIFE) {
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
    active_actions: HashSet<String>,
    mouse1_held: bool,
    mouse2_held: bool,
    pending_impulse: u8,
    cmd_clock: CmdClock,
    mouse_sens_scale: f32,
    view_stats: ViewStats,
    last_stats: BspRenderStats,
    feeds: HudFeeds,
    weapon_switch_timer: f32,
    last_weapon: Option<u8>,
    mod_transfer: ClientModTransfer,
    mod_host: ClientModHost,

    menu_ui: MainMenuUi,
    console_ui: ConsoleUi,
    options_ui: OptionsUi,
    chat_ui: ChatInputUi,

    start_time: Instant,
    last_frame: Instant,
    fps_smooth: f32,
    frame_ms_smooth: f32,
    frame_count: u64,
    pending_screenshot: Option<PathBuf>,
    exit_after_screenshot: bool,
    config_path: PathBuf,
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
                    let loc = LocalizationTable::load(&vfs);
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

        let now = Instant::now();
        let exit_after_screenshot = launch.auto_screenshot.is_some();
        Self {
            launch,
            vfs,
            available_maps,
            cvars,
            palette,
            loc,
            audio: AudioEngine::new(),
            runtime: None,
            session: Session::none(),
            camera: Camera::default(),
            active_actions: HashSet::new(),
            mouse1_held: false,
            mouse2_held: false,
            pending_impulse: 0,
            cmd_clock: CmdClock::default(),
            mouse_sens_scale: 1.0,
            view_stats: ViewStats::default(),
            last_stats: BspRenderStats {
                draw_calls: 0,
                triangles: 0,
            },
            feeds: HudFeeds::default(),
            weapon_switch_timer: 0.0,
            last_weapon: None,
            mod_transfer: ClientModTransfer::default(),
            mod_host: ClientModHost::new(),
            menu_ui: MainMenuUi::default(),
            console_ui,
            options_ui: OptionsUi::default(),
            chat_ui: ChatInputUi::default(),
            start_time: now,
            last_frame: now,
            fps_smooth: 100.0,
            frame_ms_smooth: 10.0,
            frame_count: 0,
            pending_screenshot: None,
            exit_after_screenshot,
            config_path,
        }
    }

    fn is_in_game(&self) -> bool {
        self.session.in_game()
    }

    fn any_modal_open(&self) -> bool {
        self.menu_ui.visible
            || self.console_ui.open
            || self.options_ui.open
            || self.chat_ui.open
            || self.feeds.motd_open
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
            if want {
                let grabbed = rt
                    .window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| rt.window.set_cursor_grab(CursorGrabMode::Confined));
                if grabbed.is_ok() {
                    rt.window.set_cursor_visible(false);
                    rt.cursor_captured = true;
                }
            } else {
                let _ = rt.window.set_cursor_grab(CursorGrabMode::None);
                rt.window.set_cursor_visible(true);
                rt.cursor_captured = false;
                self.active_actions.clear();
                self.mouse1_held = false;
                self.mouse2_held = false;
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
            rt.window.set_title(&format!(
                "Counter-Strike 1.6 (Rust) - {} [{}]",
                map.name, rt.renderer.adapter_name
            ));
        }
        // The prediction walks the map's hulls and the effects trace against it; the sound
        // surfaces come with the materials file.
        if let Ok(txt) = vfs.read_bytes("sound/materials.txt") {
            self.session.connection_predictor_materials(&txt);
        }
        self.session.set_map(&map.name, Some(collision));
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
            // The shell draws the server's text menus and answers them with `menuselect`.
            text_menus: true,
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
        match Session::connect_remote(self.vfs.as_ref(), addr, &name) {
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
        self.mod_transfer = ClientModTransfer::default();
        self.mod_host = ClientModHost::new();
        self.menu_ui.visible = false;
        self.console_ui.open = false;
        self.options_ui.open = false;
        self.chat_ui.open = false;
        self.sync_cursor_capture();
    }

    /// Drops the session and the map; the menu comes back.
    fn end_session(&mut self) {
        if !self.session.is_active() {
            return;
        }
        self.session.close();
        if let Some(rt) = &mut self.runtime {
            rt.renderer.unload_map();
            rt.renderer.set_entity_scene(None);
            rt.renderer.set_model_registry(ModelRegistry::default());
            rt.renderer.clear_model_caches();
            rt.window.set_title("Counter-Strike 1.6 (Rust)");
        }
        self.audio.stop_all();
        self.feeds = HudFeeds::default();
        self.view_stats = ViewStats::default();
        self.menu_ui.visible = true;
        self.chat_ui.open = false;
        self.active_actions.clear();
        self.mouse1_held = false;
        self.mouse2_held = false;
        self.pending_impulse = 0;
        self.cmd_clock = CmdClock::default();
        self.last_weapon = None;
        self.weapon_switch_timer = 0.0;
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

    /// Plays what the client core queued: the sounds of the server's messages, of the events
    /// and temp entities, and of the predicted footsteps.
    fn play_sounds(&mut self, listener: Vec3, local_entity: Option<u16>) {
        let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
        for cmd in self.session.take_sounds() {
            match cmd {
                SoundCommand::Stop { entity, channel } => {
                    self.audio.stop_channel(entity, channel);
                }
                SoundCommand::Start(s) | SoundCommand::Ambient(s) => {
                    if s.is_stop() {
                        self.audio.stop_channel(s.entity, s.channel);
                        continue;
                    }
                    if s.is_sentence() {
                        continue;
                    }
                    let Some(name) = s.name.as_deref() else {
                        continue;
                    };
                    if name.starts_with(['!', '#', '*']) || !s.volume.is_finite() {
                        continue;
                    }
                    if Some(s.entity) == local_entity {
                        self.audio.play_2d_tracked(
                            self.vfs.as_ref(),
                            name,
                            s.volume * master,
                            Some((s.entity, s.channel)),
                        );
                    } else if s.origin.is_finite() {
                        self.audio.play_3d_tracked(
                            self.vfs.as_ref(),
                            name,
                            s.origin,
                            listener,
                            s.volume * master,
                            Some((s.entity, s.channel)),
                        );
                    }
                }
                SoundCommand::Fade { .. } => {}
            }
        }
        for s in self.session.take_predicted_sounds() {
            if s.volume.is_finite() {
                self.audio.play_2d(self.vfs.as_ref(), &s.sample, s.volume * master);
            }
        }
    }

    /// Moves what the HUD state collects into the timed logs the HUD draws.
    fn collect_hud_feeds(&mut self, now: f64) {
        let loc = self.loc.as_ref();
        let feeds = &mut self.feeds;
        let weapon = self
            .session
            .with_hud_mut(|hud| {
                for d in hud.deaths.drain(..) {
                    feeds.deaths.push((d, now));
                }
                for line in hud.chat.drain(..) {
                    let text = format_message(&line.text, &line.params, loc);
                    if !text.is_empty() {
                        feeds.chat.push((text, now));
                    }
                }
                let mut got_dest4 = false;
                for line in hud.text.drain(..) {
                    let text = format_message(&line.message, &line.params, loc);
                    if text.is_empty() {
                        continue;
                    }
                    match line.dest {
                        4 => {
                            feeds.center = Some((text, now));
                            got_dest4 = true;
                        }
                        3 | 5 => feeds.chat.push((text, now)),
                        1 => feeds.notify.push((text, now)),
                        _ => {}
                    }
                }
                if let Some(text) = hud.center_print.take()
                    && !got_dest4
                {
                    let text = format_message(&text, &[], loc);
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
        self.feeds.expire(now);

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
        }
    }

    fn forward_to_server(&mut self, line: &str) {
        if self.session.in_game() {
            self.session.send_stringcmd(line);
        } else {
            self.cvars.warn(format!("Not connected: \"{line}\""));
        }
    }

    fn process_actions(&mut self, actions: Vec<EngineAction>, event_loop: &ActiveEventLoop) {
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
                        event_loop.exit();
                        return;
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
                        } else if self.feeds.motd_open {
                            self.feeds.motd_open = false;
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
                        if pressed {
                            self.active_actions.insert(name);
                        } else {
                            self.active_actions.remove(&name);
                        }
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

    /// `slot1`..`slot5`: selects the next weapon of the slot the player carries.
    fn select_slot(&mut self, slot: u8) {
        let owned_bits = self.session.owned_weapons();
        let pick = self
            .session
            .with_hud(|hud| {
                let owned: HashSet<u8> = hud
                    .weapons
                    .values()
                    .map(|w| w.weapon_id)
                    .filter(|id| *id < 32 && owned_bits & (1 << id) != 0)
                    .collect();
                weapon_for_slot(
                    &hud.weapons,
                    &owned,
                    hud.current_weapon.map(|c| c.weapon_id),
                    slot,
                )
                .map(str::to_string)
            })
            .flatten();
        if let Some(name) = pick {
            self.session.send_stringcmd(&name);
        }
    }

    /// Queues this frame's usercmd. A cmd is `msec` of the frame, view angles from the mouse.
    fn queue_usercmd(&mut self, dt: f64) {
        if !self.session.in_game() {
            return;
        }
        let capture = self.wants_mouse_capture();
        let held = Held {
            actions: capture.then_some(&self.active_actions),
            mouse1: capture && self.mouse1_held,
            mouse2: capture && self.mouse2_held,
        };
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
            self.process_actions(vec![EngineAction::ToggleMenu], event_loop);
            return true;
        }

        // Allow releasing TAB even while the scoreboard is visible
        if code == KeyCode::Tab
            && !self.console_ui.open
            && !self.chat_ui.open
            && !self.menu_ui.visible
            && self.is_in_game()
        {
            if pressed {
                self.active_actions.insert("showscores".into());
            } else {
                self.active_actions.remove("showscores");
            }
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
            let actions = self.cvars.handle_key_bind(key_name, pressed);
            if !actions.is_empty() {
                self.process_actions(actions, event_loop);
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

    fn render_and_step(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let raw_dt = (now - self.last_frame).as_secs_f32();
        let dt = raw_dt.clamp(0.0001, 0.1);
        self.last_frame = now;

        let inst_fps = 1.0 / raw_dt.max(0.0001);
        self.fps_smooth = self.fps_smooth * 0.9 + inst_fps * 0.1;
        self.frame_ms_smooth = self.frame_ms_smooth * 0.9 + (raw_dt * 1000.0) * 0.1;

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
        self.session.advance(f64::from(dt));
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
        if let Some(state) = self.session.state()
            && state.is_terminal()
        {
            match &state {
                ConnState::Rejected(why) | ConnState::Disconnected(why) => {
                    self.cvars.error(format!("Disconnected: {why}"));
                }
                _ => {}
            }
            self.end_session();
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
                    self.session.with_hud_mut(|h| {
                        h.menu = None;
                        h.vgui_menu = None;
                    });
                }
                Some(ShellStuffAction::PlaySound(sample)) => {
                    self.audio.play_2d(self.vfs.as_ref(), &sample, master);
                }
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
        let show_scores =
            self.active_actions.contains("showscores") && in_game && !self.menu_ui.visible;
        let mp3_vol = finite_or(self.cvars.get_f32("mp3volume"), 0.8).clamp(0.0, 1.0);
        let want_menu_music = !self.session.is_active() && self.launch.auto_screenshot.is_none();
        self.audio
            .sync_menu_music(self.vfs.as_ref(), want_menu_music, mp3_vol);

        let default_fov = finite_or(self.cvars.get_f32("default_fov"), 90.0)
            .clamp(60.0, 120.0);

        // 2. The frame of the session.
        let mut view: Option<FrameView> = None;
        if in_game {
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
        }
        self.weapon_switch_timer = (self.weapon_switch_timer - dt).max(0.0);

        if let Some(v) = &view {
            self.view_stats = v.stats;
            if v.eye.is_finite() {
                self.camera.position = v.eye;
            }
            let local_entity = self.session.local_slot().map(|s| u16::from(s) + 1);
            self.play_sounds(v.eye, local_entity);
            let master = finite_or(self.cvars.get_f32("volume"), 1.0).clamp(0.0, 1.0);
            for path in &v.anim_sounds {
                self.audio.play_2d(self.vfs.as_ref(), path, 0.8 * master);
            }
        }
        let paused = self.any_modal_open();
        let names = self.session.player_names();
        let (score_rows, ct_score, t_score) = self.score_rows(&names);
        let owned: HashSet<u8> = {
            let bits = self.session.owned_weapons();
            (0u8..32).filter(|id| bits & (1 << id) != 0).collect()
        };

        let sess_now = self.session.now();
        let deaths: Vec<DeathNotice> = self.feeds.deaths.iter().map(|(d, _)| d.clone()).collect();
        let chat: Vec<String> = self.feeds.chat.iter().map(|(c, _)| c.clone()).collect();
        let notify: Vec<String> = self.feeds.notify.iter().map(|(c, _)| c.clone()).collect();
        let center = self.feeds.center.as_ref().map(|(c, _)| c.clone());
        let menu_lines_now: Option<Vec<String>> = self
            .session
            .with_hud(|h| {
                if let Some(m) = h
                    .menu
                    .as_ref()
                    .filter(|m| !m.multipart && m.keys != 0 && !m.text.is_empty())
                {
                    Some(menu_lines(&m.text, self.loc.as_ref()))
                } else {
                    h.vgui_menu
                        .as_ref()
                        .filter(|v| !v.multipart && v.keys != 0)
                        .map(vgui_menu_fallback_lines)
                }
            })
            .flatten();
        let motd_text: Option<String> = self
            .feeds
            .motd_open
            .then(|| self.session.with_hud(|h| strip_html(&h.motd)))
            .flatten();

        let local_speed = self.session.local_speed();
        let map_name = self.session.loaded_map().unwrap_or("").to_string();
        let hostname = self
            .session
            .with_hud(|h| h.server_name.clone())
            .unwrap_or_default();
        // Sniper scope overlay applies at <= 40 deg (AWP/Scout/auto-sniper), keeping AUG/SG552 (55 deg) unmasked.
        let scoped = view
            .as_ref()
            .is_some_and(|v| v.fov.is_finite() && v.fov > 0.0 && v.fov <= 40.0);
        let alive = view.as_ref().is_some_and(|v| v.alive);

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

        let Some(rt) = &mut self.runtime else {
            return;
        };
        let raw_input = rt.egui_state.take_egui_input(&rt.window);
        rt.egui_ctx.begin_pass(raw_input);

        if let Some(v) = &view {
            if !paused && alive && !scoped {
                let dyn_gap = if self.cvars.get_bool("cl_dynamiccrosshair") {
                    local_speed / 45.0
                } else {
                    0.0
                };
                draw_crosshair(&rt.egui_ctx, &self.cvars, dyn_gap);
            }
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
                draw_cs16_hud(&rt.egui_ctx, rt.hud_sprites.as_ref(), &view);
            });
            let _ = hud_state;
            draw_mod_hud(&rt.egui_ctx, &mod_hud_cmds);
            if let Some(lines) = &menu_lines_now {
                let scale = (rt.egui_ctx.content_rect().height() / 768.0).clamp(0.85, 1.45);
                draw_text_menu(&rt.egui_ctx, lines, scale);
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
                show_scoreboard(
                    &rt.egui_ctx,
                    &hostname,
                    &map_name,
                    ct_score,
                    t_score,
                    &score_rows,
                    self.session.local_slot(),
                    rt.vgui_bank.client_palette,
                );
            }
            if self.chat_ui.open {
                sent_chat = self.chat_ui.show(&rt.egui_ctx, self.palette);
            }
            if let Some(text) = &motd_text {
                egui::Window::new("Message of the day")
                    .collapsible(false)
                    .resizable(true)
                    .default_width(520.0)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(&rt.egui_ctx, |ui| {
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            ui.label(text);
                        });
                        if ui.button("Close").clicked() {
                            motd_close = true;
                        }
                    });
            }
        }

        let steam_icon = rt.vgui_bank.steam_icon.as_ref();
        let menu_cmds = self.menu_ui.show(
            &rt.egui_ctx,
            &mut self.cvars,
            &self.available_maps,
            self.session.is_active(),
            rt.bg_texture.as_ref(),
            steam_icon,
            self.palette,
        );
        pending_actions.extend(menu_cmds.actions);
        if menu_cmds.open_options {
            self.options_ui.open = true;
        }
        if menu_cmds.resume_game {
            should_resume = true;
        }
        self.options_ui
            .show(&rt.egui_ctx, &mut self.cvars, self.palette, steam_icon);
        let mod_decisions = draw_mod_approval_prompt(&rt.egui_ctx, &pending_mod_approvals);
        pending_actions.extend(self.console_ui.show(
            &rt.egui_ctx,
            &mut self.cvars,
            &self.available_maps,
            self.palette,
            steam_icon,
        ));

        let full_output = rt.egui_ctx.end_pass();
        rt.egui_state
            .handle_platform_output(&rt.window, full_output.platform_output);
        let paint_jobs = rt
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

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
            render_cam.position.z += finite_or(v.bob, 0.0) + finite_or(v.shake_z, 0.0);
            render_cam.pitch_deg =
                (finite_or(render_cam.pitch_deg, 0.0) + punch_x + finite_or(v.shake_angles.x, 0.0))
                    .clamp(-89.0, 89.0);
            render_cam.yaw_deg =
                (finite_or(render_cam.yaw_deg, 0.0) + punch_y + finite_or(v.shake_angles.y, 0.0))
                    .rem_euclid(360.0);
            render_cam.roll_deg =
                (punch_z + finite_or(v.roll_deg, 0.0) + finite_or(v.shake_angles.z, 0.0))
                    .clamp(-90.0, 90.0);
            fov = (finite_or(v.fov, default_fov) + finite_or(v.underwater_fov_delta, 0.0))
                .clamp(10.0, 120.0);
            sensitivity_scale = (fov / 90.0).clamp(0.2, 1.0);
            if let Some(vm) = &v.viewmodel {
                let eye = if v.eye.is_finite() { v.eye } else { Vec3::ZERO };
                let bob_off = Vec3::new(0.0, 0.0, finite_or(v.bob, 0.0) * 0.4) + v.viewmodel_lag;
                let xf = viewmodel_transform(
                    eye,
                    render_cam.yaw_deg,
                    render_cam.pitch_deg,
                    bob_off,
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

        let debug_mode = if self.cvars.get_bool("r_lightmap_only") {
            1.0
        } else if self.cvars.get_bool("r_fullbright") || !self.cvars.get_bool("r_lightmap") {
            2.0
        } else {
            0.0
        };
        let texture_mode = TextureFilterMode::parse(self.cvars.get_str("gl_texturemode"));
        let anisotropy = self.cvars.get_i32("gl_ansio").clamp(1, 16) as u16;
        let msaa_samples = if self.cvars.get_i32("gl_msaa") >= 4 { 4 } else { 1 };
        let viewmodel_fov = finite_or(self.cvars.get_f32("viewmodel_fov"), 90.0).clamp(50.0, 120.0);
        let settings = RenderSettings {
            gamma: finite_or(self.cvars.get_f32("gamma"), 2.5).clamp(1.0, 4.0),
            texgamma: finite_or(self.cvars.get_f32("texgamma"), 2.0).clamp(0.5, 4.0),
            brightness: finite_or(self.cvars.get_f32("brightness"), 1.0).clamp(0.3, 2.5),
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
        let auto_capture_now = self.frame_count == 6 && self.launch.auto_screenshot.is_some();
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
                event_loop.exit();
                return;
            }
        }

        // 5. Post-frame actions.
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
        if !pending_actions.is_empty() {
            self.process_actions(pending_actions, event_loop);
        }
        self.sync_cursor_capture();

        let fps_max = self.cvars.get_i32("fps_max");
        if fps_max > 0 {
            let target = Duration::from_secs_f64(1.0 / (fps_max as f64));
            let elapsed = now.elapsed();
            if let Some(rem) = target.checked_sub(elapsed)
                && rem > Duration::from_millis(1)
            {
                std::thread::sleep(rem - Duration::from_millis(1));
            }
        }

        if let Some(rt) = &self.runtime {
            rt.window.request_redraw();
        }
    }
}

impl ApplicationHandler for Cs16App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.runtime.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("Counter-Strike 1.6 (Rust)")
            .with_inner_size(LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));

        let renderer = Renderer::new(Arc::clone(&window));
        let egui_ctx = egui::Context::default();
        apply_vgui2_theme(&egui_ctx, self.palette);

        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

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

        info!(gpu = %renderer.adapter_name, "Counter-Strike 1.6 (Rust) window ready");

        self.runtime = Some(WindowRuntime {
            window,
            renderer,
            egui_ctx,
            egui_state,
            bg_texture,
            hud_sprites,
            vgui_bank,
            cursor_captured: false,
        });

        if let Some(map) = self.launch.startup_map.clone() {
            self.start_local_map(&map);
        }
        let startup_cmds = std::mem::take(&mut self.launch.startup_exec);
        for cmd in startup_cmds {
            let actions = self.cvars.execute_script(&cmd);
            self.process_actions(actions, event_loop);
        }
        if let Some(ui_mode) = self.launch.startup_ui.take() {
            if ui_mode.eq_ignore_ascii_case("options") {
                self.options_ui.open = true;
            }
            self.sync_cursor_capture();
        }
        if let Some(rt) = &self.runtime {
            rt.window.request_redraw();
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
                {
                    let _ = rt.window.set_cursor_grab(CursorGrabMode::None);
                    rt.window.set_cursor_visible(true);
                    rt.cursor_captured = false;
                }
                self.active_actions.clear();
                self.mouse1_held = false;
                self.mouse2_held = false;
            }
            WindowEvent::Resized(new_size) => {
                if let Some(rt) = &mut self.runtime {
                    rt.renderer.resize(new_size.width, new_size.height);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.wants_mouse_capture() {
                    let pressed = *state == ElementState::Pressed;
                    match button {
                        MouseButton::Left => self.mouse1_held = pressed,
                        MouseButton::Right => self.mouse2_held = pressed,
                        _ => {}
                    }
                    return;
                } else if *state == ElementState::Pressed {
                    let vol = self.cvars.get_f32("volume").clamp(0.0, 1.0);
                    self.audio.play_ui_click(self.vfs.as_ref(), vol);
                }
            }
            WindowEvent::KeyboardInput { event: key_ev, .. } => {
                if self.handle_key_event(key_ev, event_loop) {
                    return;
                }
            }
            WindowEvent::RedrawRequested => {
                self.render_and_step(event_loop);
                return;
            }
            _ => {}
        }

        if !self.wants_mouse_capture()
            && let Some(rt) = &mut self.runtime
        {
            let _ = rt.egui_state.on_window_event(&rt.window, &event);
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
            // A zoomed view turns slower, in proportion to the fov.
            let sens = self.cvars.get_f32("sensitivity").max(0.05) * self.mouse_sens_scale;
            let m_yaw = self.cvars.get_f32("m_yaw");
            let m_pitch = self.cvars.get_f32("m_pitch");
            self.camera
                .apply_mouse_delta(dx as f32, dy as f32, sens, m_yaw, m_pitch);
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

/// Converts 1-based `HudState.players` entity keys to 0-based `ScoreRow.slot` and `names` keys.
fn build_score_rows(hud: &HudState, names: &BTreeMap<u8, String>) -> (Vec<ScoreRow>, i16, i16) {
    let rows = hud
        .players
        .iter()
        .filter(|(_, p)| p.team == "CT" || p.team == "TERRORIST")
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
    StopSound,
    Echo(String),
}

/// Allowlisted `svc_stufftext` commands handled by the shell without re-parsing through `execute_script`.
fn classify_shell_command(cmd: &ConsoleCommand) -> Option<ShellStuffAction> {
    match cmd.name.to_ascii_lowercase().as_str() {
        "slot10" => Some(ShellStuffAction::CloseMenu),
        "stopsound" => Some(ShellStuffAction::StopSound),
        "echo" => Some(ShellStuffAction::Echo(cmd.args.join(" "))),
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
            Some(ShellStuffAction::PlaySound(sample.to_string()))
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
}
