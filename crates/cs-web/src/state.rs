//! Platform-agnostic browser runtime state for `cs-web`: staged layered VFS mounting,
//! `ConfigStorage`, local (`LocalGame`) + remote (`ChannelTransport`) session management,
//! gesture-gated audio event queueing, and headless `FrameView` generation.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_client::camera::Camera;
use cs_client::input::{MoveSpeeds, weapon_for_slot};
use cs_client::render::scene::ModelRegistry;
use cs_client::ui::hud::{HudView, draw_cs16_hud};
use cs_client::ui::mod_hud::draw_mod_hud;
use cs_client::ui::scoreboard::{ScoreRow, show_scoreboard};
use cs_client::ui::server_menu::{draw_text_menu, menu_key_enabled, menu_lines};
use cs_client::ui::{FrameTelemetry, draw_crosshair, draw_net_graph, hud::HudSprites};
use cs_client::view::{FrameView, MapFxWorld, ViewBuilder, ViewInputs};
use cs_clientcore::hud::HudState;
use cs_clientcore::{
    ConnState, Connection, ConnectionConfig, ResourceFiles, SoundCommand, ViewAngleUpdate,
};
use cs_cvar::CVarRegistry;
use cs_local::{FRAME_SECS, LocalGame, LocalOptions};
use cs_mod_api::{HudDrawCmd, MOD_CAPABILITY_KEY};
use cs_mod_host::{ClientModHost, ClientModTransfer};
use cs_platform::{ConfigStorage, MemoryStorage};
use cs_pm::collision::WorldCollision;
use cs_proto48::crypto::{md5_digest, md5_hex};
use cs_proto48::info::InfoString;
use cs_transport::{ChannelHandle, ChannelTransport, PeerAddr};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

use crate::input_web::WebInputState;

const MAX_STEP: f64 = 0.1;

/// Plain audio event forwarded to the browser's `WebAudioMixer`.
#[derive(Debug, Clone, PartialEq)]
pub enum WebAudioEvent {
    Play2d {
        path: String,
        volume: f32,
        entity: u16,
        channel: u8,
    },
    Play3d {
        path: String,
        volume: f32,
        pos: [f32; 3],
        listener: [f32; 3],
        entity: u16,
        channel: u8,
    },
    StopChannel {
        entity: u16,
        channel: u8,
    },
    StopAll,
}

struct VfsResourceFiles(Vfs);

impl ResourceFiles for VfsResourceFiles {
    fn exists(&self, path: &str) -> bool {
        self.0.exists(path)
    }

    fn md5(&self, path: &str) -> Option<[u8; 16]> {
        self.0.read_bytes(path).ok().map(|b| md5_digest(&b))
    }

    fn studio_bounds(&self, path: &str) -> Option<([f32; 3], [f32; 3])> {
        self.0
            .read_bytes(path)
            .ok()
            .and_then(|b| cs_assets::mdl::studio_bounds(&b))
    }
}

pub enum SessionMode {
    Idle,
    Local {
        game: Box<LocalGame>,
        pending: f64,
    },
    Remote {
        conn: Box<Connection<ChannelTransport>>,
        handle: ChannelHandle,
        now: f64,
    },
}

/// Signal emitted when `WebClientState::step` detects the active map changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapSyncAction {
    None,
    Load(String),
    Unload,
}

/// Output of one simulation/view tick before GPU rendering.
pub struct StepOutput {
    pub view: Option<FrameView>,
    pub new_registry: Option<ModelRegistry>,
    pub map_sync: MapSyncAction,
}

/// Complete non-GPU web client state, shared between native tests and the `wasm32` canvas loop.
pub struct WebClientState {
    staged_files: Vec<(String, Vec<u8>)>,
    pub vfs: Option<Vfs>,
    pub storage: Box<dyn ConfigStorage>,
    pub cvars: CVarRegistry,
    pub camera: Camera,
    pub input: WebInputState,
    pub view: ViewBuilder,
    pub session: SessionMode,
    loaded_map: Option<String>,
    collision: Option<Arc<WorldCollision>>,
    audio_unlocked: bool,
    queued_audio: Vec<WebAudioEvent>,
    weapon_switch_timer: f32,
    time_sec: f32,
    pub mod_transfer: ClientModTransfer,
    pub mod_host: ClientModHost,
    last_mod_hud: Vec<HudDrawCmd>,
}

