//! Platform-agnostic browser runtime state for `cs-web`: staged layered VFS mounting,
//! `ConfigStorage`, local (`LocalGame`) + remote (`ChannelTransport`) session management,
//! gesture-gated audio event queueing, and headless `FrameView` generation.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_assets::demo_pack::{DEMO_MAP_NAME, build_instant_demo_pack};
use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::{ControlKind, VguiCoordMode};
use cs_client::audio::mixer::{SND_FL_CHANGE_PITCH, SND_FL_CHANGE_VOL};
use cs_client::audio::{
    MusicCommand, MusicController, SentenceTable, SoundFadeState, VoiceManager,
    normalize_sound_path, parse_wav_loop_points, reload_sound_path,
};
use cs_client::camera::Camera;
use cs_client::input::{MoveSpeeds, step_inventory_cycle_mode};
use cs_client::render::scene::ModelRegistry;
use cs_client::ui::hud::{
    HudAnimState, HudExtrasContext, HudView, compute_hud_visibility, draw_cs16_hud_with_extras,
    draw_voice_speakers,
};
use cs_client::ui::hud_extras::{WeaponBucketSound, WeaponBucketState};
use cs_client::ui::mod_hud::draw_mod_hud;
use cs_client::ui::scoreboard::{ScoreRow, show_scoreboard};
use cs_client::ui::screens::buy_team_class::{
    BuyClassTeamMenuUi, BuyMenuRenderCtx, VguiMenuKind, find_button_by_hotkey,
};
use cs_client::ui::server_menu::{draw_styled_text_menu, menu_key_enabled, menu_styled_lines};
use cs_client::ui::vgui2::VguiRuntime;
use cs_client::ui::{
    CrosshairInputs, CrosshairState, FrameTelemetry, draw_crosshair_vgui_dt, draw_net_graph,
    hud::HudSprites, should_draw_dynamic_crosshair,
};
use cs_client::view::{FrameView, MapFxWorld, ViewBuilder, ViewInputs};
use cs_clientcore::hud::HudState;
use cs_clientcore::sound::{PITCH_NORM, SoundStart};
use cs_clientcore::{
    ConnState, Connection, ConnectionConfig, ResourceFiles, SoundCommand, ViewAngleUpdate,
    VoiceEvent,
};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_local::{FRAME_SECS, LocalGame, LocalOptions};
use cs_mod_api::{HudDrawCmd, MOD_CAPABILITY_KEY};
use cs_mod_host::{ClientModHost, ClientModTransfer};
use cs_platform::{ConfigStorage, MemoryStorage};
use cs_pm::collision::{Hull, WorldCollision};
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
        pitch: u8,
        entity: u16,
        channel: u8,
        loop_start: Option<u32>,
        loop_end: Option<u32>,
        delay_sec: f32,
    },
    Play3d {
        path: String,
        volume: f32,
        pitch: u8,
        attenuation: f32,
        pos: [f32; 3],
        listener: [f32; 3],
        right: [f32; 3],
        occluded: bool,
        entity: u16,
        channel: u8,
        loop_start: Option<u32>,
        loop_end: Option<u32>,
        delay_sec: f32,
    },
    UpdateChannel {
        entity: u16,
        channel: u8,
        volume: Option<f32>,
        pitch: Option<u8>,
        pos: Option<[f32; 3]>,
    },
    StopChannel {
        entity: u16,
        channel: u8,
    },
    StopAll,
    SetRoomType(u16),
    SetUnderwater(bool),
    SetFade {
        gain: f32,
    },
    PlayMusic {
        path: String,
        looping: bool,
        volume: f32,
    },
    SetMusicVolume(f32),
    StopMusic,
    VoicePcm {
        speaker: u8,
        sample_rate: u32,
        samples: Vec<f32>,
        volume: f32,
    },
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
    sentences: SentenceTable,
    sound_fade: SoundFadeState,
    music: MusicController,
    voice: VoiceManager,
    last_room_type: u16,
    last_underwater: bool,
    last_fade_gain: f32,
    weapon_buckets: WeaponBucketState,
    crosshair_state: CrosshairState,
    crosshair_shots_fired: i32,
    crosshair_last_weapon: u8,
    crosshair_last_clip: i32,
    last_dt: f32,
    loc: Option<LocalizationTable>,
    vgui_rt: VguiRuntime,
    buy_team_class_ui: BuyClassTeamMenuUi,
    hud_anim: HudAnimState,
    time_sec: f32,
    pub mod_transfer: ClientModTransfer,
    pub mod_host: ClientModHost,
    last_mod_hud: Vec<HudDrawCmd>,
    p2p_host: bool,
}

