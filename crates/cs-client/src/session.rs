//! The session layer between the window and `cs-clientcore`: no session, a local game (the
//! engine and the game in this process over loopback) or a connection to a server over UDP.
//! Both run the same [`Connection`]; only the transport differs. Nothing here touches a window
//! or a GPU, so the tests run headless.

use std::collections::BTreeMap;
use std::net::ToSocketAddrs;
use std::sync::Arc;

use cs_clientcore::{
    ConnState, Connection, ConnectionConfig, ConsoleCommand, DemoPlayer, ResourceFiles,
    SoundCommand, ViewAngleUpdate, VoiceEvent,
};
use cs_clientcore::events::FiredEvent;
use cs_clientcore::hud::HudState;
use cs_local::{FRAME_SECS, LocalGame, LocalOptions};
use cs_pm::collision::WorldCollision;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::info::InfoString;
use cs_transport::{PeerAddr, UdpTransport};
use cs_vfs::Vfs;
use glam::Vec3;

use cs_client::view::{FrameView, MapFxWorld, ViewBuilder, ViewInputs};

/// The port a server listens on when the address has none.
pub const DEFAULT_PORT: u16 = 27015;
/// The longest stretch of real time one frame steps the local server (a hitch is not caught up).
const MAX_STEP: f64 = 0.1;

enum Kind {
    None,
    Local {
        game: Box<LocalGame>,
        /// Real time not yet run as server frames.
        pending: f64,
    },
    Remote {
        conn: Box<Connection<UdpTransport>>,
        now: f64,
    },
    Demo {
        player: Box<DemoPlayer>,
        fired: Vec<FiredEvent>,
    },
}

/// Runs `$body` with `$conn` bound to the session's connection, whichever transport it has.
macro_rules! with_conn {
    ($self:expr, $conn:ident => $body:expr, else $none:expr) => {
        match &$self.kind {
            Kind::None | Kind::Demo { .. } => $none,
            Kind::Local { game, .. } => {
                let $conn = game.connection();
                $body
            }
            Kind::Remote { conn, .. } => {
                let $conn = &**conn;
                $body
            }
        }
    };
}

macro_rules! with_conn_mut {
    ($self:expr, $conn:ident => $body:expr, else $none:expr) => {
        match &mut $self.kind {
            Kind::None | Kind::Demo { .. } => $none,
            Kind::Local { game, .. } => {
                let $conn = game.connection_mut();
                $body
            }
            Kind::Remote { conn, .. } => {
                let $conn = &mut **conn;
                $body
            }
        }
    };
}

struct VfsResourceFiles(Vfs);

impl ResourceFiles for VfsResourceFiles {
    fn exists(&self, path: &str) -> bool {
        self.0.exists(path)
    }

    fn md5(&self, path: &str) -> Option<[u8; 16]> {
        self.0
            .read_bytes(path)
            .ok()
            .map(|b| cs_proto48::crypto::md5_digest(&b))
    }

    fn studio_bounds(&self, path: &str) -> Option<([f32; 3], [f32; 3])> {
        self.0
            .read_bytes(path)
            .ok()
            .and_then(|b| cs_assets::mdl::studio_bounds(&b))
    }
}

/// A session and the things the shell keeps for it.
pub struct Session {
    kind: Kind,
    pub view: ViewBuilder,
    map_name: Option<String>,
    collision: Option<Arc<WorldCollision>>,
    vfs: Option<Vfs>,
}

impl Session {
    pub fn none() -> Self {
        Self {
            kind: Kind::None,
            view: ViewBuilder::new(),
            map_name: None,
            collision: None,
            vfs: None,
        }
    }

    /// Starts a local game on `map` and signs the client on.
    pub fn start_local(vfs: &Vfs, map: &str, opts: &LocalOptions) -> Result<Self, String> {
        let game = LocalGame::start(vfs, map, opts)?;
        let mut s = Self::none();
        s.kind = Kind::Local {
            game: Box::new(game),
            pending: 0.0,
        };
        s.vfs = Some(vfs.clone());
        s.map_name = Some(map.to_string());
        s.setup_connection(opts.player_name.as_str(), true);
        Ok(s)
    }