impl Default for WebClientState {
    fn default() -> Self {
        Self::new(Box::new(MemoryStorage::default()))
    }
}

impl WebClientState {
    pub fn new(storage: Box<dyn ConfigStorage>) -> Self {
        let mut cvars = CVarRegistry::new_with_defaults();
        if let Some(cfg) = storage.load("config.cfg") {
            let _ = cvars.execute_script(&cfg);
        }
        Self {
            staged_files: Vec::new(),
            vfs: None,
            storage,
            cvars,
            camera: Camera::default(),
            input: WebInputState::new(),
            view: ViewBuilder::new(),
            session: SessionMode::Idle,
            loaded_map: None,
            collision: None,
            audio_unlocked: false,
            queued_audio: Vec::new(),
            weapon_switch_timer: 0.0,
            time_sec: 0.0,
            mod_transfer: ClientModTransfer::default(),
            mod_host: ClientModHost::new(),
            last_mod_hud: Vec::new(),
        }
    }

    pub fn mount_file(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.staged_files.push((path.into(), bytes));
    }

    pub fn finish_mount(&mut self) {
        let entries = std::mem::take(&mut self.staged_files);
        if !entries.is_empty() {
            self.vfs = Some(Vfs::from_layered_memory(entries, VfsConfig::default()));
        }
    }

    pub fn set_vfs(&mut self, vfs: Vfs) {
        self.vfs = Some(vfs);
    }

    pub fn list_maps(&self) -> Vec<String> {
        self.vfs.as_ref().map_or_else(Vec::new, Vfs::list_maps)
    }

