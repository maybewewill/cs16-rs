//! Main `winit` ApplicationHandler uniting VFS, CVar/Console, 100 Hz CS 1.6 Simulation (`cs-sim`),
//! UDP Multiplayer (`cs-net`), GoldSrc Audio (`rodio`), `wgpu` World/MDL/FX Renderer, and VGUI2 + Amber HUD.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cs_assets::bsp::{BspMap, SpawnTeam};
use cs_assets::materials::MaterialTable;
use cs_assets::scheme::{SkyboxTextures, VguiColors, load_menu_background};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_net::{ClientPacket, NetClient, ServerPacket, WorldSnapshot};
use cs_sim::pmove::{MoveInputCmd, MoveParams};
use cs_sim::rules::{
    MatchConfig, MatchWorld, PlayerInput, PlayerModelSkin, PlayerState, RoundPhase, SimEvent,
    TICK_DT, ViewAnimKind,
};
use cs_sim::weapons::WeaponId;
use cs_vfs::{Vfs, VfsConfig};
use glam::{Mat4, Vec3};
use tracing::{error, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::audio::AudioEngine;
use crate::camera::Camera;
use crate::render::bsp_renderer::BspRenderStats;
use crate::render::mdl_renderer::{MdlDrawInstance, player_model_transform, viewmodel_transform};
use crate::render::{RenderSettings, Renderer, SceneDynamicData};
use crate::ui::buy_menu::{BuyCategory, BuyMenuUi};
use crate::ui::console::ConsoleUi;
use crate::ui::hud::{HudSprites, HudState, draw_cs16_hud, radio_menu_entries};
use crate::ui::main_menu::MainMenuUi;
use crate::ui::options::OptionsUi;
use crate::ui::scoreboard::show_scoreboard;
use crate::ui::team_menu::{ChatInputUi, TeamMenuUi};
use crate::ui::vgui_assets::VguiAssetBank;
use crate::ui::vgui_theme::{VguiPalette, apply_vgui2_theme};
use crate::ui::{FrameTelemetry, draw_crosshair, draw_net_graph};

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

enum SessionMode {
    None,
    Local {
        world: Box<MatchWorld>,
        local_id: u32,
    },
    Remote {
        client: NetClient,
        snapshot: Option<Box<WorldSnapshot>>,
    },
}

pub struct Cs16App {
    launch: LaunchConfig,
    vfs: Option<Vfs>,
    materials: MaterialTable,
    available_maps: Vec<String>,
    cvars: CVarRegistry,
    palette: VguiPalette,
    audio: AudioEngine,
    runtime: Option<WindowRuntime>,

    // Active map & simulation session
    loaded_bsp: Option<BspMap>,
    session: SessionMode,
    sim_accumulator: f32,
    camera: Camera,
    active_actions: HashSet<String>,
    pending_slot: Option<u8>,
    pending_last_inv: bool,
    pending_drop: bool,
    mouse1_held: bool,
    mouse2_held: bool,
    muzzle_flashes: Vec<(Vec3, f32)>,
    last_stats: BspRenderStats,
    last_weapon_id: Option<WeaponId>,
    weapon_switch_timer: f32,
    radio_menu: Option<u8>,
    spectate_index: usize,
    spectate_free: bool,

    // UI windows
    menu_ui: MainMenuUi,
    console_ui: ConsoleUi,
    options_ui: OptionsUi,
    buy_ui: BuyMenuUi,
    team_ui: TeamMenuUi,
    chat_ui: ChatInputUi,

    // Frame timing
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

        let (vfs, materials, available_maps, palette) =
            match Vfs::discover(launch.gamedir.as_deref(), vfs_cfg) {
                Ok(vfs) => {
                    let maps = vfs.list_maps();
                    let colors = VguiColors::load(&vfs);
                    let mats = MaterialTable::load(&vfs);
                    cvars.info(format!(
                        "Mounted GoldSrc assets from: {}",
                        vfs.root().display()
                    ));
                    cvars.info(format!(
                        "Indexed {} files ({} maps). Click New Game or type `map de_dust2`.",
                        vfs.file_count(),
                        maps.len()
                    ));
                    (Some(vfs), mats, maps, VguiPalette::from(colors))
                }
                Err(err) => {
                    cvars.error(format!("{err}"));
                    warn!("{err}");
                    (
                        None,
                        MaterialTable::default(),
                        Vec::new(),
                        VguiPalette::from(VguiColors::default()),
                    )
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
            materials,
            available_maps,
            cvars,
            palette,
            audio: AudioEngine::new(),
            runtime: None,
            loaded_bsp: None,
            session: SessionMode::None,
            sim_accumulator: 0.0,
            camera: Camera::default(),
            active_actions: HashSet::new(),
            pending_slot: None,
            pending_last_inv: false,
            pending_drop: false,
            mouse1_held: false,
            mouse2_held: false,
            muzzle_flashes: Vec::new(),
            last_stats: BspRenderStats {
                draw_calls: 0,
                triangles: 0,
            },
            last_weapon_id: None,
            weapon_switch_timer: 0.0,
            radio_menu: None,
            spectate_index: 0,
            spectate_free: false,
            menu_ui: MainMenuUi::default(),
            console_ui,
            options_ui: OptionsUi::default(),
            buy_ui: BuyMenuUi::default(),
            team_ui: TeamMenuUi::default(),
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
        !matches!(self.session, SessionMode::None)
    }

    fn any_modal_open(&self) -> bool {
        self.menu_ui.visible
            || self.console_ui.open
            || self.options_ui.open
            || self.buy_ui.open
            || self.team_ui.open
            || self.chat_ui.open
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

    fn read_move_params(&self) -> MoveParams {
        MoveParams {
            gravity: self.cvars.get_f32("sv_gravity").max(50.0),
            accelerate: self.cvars.get_f32("sv_accelerate").max(0.5),
            air_accelerate: self.cvars.get_f32("sv_airaccelerate").max(0.5),
            friction: self.cvars.get_f32("sv_friction").max(0.1),
            stop_speed: self.cvars.get_f32("sv_stopspeed").max(1.0),
            step_size: self.cvars.get_f32("sv_stepsize").clamp(1.0, 48.0),
            max_speed: self.cvars.get_f32("sv_maxspeed").max(100.0),
        }
    }

    fn read_match_config(&self) -> MatchConfig {
        MatchConfig {
            round_time_sec: (self.cvars.get_f32("mp_roundtime").max(0.5)) * 60.0,
            freeze_time_sec: self.cvars.get_f32("mp_freezetime").clamp(0.0, 15.0),
            c4_timer_sec: self.cvars.get_f32("mp_c4timer").clamp(10.0, 90.0),
            start_money: self.cvars.get_i32("mp_startmoney").clamp(800, 16000),
            friendly_fire: self.cvars.get_bool("mp_friendlyfire"),
            bot_difficulty: self.cvars.get_i32("bot_difficulty").clamp(0, 3) as u8,
        }
    }

    fn load_bsp_to_gpu(&mut self, map_name: &str) -> Result<BspMap, String> {
        let Some(vfs) = &self.vfs else {
            return Err("Half-Life / CS 1.6 directory not mounted.".into());
        };
        let t0 = Instant::now();
        let map = BspMap::load(vfs, map_name).map_err(|e| e.to_string())?;
        let sky = SkyboxTextures::load(vfs, &map.skyname);
        if let Some(rt) = &mut self.runtime {
            rt.renderer.load_map(&map, &sky);
            rt.window.set_title(&format!(
                "Counter-Strike 1.6 (Rust) — {} [{}]",
                map.name, rt.renderer.adapter_name
            ));
        }
        self.cvars.info(format!(
            "Loaded {} in {:.0} ms ({} faces, {} batches, sky: {})",
            map.name,
            t0.elapsed().as_secs_f32() * 1000.0,
            map.faces.len(),
            map.batches.len(),
            sky.skyname
        ));
        Ok(map)
    }

    fn start_local_map(&mut self, map_name: &str) {
        self.cvars.info(format!("Starting local match on \"{map_name}\"..."));
        match self.load_bsp_to_gpu(map_name) {
            Ok(map) => {
                let mut world = MatchWorld::new(
                    &map,
                    self.materials.clone(),
                    self.read_match_config(),
                );
                world.move_params = self.read_move_params();
                let player_name = self.cvars.get_str("name").to_string();
                let local_id = world.add_player(player_name, SpawnTeam::CounterTerrorist, false);
                let quota = self.cvars.get_i32("bot_quota").clamp(0, 15) as usize;
                world.set_bot_quota(quota);
                world.start_new_round();

                if let Some(me) = world.player(local_id) {
                    self.camera.position = me.eye_pos();
                    self.camera.yaw_deg = me.yaw_deg;
                    self.camera.pitch_deg = me.pitch_deg;
                }

                self.cvars.info(format!(
                    "Match ready: {} players ({} nav waypoints). Press B to buy, M to switch team, TAB for scores.",
                    world.players.len(),
                    world.nav.nodes.len()
                ));

                self.loaded_bsp = Some(map);
                self.session = SessionMode::Local {
                    world: Box::new(world),
                    local_id,
                };
                self.sim_accumulator = 0.0;
                self.menu_ui.visible = false;
                self.console_ui.open = false;
                self.options_ui.open = false;
                self.buy_ui.close();
                self.team_ui.open = false;
                self.sync_cursor_capture();
            }
            Err(err) => {
                error!(map = map_name, %err, "Failed to load BSP map");
                self.cvars.error(format!("Failed to load map \"{map_name}\": {err}"));
            }
        }
    }

    fn connect_remote(&mut self, addr: &str) {
        let name = self.cvars.get_str("name").to_string();
        match NetClient::connect(addr, &name) {
            Ok(client) => {
                self.cvars.info(format!("Connecting to {addr}..."));
                self.session = SessionMode::Remote {
                    client,
                    snapshot: None,
                };
                self.menu_ui.visible = false;
                self.menu_ui.find_servers_open = false;
                self.sync_cursor_capture();
            }
            Err(err) => {
                self.cvars.error(format!("Failed to connect to {addr}: {err}"));
            }
        }
    }

    fn disconnect_map(&mut self) {
        if let SessionMode::Remote { client, .. } = &self.session {
            let _ = client.send(&ClientPacket::Disconnect);
        }
        if let Some(rt) = &mut self.runtime {
            rt.renderer.unload_map();
            rt.window.set_title("Counter-Strike 1.6 (Rust)");
        }
        self.loaded_bsp = None;
        self.session = SessionMode::None;
        self.menu_ui.visible = true;
        self.buy_ui.close();
        self.team_ui.open = false;
        self.chat_ui.open = false;
        self.active_actions.clear();
        self.sync_cursor_capture();
        self.cvars.info("Disconnected to main menu.");
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

    fn handle_sim_events(&mut self, events: Vec<SimEvent>, listener_pos: Vec3) {
        let master_vol = self.cvars.get_f32("volume").clamp(0.0, 1.0);
        for ev in events {
            match ev {
                SimEvent::Sound {
                    path,
                    position,
                    volume,
                } => {
                    if let Some(pos) = position {
                        self.audio.play_3d(
                            self.vfs.as_ref(),
                            &path,
                            pos,
                            listener_pos,
                            volume,
                            master_vol,
                        );
                    } else {
                        self.audio
                            .play_2d(self.vfs.as_ref(), &path, volume * master_vol);
                    }
                }
                SimEvent::MuzzleFlash { position, .. } => {
                    self.muzzle_flashes.push((position, 0.055));
                }
                SimEvent::Explosion { position, .. } => {
                    self.muzzle_flashes.push((position + Vec3::Z * 16.0, 0.16));
                }
                SimEvent::Banner { .. } => {}
            }
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
                    EngineAction::Disconnect => self.disconnect_map(),
                    EngineAction::Quit => {
                        if self.cvars.is_dirty() {
                            self.save_config();
                        }
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
                        } else if self.buy_ui.open {
                            self.buy_ui.close();
                        } else if self.team_ui.open {
                            self.team_ui.open = false;
                        } else if self.chat_ui.open {
                            self.chat_ui.open = false;
                        } else if self.options_ui.open {
                            self.options_ui.open = false;
                        } else if self.menu_ui.new_game_open {
                            self.menu_ui.new_game_open = false;
                        } else if self.menu_ui.find_servers_open {
                            self.menu_ui.find_servers_open = false;
                        } else if self.is_in_game() {
                            self.menu_ui.visible = !self.menu_ui.visible;
                        }
                        self.sync_cursor_capture();
                    }
                    EngineAction::ToggleNoclip => {
                        if let SessionMode::Local { world, local_id } = &mut self.session
                            && let Some(p) = world.player_mut(*local_id)
                        {
                            p.mv.noclip = !p.mv.noclip;
                            let state = if p.mv.noclip { "ON" } else { "OFF" };
                            self.cvars.info(format!("noclip {state}"));
                        }
                    }
                    EngineAction::ToggleGod => {
                        if let SessionMode::Local { world, local_id } = &mut self.session
                            && let Some(p) = world.player_mut(*local_id)
                        {
                            p.god_mode = !p.god_mode;
                            self.cvars
                                .info(format!("godmode {}", if p.god_mode { "ON" } else { "OFF" }));
                        }
                    }
                    EngineAction::Suicide => {
                        if let SessionMode::Local { world, local_id } = &mut self.session
                            && let Some(p) = world.player_mut(*local_id)
                        {
                            p.health = 0;
                            p.alive = false;
                            p.deaths += 1;
                        }
                    }
                    EngineAction::RestartRound => {
                        let cfg = self.read_match_config();
                        if let SessionMode::Local { world, .. } = &mut self.session {
                            world.config = cfg;
                            world.restart_match();
                            self.cvars.info("The game will restart now!");
                        }
                    }
                    EngineAction::BotAdd(team_opt) => {
                        if let SessionMode::Local { world, .. } = &mut self.session {
                            let team = match team_opt.as_deref().map(str::to_ascii_lowercase).as_deref() {
                                Some("t" | "terrorist") => Some(SpawnTeam::Terrorist),
                                Some("ct" | "counterterrorist") => Some(SpawnTeam::CounterTerrorist),
                                _ => None,
                            };
                            let id = world.add_bot(team);
                            if let Some(p) = world.player(id) {
                                self.cvars.info(format!("Added bot {} ({:?})", p.name, p.team));
                            }
                        }
                    }
                    EngineAction::BotKick => {
                        if let SessionMode::Local { world, .. } = &mut self.session {
                            world.kick_all_bots();
                            self.cvars.info("Kicked all bots.");
                        }
                    }
                    EngineAction::OpenBuyMenu => {
                        if self.is_in_game() {
                            if self.buy_ui.open {
                                self.buy_ui.close();
                            } else {
                                self.buy_ui.open_menu();
                            }
                            self.sync_cursor_capture();
                        }
                    }
                    EngineAction::BuyItem(item) => self.perform_buy(&item),
                    EngineAction::ChooseTeam => {
                        if self.is_in_game() {
                            if self.team_ui.open {
                                self.team_ui.close();
                            } else {
                                self.team_ui.open_team_select();
                            }
                            self.sync_cursor_capture();
                        }
                    }
                    EngineAction::GiveWeapon(w) => {
                        if let SessionMode::Local { world, local_id } = &mut self.session {
                            match world.give_weapon_cheat(*local_id, &w) {
                                Ok(msg) => self.cvars.info(msg),
                                Err(err) => self.cvars.warn(err),
                            }
                        }
                    }
                    EngineAction::Say { team_only, text } => self.send_chat(team_only, text),
                    EngineAction::OpenChat { team_only } => {
                        if self.is_in_game() {
                            self.chat_ui.open_chat(team_only);
                            self.sync_cursor_capture();
                        }
                    }
                    EngineAction::DropWeapon => self.pending_drop = true,
                    EngineAction::LastInv => self.pending_last_inv = true,
                    EngineAction::SelectSlot(slot) => self.pending_slot = Some(slot),
                    EngineAction::ToggleNightVision => {
                        if let SessionMode::Local { world, local_id } = &mut self.session {
                            world.toggle_nightvision(*local_id);
                        }
                    }
                    EngineAction::OpenRadio(group) => {
                        if self.is_in_game() {
                            self.radio_menu = if self.radio_menu == Some(group) {
                                None
                            } else {
                                Some(group)
                            };
                        }
                    }
                    EngineAction::Spray => {
                        if let SessionMode::Local { world, local_id } = &mut self.session {
                            world.spray_logo(*local_id);
                        }
                    }
                    EngineAction::SetPos([x, y, z]) => {
                        if let SessionMode::Local { world, local_id } = &mut self.session
                            && let Some(p) = world.player_mut(*local_id)
                        {
                            p.mv.origin = Vec3::new(x, y, z);
                            p.mv.velocity = Vec3::ZERO;
                        }
                        self.camera.position = Vec3::new(x, y, z);
                    }
                    EngineAction::GetPos => {
                        let p = self.camera.position;
                        self.cvars.info(format!(
                            "setpos {:.2} {:.2} {:.2}; // yaw {:.1} pitch {:.1}",
                            p.x, p.y, p.z, self.camera.yaw_deg, self.camera.pitch_deg
                        ));
                    }
                    EngineAction::Status => {
                        let map = self
                            .loaded_bsp
                            .as_ref()
                            .map(|m| m.name.as_str())
                            .unwrap_or("<none>");
                        let players = match &self.session {
                            SessionMode::Local { world, .. } => world.players.len(),
                            SessionMode::Remote { snapshot: Some(s), .. } => s.players.len(),
                            _ => 0,
                        };
                        self.cvars.info(format!(
                            "hostname: {} | map: {map} | players: {players} | fps: {:.1}",
                            self.cvars.get_str("hostname"),
                            self.fps_smooth
                        ));
                    }
                    EngineAction::Connect(addr) => self.connect_remote(&addr),
                    EngineAction::Screenshot(custom_path) => {
                        let path = custom_path.map(PathBuf::from).unwrap_or_else(|| {
                            let map = self
                                .loaded_bsp
                                .as_ref()
                                .map(|m| m.name.as_str())
                                .unwrap_or("menu");
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
                }
            }
        }
    }

    fn perform_buy(&mut self, item: &str) {
        match &mut self.session {
            SessionMode::Local { world, local_id } => match world.buy_item(*local_id, item) {
                Ok(msg) => self.cvars.info(msg),
                Err(err) => self.cvars.warn(err),
            },
            SessionMode::Remote { client, .. } => {
                let _ = client.send(&ClientPacket::Buy(item.to_string()));
            }
            SessionMode::None => {}
        }
    }

    fn send_chat(&mut self, team_only: bool, text: String) {
        match &mut self.session {
            SessionMode::Local { world, local_id } => {
                if let Some(p) = world.player(*local_id) {
                    let sender = p.name.clone();
                    let team = if team_only { Some(p.team) } else { None };
                    world.push_chat(sender, team, text);
                }
            }
            SessionMode::Remote { client, .. } => {
                let _ = client.send(&ClientPacket::Chat { team_only, text });
            }
            SessionMode::None => {}
        }
    }

    fn build_player_input(&mut self) -> PlayerInput {
        let capture = self.wants_mouse_capture();
        let fwd = if capture {
            f32::from(self.active_actions.contains("forward"))
                - f32::from(self.active_actions.contains("back"))
        } else {
            0.0
        };
        let right = if capture {
            f32::from(self.active_actions.contains("moveright"))
                - f32::from(self.active_actions.contains("moveleft"))
        } else {
            0.0
        };
        PlayerInput {
            move_cmd: MoveInputCmd {
                forward: fwd,
                right,
                yaw_deg: self.camera.yaw_deg,
                pitch_deg: self.camera.pitch_deg,
                jump: capture && self.active_actions.contains("jump"),
                duck: capture && self.active_actions.contains("duck"),
                walk: capture && self.active_actions.contains("speed"),
            },
            attack: capture && (self.mouse1_held || self.active_actions.contains("attack")),
            attack2: capture && (self.mouse2_held || self.active_actions.contains("attack2")),
            reload: capture && self.active_actions.contains("reload"),
            use_key: capture && self.active_actions.contains("use"),
            select_slot: self.pending_slot.take(),
            last_inv: std::mem::take(&mut self.pending_last_inv),
            drop_weapon: std::mem::take(&mut self.pending_drop),
        }
    }

    fn step_session(&mut self, dt: f32) {
        // Decay muzzle flashes
        for (_, rem) in &mut self.muzzle_flashes {
            *rem -= dt;
        }
        self.muzzle_flashes.retain(|(_, rem)| *rem > 0.0);

        match &mut self.session {
            SessionMode::None => {}
            SessionMode::Local { .. } => {
                let move_params = self.read_move_params();
                self.sim_accumulator = (self.sim_accumulator + dt).min(0.1);
                let mut all_events = Vec::new();
                let mut listener = self.camera.position;

                while self.sim_accumulator >= TICK_DT {
                    self.sim_accumulator -= TICK_DT;
                    let inp = self.build_player_input();
                    if let SessionMode::Local { world, local_id } = &mut self.session {
                        world.move_params = move_params;
                        let lid = *local_id;
                        world.step(&[(lid, inp)], TICK_DT);
                        all_events.extend(world.take_events());
                        if let Some(me) = world.player(lid) {
                            if me.alive {
                                self.camera.position = me.eye_pos();
                                self.spectate_free = false;
                            } else if !self.spectate_free {
                                let alive_mates: Vec<_> = world
                                    .players
                                    .iter()
                                    .filter(|p| p.alive && p.id != lid && p.team == me.team)
                                    .collect();
                                let alive_any: Vec<_> =
                                    world.players.iter().filter(|p| p.alive && p.id != lid).collect();
                                let pool = if !alive_mates.is_empty() {
                                    alive_mates
                                } else {
                                    alive_any
                                };
                                if let Some(tgt) = pool.get(self.spectate_index % pool.len().max(1)) {
                                    let yaw_r = tgt.yaw_deg.to_radians();
                                    let back = Vec3::new(-yaw_r.cos(), -yaw_r.sin(), 0.25);
                                    self.camera.position = tgt.eye_pos() + back * 72.0;
                                    self.camera.yaw_deg = tgt.yaw_deg;
                                    self.camera.pitch_deg = tgt.pitch_deg.clamp(-60.0, 60.0);
                                }
                            } else {
                                // Free-look spectator flying camera
                                let fwd = f32::from(self.active_actions.contains("forward"))
                                    - f32::from(self.active_actions.contains("back"));
                                let right = f32::from(self.active_actions.contains("moveright"))
                                    - f32::from(self.active_actions.contains("moveleft"));
                                let up = f32::from(self.active_actions.contains("jump"))
                                    - f32::from(self.active_actions.contains("duck"));
                                let spd = self.cvars.get_f32("r_noclip_speed").clamp(100.0, 1200.0);
                                let move_dir = self.camera.forward() * fwd
                                    + self.camera.right() * right
                                    + Vec3::Z * up;
                                if move_dir.length_squared() > 1e-4 {
                                    self.camera.position += move_dir.normalize() * spd * TICK_DT;
                                }
                            }
                            listener = self.camera.position;
                        }
                    }
                }
                self.handle_sim_events(all_events, listener);
            }
            SessionMode::Remote { .. } => {
                let inp = self.build_player_input();
                let mut map_to_load: Option<String> = None;
                let mut net_events = Vec::new();

                if let SessionMode::Remote { client, snapshot } = &mut self.session {
                    let _ = client.send_input(inp);
                    for pkt in client.poll() {
                        match pkt {
                            ServerPacket::Welcome { map_name, .. } => {
                                map_to_load = Some(map_name);
                            }
                            ServerPacket::Snapshot(snap) => {
                                if let Some(pid) = client.player_id
                                    && let Some(me) = snap.players.iter().find(|p| p.id == pid)
                                {
                                    self.camera.position = me.eye_pos();
                                }
                                *snapshot = Some(snap);
                            }
                            ServerPacket::Events { events, .. } => {
                                net_events.extend(events);
                            }
                            ServerPacket::LanInfo(_) => {}
                            ServerPacket::Kicked(reason) => {
                                self.cvars.error(format!("Kicked from server: {reason}"));
                            }
                        }
                    }
                }

                if let Some(m) = map_to_load
                    && self.loaded_bsp.as_ref().map(|b| b.name.as_str()) != Some(m.as_str())
                    && let Ok(bsp) = self.load_bsp_to_gpu(&m)
                {
                    self.loaded_bsp = Some(bsp);
                }
                self.handle_sim_events(net_events, self.camera.position);
            }
        }
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

        // Allow releasing TAB even while scoreboard is visible
        if code == KeyCode::Tab && !self.console_ui.open && !self.chat_ui.open && !self.menu_ui.visible && self.is_in_game() {
            if pressed {
                self.active_actions.insert("showscores".into());
            } else {
                self.active_actions.remove("showscores");
            }
            return true;
        }

        // Toggle Buy Menu cleanly with B even while Buy Menu is already open
        if code == KeyCode::KeyB && pressed && !event.repeat && self.buy_ui.open {
            self.buy_ui.close();
            self.sync_cursor_capture();
            return true;
        }

        if !self.wants_mouse_capture() {
            return false;
        }

        // Intercept `0`..`9` when a CS 1.6 Radio Menu (`Z`/`X`/`C`) is open!
        if let Some(rm) = self.radio_menu
            && pressed
            && !event.repeat
        {
            let digit = match code {
                KeyCode::Digit0 => Some(0usize),
                KeyCode::Digit1 => Some(1),
                KeyCode::Digit2 => Some(2),
                KeyCode::Digit3 => Some(3),
                KeyCode::Digit4 => Some(4),
                KeyCode::Digit5 => Some(5),
                KeyCode::Digit6 => Some(6),
                KeyCode::Digit7 => Some(7),
                KeyCode::Digit8 => Some(8),
                KeyCode::Digit9 => Some(9),
                _ => None,
            };
            if let Some(d) = digit {
                if d == 0 {
                    self.radio_menu = None;
                    return true;
                }
                let entries = radio_menu_entries(rm);
                if let Some(&(msg, wav)) = entries.get(d - 1) {
                    if let SessionMode::Local { world, local_id } = &mut self.session {
                        world.send_radio(*local_id, msg, wav);
                    }
                    self.radio_menu = None;
                    return true;
                }
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

    fn render_and_step(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let raw_dt = (now - self.last_frame).as_secs_f32();
        let dt = raw_dt.clamp(0.0001, 0.1);
        self.last_frame = now;

        let inst_fps = 1.0 / raw_dt.max(0.0001);
        self.fps_smooth = self.fps_smooth * 0.9 + inst_fps * 0.1;
        self.frame_ms_smooth = self.frame_ms_smooth * 0.9 + (raw_dt * 1000.0) * 0.1;

        // 1. Advance 100 Hz simulation / networking
        self.step_session(dt);

        // Extract current read-only snapshot references for rendering & HUD
        let time_sec = self.start_time.elapsed().as_secs_f32();
        let in_game = self.is_in_game();
        let show_scores = self.active_actions.contains("showscores") && in_game && !self.menu_ui.visible;

        // Sync Main Menu music cleanly (`mp3volume`) — silent during active match or auto-screenshot
        let mp3_vol = self.cvars.get_f32("mp3volume").clamp(0.0, 1.0);
        let want_menu_music = !in_game && self.launch.auto_screenshot.is_none();
        self.audio
            .sync_menu_music(self.vfs.as_ref(), want_menu_music, mp3_vol);

        // Build 3D world model instances (players w/ EF_BONEMERGE p_*.mdl, corpses, hostages, dropped weapons)
        struct OwnedWorldMdl {
            path: String,
            xf: Mat4,
            seq: String,
            anim_time: f32,
            loop_anim: bool,
            light: Vec3,
            attached: Option<String>,
        }
        let mut world_mdl_owned: Vec<OwnedWorldMdl> = Vec::new();
        let mut viewmodel_owned: Option<(String, Mat4, String, f32, Vec3)> = None;
        let mut effective_fov = self.cvars.get_f32("fov_default").clamp(60.0, 120.0);
        let mut local_speed = 0.0f32;
        let mut local_punch_pitch = 0.0f32;
        let mut local_punch_yaw = 0.0f32;

        let local_player_clone: Option<PlayerState> = match &self.session {
            SessionMode::Local { world, local_id } => world.player(*local_id).cloned(),
            SessionMode::Remote { client, snapshot: Some(s) } => {
                client.player_id.and_then(|id| s.players.iter().find(|p| p.id == id).cloned())
            }
            _ => None,
        };

        let all_players_slice: Vec<PlayerState> = match &self.session {
            SessionMode::Local { world, .. } => world.players.clone(),
            SessionMode::Remote { snapshot: Some(s), .. } => s.players.clone(),
            _ => Vec::new(),
        };

        if let Some(ref me) = local_player_clone {
            local_speed = Vec3::new(me.mv.velocity.x, me.mv.velocity.y, 0.0).length();
            local_punch_pitch = me.punch_pitch;
            local_punch_yaw = me.punch_yaw;

            // Zoom FOV adjustment for AWP / Scout / AUG / SG552
            if me.alive && me.zoom_level > 0 {
                let spec = me.active_weapon_id().spec();
                effective_fov = if spec.double_zoom {
                    if me.zoom_level == 1 { 40.0 } else { 15.0 }
                } else {
                    55.0
                };
            }

            // First-person weapon viewmodel (hidden when dead or inside sniper scope)
            let hide_for_scope = me.zoom_level > 0 && me.active_weapon_id().spec().double_zoom;
            if me.alive && self.cvars.get_bool("r_drawviewmodel") && !hide_for_scope {
                let wid = me.active_weapon_id();
                let spec = wid.spec();
                let bob_phase = time_sec * 9.0;
                let bob_amp = (local_speed / 250.0).min(1.0) * if me.mv.on_ground { 0.85 } else { 0.1 };
                let bob = Vec3::new(
                    -me.punch_pitch.abs() * 0.25,
                    (bob_phase * 0.5).sin() * bob_amp * 0.45,
                    bob_phase.sin().abs() * -bob_amp * 0.65 - 0.5,
                );
                let right_handed = self.cvars.get_bool("cl_righthand");
                // Note: GoldSrc CS 1.6 v_*.mdl are authored right-handed by default, mirror Y when cl_righthand == 0!
                let xf = viewmodel_transform(
                    self.camera.position,
                    self.camera.yaw_deg + local_punch_yaw,
                    self.camera.pitch_deg + local_punch_pitch,
                    bob,
                    right_handed,
                );
                let seq_hint = match me.view_anim {
                    ViewAnimKind::Idle => "idle",
                    ViewAnimKind::Draw => "draw",
                    ViewAnimKind::Shoot => "shoot",
                    ViewAnimKind::Reload => "reload",
                };
                let light = self
                    .loaded_bsp
                    .as_ref()
                    .map(|b| b.sample_light_point(me.mv.origin))
                    .unwrap_or(Vec3::splat(0.95));
                viewmodel_owned = Some((
                    spec.viewmodel_path.to_string(),
                    xf,
                    seq_hint.to_string(),
                    me.view_anim_time,
                    light,
                ));
            }
        }

        // Third-person player models (bots & other networked players) WITH `EF_BONEMERGE` `p_*.mdl` in hands!
        let local_id_opt = local_player_clone.as_ref().map(|p| p.id);
        for p in &all_players_slice {
            if !p.alive || Some(p.id) == local_id_opt {
                continue;
            }
            let spd = Vec3::new(p.mv.velocity.x, p.mv.velocity.y, 0.0).length();
            let w_spec = p.active_weapon_id().spec();
            let seq = if p.mv.ducking {
                if spd > 20.0 {
                    "crouchrun".to_string()
                } else {
                    format!("crouch_aim_{}", w_spec.anim_suffix)
                }
            } else if spd > 40.0 {
                "run".to_string()
            } else if p.attack_cooldown > 0.0 {
                format!("ref_shoot_{}", w_spec.anim_suffix)
            } else {
                format!("ref_aim_{}", w_spec.anim_suffix)
            };
            let xf = player_model_transform(p.mv.origin, p.yaw_deg, p.mv.ducking);
            let light = self
                .loaded_bsp
                .as_ref()
                .map(|b| b.sample_light_point(p.mv.origin))
                .unwrap_or(Vec3::splat(0.9));
            world_mdl_owned.push(OwnedWorldMdl {
                path: p.skin.model_path().to_string(),
                xf,
                seq,
                anim_time: time_sec,
                loop_anim: true,
                light,
                attached: Some(w_spec.p_model_path.to_string()),
            });
        }

        // Persistent round corpses (clamped Valve death sequences!)
        let corpses_slice: Vec<_> = match &self.session {
            SessionMode::Local { world, .. } => world.corpses.clone(),
            SessionMode::Remote { snapshot: Some(s), .. } => s.corpses.clone(),
            _ => Vec::new(),
        };
        for c in &corpses_slice {
            let xf = player_model_transform(c.position, c.yaw_deg, false);
            let light = self
                .loaded_bsp
                .as_ref()
                .map(|b| b.sample_light_point(c.position))
                .unwrap_or(Vec3::splat(0.85));
            world_mdl_owned.push(OwnedWorldMdl {
                path: c.skin.model_path().to_string(),
                xf,
                seq: c.seq_name.clone(),
                anim_time: c.anim_time,
                loop_anim: false,
                light,
                attached: None,
            });
        }

        // Hostages on `cs_*` maps (`models/hostage.mdl`)
        let hostages_slice: Vec<_> = match &self.session {
            SessionMode::Local { world, .. } => world.hostages.clone(),
            SessionMode::Remote { snapshot: Some(s), .. } => s.hostages.clone(),
            _ => Vec::new(),
        };
        for h in &hostages_slice {
            if !h.alive || h.rescued {
                continue;
            }
            let xf = player_model_transform(h.position, h.yaw_deg, false);
            let light = self
                .loaded_bsp
                .as_ref()
                .map(|b| b.sample_light_point(h.position))
                .unwrap_or(Vec3::splat(0.9));
            world_mdl_owned.push(OwnedWorldMdl {
                path: "models/hostage.mdl".to_string(),
                xf,
                seq: if h.moving { "run".into() } else { "idle1".into() },
                anim_time: time_sec,
                loop_anim: true,
                light,
                attached: None,
            });
        }

        // Dropped weapons on floor
        let dropped_slice: Vec<_> = match &self.session {
            SessionMode::Local { world, .. } => world.dropped_weapons.clone(),
            SessionMode::Remote { snapshot: Some(s), .. } => s.dropped_weapons.clone(),
            _ => Vec::new(),
        };
        for dw in &dropped_slice {
            let w_path = format!("models/w_{}.mdl", dw.id.spec().console_name);
            let light = self
                .loaded_bsp
                .as_ref()
                .map(|b| b.sample_light_point(dw.position))
                .unwrap_or(Vec3::splat(0.9));
            world_mdl_owned.push(OwnedWorldMdl {
                path: w_path,
                xf: Mat4::from_translation(dw.position),
                seq: "idle".to_string(),
                anim_time: 0.0,
                loop_anim: true,
                light,
                attached: None,
            });
        }

        // Collect transient UI interactions
        let mut pending_actions = Vec::new();
        let mut should_resume = false;
        let mut bought_item: Option<String> = None;
        let mut chosen_team_skin: Option<(SpawnTeam, PlayerModelSkin)> = None;
        let mut sent_chat: Option<(bool, String)> = None;

        // Track weapon switch timer for top bucket1..5 HUD fade
        self.weapon_switch_timer = (self.weapon_switch_timer - dt).max(0.0);
        if let Some(ref me) = local_player_clone {
            let wid = me.active_weapon_id();
            if self.last_weapon_id.is_some() && self.last_weapon_id != Some(wid) {
                self.weapon_switch_timer = 1.8;
            }
            self.last_weapon_id = Some(wid);
        }

        let paused = self.any_modal_open();

        let Some(rt) = &mut self.runtime else {
            return;
        };

        let raw_input = rt.egui_state.take_egui_input(&rt.window);
        rt.egui_ctx.begin_pass(raw_input);

        if let Some(ref me) = local_player_clone {
            let (phase, phase_timer, ct_score, t_score, planted_c4, killfeed, chat, banner, in_buy, in_bomb, in_rescue) =
                match &self.session {
                    SessionMode::Local { world, .. } => (
                        world.phase,
                        world.phase_timer,
                        world.ct_score,
                        world.t_score,
                        world.planted_c4.as_ref(),
                        world.killfeed.as_slice(),
                        world.chat.as_slice(),
                        world.banner.as_ref().map(|(s, _)| s.as_str()),
                        world.collision.is_in_buy_zone(me.mv.origin, me.team),
                        world.collision.is_in_bomb_target(me.mv.origin),
                        world.collision.is_in_hostage_rescue(me.mv.origin),
                    ),
                    SessionMode::Remote { snapshot: Some(s), .. } => (
                        s.phase,
                        s.phase_timer,
                        s.ct_score,
                        s.t_score,
                        s.planted_c4.as_ref(),
                        s.killfeed.as_slice(),
                        s.chat.as_slice(),
                        s.banner.as_ref().map(|(t, _)| t.as_str()),
                        true,
                        false,
                        false,
                    ),
                    _ => (
                        RoundPhase::Live,
                        0.0,
                        0,
                        0,
                        None,
                        &[][..],
                        &[][..],
                        None,
                        false,
                        false,
                        false,
                    ),
                };

            let spectate_target: Option<&PlayerState> = if !me.alive && !self.spectate_free {
                let mates: Vec<&PlayerState> = all_players_slice
                    .iter()
                    .filter(|p| p.alive && p.id != me.id && p.team == me.team)
                    .collect();
                let pool = if !mates.is_empty() {
                    mates
                } else {
                    all_players_slice.iter().filter(|p| p.alive && p.id != me.id).collect()
                };
                pool.get(self.spectate_index % pool.len().max(1)).copied()
            } else {
                None
            };

            let hide_crosshair = me.zoom_level > 0 && me.active_weapon_id().spec().double_zoom;
            if !paused && me.alive && !hide_crosshair {
                let dyn_gap = if self.cvars.get_bool("cl_dynamiccrosshair") {
                    (local_speed / 45.0) + (me.shots_fired.min(8) as f32) * 1.4
                } else {
                    0.0
                };
                draw_crosshair(&rt.egui_ctx, &self.cvars, dyn_gap);
            }

            draw_cs16_hud(
                &rt.egui_ctx,
                rt.hud_sprites.as_ref(),
                &HudState {
                    local: me,
                    all_players: &all_players_slice,
                    phase,
                    phase_timer,
                    in_buy_zone: in_buy,
                    in_bomb_zone: in_bomb,
                    in_rescue_zone: in_rescue,
                    planted_c4,
                    killfeed,
                    chat,
                    banner,
                    time_sec,
                    weapon_switch_timer: self.weapon_switch_timer,
                    spectate_target,
                    spectate_free: self.spectate_free,
                    radio_menu: self.radio_menu,
                },
            );

            let map_name = self
                .loaded_bsp
                .as_ref()
                .map(|m| m.name.as_str())
                .unwrap_or("");
            draw_net_graph(
                &rt.egui_ctx,
                &self.cvars,
                &FrameTelemetry {
                    fps: self.fps_smooth,
                    frame_ms: self.frame_ms_smooth,
                    map_name,
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
                    self.cvars.get_str("hostname"),
                    map_name,
                    ct_score,
                    t_score,
                    &all_players_slice,
                    me.id,
                    rt.vgui_bank.client_palette,
                );
            }

            if self.buy_ui.open {
                bought_item = self.buy_ui.show(
                    &rt.egui_ctx,
                    me.team,
                    me.money,
                    self.cvars.get_bool("_vgui_menus"),
                    self.vfs.as_ref(),
                    &mut rt.vgui_bank,
                );
            }
            if self.team_ui.open {
                chosen_team_skin = self.team_ui.show(
                    &rt.egui_ctx,
                    map_name,
                    self.vfs.as_ref(),
                    &mut rt.vgui_bank,
                );
            }
            if self.chat_ui.open {
                sent_chat = self.chat_ui.show(&rt.egui_ctx, self.palette);
            }
        }

        let steam_icon = rt.vgui_bank.steam_icon.as_ref();
        let menu_cmds = self.menu_ui.show(
            &rt.egui_ctx,
            &mut self.cvars,
            &self.available_maps,
            in_game,
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

        // Apply punchangle to rendered camera cleanly
        let mut render_cam = self.camera.clone();
        render_cam.pitch_deg = (render_cam.pitch_deg + local_punch_pitch).clamp(-89.0, 89.0);
        render_cam.yaw_deg = (render_cam.yaw_deg + local_punch_yaw).rem_euclid(360.0);

        let debug_mode = if self.cvars.get_bool("r_lightmap_only") {
            1.0
        } else if self.cvars.get_bool("r_fullbright") {
            2.0
        } else {
            0.0
        };
        let use_nearest = self
            .cvars
            .get_str("gl_texturemode")
            .eq_ignore_ascii_case("GL_NEAREST");

        let settings = RenderSettings {
            gamma: self.cvars.get_f32("gamma").clamp(1.0, 4.0),
            brightness: self.cvars.get_f32("brightness").clamp(0.3, 2.5),
            fov_deg: effective_fov,
            aspect_mode: self.cvars.get_str("vid_aspect_mode"),
            use_nearest_filter: use_nearest,
            debug_mode,
            time_sec,
        };

        let world_mdl_refs: Vec<MdlDrawInstance<'_>> = world_mdl_owned
            .iter()
            .map(|m| MdlDrawInstance {
                model_path: m.path.as_str(),
                transform: m.xf,
                sequence_hint: m.seq.as_str(),
                anim_time_sec: m.anim_time,
                loop_anim: m.loop_anim,
                light_rgb: m.light,
                flip_winding: false,
                attached_model: m.attached.as_deref(),
            })
            .collect();

        let vm_ref = viewmodel_owned
            .as_ref()
            .map(|(path, xf, seq, t, light)| MdlDrawInstance {
                model_path: path.as_str(),
                transform: *xf,
                sequence_hint: seq.as_str(),
                anim_time_sec: *t,
                loop_anim: true,
                light_rgb: *light,
                flip_winding: false,
                attached_model: None,
            });

        let (decals_vec, tracers_slice, grenades_slice, smokes_slice, planted_c4_ref, hidden_bsp_models) =
            match &self.session {
                SessionMode::Local { world, .. } => {
                    let mut hidden = world.broken_models.clone();
                    for &(mid, off) in &world.door_offsets {
                        if off.length_squared() > 16.0 {
                            hidden.push(mid);
                        }
                    }
                    (
                        world.decals.iter().cloned().collect::<Vec<_>>(),
                        world.tracers.as_slice(),
                        world.grenades.as_slice(),
                        world.smokes.as_slice(),
                        world.planted_c4.as_ref(),
                        hidden,
                    )
                }
                SessionMode::Remote { snapshot: Some(s), .. } => {
                    let mut hidden = s.broken_models.clone();
                    for &(mid, off) in &s.door_offsets {
                        if off.length_squared() > 16.0 {
                            hidden.push(mid);
                        }
                    }
                    (
                        Vec::new(),
                        s.tracers.as_slice(),
                        s.grenades.as_slice(),
                        s.smokes.as_slice(),
                        s.planted_c4.as_ref(),
                        hidden,
                    )
                }
                _ => (Vec::new(), &[][..], &[][..], &[][..], None, Vec::new()),
            };
        let mf_positions: Vec<Vec3> = self.muzzle_flashes.iter().map(|(p, _)| *p).collect();

        let scene_data = SceneDynamicData {
            vfs: self.vfs.as_ref(),
            world_models: &world_mdl_refs,
            viewmodel: vm_ref,
            decals: &decals_vec,
            tracers: tracers_slice,
            grenades: grenades_slice,
            smokes: smokes_slice,
            planted_c4: planted_c4_ref,
            muzzle_flashes: &mf_positions,
            hidden_bsp_models: &hidden_bsp_models,
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

        // Post-frame actions
        if let Some(item) = bought_item {
            self.perform_buy(&item);
        }
        if let Some((team, skin)) = chosen_team_skin {
            match &mut self.session {
                SessionMode::Local { world, local_id } => {
                    if let Some(p) = world.player_mut(*local_id) {
                        p.team = team;
                        p.skin = skin;
                    }
                    world.start_new_round();
                }
                SessionMode::Remote { client, .. } => {
                    let _ = client.send(&ClientPacket::ChooseTeam(team));
                }
                SessionMode::None => {}
            }
        }
        if let Some((team_only, text)) = sent_chat {
            self.send_chat(team_only, text);
        }

        if should_resume {
            self.menu_ui.visible = false;
            self.options_ui.open = false;
            self.console_ui.open = false;
            self.buy_ui.close();
            self.team_ui.open = false;
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
                    if enabled { 1 } else { 0 }
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
            match ui_mode.to_ascii_lowercase().as_str() {
                "buy" => self.buy_ui.open_menu(),
                "buy_rifles" | "rifles" => self.buy_ui.open_category(BuyCategory::Rifles),
                "buy_pistols" | "pistols" => self.buy_ui.open_category(BuyCategory::Pistols),
                "buy_mg" | "mg" => self.buy_ui.open_category(BuyCategory::MachineGuns),
                "buy_equipment" | "equipment" => self.buy_ui.open_category(BuyCategory::Equipment),
                "team" => self.team_ui.open_team_select(),
                "class_ct" | "class" => self.team_ui.open_class_select(SpawnTeam::CounterTerrorist),
                "class_t" => self.team_ui.open_class_select(SpawnTeam::Terrorist),
                "options" => self.options_ui.open = true,
                _ => {}
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
                    // When dead, Mouse1 cycles live spectator targets and Mouse2 toggles Free-Look!
                    let is_dead = match &self.session {
                        SessionMode::Local { world, local_id } => {
                            world.player(*local_id).is_some_and(|p| !p.alive)
                        }
                        SessionMode::Remote { client, snapshot: Some(s) } => client
                            .player_id
                            .and_then(|id| s.players.iter().find(|p| p.id == id))
                            .is_some_and(|p| !p.alive),
                        _ => false,
                    };
                    if is_dead && pressed {
                        match button {
                            MouseButton::Left => {
                                self.spectate_free = false;
                                self.spectate_index = self.spectate_index.wrapping_add(1);
                            }
                            MouseButton::Right => {
                                self.spectate_free = !self.spectate_free;
                            }
                            _ => {}
                        }
                        return;
                    }
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
            let mut sens = self.cvars.get_f32("sensitivity").max(0.05);
            // Scale sensitivity smoothly while scoped
            let zoom_level = match &self.session {
                SessionMode::Local { world, local_id } => {
                    world.player(*local_id).map(|p| p.zoom_level).unwrap_or(0)
                }
                SessionMode::Remote { client, snapshot: Some(s) } => client
                    .player_id
                    .and_then(|id| s.players.iter().find(|p| p.id == id))
                    .map(|p| p.zoom_level)
                    .unwrap_or(0),
                _ => 0,
            };
            if zoom_level == 1 {
                sens *= 0.45;
            } else if zoom_level >= 2 {
                sens *= 0.22;
            }
            let m_yaw = self.cvars.get_f32("m_yaw");
            let m_pitch = self.cvars.get_f32("m_pitch");
            self.camera
                .apply_mouse_delta(dx as f32, dy as f32, sens, m_yaw, m_pitch);
        }
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