    /// Connects to the server at `addr` (`host`, `host:port`); the answer comes in the frames.
    pub fn connect_remote(
        vfs: Option<&Vfs>,
        addr: &str,
        name: &str,
        vgui_menus: bool,
    ) -> Result<Self, String> {
        let target = if addr.contains(':') {
            addr.to_string()
        } else {
            format!("{addr}:{DEFAULT_PORT}")
        };
        let peer = target
            .to_socket_addrs()
            .map_err(|e| format!("cannot resolve {addr}: {e}"))?
            .find(std::net::SocketAddr::is_ipv4)
            .ok_or(format!("{addr} has no IPv4 address"))?;
        let transport = UdpTransport::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
        let steam_id = 76_561_197_960_265_728 + u64::from(std::process::id());
        let cfg = ConnectionConfig::new(name, steam_id, &md5_hex(b"cs16-rs client"));
        let mut conn = Connection::new(transport, PeerAddr::Udp(peer), cfg);
        conn.set_http_fetcher(Box::new(cs_client::http_dl::NativeHttpFetcher::new()));
        if let Some(v) = vfs {
            conn.set_files(Box::new(VfsResourceFiles(v.clone())));
            if let Ok(wad) = v.read_bytes("tempdecal.wad")
                && cs_assets::hpak::validate_spray_wad(&wad, 65536).is_ok()
            {
                let _ = conn.set_custom_spray(wad);
            }
        }
        conn.set_userinfo("_vgui_menus", if vgui_menus { "1" } else { "0" });
        conn.set_userinfo(cs_mod_api::MOD_CAPABILITY_KEY, "1");
        conn.connect();
        let mut s = Self::none();
        s.kind = Kind::Remote {
            conn: Box::new(conn),
            now: 0.0,
        };
        s.vfs = vfs.cloned();
        s.setup_connection(name, false);
        Ok(s)
    }

    fn setup_connection(&mut self, _name: &str, local: bool) {
        let vfs = self.vfs.clone();
        let names = self.view.names();
        let collision = self.collision.clone();
        with_conn_mut!(self, c => {
            c.frames_mut().set_remote_is_local(local);
            c.set_fx_world(Box::new(MapFxWorld::new(collision, vfs, names)));
        }, else ());
    }

    /// Starts offline `.dem` playback from parsed `HLDEMO` Protocol 5 bytes.
    pub fn start_demo(vfs: Option<&Vfs>, bytes: &[u8]) -> Result<Self, String> {
        let player = DemoPlayer::from_bytes(bytes).map_err(|e| format!("bad demo: {e}"))?;
        let mut s = Self::none();
        s.vfs = vfs.cloned();
        s.kind = Kind::Demo {
            player: Box::new(player),
            fired: Vec::new(),
        };
        Ok(s)
    }

    pub fn is_demo(&self) -> bool {
        matches!(self.kind, Kind::Demo { .. })
    }

    pub fn demo_player(&self) -> Option<&DemoPlayer> {
        match &self.kind {
            Kind::Demo { player, .. } => Some(player),
            _ => None,
        }
    }