    pub fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.vfs.as_ref()?.read_bytes(path).ok()
    }

    pub fn is_audio_unlocked(&self) -> bool {
        self.audio_unlocked
    }

    pub fn unlock_audio(&mut self) {
        self.audio_unlocked = true;
    }

    pub fn start_local(&mut self, map: &str, bots: u32) -> Result<(), String> {
        let vfs = self
            .vfs
            .as_ref()
            .ok_or_else(|| "no Half-Life / cstrike folder mounted".to_string())?
            .clone();
        let name = self.cvars.get_str("name").to_string();
        let opts = LocalOptions {
            bots,
            player_name: if name.is_empty() {
                "Player".to_string()
            } else {
                name
            },
            text_menus: true,
            ..LocalOptions::default()
        };
        let mut game = LocalGame::start(&vfs, map, &opts)?;
        let names = self.view.names();
        let coll = BspMap::load(&vfs, map)
            .ok()
            .map(|b| Arc::new(WorldCollision::from_bsp(&b)));
        self.collision = coll.clone();
        self.loaded_map = None;
        self.view.reset_map();
        self.view.set_map_bounds(coll.as_deref());
        self.view.set_world_context(coll.clone(), Some(vfs.clone()));
        let conn = game.connection_mut();
        conn.frames_mut().set_remote_is_local(true);
        conn.set_world_collision(coll.clone());
        conn.set_fx_world(Box::new(MapFxWorld::new(coll, Some(vfs), names)));
        self.session = SessionMode::Local {
            game: Box::new(game),
            pending: 0.0,
        };
        Ok(())
    }

    pub fn connect_remote(&mut self) -> Result<(), String> {
        self.disconnect();
        let (transport, handle) = ChannelTransport::new_pair(PeerAddr::Web(0));
        let name = {
            let n = self.cvars.get_str("name");
            if n.is_empty() { "Player" } else { n }
        };
        let steam_id = 76_561_197_960_265_728u64;
        let cfg = ConnectionConfig::new(name, steam_id, &md5_hex(b"cs16-rs web"));
        let mut conn = Connection::new(transport, PeerAddr::Web(0), cfg);
        if let Some(vfs) = &self.vfs {
            conn.set_files(Box::new(VfsResourceFiles(vfs.clone())));
        }
        conn.set_userinfo("_vgui_menus", "0");
        conn.set_userinfo(MOD_CAPABILITY_KEY, "1");
        let names = self.view.names();
        conn.set_fx_world(Box::new(MapFxWorld::new(None, self.vfs.clone(), names)));
        conn.connect();
        self.loaded_map = None;
        self.collision = None;
        self.view.reset_map();
        self.session = SessionMode::Remote {
            conn: Box::new(conn),
            handle,
            now: 0.0,
        };
        Ok(())
    }

    pub fn disconnect(&mut self) {
        match &mut self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, .. } => game.shutdown(),
            SessionMode::Remote { conn, handle, .. } => {
                conn.execute_local("disconnect");
                handle.close();
            }
        }
        self.session = SessionMode::Idle;
        self.loaded_map = None;
        self.collision = None;
        self.view.reset_map();
        self.mod_transfer = ClientModTransfer::default();
        self.mod_host = ClientModHost::new();
        self.last_mod_hud.clear();
        self.queued_audio.push(WebAudioEvent::StopAll);
    }

    pub fn push_net_packet(&mut self, bytes: Vec<u8>) {
        if let SessionMode::Remote { handle, .. } = &self.session {
            let _ = handle.push_inbound(bytes);
        }
    }

    pub fn take_net_packets(&mut self) -> Vec<Vec<u8>> {
        if let SessionMode::Remote { handle, .. } = &self.session {
            handle.drain_outbound()
        } else {
            Vec::new()
        }
    }

    pub fn take_audio_events(&mut self) -> Vec<WebAudioEvent> {
        std::mem::take(&mut self.queued_audio)
    }

    pub fn conn_state(&self) -> Option<ConnState> {
        match &self.session {
            SessionMode::Idle => None,
            SessionMode::Local { game, .. } => Some(game.connection().state().clone()),
            SessionMode::Remote { conn, .. } => Some(conn.state().clone()),
        }
    }

    pub fn loaded_map(&self) -> Option<&str> {
        self.loaded_map.as_deref()
    }

    pub fn mark_map_loaded(&mut self, map_name: &str) {
        self.loaded_map = Some(map_name.to_string());
        if let Some(vfs) = &self.vfs
            && let Ok(bsp) = BspMap::load(vfs, map_name)
        {
            let coll = Arc::new(WorldCollision::from_bsp(&bsp));
            self.collision = Some(coll.clone());
            self.view.set_map_bounds(Some(coll.as_ref()));
            self.view.set_world_context(Some(coll.clone()), Some(vfs.clone()));
            let names = self.view.names();
            let vfs_c = vfs.clone();
            match &mut self.session {
                SessionMode::Idle => {}
                SessionMode::Local { game, .. } => {
                    let c = game.connection_mut();
                    c.set_world_collision(Some(coll.clone()));
                    c.set_fx_world(Box::new(MapFxWorld::new(Some(coll), Some(vfs_c), names)));
                }
                SessionMode::Remote { conn, .. } => {
                    conn.set_world_collision(Some(coll.clone()));
                    conn.set_fx_world(Box::new(MapFxWorld::new(Some(coll), Some(vfs_c), names)));
                }
            }
        }
    }

    pub fn exec_console(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        if let Some(rest) = trimmed.strip_prefix('+') {
            self.input.actions.insert(rest.to_string());
            return;
        }
        if let Some(rest) = trimmed.strip_prefix('-') {
            self.input.actions.remove(rest);
            return;
        }
        if let Some(id) = trimmed
            .strip_prefix("mod_approve ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.mod_transfer.approve(id);
            return;
        }
        if let Some(id) = trimmed
            .strip_prefix("mod_deny ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.mod_transfer.deny(id);
            return;
        }
        let _ = self.cvars.execute_script(trimmed);
        match &mut self.session {
            SessionMode::Local { game, .. } => {
                let _ = game.console(trimmed);
                game.connection_mut().execute_local(trimmed);
            }
            SessionMode::Remote { conn, .. } => {
                conn.execute_local(trimmed);
            }
            SessionMode::Idle => {}
        }
        self.save_config();
    }

    pub fn save_config(&mut self) {
        let mut lines = String::new();
        for name in ["name", "sensitivity", "cl_righthand", "gamma", "brightness"] {
            let val = self.cvars.get_str(name);
            lines.push_str(&format!("{name} \"{val}\"\n"));
        }
        let _ = self.storage.save("config.cfg", &lines);
    }

    /// Advances the active session by `dt_secs`, runs usercmds, queues audio events, and emits `FrameView`.
    pub fn step(&mut self, dt_secs: f64) -> StepOutput {
        let dt = dt_secs.clamp(0.0, MAX_STEP);
        self.time_sec += dt as f32;
        self.weapon_switch_timer = (self.weapon_switch_timer - dt as f32).max(0.0);

        let interp = match &self.session {
            SessionMode::Idle => 0.1,
            SessionMode::Local { game, .. } => game.connection().frames().settings.interp,
            SessionMode::Remote { conn, .. } => conn.frames().settings.interp,
        };
        let right_hand = self.cvars.get_bool("cl_righthand");
        let draw_viewmodel = self.cvars.get_bool("r_drawviewmodel");

        let pending_cmds = std::mem::take(&mut self.input.pending_commands);
        for cmd in pending_cmds {
            self.dispatch_command(&cmd);
        }

        let cmd = self
            .input
            .build_usercmd(&mut self.camera, dt, interp, &MoveSpeeds::default(), 180.0);

        match &mut self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, pending } => {
                if game.connection().state() == &ConnState::InGame {
                    game.connection_mut().queue_cmd(cmd);
                }
                *pending += dt;
                while *pending >= FRAME_SECS {
                    game.step();
                    *pending -= FRAME_SECS;
                }
            }
            SessionMode::Remote { conn, now, .. } => {
                if conn.state() == &ConnState::InGame {
                    conn.queue_cmd(cmd);
                }
                *now += dt;
                conn.pump(*now);
            }
        }

        let server_map = match &self.session {
            SessionMode::Idle => None,
            SessionMode::Local { game, .. } => game
                .connection()
                .precache()
                .world_model()
                .map(normalize_map_name),
            SessionMode::Remote { conn, .. } => {
                conn.precache().world_model().map(normalize_map_name)
            }
        };

        let map_sync = match (server_map, self.loaded_map.as_deref()) {
            (Some(srv), Some(cur)) if srv == cur => MapSyncAction::None,
            (Some(srv), _) => MapSyncAction::Load(srv),
            (None, Some(_)) if matches!(self.session, SessionMode::Idle) => MapSyncAction::Unload,
            _ => MapSyncAction::None,
        };

        let sess_now = match &self.session {
            SessionMode::Idle => 0.0,
            SessionMode::Local { game, pending } => game.time() + *pending,
            SessionMode::Remote { now, .. } => *now,
        };

        let default_fov = self.cvars.get_f32("default_fov").clamp(60.0, 120.0);
        let inputs = ViewInputs {
            now: sess_now,
            frametime: dt,
            right_hand,
            draw_viewmodel,
            default_fov,
            viewmodel_fov: self.cvars.get_f32("viewmodel_fov").clamp(50.0, 120.0),
            cl_bob: self.cvars.get_f32("cl_bob"),
            cl_bobcycle: self.cvars.get_f32("cl_bobcycle"),
            cl_bobup: self.cvars.get_f32("cl_bobup"),
            cl_rollangle: self.cvars.get_f32("cl_rollangle"),
            cl_rollspeed: self.cvars.get_f32("cl_rollspeed"),
            cl_viewmodel_lag: self.cvars.get_f32("cl_viewmodel_lag"),
        };
        let view_angles = Vec3::new(self.camera.pitch_deg, self.camera.yaw_deg, 0.0);

        let mut built_view = None;
        let mut new_registry = None;
        let mut drained_sounds = Vec::new();
        let mut angle_updates = Vec::new();
        let mut unknown_msgs = Vec::new();
        let mut mod_hud_stats = (100i32, 0i32, 0i32);
        let mut local_entity = None;

        match &mut self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, .. } => {
                let conn = game.connection_mut();
                if conn.state() == &ConnState::InGame {
                    let (fv, reg) = self.view.build(conn, view_angles, &inputs);
                    built_view = Some(fv);
                    new_registry = reg;
                }
                local_entity = conn.player_slot().map(|s| u16::from(s) + 1);
                drained_sounds = conn.fx_mut().sounds.drain();
                unknown_msgs = conn.fx_mut().hud.unknown_messages.drain(..).collect();
                mod_hud_stats = (
                    i32::from(conn.fx().hud.health),
                    i32::from(conn.fx().hud.armor),
                    conn.fx().hud.money,
                );
                angle_updates = conn.take_view_angle_updates();
            }
            SessionMode::Remote { conn, .. } => {
                if conn.state() == &ConnState::InGame {
                    let (fv, reg) = self.view.build(&mut **conn, view_angles, &inputs);
                    built_view = Some(fv);
                    new_registry = reg;
                }
                local_entity = conn.player_slot().map(|s| u16::from(s) + 1);
                drained_sounds = conn.fx_mut().sounds.drain();
                unknown_msgs = conn.fx_mut().hud.unknown_messages.drain(..).collect();
                mod_hud_stats = (
                    i32::from(conn.fx().hud.health),
                    i32::from(conn.fx().hud.armor),
                    conn.fx().hud.money,
                );
                angle_updates = conn.take_view_angle_updates();
            }
        }

        for um in unknown_msgs {
            if !self
                .mod_transfer
                .ingest_usermsg(&um.name, &um.payload, &mut self.mod_host, self.vfs.as_mut())
            {
                self.mod_host.on_usermsg(&um.name, &um.payload);
            }
        }
        let _ = self.mod_transfer.take_errors();
        for cmd in self.mod_transfer.take_outgoing_commands() {
            self.send_stringcmd(&cmd);
        }
        for cmd in self.mod_host.take_execs() {
            self.send_stringcmd(&cmd);
        }
        self.last_mod_hud = if matches!(self.session, SessionMode::Idle) {
            Vec::new()
        } else {
            self.mod_host.render_hud(
                (dt * 1000.0).round() as i32,
                mod_hud_stats.0,
                mod_hud_stats.1,
                mod_hud_stats.2,
            )
        };

        for upd in angle_updates {
            match upd {
                ViewAngleUpdate::Set(v) => {
                    if v.x.is_finite() {
                        self.camera.pitch_deg = v.x.clamp(-89.0, 89.0);
                    }
                    if v.y.is_finite() {
                        self.camera.yaw_deg = v.y.rem_euclid(360.0);
                    }
                }
                ViewAngleUpdate::AddYaw(yaw) => {
                    if yaw.is_finite() {
                        self.camera.yaw_deg = (self.camera.yaw_deg + yaw).rem_euclid(360.0);
                    }
                }
            }
        }

        if let Some(fv) = &built_view
            && fv.eye.is_finite()
        {
            self.camera.position = fv.eye;
        }

        let listener = self.camera.position.to_array();
        for sc in drained_sounds {
            self.push_sound_event(sc, listener, local_entity);
        }
        if self.audio_unlocked
            && let Some(fv) = &built_view
        {
            for path in &fv.anim_sounds {
                self.queued_audio.push(WebAudioEvent::Play2d {
                    path: path.clone(),
                    volume: 0.8,
                    entity: local_entity.unwrap_or(1),
                    channel: 1,
                });
            }
        }

        StepOutput {
            view: built_view,
            new_registry,
            map_sync,
        }
    }

    fn dispatch_command(&mut self, cmd: &str) {
        if let Some(digit) = parse_slot_cmd(cmd) {
            let menu_open = self
                .with_hud(|h| h.menu.as_ref().is_some_and(|m| m.keys != 0))
                .unwrap_or(false);
            if menu_open {
                let valid = self
                    .with_hud(|h| h.menu.as_ref().is_some_and(|m| menu_key_enabled(m.keys, digit)))
                    .unwrap_or(false);
                if valid {
                    self.send_stringcmd(&format!("menuselect {digit}"));
                    self.clear_text_menu();
                    return;
                }
            }
            if digit >= 1 {
                let owned: HashSet<u8> = {
                    let bits = self.owned_weapons();
                    (0u8..32).filter(|id| bits & (1 << id) != 0).collect()
                };
                let picked = self
                    .with_hud(|hud| {
                        weapon_for_slot(
                            &hud.weapons,
                            &owned,
                            hud.current_weapon.map(|c| c.weapon_id),
                            digit,
                        )
                        .map(str::to_string)
                    })
                    .flatten();
                if let Some(wname) = picked {
                    self.send_stringcmd(&wname);
                    self.weapon_switch_timer = 0.6;
                }
            }
            return;
        }
        self.send_stringcmd(cmd);
    }

    fn clear_text_menu(&mut self) {
        match &mut self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, .. } => game.connection_mut().fx_mut().hud.menu = None,
            SessionMode::Remote { conn, .. } => conn.fx_mut().hud.menu = None,
        }
    }

    fn send_stringcmd(&mut self, cmd: &str) {
        match &mut self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, .. } => game.connection_mut().send_stringcmd(cmd),
            SessionMode::Remote { conn, .. } => conn.send_stringcmd(cmd),
        }
    }

    fn owned_weapons(&self) -> u32 {
        match &self.session {
            SessionMode::Idle => 0,
            SessionMode::Local { game, .. } => {
                game.connection().client_data().map_or(0, |d| d.weapons)
            }
            SessionMode::Remote { conn, .. } => conn.client_data().map_or(0, |d| d.weapons),
        }
    }

    fn with_hud<R>(&self, f: impl FnOnce(&HudState) -> R) -> Option<R> {
        match &self.session {
            SessionMode::Idle => None,
            SessionMode::Local { game, .. } => Some(f(&game.connection().fx().hud)),
            SessionMode::Remote { conn, .. } => Some(f(&conn.fx().hud)),
        }
    }

    fn push_sound_event(&mut self, sc: SoundCommand, listener: [f32; 3], local_entity: Option<u16>) {
        if !self.audio_unlocked {
            return;
        }
        match sc {
            SoundCommand::Start(start) | SoundCommand::Ambient(start) => {
                if start.is_stop() {
                    self.queued_audio.push(WebAudioEvent::StopChannel {
                        entity: start.entity,
                        channel: start.channel,
                    });
                    return;
                }
                let Some(path) = self.resolve_sound_name(&start) else {
                    return;
                };
                if local_entity == Some(start.entity) || start.attenuation <= 0.0 {
                    self.queued_audio.push(WebAudioEvent::Play2d {
                        path,
                        volume: start.volume,
                        entity: start.entity,
                        channel: start.channel,
                    });
                } else {
                    self.queued_audio.push(WebAudioEvent::Play3d {
                        path,
                        volume: start.volume,
                        pos: start.origin.to_array(),
                        listener,
                        entity: start.entity,
                        channel: start.channel,
                    });
                }
            }
            SoundCommand::Stop { entity, channel } => {
                self.queued_audio
                    .push(WebAudioEvent::StopChannel { entity, channel });
            }
            SoundCommand::Fade { .. } => {}
        }
    }

    fn resolve_sound_name(&self, start: &cs_clientcore::SoundStart) -> Option<String> {
        if let Some(name) = &start.name {
            return Some(name.clone());
        }
        let precache = match &self.session {
            SessionMode::Idle => return None,
            SessionMode::Local { game, .. } => game.connection().precache(),
            SessionMode::Remote { conn, .. } => conn.precache(),
        };
        precache.sounds.get(&start.index).map(|e| e.name.clone())
    }

    /// Renders HUD, crosshair, net_graph, server text menus, and scoreboard into `ctx`.
    pub fn draw_hud_ui(
        &self,
        ctx: &egui::Context,
        hud_sprites: Option<&HudSprites>,
        client_palette: cs_client::ui::vgui_assets::ClientPalette,
        vgui_palette: cs_client::ui::vgui_theme::VguiPalette,
        view: Option<&FrameView>,
    ) {
        let Some(fv) = view else {
            return;
        };
        let scoped = fv.fov.is_finite() && fv.fov > 0.0 && fv.fov <= 40.0;
        if fv.alive && !scoped {
            draw_crosshair(ctx, &self.cvars, 0.0);
        }

        let (names, owned, sess_now) = match &self.session {
            SessionMode::Idle => (BTreeMap::new(), HashSet::new(), 0.0),
            SessionMode::Local { game, pending } => {
                let names = extract_names(game.connection().userinfos());
                let owned = owned_set(game.connection().client_data().map_or(0, |d| d.weapons));
                (names, owned, game.time() + *pending)
            }
            SessionMode::Remote { conn, now, .. } => {
                let names = extract_names(conn.userinfos());
                let owned = owned_set(conn.client_data().map_or(0, |d| d.weapons));
                (names, owned, *now)
            }
        };

        self.with_hud(|h| {
            let hv = HudView {
                hud: h,
                now: sess_now,
                time_sec: self.time_sec,
                names: &names,
                owned: &owned,
                feed: &[],
                chat: &[],
                notify: &[],
                center: None,
                weapon_switch_timer: self.weapon_switch_timer,
                alive: fv.alive,
                scoped,
                eye: fv.eye,
                yaw_deg: self.camera.yaw_deg,
                underwater_tint: fv.underwater_tint,
            };
            draw_cs16_hud(ctx, hud_sprites, &hv);
            draw_mod_hud(ctx, &self.last_mod_hud);

            if let Some(m) = h.menu.as_ref().filter(|m| !m.multipart && m.keys != 0 && !m.text.is_empty()) {
                let lines = menu_lines(&m.text, None);
                let scale = (ctx.content_rect().height() / 768.0).clamp(0.85, 1.45);
                draw_text_menu(ctx, &lines, scale);
            }

            if self.input.actions.contains("showscores") {
                let rows: Vec<ScoreRow> = h
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
                let ct_score = h.team_scores.get("CT").copied().unwrap_or(0);
                let t_score = h.team_scores.get("TERRORIST").copied().unwrap_or(0);
                let map_name = self.loaded_map.as_deref().unwrap_or("");
                show_scoreboard(
                    ctx,
                    &h.server_name,
                    map_name,
                    ct_score,
                    t_score,
                    &rows,
                    None,
                    client_palette,
                );
            }
        });

        draw_net_graph(
            ctx,
            &self.cvars,
            &FrameTelemetry {
                fps: 0.0,
                frame_ms: 0.0,
                map_name: self.loaded_map.as_deref().unwrap_or(""),
                pos: self.camera.position.to_array(),
                yaw_deg: self.camera.yaw_deg,
                pitch_deg: self.camera.pitch_deg,
                speed: 0.0,
                draw_calls: 0,
                triangles: 0,
            },
            vgui_palette,
        );
    }
}