#[derive(Clone, Copy)]
struct VoxSpatial {
    pos: [f32; 3],
    listener: [f32; 3],
    right: [f32; 3],
    attenuation: f32,
    entity: u16,
    channel: u8,
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
            sentences: SentenceTable::default(),
            sound_fade: SoundFadeState::default(),
            music: MusicController::new(),
            voice: VoiceManager::new(),
            last_room_type: 0,
            last_underwater: false,
            last_fade_gain: 1.0,
            weapon_buckets: WeaponBucketState::default(),
            crosshair_state: CrosshairState::default(),
            crosshair_shots_fired: 0,
            crosshair_last_weapon: 0,
            crosshair_last_clip: 0,
            last_dt: 0.016,
            loc: None,
            vgui_rt: VguiRuntime::new(),
            buy_team_class_ui: BuyClassTeamMenuUi::default(),
            hud_anim: HudAnimState::default(),
            time_sec: 0.0,
            mod_transfer: ClientModTransfer::default(),
            mod_host: ClientModHost::new(),
            last_mod_hud: Vec::new(),
            p2p_host: false,
        }
    }

    pub fn mount_file(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.staged_files.push((path.into(), bytes));
    }

    pub fn finish_mount(&mut self) {
        let entries = std::mem::take(&mut self.staged_files);
        if !entries.is_empty() {
            let vfs = Vfs::from_layered_memory(entries, VfsConfig::default());
            self.load_sentences(&vfs);
            self.vfs = Some(vfs);
        }
    }

    pub fn set_vfs(&mut self, vfs: Vfs) {
        self.load_sentences(&vfs);
        self.vfs = Some(vfs);
    }

    /// Generates and mounts the clean-room `dm_rusthouse` Instant Demo asset pack as a
    /// lowest-priority fallback layer (never shadowing already-mounted Steam `cstrike`/`valve`
    /// assets). Returns `(file_count, total_bytes)`.
    pub fn mount_instant_demo(&mut self) -> (usize, usize) {
        let pack = build_instant_demo_pack();
        let summary = (pack.file_count(), pack.total_bytes());
        match self.vfs.as_mut() {
            Some(vfs) => {
                pack.mount_as_fallback(vfs);
            }
            None => {
                let vfs = pack.to_vfs();
                self.load_sentences(&vfs);
                self.vfs = Some(vfs);
            }
        }
        summary
    }

    /// Mounts the clean-room `dm_rusthouse` demo pack as a lowest-priority fallback and
    /// immediately launches a local ZBot match (e.g. `bots = 9` for 5v5) with auto-join into
    /// active gameplay.
    pub fn start_instant_demo(&mut self, bots: u32) -> Result<(usize, usize), String> {
        let summary = self.mount_instant_demo();
        self.start_local(DEMO_MAP_NAME, bots)?;
        if let SessionMode::Local { game, .. } = &mut self.session {
            game.connection_mut().execute_local("jointeam 2");
            game.run_frames(4);
            game.connection_mut().execute_local("joinclass 1");
            game.run_frames(8);
            let hud = &mut game.connection_mut().fx_mut().hud;
            hud.menu = None;
            hud.vgui_menu = None;
        }
        Ok(summary)
    }

    fn load_sentences(&mut self, vfs: &Vfs) {
        if let Ok(text) = vfs.read_string_lossy("sound/sentences.txt") {
            self.sentences = SentenceTable::parse(&text);
        }
        let lang = self.cvars.get_str("cl_language").to_string();
        self.loc = Some(LocalizationTable::load_language(vfs, &lang));
        self.vgui_rt.invalidate_cache();
        self.vgui_rt.load_scheme_fonts(vfs);
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
        self.start_local_mode(map, bots, 16, false)
    }

    /// Starts an in-browser listen server supporting the local host player plus external
    /// WebRTC DataChannel peers (`1..max_players`, clamped to `2..=16`).
    pub fn start_p2p_host(&mut self, map: &str, bots: u32, max_players: u8) -> Result<(), String> {
        self.start_local_mode(map, bots, max_players.clamp(2, 16), true)
    }

    fn start_local_mode(
        &mut self,
        map: &str,
        bots: u32,
        max_clients: u8,
        p2p_host: bool,
    ) -> Result<(), String> {
        self.disconnect();
        let vfs = self
            .vfs
            .as_ref()
            .ok_or_else(|| "no Half-Life / cstrike folder mounted".to_string())?
            .clone();
        let opts = self.local_options(bots, max_clients);
        let game = LocalGame::start(&vfs, map, &opts)?;
        let bsp = BspMap::load(&vfs, map).ok().map(Arc::new);
        self.configure_local_game(game, bsp, Some(vfs), p2p_host);
        Ok(())
    }

    fn local_options(&self, bots: u32, max_clients: u8) -> LocalOptions {
        let name = self.cvars.get_str("name").to_string();
        let initial_cvars: Vec<(String, String)> = [
            "bot_difficulty",
            "bot_join_team",
            "bot_chatter",
            "bot_join_after_player",
            "mp_startmoney",
            "mp_freezetime",
            "mp_roundtime",
            "mp_buytime",
            "mp_c4timer",
            "violence_hblood",
            "violence_ablood",
        ]
        .iter()
        .filter_map(|&k| {
            let v = self.cvars.get_str(k);
            (!v.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect();
        LocalOptions {
            max_clients,
            bots,
            player_name: if name.is_empty() {
                "Player".to_string()
            } else {
                name
            },
            text_menus: !self.cvars.get_bool("_vgui_menus"),
            initial_cvars,
            ..LocalOptions::default()
        }
    }

    fn configure_local_game(
        &mut self,
        mut game: LocalGame,
        bsp: Option<Arc<BspMap>>,
        vfs: Option<Vfs>,
        p2p_host: bool,
    ) {
        let names = self.view.names();
        let coll = bsp
            .as_deref()
            .map(|b| Arc::new(WorldCollision::from_bsp(b)));
        self.collision = coll.clone();
        self.loaded_map = None;
        self.view.reset_map();
        self.view.set_map_bounds(coll.as_deref());
        self.view.set_map_bsp(bsp);
        self.view.set_world_context(coll.clone(), vfs.clone());
        let materials_txt = vfs
            .as_ref()
            .and_then(|v| v.read_bytes("sound/materials.txt").ok());
        let conn = game.connection_mut();
        conn.frames_mut().set_remote_is_local(true);
        conn.set_world_collision(coll.clone());
        if let Some(txt) = materials_txt {
            conn.predictor_mut().set_materials(&txt);
        }
        conn.set_fx_world(Box::new(MapFxWorld::new(coll, vfs, names)));
        self.p2p_host = p2p_host;
        self.session = SessionMode::Local {
            game: Box::new(game),
            pending: 0.0,
        };
    }

    #[cfg(test)]
    fn start_p2p_host_with_map(
        &mut self,
        map: cs_engine::ServerMap,
        bsp: Arc<BspMap>,
        bots: u32,
        max_players: u8,
    ) -> Result<(), String> {
        self.disconnect();
        let opts = self.local_options(bots, max_players.clamp(2, 16));
        let game = LocalGame::start_with_map(map, self.vfs.clone(), &opts)?;
        self.configure_local_game(game, Some(bsp), self.vfs.clone(), true);
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
        let vgui_flag = if self.cvars.get_bool("_vgui_menus") {
            "1"
        } else {
            "0"
        };
        conn.set_userinfo("_vgui_menus", vgui_flag);
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
        self.p2p_host = false;
        self.loaded_map = None;
        self.collision = None;
        self.view.reset_map();
        self.mod_transfer = ClientModTransfer::default();
        self.mod_host = ClientModHost::new();
        self.last_mod_hud.clear();
        self.hud_anim = HudAnimState::default();
        self.weapon_buckets = WeaponBucketState::default();
        self.crosshair_state = CrosshairState::default();
        self.crosshair_shots_fired = 0;
        self.crosshair_last_weapon = 0;
        self.crosshair_last_clip = 0;
        self.buy_team_class_ui.close();
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

    /// Pushes an inbound Protocol 48 packet from WebRTC peer `peer_id` (`1..=16`) into the
    /// active P2P listen server.
    pub fn push_p2p_packet(&mut self, peer_id: u32, bytes: Vec<u8>) -> bool {
        if !self.p2p_host {
            return false;
        }
        if let SessionMode::Local { game, .. } = &self.session {
            game.push_p2p_packet(peer_id, bytes)
        } else {
            false
        }
    }

    /// Drains outbound `(peer_id, bytes)` packets from the P2P listen server for WebRTC DataChannels.
    pub fn drain_p2p_packets(&mut self) -> Vec<(u32, Vec<u8>)> {
        if !self.p2p_host {
            return Vec::new();
        }
        if let SessionMode::Local { game, .. } = &self.session {
            game.drain_p2p_packets()
        } else {
            Vec::new()
        }
    }

    pub fn is_p2p_host(&self) -> bool {
        self.p2p_host && matches!(self.session, SessionMode::Local { .. })
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
            let bsp_arc = Arc::new(bsp);
            let coll = Arc::new(WorldCollision::from_bsp(&bsp_arc));
            self.collision = Some(coll.clone());
            self.view.set_map_bounds(Some(coll.as_ref()));
            self.view.set_map_bsp(Some(bsp_arc));
            self.view
                .set_world_context(Some(coll.clone()), Some(vfs.clone()));
            let materials_txt = vfs.read_bytes("sound/materials.txt").ok();
            let names = self.view.names();
            let vfs_c = vfs.clone();
            match &mut self.session {
                SessionMode::Idle => {}
                SessionMode::Local { game, .. } => {
                    let c = game.connection_mut();
                    c.set_world_collision(Some(coll.clone()));
                    if let Some(txt) = materials_txt {
                        c.predictor_mut().set_materials(&txt);
                    }
                    c.set_fx_world(Box::new(MapFxWorld::new(Some(coll), Some(vfs_c), names)));
                }
                SessionMode::Remote { conn, .. } => {
                    conn.set_world_collision(Some(coll.clone()));
                    if let Some(txt) = materials_txt {
                        conn.predictor_mut().set_materials(&txt);
                    }
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
        let actions = self.cvars.execute_script(trimmed);
        for action in actions {
            match action {
                EngineAction::Mp3Command(args) => self.music.handle_mp3_command(&args),
                EngineAction::CdCommand(args) => self.music.handle_cd_command(&args),
                EngineAction::PlaySound { sample, volume } => {
                    let norm = normalize_sound_path(&sample);
                    let master = self.master_volume();
                    self.push_2d_event(norm, volume * master, 100, 0, 0, 0.0);
                }
                EngineAction::SpeakSentence(spec) => {
                    let suit = self.suit_volume();
                    self.queue_vox_sentence(&spec, suit, None);
                }
                EngineAction::StopSound => self.queued_audio.push(WebAudioEvent::StopAll),
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
                EngineAction::InvNext => self.dispatch_command("invnext"),
                EngineAction::InvPrev => self.dispatch_command("invprev"),
                EngineAction::ToggleScores
                | EngineAction::OpenPlayerList
                | EngineAction::OpenDemoPlayer(_)
                | EngineAction::SpecMode(_)
                | EngineAction::SpecMenu(_) => {}
                _ => {}
            }
        }
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

    /// Accepts captured mono microphone PCM from the browser (Web Audio AudioWorklet / ScriptProcessor)
    /// when `+voicerecord` is active (or during local `voice_loopback` testing).
    pub fn push_voice_mic_pcm(&mut self, samples: &[f32], sample_rate: u32) {
        if samples.is_empty() {
            return;
        }
        let voice_enable = self.cvars.get_bool("voice_enable");
        let voice_loopback = self.cvars.get_bool("voice_loopback");
        let recording = self.input.actions.contains("voicerecord") || voice_loopback;
        if !recording {
            return;
        }
        let avg_gain = self.cvar_f32_clamped("voice_avggain", 0.5, 0.0, 5.0);
        let max_gain = self.cvar_f32_clamped("voice_maxgain", 5.0, 0.0, 20.0);
        let now = f64::from(self.time_sec);
        self.voice.push_mic_samples(
            samples,
            sample_rate,
            now,
            voice_enable,
            voice_loopback,
            avg_gain,
            max_gain,
        );
        let outgoing = self.voice.take_outgoing();
        for pkt in outgoing {
            match &mut self.session {
                SessionMode::Idle => {}
                SessionMode::Local { game, .. } => game.connection_mut().send_voice_data(&pkt),
                SessionMode::Remote { conn, .. } => conn.send_voice_data(&pkt),
            }
        }
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
        self.last_dt = dt as f32;
        self.time_sec += dt as f32;
        self.weapon_buckets.step(dt as f32);

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

        // Confirm an open weapon bucket selection on `+attack` without firing a shot.
        if self.weapon_buckets.active_slot.is_some() && self.input.actions.contains("attack") {
            let confirmed = match &self.session {
                SessionMode::Idle => None,
                SessionMode::Local { game, .. } => self
                    .weapon_buckets
                    .confirm_selection(&game.connection().fx().hud),
                SessionMode::Remote { conn, .. } => {
                    self.weapon_buckets.confirm_selection(&conn.fx().hud)
                }
            };
            if let Some(wname) = confirmed {
                self.input.actions.remove("attack");
                self.play_ui_sound(WeaponBucketSound::Select.vfs_path());
                self.send_stringcmd(&wname);
            }
        }

        let cmd =
            self.input
                .build_usercmd(&mut self.camera, dt, interp, &MoveSpeeds::default(), 180.0);

        match &mut self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, pending } => {
                let conn = game.connection_mut();
                for name in ["cl_nopred", "cl_lw"] {
                    let desired = self.cvars.get_str(name);
                    if conn.cvars().find(name).map(|v| v.value.as_str()) != Some(desired) {
                        conn.set_cvar(name, desired);
                    }
                }
                if conn.state() == &ConnState::InGame {
                    conn.queue_cmd(cmd);
                }
                *pending += dt;
                while *pending >= FRAME_SECS {
                    game.step();
                    *pending -= FRAME_SECS;
                }
            }
            SessionMode::Remote { conn, now, .. } => {
                for name in ["cl_nopred", "cl_lw"] {
                    let desired = self.cvars.get_str(name);
                    if conn.cvars().find(name).map(|v| v.value.as_str()) != Some(desired) {
                        conn.set_cvar(name, desired);
                    }
                }
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
            cl_weather: self.cvars.get_bool("cl_weather"),
            sv_cheats: self.cvars.get_bool("sv_cheats"),
            sv_showimpacts: self.cvars.get_i32("sv_showimpacts"),
            sv_showhitboxes: self.cvars.get_bool("sv_showhitboxes"),
            spec_xray: self.cvars.get_bool("spec_xray"),
        };
        let view_angles = Vec3::new(self.camera.pitch_deg, self.camera.yaw_deg, 0.0);

        let mut built_view = None;
        let mut new_registry = None;
        let mut drained_sounds = Vec::new();
        let mut drained_voice: Vec<VoiceEvent> = Vec::new();
        let mut drained_predicted = Vec::new();
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
                drained_voice = conn.fx_mut().voice.drain();
                drained_predicted = conn.predictor_mut().take_sounds();
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
                drained_voice = conn.fx_mut().voice.drain();
                drained_predicted = conn.predictor_mut().take_sounds();
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
            if !self.mod_transfer.ingest_usermsg(
                &um.name,
                &um.payload,
                &mut self.mod_host,
                self.vfs.as_mut(),
            ) {
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
        let anim_eye = built_view
            .as_ref()
            .map_or(self.camera.position, |fv| fv.eye);
        let anim_yaw = self.camera.yaw_deg;
        match &self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, .. } => {
                self.hud_anim.step_with_pose(
                    dt as f32,
                    &game.connection().fx().hud,
                    sess_now,
                    anim_eye,
                    anim_yaw,
                );
            }
            SessionMode::Remote { conn, .. } => {
                self.hud_anim.step_with_pose(
                    dt as f32,
                    &conn.fx().hud,
                    sess_now,
                    anim_eye,
                    anim_yaw,
                );
            }
        }

        let listener = self.camera.position.to_array();
        let right = if let Some(fv) = &built_view {
            fv.listener_right.to_array()
        } else {
            let yaw = self.camera.yaw_deg.to_radians();
            [yaw.sin(), -yaw.cos(), 0.0]
        };

        for ev in drained_voice {
            self.voice.handle_event(ev, sess_now);
        }

        self.step_music_and_dsp(
            dt as f32,
            built_view.as_ref().is_some_and(|v| v.waterlevel >= 3),
        );

        for sc in drained_sounds {
            self.push_sound_event(sc, listener, right, local_entity);
        }

        if let Some(fv) = &built_view {
            for impact in &fv.impact_sounds {
                self.push_sound_event(
                    SoundCommand::Start(impact.clone()),
                    listener,
                    right,
                    local_entity,
                );
            }
            let master = self.master_volume();
            for (_sender, code, _pitch) in &fv.radio_audio {
                if self.audio_unlocked {
                    self.queue_vox_sentence(code, master, None);
                }
            }
            for (vol, kind) in &fv.reload_sounds {
                if self.audio_unlocked {
                    let v = (f32::from(*vol) / 255.0) * master;
                    self.push_2d_event(
                        reload_sound_path(*kind).to_string(),
                        v,
                        PITCH_NORM,
                        local_entity.unwrap_or(1),
                        0,
                        0.0,
                    );
                }
            }
            for path in &fv.anim_sounds {
                if self.audio_unlocked {
                    self.push_2d_event(
                        normalize_sound_path(path),
                        0.8 * master,
                        PITCH_NORM,
                        local_entity.unwrap_or(1),
                        1,
                        0.0,
                    );
                }
            }
        }

        for ps in drained_predicted {
            if ps.volume.is_finite() {
                let start = SoundStart::by_name(
                    &ps.sample,
                    local_entity.unwrap_or(1),
                    ps.channel.clamp(0, 7) as u8,
                    self.camera.position,
                    ps.volume,
                    ps.attenuation,
                    ps.pitch.clamp(2, 255) as u8,
                );
                self.push_sound_event(SoundCommand::Start(start), listener, right, local_entity);
            }
        }

        let voice_enable = self.cvars.get_bool("voice_enable");
        let voice_scale = self.cvar_f32_clamped("voice_scale", 1.0, 0.0, 4.0);
        let decoded_voice = self.voice.drain_decoded(voice_enable);
        if self.audio_unlocked {
            for (speaker, sample_rate, samples) in decoded_voice {
                if !samples.is_empty() {
                    self.queued_audio.push(WebAudioEvent::VoicePcm {
                        speaker,
                        sample_rate,
                        samples,
                        volume: voice_scale,
                    });
                }
            }
        }

        self.sync_vgui_menu_state();

        StepOutput {
            view: built_view,
            new_registry,
            map_sync,
        }
    }

    fn play_ui_sound(&mut self, path: &str) {
        let vol = self.master_volume();
        self.push_2d_event(path.to_string(), vol, PITCH_NORM, 0, 0, 0.0);
    }

    fn local_is_ct(&self) -> bool {
        let slot = match &self.session {
            SessionMode::Idle => return true,
            SessionMode::Local { game, .. } => game.connection().player_slot(),
            SessionMode::Remote { conn, .. } => conn.player_slot(),
        };
        let Some(s) = slot else {
            return true;
        };
        self.with_hud(|h| h.players.get(&(s + 1)).is_some_and(|p| p.team == "CT"))
            .unwrap_or(true)
    }

    fn sync_vgui_menu_state(&mut self) {
        let use_vgui = self.cvars.get_bool("_vgui_menus");
        if !use_vgui {
            self.buy_team_class_ui.close();
            return;
        }
        let active = self
            .with_hud(|h| {
                h.vgui_menu
                    .as_ref()
                    .filter(|v| !v.multipart && v.keys != 0)
                    .cloned()
            })
            .flatten();
        if let Some(vm) = active
            && let Some(kind) = VguiMenuKind::from_vgui_menu_id(vm.menu_id, &vm.name)
        {
            let is_ct = self.local_is_ct();
            self.buy_team_class_ui.open_kind(kind, is_ct);
        }
    }

    fn dispatch_command(&mut self, cmd: &str) {
        if cmd == "invnext" || cmd == "invprev" {
            if self.buy_team_class_ui.is_open() {
                return;
            }
            let forward = cmd == "invnext";
            let owned = owned_set(self.owned_weapons());
            let fastswitch_mode = self.cvars.get_i32("hud_fastswitch");
            let (picked, snd) = match &self.session {
                SessionMode::Idle => (None, None),
                SessionMode::Local { game, .. } => step_inventory_cycle_mode(
                    &mut self.weapon_buckets,
                    &game.connection().fx().hud,
                    &owned,
                    forward,
                    fastswitch_mode,
                ),
                SessionMode::Remote { conn, .. } => step_inventory_cycle_mode(
                    &mut self.weapon_buckets,
                    &conn.fx().hud,
                    &owned,
                    forward,
                    fastswitch_mode,
                ),
            };
            if let Some(sound) = snd {
                self.play_ui_sound(sound.vfs_path());
            }
            if let Some(wname) = picked {
                self.send_stringcmd(&wname);
            }
            return;
        }
        if let Some(digit) = parse_slot_cmd(cmd) {
            // 1. VGUI2 Buy/Team/Class menu (`_vgui_menus 1`) owns `1..9` & `0` exclusively.
            if self.buy_team_class_ui.is_open() {
                self.handle_vgui_menu_digit(digit);
                return;
            }
            // 2. Classic text `ShowMenu` (`_vgui_menus 0`, radio `Z`/`X`/`C`, AMXX menus).
            let menu_open = self
                .with_hud(|h| {
                    h.menu
                        .as_ref()
                        .is_some_and(|m| !m.multipart && m.keys != 0 && !m.text.is_empty())
                })
                .unwrap_or(false);
            if menu_open {
                let valid = self
                    .with_hud(|h| {
                        h.menu
                            .as_ref()
                            .is_some_and(|m| menu_key_enabled(m.keys, digit))
                    })
                    .unwrap_or(false);
                if valid {
                    let n = if digit == 0 { 10 } else { digit };
                    self.send_stringcmd(&format!("menuselect {n}"));
                    self.clear_text_menu();
                    return;
                }
            }
            if digit >= 1 {
                let owned = owned_set(self.owned_weapons());
                let fastswitch_mode = self.cvars.get_i32("hud_fastswitch");
                let (picked, snd) = match &self.session {
                    SessionMode::Idle => (None, None),
                    SessionMode::Local { game, .. } => self.weapon_buckets.on_slot_press_mode(
                        &game.connection().fx().hud,
                        &owned,
                        digit,
                        fastswitch_mode,
                    ),
                    SessionMode::Remote { conn, .. } => self.weapon_buckets.on_slot_press_mode(
                        &conn.fx().hud,
                        &owned,
                        digit,
                        fastswitch_mode,
                    ),
                };
                if let Some(sound) = snd {
                    self.play_ui_sound(sound.vfs_path());
                }
                if let Some(wname) = picked {
                    self.send_stringcmd(&wname);
                }
            }
            return;
        }
        self.send_stringcmd(cmd);
    }

    fn handle_vgui_menu_digit(&mut self, digit: u8) {
        let Some(res_path) = self.buy_team_class_ui.active_res.clone() else {
            return;
        };
        if digit == 0 {
            self.buy_team_class_ui.close();
            self.clear_vgui_menu();
            return;
        }
        if let Some(v) = &self.vfs {
            self.vgui_rt.load_res(v, &res_path);
        }
        let nodes = self.vgui_rt.compute_layout_mode(
            &res_path,
            [0.0, 0.0, 640.0, 480.0],
            VguiCoordMode::Proportional640,
            self.loc.as_ref(),
        );
        let buttons: Vec<_> = nodes
            .into_iter()
            .filter(|n| {
                n.visible
                    && matches!(
                        n.kind,
                        ControlKind::Button | ControlKind::MouseOverPanelButton
                    )
            })
            .collect();
        let lower = res_path.to_ascii_lowercase();
        let is_team = lower.contains("teammenu");
        let is_buy_submenu =
            !is_team && !lower.contains("classmenu") && !lower.contains("mainbuymenu");
        let money = self.with_hud(|h| h.money).unwrap_or(0);
        let ch = char::from(b'0' + digit);
        if let Some(btn) = find_button_by_hotkey(&buttons, ch, is_team)
            && btn.enabled
            && !(is_buy_submenu && btn.cost.is_some_and(|c| money < c as i32))
        {
            let is_ct = self.local_is_ct();
            if let Some(send) =
                self.buy_team_class_ui
                    .handle_command(&btn.command, is_ct, self.vfs.as_ref())
            {
                self.send_stringcmd(&send);
                self.clear_vgui_menu();
            } else if !self.buy_team_class_ui.is_open() {
                self.clear_vgui_menu();
            }
        }
    }

    fn clear_vgui_menu(&mut self) {
        match &mut self.session {
            SessionMode::Idle => {}
            SessionMode::Local { game, .. } => {
                game.connection_mut().fx_mut().hud.vgui_menu = None;
            }
            SessionMode::Remote { conn, .. } => conn.fx_mut().hud.vgui_menu = None,
        }
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

    fn cvar_f32_clamped(&self, name: &str, default: f32, lo: f32, hi: f32) -> f32 {
        let v = self.cvars.get_f32(name);
        if v.is_finite() {
            v.clamp(lo, hi)
        } else {
            default
        }
    }

    fn master_volume(&self) -> f32 {
        self.cvar_f32_clamped("volume", 0.8, 0.0, 1.0)
    }

    fn suit_volume(&self) -> f32 {
        self.cvar_f32_clamped("suitvolume", 0.25, 0.0, 1.0)
    }

    fn step_music_and_dsp(&mut self, dt: f32, underwater: bool) {
        let mp3_vol = self.cvar_f32_clamped("mp3volume", 0.5, 0.0, 1.0);
        let bgm = self.cvars.get_bool("bgmvolume");
        let fade_time = self.cvar_f32_clamped("mp3fadetime", 2.0, 0.0, 30.0);
        self.music.set_params(mp3_vol, bgm, fade_time);

        if matches!(self.session, SessionMode::Idle) {
            let menu_path = self.vfs.as_ref().and_then(|v| {
                ["media/gamestartup.mp3", "media/Half-Life01.mp3"]
                    .into_iter()
                    .find(|p| v.exists(p))
            });
            self.music.sync_menu_music(true, menu_path);
        }

        self.sound_fade.step(dt);

        if !self.audio_unlocked {
            let _ = self.music.step(dt);
            return;
        }

        let fade_gain = self.sound_fade.gain();
        if (fade_gain - self.last_fade_gain).abs() > 0.005 {
            self.last_fade_gain = fade_gain;
            self.queued_audio
                .push(WebAudioEvent::SetFade { gain: fade_gain });
        }

        if underwater != self.last_underwater {
            self.last_underwater = underwater;
            self.queued_audio
                .push(WebAudioEvent::SetUnderwater(underwater));
        }

        let room_off = self.cvars.get_bool("room_off");
        let rt = if room_off {
            0
        } else {
            self.cvars.get_f32("room_type").max(0.0).round() as u16
        };
        if rt != self.last_room_type {
            self.last_room_type = rt;
            self.queued_audio.push(WebAudioEvent::SetRoomType(rt));
        }

        for mcmd in self.music.step(dt) {
            match mcmd {
                MusicCommand::Play {
                    path,
                    looping,
                    volume,
                } => self.queued_audio.push(WebAudioEvent::PlayMusic {
                    path,
                    looping,
                    volume,
                }),
                MusicCommand::SetVolume(vol) => {
                    self.queued_audio.push(WebAudioEvent::SetMusicVolume(vol));
                }
                MusicCommand::Stop => self.queued_audio.push(WebAudioEvent::StopMusic),
            }
        }
    }

    fn wav_loop_points(&self, path: &str) -> (Option<u32>, Option<u32>) {
        let Some(vfs) = &self.vfs else {
            return (None, None);
        };
        let norm = normalize_sound_path(path);
        let Ok(bytes) = vfs.read_bytes(&norm) else {
            return (None, None);
        };
        if let Some((start, end, _rate)) = parse_wav_loop_points(&bytes) {
            (Some(start as u32), Some(end as u32))
        } else {
            (None, None)
        }
    }

    fn wav_duration_sec(&self, path: &str) -> f32 {
        let Some(vfs) = &self.vfs else {
            return 0.35;
        };
        let norm = normalize_sound_path(path);
        let Ok(bytes) = vfs.read_bytes(&norm) else {
            return 0.35;
        };
        if let Ok(w) = cs_client::audio::decode_wav(&bytes) {
            w.frame_count() as f32 / w.sample_rate.max(1) as f32
        } else {
            0.35
        }
    }

    fn is_occluded(&self, listener: [f32; 3], pos: [f32; 3]) -> bool {
        if !self.cvars.get_bool("snd_occlusion")
            || self.cvar_f32_clamped("s_occfactor", 0.0, 0.0, 0.9) <= 0.0
        {
            return false;
        }
        let Some(coll) = &self.collision else {
            return false;
        };
        coll.trace_world(
            Vec3::from_array(listener),
            Vec3::from_array(pos),
            Hull::Point,
        )
        .fraction
            < 0.99
    }

    fn push_2d_event(
        &mut self,
        path: String,
        volume: f32,
        pitch: u8,
        entity: u16,
        channel: u8,
        delay_sec: f32,
    ) {
        if !self.audio_unlocked || path.is_empty() {
            return;
        }
        let norm = normalize_sound_path(&path);
        let (loop_start, loop_end) = self.wav_loop_points(&norm);
        self.queued_audio.push(WebAudioEvent::Play2d {
            path: norm,
            volume,
            pitch,
            entity,
            channel,
            loop_start,
            loop_end,
            delay_sec,
        });
    }

    fn queue_vox_sentence(&mut self, spec: &str, volume: f32, spatial: Option<VoxSpatial>) {
        if !self.audio_unlocked {
            return;
        }
        let Some(words) = self.sentences.resolve(spec) else {
            return;
        };
        let mut cursor_sec = 0.0f32;
        for w in words {
            if w.pause_sec > 0.0 {
                cursor_sec += w.pause_sec;
                continue;
            }
            let norm = normalize_sound_path(&w.path);
            let dur = self.wav_duration_sec(&norm);
            let vol = (volume * w.volume).clamp(0.0, 2.0);
            if let Some(sp) = spatial {
                let occluded = self.is_occluded(sp.listener, sp.pos);
                let (loop_start, loop_end) = self.wav_loop_points(&norm);
                self.queued_audio.push(WebAudioEvent::Play3d {
                    path: norm,
                    volume: vol,
                    pitch: w.pitch,
                    attenuation: sp.attenuation,
                    pos: sp.pos,
                    listener: sp.listener,
                    right: sp.right,
                    occluded,
                    entity: sp.entity,
                    channel: sp.channel,
                    loop_start,
                    loop_end,
                    delay_sec: cursor_sec,
                });
            } else {
                self.push_2d_event(norm, vol, w.pitch, 0, 2, cursor_sec);
            }
            let pitch_rate = f32::from(w.pitch.max(20)) / f32::from(PITCH_NORM);
            cursor_sec += (dur / pitch_rate).max(0.04);
        }
    }

    fn push_sound_event(
        &mut self,
        sc: SoundCommand,
        listener: [f32; 3],
        right: [f32; 3],
        local_entity: Option<u16>,
    ) {
        match sc {
            SoundCommand::RoomType(rt) => {
                self.cvars.set("room_type", rt.to_string());
                return;
            }
            SoundCommand::CdTrack { track, loop_track } => {
                self.music.handle_svc_cdtrack(track, loop_track);
                return;
            }
            SoundCommand::Fade {
                percent,
                hold_time,
                fade_out,
                fade_in,
            } => {
                self.sound_fade
                    .trigger(percent, hold_time, fade_out, fade_in);
                return;
            }
            _ => {}
        }

        if !self.audio_unlocked {
            return;
        }
        let master = self.master_volume();
        let suit = self.suit_volume();
        match sc {
            SoundCommand::Start(start) | SoundCommand::Ambient(start) => {
                if start.is_stop() {
                    self.queued_audio.push(WebAudioEvent::StopChannel {
                        entity: start.entity,
                        channel: start.channel,
                    });
                    return;
                }
                if start.flags & (SND_FL_CHANGE_VOL | SND_FL_CHANGE_PITCH) != 0 {
                    let volume = if start.flags & SND_FL_CHANGE_VOL != 0 {
                        Some(start.volume * master)
                    } else {
                        None
                    };
                    let pitch = if start.flags & SND_FL_CHANGE_PITCH != 0 {
                        Some(start.pitch)
                    } else {
                        None
                    };
                    self.queued_audio.push(WebAudioEvent::UpdateChannel {
                        entity: start.entity,
                        channel: start.channel,
                        volume,
                        pitch,
                        pos: Some(start.origin.to_array()),
                    });
                    return;
                }
                let Some(raw) = self.resolve_sound_name(&start) else {
                    return;
                };
                if raw.starts_with('!') || raw.starts_with("%!") {
                    let is_2d = local_entity == Some(start.entity) || start.attenuation <= 0.0;
                    let base_vol = if is_2d {
                        start.volume * suit.max(0.25) * master
                    } else {
                        start.volume * master
                    };
                    let spatial = (!is_2d).then_some(VoxSpatial {
                        pos: start.origin.to_array(),
                        listener,
                        right,
                        attenuation: start.attenuation,
                        entity: start.entity,
                        channel: start.channel,
                    });
                    self.queue_vox_sentence(&raw, base_vol, spatial);
                    return;
                }
                let path = normalize_sound_path(&raw);
                let (loop_start, loop_end) = self.wav_loop_points(&path);
                let vol = start.volume * master;
                if local_entity == Some(start.entity) || start.attenuation <= 0.0 {
                    self.queued_audio.push(WebAudioEvent::Play2d {
                        path,
                        volume: vol,
                        pitch: start.pitch,
                        entity: start.entity,
                        channel: start.channel,
                        loop_start,
                        loop_end,
                        delay_sec: 0.0,
                    });
                } else {
                    let pos = start.origin.to_array();
                    let occluded = self.is_occluded(listener, pos);
                    self.queued_audio.push(WebAudioEvent::Play3d {
                        path,
                        volume: vol,
                        pitch: start.pitch,
                        attenuation: start.attenuation,
                        pos,
                        listener,
                        right,
                        occluded,
                        entity: start.entity,
                        channel: start.channel,
                        loop_start,
                        loop_end,
                        delay_sec: 0.0,
                    });
                }
            }
            SoundCommand::Stop { entity, channel } => {
                self.queued_audio
                    .push(WebAudioEvent::StopChannel { entity, channel });
            }
            SoundCommand::Fade { .. }
            | SoundCommand::RoomType(_)
            | SoundCommand::CdTrack { .. } => {}
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

    /// Renders HUD, crosshair, net_graph, VGUI2 Buy/Team/Class menus, server text menus, and scoreboard into `ctx`.
    pub fn draw_hud_ui(
        &mut self,
        ctx: &egui::Context,
        hud_sprites: Option<&HudSprites>,
        client_palette: cs_client::ui::vgui_assets::ClientPalette,
        vgui_palette: cs_client::ui::vgui_theme::VguiPalette,
        view: Option<&FrameView>,
    ) {
        let Some(fv) = view else {
            return;
        };
        // Sniper scope overlay applies at <= 40 deg (AWP/Scout/auto-sniper), keeping AUG/SG552 (55 deg) unmasked.
        let scoped = fv.fov.is_finite() && fv.fov > 0.0 && fv.fov <= 40.0;
        let (
            hud_health,
            hud_hidden,
            hud_timer_vis,
            hud_nvg,
            hud_money,
            wid,
            clip,
            active_vgui_keys,
        ) = self
            .with_hud(|h| {
                (
                    i32::from(h.health),
                    h.hidden,
                    h.timer_visible,
                    h.night_vision,
                    h.money,
                    h.current_weapon.map_or(0, |w| w.weapon_id),
                    h.current_weapon.map_or(-1, |w| i32::from(w.clip)),
                    h.vgui_menu.as_ref().map_or(0, |v| v.keys),
                )
            })
            .unwrap_or((100, 0, false, false, 0, 0, -1, 0));
        let hud_vis = compute_hud_visibility(fv.alive, hud_health, hud_hidden, hud_timer_vis);
        if hud_vis.crosshair && !scoped && should_draw_dynamic_crosshair(fv.fov, wid) {
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
            let (on_ground, ducking, speed_2d, wflags) = match &self.session {
                SessionMode::Idle => (true, false, 0.0, 0),
                SessionMode::Local { game, .. } => {
                    game.connection()
                        .client_data()
                        .map_or((true, false, 0.0, 0), |d| {
                            let spd =
                                (d.velocity.x * d.velocity.x + d.velocity.y * d.velocity.y).sqrt();
                            ((d.flags & 512) != 0, (d.flags & 16384) != 0, spd, 0)
                        })
                }
                SessionMode::Remote { conn, .. } => {
                    conn.client_data().map_or((true, false, 0.0, 0), |d| {
                        let spd =
                            (d.velocity.x * d.velocity.x + d.velocity.y * d.velocity.y).sqrt();
                        ((d.flags & 512) != 0, (d.flags & 16384) != 0, spd, 0)
                    })
                }
            };
            let ch_inputs = CrosshairInputs {
                weapon_id: wid,
                weapon_flags: wflags,
                on_ground,
                ducking,
                speed_2d,
                shots_fired: self.crosshair_shots_fired,
                night_vision: hud_nvg,
                dynamic: self.cvars.get_bool("cl_dynamiccrosshair"),
                hide_hud: hud_hidden,
            };
            draw_crosshair_vgui_dt(
                ctx,
                &self.cvars,
                &mut self.crosshair_state,
                &ch_inputs,
                self.last_dt,
            );
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
                weapon_switch_timer: 0.0,
                alive: fv.alive,
                scoped,
                eye: fv.eye,
                yaw_deg: self.camera.yaw_deg,
                underwater_tint: fv.underwater_tint,
            };
            let mut extras = HudExtrasContext {
                anim: Some(&self.hud_anim),
                buckets: Some(&self.weapon_buckets),
                ..Default::default()
            };
            extras.apply_cvars(&self.cvars);
            draw_cs16_hud_with_extras(ctx, hud_sprites, &hv, &extras);
            let speakers = self.voice.active_speakers(sess_now);
            let local_talking = self.voice.is_local_transmitting(sess_now);
            draw_voice_speakers(ctx, &names, &speakers, local_talking);
            draw_mod_hud(ctx, &self.last_mod_hud);

            if let Some(m) = h
                .menu
                .as_ref()
                .filter(|m| !m.multipart && m.keys != 0 && !m.text.is_empty())
            {
                let lines = menu_styled_lines(&m.text, None);
                let scale = (ctx.content_rect().height() / 768.0).clamp(0.85, 1.45);
                draw_styled_text_menu(ctx, &lines, scale);
            }

            if self.input.actions.contains("showscores") {
                let rows: Vec<ScoreRow> = h
                    .players
                    .iter()
                    .filter(|(slot, p)| {
                        !p.team.is_empty()
                            || slot
                                .checked_sub(1)
                                .is_some_and(|s0| names.contains_key(&s0))
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

        if self.buy_team_class_ui.is_open() {
            let is_ct = self.local_is_ct();
            let map_name = self.loaded_map.clone().unwrap_or_default();
            let buy_ctx = BuyMenuRenderCtx {
                vfs: self.vfs.as_ref(),
                loc: self.loc.as_ref(),
                map_name: &map_name,
                is_ct,
                allowed_keys: active_vgui_keys,
                can_cancel: true,
                money: hud_money,
            };
            let was_open = self.buy_team_class_ui.is_open();
            let out_cmd = self
                .buy_team_class_ui
                .show(ctx, &mut self.vgui_rt, &buy_ctx);
            if let Some(cmd) = out_cmd {
                self.send_stringcmd(&cmd);
                self.clear_vgui_menu();
            } else if was_open && !self.buy_team_class_ui.is_open() {
                self.clear_vgui_menu();
            }
        }

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

        assert_eq!(
            st.read_file("config.cfg").as_deref(),
            Some(b"cstrike_line\n".as_slice())
        );
        assert_eq!(st.list_maps(), vec!["de_dust2".to_string()]);
    }

    #[test]
    fn audio_events_gated_until_user_gesture() {
        let mut st = WebClientState::default();
        assert!(!st.is_audio_unlocked());
        let start = cs_clientcore::SoundStart::by_name(
            "weapons/ak47-1.wav",
            1,
            1,
            Vec3::ZERO,
            1.0,
            0.8,
            100,
        );
        st.push_sound_event(
            SoundCommand::Start(start.clone()),
            [0.0; 3],
            [0.0, 1.0, 0.0],
            Some(1),
        );
        assert!(st.take_audio_events().is_empty());

        st.unlock_audio();
        assert!(st.is_audio_unlocked());
        st.push_sound_event(
            SoundCommand::Start(start),
            [0.0; 3],
            [0.0, 1.0, 0.0],
            Some(1),
        );
        let evs = st.take_audio_events();
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], WebAudioEvent::Play2d { .. }));
    }

    #[test]
    fn web_audio_backend_full_event_pipeline() {
        let mut st = WebClientState::default();
        let looped_wav = cs_client::audio::encode_test_wav_pcm16(
            22_050,
            1,
            &[0i16; 2205],
            None,
            Some((100, 2000)),
        );
        st.mount_file("Half-Life/valve/sound/ambience/hum.wav", looped_wav);
        st.mount_file(
            "Half-Life/valve/sound/vox/hello.wav",
            cs_client::audio::encode_test_wav_pcm16(22_050, 1, &[500i16; 4410], None, None),
        );
        st.mount_file(
            "Half-Life/valve/sound/sentences.txt",
            b"TEST_VOX vox/hello\n".to_vec(),
        );
        st.finish_mount();
        st.unlock_audio();

        // 1. Looped 3D sound + channel pitch/vol alter + room type + soundfade
        let start = SoundStart::by_name(
            "ambience/hum.wav",
            5,
            6,
            Vec3::new(100.0, 0.0, 0.0),
            0.9,
            0.8,
            120,
        );
        st.push_sound_event(
            SoundCommand::Start(start),
            [0.0; 3],
            [0.0, 1.0, 0.0],
            Some(1),
        );
        let mut alter = SoundStart::by_name(
            "ambience/hum.wav",
            5,
            6,
            Vec3::new(120.0, 0.0, 0.0),
            0.4,
            0.8,
            90,
        );
        alter.flags = SND_FL_CHANGE_VOL;
        st.push_sound_event(
            SoundCommand::Start(alter),
            [0.0; 3],
            [0.0, 1.0, 0.0],
            Some(1),
        );
        st.push_sound_event(
            SoundCommand::RoomType(14),
            [0.0; 3],
            [0.0, 1.0, 0.0],
            Some(1),
        );
        st.push_sound_event(
            SoundCommand::Fade {
                percent: 80,
                hold_time: 1,
                fade_out: 1,
                fade_in: 1,
            },
            [0.0; 3],
            [0.0, 1.0, 0.0],
            Some(1),
        );

        // 2. Music command + VOX sentence + voice loopback PCM
        st.exec_console("mp3fadetime 0");
        st.exec_console("mp3 play Half-Life02.mp3");
        st.exec_console("spk TEST_VOX");
        st.exec_console("voice_loopback 1");
        let mic = vec![0.25f32; 960];
        st.push_voice_mic_pcm(&mic, 24_000);
        let _ = st.step(0.5);

        let evs = st.take_audio_events();
        assert!(evs.iter().any(|e| matches!(
            e,
            WebAudioEvent::Play3d { path, pitch: 120, loop_start: Some(100), loop_end: Some(2000), .. }
                if path == "sound/ambience/hum.wav"
        )));
        assert!(evs.iter().any(|e| matches!(
            e,
            WebAudioEvent::UpdateChannel {
                entity: 5,
                channel: 6,
                ..
            }
        )));
        assert!(
            evs.iter()
                .any(|e| matches!(e, WebAudioEvent::SetRoomType(14)))
        );
        assert!(
            evs.iter()
                .any(|e| matches!(e, WebAudioEvent::SetFade { gain } if *gain < 0.99))
        );
        assert!(evs.iter().any(|e| matches!(e, WebAudioEvent::PlayMusic { path, .. } if path == "media/Half-Life02.mp3")));
        assert!(evs.iter().any(
            |e| matches!(e, WebAudioEvent::Play2d { path, .. } if path == "sound/vox/hello.wav")
        ));
        assert!(evs.iter().any(|e| matches!(e, WebAudioEvent::VoicePcm { speaker: 255, samples, .. } if !samples.is_empty())));
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
        assert!(
            in_game,
            "web ChannelTransport client should reach ConnState::InGame"
        );
    }

    #[test]
    fn local_game_with_bots_runs_headless_on_real_assets() {
        let Ok(real_vfs) = Vfs::discover(None, VfsConfig::default()) else {
            eprintln!("SKIPPED: no game directory (set CS16_GAMEDIR)");
            return;
        };
        let mut st = WebClientState::default();
        st.set_vfs(real_vfs);
        st.start_local("de_dust2", 2)
            .expect("start local game in cs-web");
        assert_eq!(st.conn_state(), Some(ConnState::InGame));
        let out = st.step(0.01);
        assert_eq!(out.map_sync, MapSyncAction::Load("de_dust2".to_string()));
        st.mark_map_loaded("de_dust2");
        let out2 = st.step(0.01);
        assert!(out2.view.is_some());
    }

    fn p2p_test_floor_map() -> (ServerMap, Arc<BspMap>) {
        use cs_assets::bsp::{
            BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane,
        };
        use glam::Vec3;

        const CONTENTS_EMPTY: i32 = -1;
        const CONTENTS_SOLID: i32 = -2;

        let leaf = |contents: i32| Leaf {
            contents,
            vis_offset: -1,
            mins: [0; 3],
            maxs: [0; 3],
            first_mark_surface: 0,
            num_mark_surfaces: 0,
            ambient_levels: [0; 4],
        };
        let planes = vec![
            Plane {
                normal: Vec3::Z,
                dist: 0.0,
                plane_type: 2,
            },
            Plane {
                normal: Vec3::Z,
                dist: 36.0,
                plane_type: 2,
            },
            Plane {
                normal: Vec3::Z,
                dist: 32.0,
                plane_type: 2,
            },
            Plane {
                normal: Vec3::Z,
                dist: 18.0,
                plane_type: 2,
            },
        ];
        let clipnodes = vec![
            ClipNode {
                plane: 1,
                children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
            },
            ClipNode {
                plane: 2,
                children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
            },
            ClipNode {
                plane: 3,
                children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
            },
            ClipNode {
                plane: 3,
                children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
            },
        ];
        let nodes = vec![
            Node {
                plane: 0,
                children: [-2, -1],
                mins: [0; 3],
                maxs: [0; 3],
                first_face: 0,
                num_faces: 0,
            },
            Node {
                plane: 0,
                children: [-2, -1],
                mins: [0; 3],
                maxs: [0; 3],
                first_face: 0,
                num_faces: 0,
            },
        ];
        let model = BspModel {
            mins: Vec3::new(-4096.0, -4096.0, -64.0),
            maxs: Vec3::new(4096.0, 4096.0, 4096.0),
            origin: Vec3::ZERO,
            head_nodes: [0, 0, 1, 2],
            vis_leafs: 1,
            first_face: 0,
            num_faces: 0,
        };
        let spawn = |classname: &str, x: f32, y: f32| -> Entity {
            let mut properties = std::collections::HashMap::new();
            properties.insert("classname".to_string(), classname.to_string());
            properties.insert("origin".to_string(), format!("{x} {y} 48"));
            Entity { properties }
        };
        let entities = vec![
            spawn("info_player_start", 0.0, 0.0),
            spawn("info_player_start", 0.0, 128.0),
            spawn("info_player_deathmatch", 256.0, 0.0),
            spawn("info_player_deathmatch", 256.0, 128.0),
        ];
        let bsp = Arc::new(BspMap {
            name: "de_p2p_test".to_string(),
            skyname: String::new(),
            entities,
            spawns: Vec::new(),
            hostage_spawns: Vec::new(),
            brush_entities: Vec::new(),
            planes,
            vertices_raw: Vec::new(),
            visibility: Vec::new(),
            nodes,
            texinfos: Vec::new(),
            faces: Vec::new(),
            lighting: Vec::new(),
            clipnodes,
            leafs: vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)],
            mark_surfaces: Vec::new(),
            edges: Vec::new(),
            surf_edges: Vec::new(),
            models: vec![model],
            textures: Vec::new(),
            lightmap_atlas: LightmapAtlas {
                width: 0,
                height: 0,
                rgba: Vec::new(),
                packed_faces: 0,
            },
            render_vertices: Vec::new(),
            render_indices: Vec::new(),
            batches: Vec::new(),
        });
        let mut raw = vec![0u8; 4 + 15 * 8];
        raw[..4].copy_from_slice(&30i32.to_le_bytes());
        let map =
            ServerMap::from_bsp("de_p2p_test", &raw, Arc::clone(&bsp)).expect("valid test BSP");
        (map, bsp)
    }

    #[test]
    fn p2p_host_exchanges_signon_movement_and_shots_with_web_peer() {
        let mut host = WebClientState::default();
        assert!(host.start_p2p_host("de_dust2", 0, 4).is_err());

        let (map, bsp) = p2p_test_floor_map();
        host.start_p2p_host_with_map(map, bsp, 0, 4)
            .expect("host signs on");
        assert!(host.is_p2p_host());
        assert_eq!(host.conn_state(), Some(ConnState::InGame));

        let mut guest = WebClientState::default();
        guest.connect_remote().expect("guest remote transport");

        let mut guest_in_game = false;
        for _ in 0..400 {
            for pkt in guest.take_net_packets() {
                assert!(host.push_p2p_packet(1, pkt));
            }
            let _ = host.step(0.01);
            for (peer_id, pkt) in host.drain_p2p_packets() {
                assert_eq!(peer_id, 1);
                guest.push_net_packet(pkt);
            }
            let _ = guest.step(0.01);
            if guest.conn_state() == Some(ConnState::InGame) {
                guest_in_game = true;
                break;
            }
        }
        assert!(guest_in_game, "WebRTC/Channel guest should reach InGame");

        // Join CT on host and T on guest, then advance past round freeze.
        host.exec_console("jointeam 2");
        host.exec_console("joinclass 1");
        guest.exec_console("jointeam 1");
        guest.exec_console("joinclass 1");

        let pump_both = |host: &mut WebClientState, guest: &mut WebClientState, frames: usize| {
            for _ in 0..frames {
                for pkt in guest.take_net_packets() {
                    let _ = host.push_p2p_packet(1, pkt);
                }
                let _ = host.step(0.01);
                for (_, pkt) in host.drain_p2p_packets() {
                    guest.push_net_packet(pkt);
                }
                let _ = guest.step(0.01);
            }
        };

        pump_both(&mut host, &mut guest, 120);
        if let SessionMode::Local { game, .. } = &mut host.session {
            let _ = game.console("mp_freezetime 0");
            let _ = game.console("sv_restart 1");
        }
        pump_both(&mut host, &mut guest, 220);

        let (start_pos, start_clip) = match &guest.session {
            SessionMode::Remote { conn, .. } => (
                conn.client_data().map_or(Vec3::ZERO, |d| d.origin),
                conn.fx().hud.current_weapon.map_or(0, |w| w.clip),
            ),
            _ => panic!("guest not in Remote session"),
        };

        // Guest moves forward and fires semi-auto Glock shots; verify position & clip change.
        guest.exec_console("+forward");
        for _ in 0..3 {
            guest.exec_console("+attack");
            pump_both(&mut host, &mut guest, 15);
            guest.exec_console("-attack");
            pump_both(&mut host, &mut guest, 15);
        }
        guest.exec_console("-forward");
        pump_both(&mut host, &mut guest, 20);

        let (end_pos, end_clip) = match &guest.session {
            SessionMode::Remote { conn, .. } => {
                assert_eq!(conn.stats().parse_errors, 0);
                (
                    conn.client_data().map_or(Vec3::ZERO, |d| d.origin),
                    conn.fx().hud.current_weapon.map_or(0, |w| w.clip),
                )
            }
            _ => panic!("guest not in Remote session"),
        };
        let moved_xy = (end_pos - start_pos).truncate().length();
        assert!(
            moved_xy > 10.0,
            "guest should move across P2P link (moved {moved_xy:.2} u)"
        );
        assert!(
            end_clip < start_clip,
            "guest shots should consume clip across P2P link ({start_clip} -> {end_clip})"
        );

        host.disconnect();
        assert!(!host.is_p2p_host());
    }

    #[test]
    fn instant_demo_boots_without_steam_and_never_shadows_real_assets() {
        // 1. Fresh WebClientState with zero Steam files -> start_instant_demo works out of the box!
        let mut st = WebClientState::default();
        assert!(st.list_maps().is_empty());
        let (files, bytes) = st
            .start_instant_demo(9)
            .expect("start_instant_demo with 9 bots");
        assert!(files > 120 && bytes > 100_000);
        assert_eq!(st.conn_state(), Some(ConnState::InGame));
        assert!(st.list_maps().contains(&"dm_rusthouse".to_string()));

        let out = st.step(0.01);
        assert_eq!(
            out.map_sync,
            MapSyncAction::Load("dm_rusthouse".to_string())
        );
        st.mark_map_loaded("dm_rusthouse");
        let mut saw_view = false;
        for _ in 0..100 {
            let step = st.step(0.01);
            if step.view.is_some() {
                saw_view = true;
            }
        }
        assert!(saw_view, "Instant Demo should emit FrameView renders");

        // 2. Real Steam assets already mounted -> mount_instant_demo MUST NOT shadow them!
        let mut st_steam = WebClientState::default();
        let steam_sentinel = b"STEAM_CSTRIKE_AK47".to_vec();
        st_steam.mount_file("cstrike/models/v_ak47.mdl", steam_sentinel.clone());
        st_steam.finish_mount();
        let _ = st_steam.mount_instant_demo();
        assert_eq!(
            st_steam.read_file("models/v_ak47.mdl").as_deref(),
            Some(steam_sentinel.as_slice())
        );
        assert!(st_steam.read_file("maps/dm_rusthouse.bsp").is_some());
    }

    #[test]
    fn web_defaults_to_vgui_menus_and_no_fake_weapon_bar_on_spawn() {
        let st = WebClientState::default();
        assert!(st.cvars.get_bool("_vgui_menus"));
        let opts = st.local_options(4, 16);
        assert!(
            !opts.text_menus,
            "_vgui_menus 1 must request VGUI menus, not text_menus"
        );
        assert!(st.weapon_buckets.active_slot.is_none());
    }
}