    pub fn demo_player_mut(&mut self) -> Option<&mut DemoPlayer> {
        match &mut self.kind {
            Kind::Demo { player, .. } => Some(player),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn is_recording(&self) -> bool {
        with_conn!(self, c => c.is_recording(), else false)
    }

    pub fn start_recording(&mut self, game_dir: &str) {
        with_conn_mut!(self, c => c.start_recording(game_dir), else ());
    }

    pub fn stop_recording(&mut self) -> Result<Option<Vec<u8>>, String> {
        with_conn_mut!(
            self,
            c => c.stop_recording().map_err(|e| format!("demo encode error: {e}")),
            else Ok(None)
        )
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.kind, Kind::None)
    }

    pub fn state(&self) -> Option<ConnState> {
        match &self.kind {
            Kind::Demo { .. } => Some(ConnState::InGame),
            _ => with_conn!(self, c => Some(c.state().clone()), else None),
        }
    }

    pub fn in_game(&self) -> bool {
        self.state() == Some(ConnState::InGame)
    }

    pub fn download_progress(&self) -> Option<(usize, Option<String>)> {
        with_conn!(self, c => Some(c.download_progress()), else None)
    }

    pub fn remote_udp_addr(&self) -> Option<std::net::SocketAddr> {
        match &self.kind {
            Kind::Remote { conn, .. } => match conn.server_addr() {
                PeerAddr::Udp(sa) => Some(sa),
                _ => None,
            },
            _ => None,
        }
    }

    /// The session clock: what the connection is pumped with and the effects run on.
    pub fn now(&self) -> f64 {
        match &self.kind {
            Kind::None => 0.0,
            Kind::Local { game, pending } => game.time() + pending,
            Kind::Remote { now, .. } => *now,
            Kind::Demo { player, .. } => player.current_time(),
        }
    }

    /// Steps the session by `dt` real seconds: a local game runs its 10 ms server frames, a
    /// remote one pumps the socket.
    pub fn advance(&mut self, dt: f64) {
        match &mut self.kind {
            Kind::None => {}
            Kind::Local { game, pending } => {
                *pending += dt.clamp(0.0, MAX_STEP);
                while *pending >= FRAME_SECS {
                    game.step();
                    *pending -= FRAME_SECS;
                }
            }
            Kind::Remote { conn, now } => {
                *now += dt;
                conn.pump(*now);
            }
            Kind::Demo { player, fired } => {
                fired.extend(player.advance(dt));
            }
        }
    }

    /// The map the server runs, once the resource list named it.
    pub fn server_map(&self) -> Option<String> {
        match &self.kind {
            Kind::Demo { player, .. } => {
                let m = player
                    .precache()
                    .world_model()
                    .unwrap_or_else(|| player.map_name());
                (!m.is_empty()).then(|| {
                    m.trim_start_matches("maps/")
                        .trim_end_matches(".bsp")
                        .to_string()
                })
            }
            _ => with_conn!(self, c => c
                .precache()
                .world_model()
                .map(|m| m.trim_start_matches("maps/").trim_end_matches(".bsp").to_string()),
                else None),
        }
    }

    pub fn loaded_map(&self) -> Option<&str> {
        self.map_name.as_deref()
    }

    pub fn set_map_bsp(&mut self, bsp: Option<Arc<cs_assets::bsp::BspMap>>) {
        self.view.set_map_bsp(bsp);
    }

    pub fn collision(&self) -> Option<&Arc<WorldCollision>> {
        self.collision.as_ref()
    }

    /// Gives the prediction and the temp entities the map's collision (after each map change).
    pub fn set_map(&mut self, name: &str, collision: Option<Arc<WorldCollision>>) {
        self.map_name = Some(name.to_string());
        self.collision = collision.clone();
        self.view.reset_map();
        self.view
            .set_map_bounds(collision.as_deref());
        self.view
            .set_world_context(collision.clone(), self.vfs.clone());
        let vfs = self.vfs.clone();
        let names = self.view.names();
        match &mut self.kind {
            Kind::Demo { player, .. } => {
                player.set_fx_world(Box::new(MapFxWorld::new(collision, vfs, names)));
            }
            _ => with_conn_mut!(self, c => {
                c.set_world_collision(collision.clone());
                c.set_fx_world(Box::new(MapFxWorld::new(collision, vfs, names)));
            }, else ()),
        }
    }

    pub fn send_stringcmd(&mut self, cmd: &str) {
        with_conn_mut!(self, c => c.send_stringcmd(cmd), else ());
    }

    /// A line of the connection's own console (`cmd`, `setinfo`).
    pub fn execute_local(&mut self, line: &str) {
        with_conn_mut!(self, c => c.execute_local(line), else ());
    }

    /// `ex_interp` after the bounds of `CL_ComputeClientInterpolationAmount`, in seconds.
    pub fn interp(&self) -> f32 {
        with_conn!(self, c => c.frames().settings.interp, else 0.1)
    }

    pub fn queue_cmd(&mut self, cmd: GoldSrcUserCmd) {
        with_conn_mut!(self, c => c.queue_cmd(cmd), else ());
    }

    pub fn take_shell_commands(&mut self) -> Vec<ConsoleCommand> {
        with_conn_mut!(self, c => c.take_shell_commands(), else Vec::new())
    }

    /// The server console of a local game; `None` when the server is somewhere else.
    pub fn server_console(&mut self, line: &str) -> Option<String> {
        match &mut self.kind {
            Kind::Local { game, .. } => Some(game.console(line)),
            _ => None,
        }
    }

    /// Ends the session: a local server is shut down, a remote one told `disconnect`.
    pub fn close(&mut self) {
        match &mut self.kind {
            Kind::Local { game, .. } => game.shutdown(),
            Kind::Remote { conn, .. } => conn.execute_local("disconnect"),
            Kind::None | Kind::Demo { .. } => {}
        }
        *self = Self::none();
    }

    /// The sound commands since the last call.
    pub fn take_sounds(&mut self) -> Vec<SoundCommand> {
        match &mut self.kind {
            Kind::Demo { player, .. } => player.take_sounds(),
            _ => with_conn_mut!(self, c => c.fx_mut().sounds.drain(), else Vec::new()),
        }
    }

    /// Incoming `svc_voiceinit` and `svc_voicedata` events since the last call.
    pub fn take_voice_events(&mut self) -> Vec<VoiceEvent> {
        with_conn_mut!(self, c => c.fx_mut().voice.drain(), else Vec::new())
    }

    /// Queues an encoded outgoing `clc_voicedata` packet for the server.
    pub fn send_voice_data(&mut self, data: &[u8]) {
        with_conn_mut!(self, c => c.send_voice_data(data), else ());
    }

    pub fn with_hud<R>(&self, f: impl FnOnce(&HudState) -> R) -> Option<R> {
        match &self.kind {
            Kind::Demo { player, .. } => Some(f(player.hud())),
            _ => with_conn!(self, c => Some(f(&c.fx().hud)), else None),
        }
    }

    pub fn with_hud_mut<R>(&mut self, f: impl FnOnce(&mut HudState) -> R) -> Option<R> {
        match &mut self.kind {
            Kind::Demo { player, .. } => Some(f(player.hud_mut())),
            _ => with_conn_mut!(self, c => Some(f(&mut c.fx_mut().hud)), else None),
        }
    }

    /// Player names by slot, from the userinfo strings.
    pub fn player_names(&self) -> BTreeMap<u8, String> {
        let map = match &self.kind {
            Kind::Demo { player, .. } => player.userinfos(),
            _ => {
                return with_conn!(self, c => c
                    .userinfos()
                    .iter()
                    .filter_map(|(slot, raw)| {
                        InfoString::parse(raw)
                            .get("name")
                            .map(|n| (*slot, n.to_string()))
                    })
                    .collect(),
                    else BTreeMap::new());
            }
        };
        map.iter()
            .filter_map(|(slot, raw)| {
                InfoString::parse(raw)
                    .get("name")
                    .map(|n| (*slot, n.to_string()))
            })
            .collect()
    }

    pub fn local_slot(&self) -> Option<u8> {
        match &self.kind {
            Kind::Demo { player, .. } => player.server_info().map(|i| i.player_slot),
            _ => with_conn!(self, c => c.player_slot(), else None),
        }
    }

    /// The weapons the player carries, as `weapon_id` bits of the client data.
    pub fn owned_weapons(&self) -> u32 {
        match &self.kind {
            Kind::Demo { player, .. } => player.client_data().map_or(0, |d| d.weapons),
            _ => with_conn!(self, c => c.client_data().map_or(0, |d| d.weapons), else 0),
        }
    }

    /// The footsteps and other sounds the prediction made.
    pub fn take_predicted_sounds(&mut self) -> Vec<cs_clientcore::predict::PredictedSound> {
        with_conn_mut!(self, c => c.predictor_mut().take_sounds(), else Vec::new())
    }

    /// `sound/materials.txt`, for the texture sounds of the prediction.
    pub fn connection_predictor_materials(&mut self, txt: &[u8]) {
        with_conn_mut!(self, c => c.predictor_mut().set_materials(txt), else ());
    }

    /// The horizontal speed of the local player, from the prediction.
    pub fn local_speed(&self) -> f32 {
        with_conn!(self, c => {
            let v = c.predictor().prediction().simvel;
            Vec3::new(v.x, v.y, 0.0).length()
        }, else 0.0)
    }

    /// Crosshair inputs from prediction & HUD state:
    /// `(weapon_id, weapon_flags, on_ground, ducking, speed_2d, clip)`.
    pub fn local_crosshair_state(&self) -> (u8, u32, bool, bool, f32, i32) {
        with_conn!(self, c => {
            let p = c.predictor().prediction();
            let cur_hud = c.fx().hud.current_weapon;
            let wid = if p.predicted && p.weapon.weapon_id > 0 {
                p.weapon.weapon_id as u8
            } else {
                cur_hud.map_or(0, |w| w.weapon_id)
            };
            let wstate = p
                .weapon
                .weapons
                .get(&wid)
                .map_or(0, |w| w.m_i_weapon_state as u32);
            let clip = p
                .weapon
                .weapons
                .get(&wid)
                .map(|w| w.m_i_clip)
                .or_else(|| cur_hud.map(|w| i32::from(w.clip)))
                .unwrap_or(0);
            let cd = c.client_data();
            let on_ground = p.onground >= 0 || cd.is_some_and(|d| (d.flags & 0x200) != 0);
            let ducking = (p.flags & 0x4000) != 0 || cd.is_some_and(|d| (d.flags & 0x4000) != 0);
            let spd = Vec3::new(p.simvel.x, p.simvel.y, 0.0).length();
            (wid, wstate, on_ground, ducking, spd, clip)
        }, else (0, 0, true, false, 0.0, 0))
    }

    /// Drains new console lines (`Con_Printf`) since the last call.
    pub fn take_console_lines(&mut self) -> Vec<String> {
        match &mut self.kind {
            Kind::Demo { player, .. } => player.take_console_lines(),
            _ => with_conn_mut!(self, c => c.take_console_lines(), else Vec::new()),
        }
    }

    /// Drains server-forced view angle updates (`svc_setangle`, `svc_addangle`) since the last call.
    pub fn take_view_angle_updates(&mut self) -> Vec<ViewAngleUpdate> {
        match &mut self.kind {
            Kind::Demo { player, .. } => player
                .take_view_angles()
                .into_iter()
                .map(ViewAngleUpdate::Set)
                .collect(),
            _ => with_conn_mut!(self, c => c.take_view_angle_updates(), else Vec::new()),
        }
    }

    /// Drains raw custom `UserMsg` packets not handled by built-in GoldSrc HUD reducers.
    pub fn drain_unknown_usermsgs(&mut self) -> Vec<cs_clientcore::hud::UnknownUserMsg> {
        match &mut self.kind {
            Kind::Demo { player, .. } => player
                .hud_mut()
                .unknown_messages
                .drain(..)
                .collect(),
            _ => with_conn_mut!(self, c => c.fx_mut().hud.unknown_messages.drain(..).collect(), else Vec::new()),
        }
    }

    pub fn has_client_cvar(&self, name: &str) -> bool {
        with_conn!(self, c => c.cvars().find(name).is_some(), else false)
    }

    pub fn client_cvar(&self, name: &str) -> Option<String> {
        with_conn!(self, c => c.cvars().find(name).map(|v| v.value.clone()), else None)
    }

    pub fn set_client_cvar(&mut self, name: &str, value: &str) {
        with_conn_mut!(self, c => c.set_cvar(name, value), else ());
    }

    #[cfg(test)]
    fn precache_model_names(&self) -> Vec<String> {
        match &self.kind {
            Kind::Demo { player, .. } => {
                player.precache().models.values().map(|e| e.name.clone()).collect()
            }
            _ => with_conn!(self, c => c.precache().models.values().map(|e| e.name.clone()).collect(), else Vec::new()),
        }
    }

    pub fn max_clients(&self) -> u8 {
        match &self.kind {
            Kind::Demo { player, .. } => player.server_info().map_or(0, |i| i.max_clients),
            _ => with_conn!(self, c => c.server_info().map_or(0, |i| i.max_clients), else 0),
        }
    }

    /// Builds the frame: advances the client clock, runs the effects, interpolates. `None`
    /// without a session or before the client is in the game.
    pub fn build_view(
        &mut self,
        view_angles: Vec3,
        inputs: &ViewInputs,
    ) -> Option<(FrameView, Option<cs_client::render::scene::ModelRegistry>)> {
        let Session { kind, view, .. } = self;
        match kind {
            Kind::None => None,
            Kind::Local { game, .. } => Some(view.build(game.connection_mut(), view_angles, inputs)),
            Kind::Remote { conn, .. } => Some(view.build(&mut **conn, view_angles, inputs)),
            Kind::Demo { player, fired } => {
                let events = std::mem::take(fired);
                Some(view.build_demo(player, events, view_angles, inputs))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_client::input::{CmdParams, Held, MoveSpeeds, build_usercmd};
    use cs_client::view::ViewInputs;
    use cs_assets::bsp::BspMap;
    use cs_vfs::VfsConfig;
    use std::collections::HashSet;

    /// The app's session layer plays a local game headless: sign-on, map, usercmds, frames,
    /// the scene with the bots, the HUD and the camera that follows the predicted player.
    #[test]
    fn a_local_game_plays_through_the_session_layer() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            eprintln!("SKIPPED: no game directory (set CS16_GAMEDIR)");
            return;
        };
        let opts = LocalOptions {
            bots: 3,
            max_clients: 8,
            text_menus: true,
            ..LocalOptions::default()
        };
        let mut s = Session::start_local(&vfs, "de_dust2", &opts).expect("local game");
        assert!(s.in_game());
        let map = BspMap::load(&vfs, "de_dust2").expect("map");
        s.set_map("de_dust2", Some(Arc::new(WorldCollision::from_bsp(&map))));
        assert_eq!(s.server_map().as_deref(), Some("de_dust2"));
        assert_eq!(s.loaded_map(), Some("de_dust2"));

        let inputs = |now: f64| ViewInputs {
            now,
            frametime: 0.01,
            right_hand: true,
            draw_viewmodel: true,
            default_fov: 90.0,
            ..ViewInputs::default()
        };
        // The server shows the team menu as a text menu (`_vgui_menus 0` was in the userinfo
        // before the sign-on); the shell answers it with `menuselect`, as a key press does.
        let mut answered = Vec::new();
        for choice in ["2", "1"] {
            let mut text = None;
            for _ in 0..600 {
                s.advance(0.01);
                text = s.with_hud(|h| h.menu.as_ref().map(|m| m.text.clone())).flatten();
                if text.is_some() {
                    break;
                }
            }
            let text = text.unwrap_or_else(|| panic!("no menu appeared before choice {choice}"));
            answered.push(text);
            s.send_stringcmd(&format!("menuselect {choice}"));
            s.with_hud_mut(|h| h.menu = None);
        }
        assert_eq!(answered.len(), 2, "the team and the class menu");
        let walk: HashSet<String> = ["forward".to_string()].into_iter().collect();
        let mut first_eye = None;
        let mut last_eye = Vec3::ZERO;
        let mut farthest = 0.0f32;
        let mut most_studio = 0;
        let mut had_viewmodel = false;
        // 15 s covers freezetime and forward movement before enemy ZBots cross de_dust2 to kill the unarmed player.
        for frame in 0..1500 {
            // Freeze time first so the bots and the round start; then walk.
            let held = Held {
                actions: Some(&walk),
                ..Held::default()
            };
            // Turning while walking keeps the player off the walls.
            let angles = Vec3::new(0.0, (frame as f32 * 0.4) % 360.0, 0.0);
            let p = CmdParams {
                msec: 10,
                ..CmdParams::default()
            };
            s.queue_cmd(build_usercmd(&held, angles, p, &MoveSpeeds::default()));
            s.advance(0.01);
            let (view, _) = s
                .build_view(angles, &inputs(s.now()))
                .expect("a view");
            if frame == 100 {
                first_eye = Some(view.eye);
            }
            if let Some(e0) = first_eye {
                farthest = farthest.max((view.eye - e0).length());
            }
            last_eye = view.eye;
            most_studio = most_studio.max(view.stats.studio);
            had_viewmodel |= view.viewmodel.is_some();
            assert!(view.eye.is_finite());
        }
        let hp = s.with_hud(|h| h.health).expect("hud");
        assert!(hp > 0, "the player is alive");
        assert!(most_studio > 0, "the bots are in the scene as studio models");
        // `viewmodel` of the client data names the weapon in hand. The game sets it from the
        // precache table; the shell shows it when it is there (see the report: the game
        // precaches no `v_*.mdl`, so the local server sends 0 until it does).
        let v_models = s.precache_model_names().iter().filter(|n| n.contains("/v_")).count();
        eprintln!("view models precached by the server: {v_models}, shown: {had_viewmodel}");
        assert!(had_viewmodel || v_models == 0, "a precached view model was not shown");
        assert!(s.local_slot().is_some());
        // The camera follows the predicted player: walking forward moved it away
        // from where it stood at frame 100 (the freeze time ends before the walk is over).
        let _ = last_eye;
        assert!(farthest > 100.0, "the camera did not follow the walking player: {farthest}");
        s.close();
        assert!(!s.is_active());
    }

    #[test]
    fn loopback_demo_record_playback_seek_and_timedemo() {
        let Ok(vfs) = Vfs::discover(None, cs_vfs::VfsConfig::default()) else {
            return;
        };
        if !vfs.exists("maps/de_dust2.bsp") {
            return;
        }
        let mut s = Session::start_local(&vfs, "de_dust2", &LocalOptions::default())
            .expect("local de_dust2");
        s.start_recording("cstrike");
        assert!(s.is_recording());
        let angles = Vec3::new(0.0, 45.0, 0.0);
        for _ in 0..80 {
            s.queue_cmd(build_usercmd(
                &Default::default(),
                angles,
                Default::default(),
                &MoveSpeeds::default(),
            ));
            s.advance(0.01);
        }
        assert!(s.in_game());
        let bytes = s.stop_recording().expect("finish").expect("demo bytes");
        assert!(!s.is_recording());
        s.close();

        let parsed = cs_proto48::demo::parse_demo(&bytes).expect("valid HLDEMO v5");
        assert!(!parsed.directory.is_empty());

        let mut demo = Session::start_demo(Some(&vfs), &bytes).expect("start demo");
        assert!(demo.is_demo());
        assert!(demo.in_game());
        let demo_inputs = |now: f64| ViewInputs {
            now,
            frametime: 0.01,
            right_hand: true,
            draw_viewmodel: true,
            default_fov: 90.0,
            ..ViewInputs::default()
        };
        for _ in 0..10 {
            demo.advance(0.05);
            let _ = demo.build_view(angles, &demo_inputs(demo.now()));
        }
        let mid_t = demo.now();
        demo.demo_player_mut().unwrap().seek(mid_t * 0.5);
        assert!(demo.now() <= mid_t * 0.5 + 1e-4);
        let report = demo.demo_player_mut().unwrap().run_timedemo_all(1.0 / 100.0);
        assert!(report.frames > 0);
        demo.close();
    }
}