fn parse_slot_cmd(cmd: &str) -> Option<u8> {
    match cmd {
        "slot10" => Some(0),
        _ => cmd
            .strip_prefix("slot")
            .and_then(|s| s.parse::<u8>().ok())
            .filter(|n| (1..=9).contains(n)),
    }
}

fn normalize_map_name(world_model: &str) -> String {
    world_model
        .trim_start_matches("maps/")
        .trim_end_matches(".bsp")
        .to_string()
}

fn extract_names(userinfos: &BTreeMap<u8, String>) -> BTreeMap<u8, String> {
    userinfos
        .iter()
        .filter_map(|(slot, raw)| {
            InfoString::parse(raw)
                .get("name")
                .map(|n| (*slot, n.to_string()))
        })
        .collect()
}

fn owned_set(bits: u32) -> HashSet<u8> {
    (0u8..32).filter(|id| bits & (1 << id) != 0).collect()
}

#[cfg(test)]
mod tests {
    use cs_engine::{Server, ServerConfig, ServerMap};
    use cs_proto48::delta::DeltaRegistry;
    use cs_sim::pmove::MoveParams;
    use cs_sim::walk_game::WalkGame;

    use super::*;

    #[test]
    fn layered_browser_vfs_mount_and_map_discovery() {
        let mut st = WebClientState::default();
        st.mount_file("Half-Life/valve/config.cfg", b"valve_line\n".to_vec());
        st.mount_file("Half-Life/cstrike/config.cfg", b"cstrike_line\n".to_vec());
        st.mount_file("Half-Life/cstrike/maps/de_dust2.bsp", vec![30, 0, 0, 0]);
        st.finish_mount();

        assert_eq!(st.read_file("config.cfg").as_deref(), Some(b"cstrike_line\n".as_slice()));
        assert_eq!(st.list_maps(), vec!["de_dust2".to_string()]);
    }

    #[test]
    fn audio_events_gated_until_user_gesture() {
        let mut st = WebClientState::default();
        assert!(!st.is_audio_unlocked());
        let start = cs_clientcore::SoundStart::by_name("weapons/ak47-1.wav", 1, 1, Vec3::ZERO, 1.0, 0.8, 100);
        st.push_sound_event(SoundCommand::Start(start.clone()), [0.0; 3], Some(1));
        assert!(st.take_audio_events().is_empty());

        st.unlock_audio();
        assert!(st.is_audio_unlocked());
        st.push_sound_event(SoundCommand::Start(start), [0.0; 3], Some(1));
        let evs = st.take_audio_events();
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], WebAudioEvent::Play2d { .. }));
    }

    #[test]
    fn remote_channel_session_completes_protocol48_signon() {
        let mut server = Server::new(
            ServerConfig {
                max_clients: 4,
                ..ServerConfig::default()
            },
            ServerMap::stub("de_dust2"),
            WalkGame::new(MoveParams::default()),
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        let (mut srv_tr, srv_handle) = ChannelTransport::new_pair(PeerAddr::Web(0));

        let mut st = WebClientState::default();
        st.connect_remote().unwrap();

        let mut in_game = false;
        let mut now = 0.0;
        for _ in 0..400 {
            now += 0.01;
            for pkt in st.take_net_packets() {
                let _ = srv_handle.push_inbound(pkt);
            }
            server.frame(&mut srv_tr, now);
            for pkt in srv_handle.drain_outbound() {
                st.push_net_packet(pkt);
            }
            let _ = st.step(0.01);
            if st.conn_state() == Some(ConnState::InGame) {
                in_game = true;
                break;
            }
        }
        assert!(in_game, "web ChannelTransport client should reach ConnState::InGame");
    }

    #[test]
    fn local_game_with_bots_runs_headless_on_real_assets() {
        let Ok(real_vfs) = Vfs::discover(None, VfsConfig::default()) else {
            eprintln!("SKIPPED: no game directory (set CS16_GAMEDIR)");
            return;
        };
        let mut st = WebClientState::default();
        st.set_vfs(real_vfs);
        st.start_local("de_dust2", 2).expect("start local game in cs-web");
        assert_eq!(st.conn_state(), Some(ConnState::InGame));
        let out = st.step(0.01);
        assert_eq!(out.map_sync, MapSyncAction::Load("de_dust2".to_string()));
        st.mark_map_loaded("de_dust2");
        let out2 = st.step(0.01);
        assert!(out2.view.is_some());
    }
}
