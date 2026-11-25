//! Client-side GoldSrc `HLDEMO` Protocol 5 recording, playback, `timedemo`, and seeking.
//!
//! Works platform-freely on both native and `wasm32-unknown-unknown`.

use std::collections::{BTreeMap, VecDeque};

use cs_proto48::bitbuf::ProtoError;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcWeaponData};
use cs_proto48::demo::{
    DEMO_PROTOCOL, DemoClientData, DemoDirectoryEntry, DemoFile, DemoFrame, DemoFrameData,
    DemoHeader, DemoNetMsgFrame, DemoNetMsgInfo, DemoUserCmd, parse_demo, write_demo,
};
use cs_proto48::messages::{
    MoveVarsMsg, ServerInfoMsg, SvcMessage, SvcParseContext, parse_svc_stream_partial,
};
use glam::Vec3;

use crate::events::{FiredEvent, FxEnv};
use crate::frames::ClientFrames;
use crate::hud::{ClientEffects, HudState};
use crate::precache::PrecacheTables;
use crate::sound::{SoundCommand, SoundStart};
use crate::tempents::{FxWorld, NoWorld};

const KEYFRAME_INTERVAL_SECS: f64 = 2.0;
const MAX_DEMO_CONSOLE_LINES: usize = 256;

/// Live `.dem` recorder fed from inbound `svc_*` packets.
#[derive(Debug, Clone)]
pub struct DemoRecorder {
    map_name: String,
    game_dir: String,
    map_crc: u32,
    net_protocol: i32,
    start_time: Option<f64>,
    signon_frames: Vec<DemoFrame>,
    playback_frames: Vec<DemoFrame>,
    signon_complete: bool,
    frame_counter: i32,
}

impl DemoRecorder {
    pub fn new(
        map_name: impl Into<String>,
        game_dir: impl Into<String>,
        map_crc: u32,
        net_protocol: i32,
    ) -> Self {
        Self {
            map_name: map_name.into(),
            game_dir: game_dir.into(),
            map_crc,
            net_protocol,
            start_time: None,
            signon_frames: Vec::new(),
            playback_frames: Vec::new(),
            signon_complete: false,
            frame_counter: 0,
        }
    }

    pub fn set_map_info(&mut self, map_name: &str, game_dir: &str, map_crc: u32) {
        if !map_name.is_empty() {
            self.map_name = map_name.to_string();
        }
        if !game_dir.is_empty() {
            self.game_dir = game_dir.to_string();
        }
        if map_crc != 0 {
            self.map_crc = map_crc;
        }
    }

    fn rel_time(&mut self, now: f64) -> f32 {
        let base = *self.start_time.get_or_insert(now);
        (now - base).max(0.0) as f32
    }

    pub fn mark_signon_complete(&mut self, now: f64) {
        if self.signon_complete {
            return;
        }
        self.signon_complete = true;
        if !self.signon_frames.is_empty() {
            let t = self.rel_time(now);
            self.signon_frames.push(DemoFrame {
                frame_type_raw: 5,
                time: t,
                frame: self.frame_counter,
                data: DemoFrameData::NextSection,
            });
        }
    }

    pub fn record_svc_packet(
        &mut self,
        now: f64,
        is_signon: bool,
        raw_svc_bytes: &[u8],
        info: DemoNetMsgInfo,
        sequences: [i32; 7],
    ) {
        if raw_svc_bytes.is_empty() {
            return;
        }
        let time = self.rel_time(now);
        let frame_num = self.frame_counter;
        self.frame_counter = self.frame_counter.saturating_add(1);
        let frame_type_raw = if is_signon && !self.signon_complete { 0 } else { 1 };
        let frame = DemoFrame {
            frame_type_raw,
            time,
            frame: frame_num,
            data: DemoFrameData::NetMsg(Box::new(DemoNetMsgFrame {
                info,
                incoming_sequence: sequences[0],
                incoming_acknowledged: sequences[1],
                incoming_reliable_acknowledged: sequences[2],
                incoming_reliable_sequence: sequences[3],
                outgoing_sequence: sequences[4],
                reliable_sequence: sequences[5],
                last_reliable_sequence: sequences[6],
                msg: raw_svc_bytes.to_vec(),
            })),
        };
        if is_signon && !self.signon_complete {
            self.signon_frames.push(frame);
        } else {
            self.playback_frames.push(frame);
        }
    }

    pub fn record_console_command(&mut self, now: f64, cmd: &str) {
        let time = self.rel_time(now);
        let frame_num = self.frame_counter;
        self.frame_counter = self.frame_counter.saturating_add(1);
        self.playback_frames.push(DemoFrame {
            frame_type_raw: 3,
            time,
            frame: frame_num,
            data: DemoFrameData::ConsoleCommand(cmd.to_string()),
        });
    }

    pub fn record_client_data(&mut self, now: f64, cd: DemoClientData) {
        let time = self.rel_time(now);
        let frame_num = self.frame_counter;
        self.frame_counter = self.frame_counter.saturating_add(1);
        self.playback_frames.push(DemoFrame {
            frame_type_raw: 4,
            time,
            frame: frame_num,
            data: DemoFrameData::ClientData(cd),
        });
    }

    pub fn frame_count(&self) -> usize {
        self.signon_frames.len() + self.playback_frames.len()
    }

    pub fn finish(mut self) -> Result<Vec<u8>, ProtoError> {
        let last_t = self
            .playback_frames
            .last()
            .or_else(|| self.signon_frames.last())
            .map(|f| f.time)
            .unwrap_or(0.0);

        if !self.playback_frames.is_empty()
            && !matches!(
                self.playback_frames.last().map(|f| &f.data),
                Some(DemoFrameData::NextSection)
            )
        {
            self.playback_frames.push(DemoFrame {
                frame_type_raw: 5,
                time: last_t,
                frame: self.frame_counter,
                data: DemoFrameData::NextSection,
            });
        }

        let mut directory = Vec::new();
        if !self.signon_frames.is_empty() {
            if !matches!(
                self.signon_frames.last().map(|f| &f.data),
                Some(DemoFrameData::NextSection)
            ) {
                let st = self.signon_frames.last().map(|f| f.time).unwrap_or(0.0);
                self.signon_frames.push(DemoFrame {
                    frame_type_raw: 5,
                    time: st,
                    frame: self.frame_counter,
                    data: DemoFrameData::NextSection,
                });
            }
            let signon_time = self.signon_frames.last().map(|f| f.time).unwrap_or(0.0);
            directory.push(DemoDirectoryEntry {
                entry_type: 0,
                description: "LOADING".into(),
                flags: 0,
                cd_track: 0,
                track_time: signon_time,
                frame_count: self.signon_frames.len() as i32,
                offset: 0,
                file_length: 0,
                frames: self.signon_frames,
            });
        }

        let play_frames = if self.playback_frames.is_empty() {
            vec![DemoFrame {
                frame_type_raw: 5,
                time: 0.0,
                frame: 0,
                data: DemoFrameData::NextSection,
            }]
        } else {
            self.playback_frames
        };
        directory.push(DemoDirectoryEntry {
            entry_type: 1,
            description: "Playback".into(),
            flags: 0,
            cd_track: 0,
            track_time: last_t,
            frame_count: play_frames.len() as i32,
            offset: 0,
            file_length: 0,
            frames: play_frames,
        });

        let demo = DemoFile {
            header: DemoHeader {
                demo_protocol: DEMO_PROTOCOL,
                net_protocol: self.net_protocol,
                map_name: self.map_name,
                game_dir: self.game_dir,
                map_crc: self.map_crc,
                directory_offset: 0,
            },
            directory,
        };
        write_demo(&demo)
    }
}

/// Queue of demo filenames for `startdemos` / `demos`.
#[derive(Debug, Default, Clone)]
pub struct DemoPlaylist {
    demos: Vec<String>,
    index: usize,
}

impl DemoPlaylist {
    pub fn start(&mut self, names: Vec<String>) -> Option<&str> {
        self.demos = names;
        self.index = 0;
        self.current()
    }

    pub fn current(&self) -> Option<&str> {
        self.demos.get(self.index).map(String::as_str)
    }

    pub fn advance(&mut self) -> Option<&str> {
        if self.demos.is_empty() {
            return None;
        }
        self.index = (self.index + 1) % self.demos.len();
        self.current()
    }

    pub fn clear(&mut self) {
        self.demos.clear();
        self.index = 0;
    }
}

/// Result summary of a `timedemo` unthrottled benchmark run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeDemoReport {
    pub frames: u64,
    pub elapsed_secs: f64,
    pub avg_fps: f64,
}

#[derive(Debug, Clone)]
struct TimelineFrame {
    time: f64,
    frame: DemoFrame,
}

#[derive(Debug, Clone)]
struct DemoSnapshotState {
    ctx: SvcParseContext,
    frames: ClientFrames,
    precache: PrecacheTables,
    fx: ClientEffects,
    latest_entities: BTreeMap<u16, GoldSrcEntityState>,
    server_info: Option<ServerInfoMsg>,
    movevars: Option<MoveVarsMsg>,
    client_data: Option<GoldSrcClientData>,
    weapon_data: BTreeMap<u8, GoldSrcWeaponData>,
    userinfos: BTreeMap<u8, String>,
    view_angles: Option<Vec3>,
    latest_cmd: Option<DemoUserCmd>,
    demo_client_data: Option<DemoClientData>,
    cursor: usize,
    playback_time: f64,
    finished: bool,
}

impl Default for DemoSnapshotState {
    fn default() -> Self {
        Self {
            ctx: SvcParseContext::new(),
            frames: ClientFrames::new(),
            precache: PrecacheTables::default(),
            fx: ClientEffects::default(),
            latest_entities: BTreeMap::new(),
            server_info: None,
            movevars: None,
            client_data: None,
            weapon_data: BTreeMap::new(),
            userinfos: BTreeMap::new(),
            view_angles: None,
            latest_cmd: None,
            demo_client_data: None,
            cursor: 0,
            playback_time: 0.0,
            finished: false,
        }
    }
}

/// Complete platform-free GoldSrc `.dem` player with speed control, `timedemo`, and seeking.
pub struct DemoPlayer {
    demo: DemoFile,
    timeline: Vec<TimelineFrame>,
    state: DemoSnapshotState,
    keyframes: Vec<DemoSnapshotState>,
    fx_world: Box<dyn FxWorld>,
    paused: bool,
    speed: f32,
    total_duration: f64,
    console_lines: VecDeque<String>,
    pending_view_angles: Vec<Vec3>,
    timedemo_mode: bool,
    timedemo_frames: u64,
    timedemo_elapsed: f64,
}

fn make_fx_env<'a>(
    precache: &'a PrecacheTables,
    latest_entities: &'a BTreeMap<u16, GoldSrcEntityState>,
    server_info: Option<&'a ServerInfoMsg>,
    client_data: Option<&'a GoldSrcClientData>,
    world: &'a dyn FxWorld,
    time: f64,
) -> FxEnv<'a> {
    let mut env = FxEnv::new(time, precache, latest_entities, world);
    env.max_clients = server_info.map(|i| u16::from(i.max_clients)).unwrap_or(32);
    env.local_player = server_info.map(|i| u16::from(i.player_slot) + 1).unwrap_or(0);
    if let Some(cd) = client_data {
        env.local_origin = cd.origin;
        env.local_view_ofs = cd.view_ofs;
        env.local_velocity = cd.velocity;
    }
    env
}

impl DemoPlayer {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtoError> {
        let demo = parse_demo(bytes)?;
        let mut signon_frames = Vec::new();
        let mut raw_timeline = Vec::new();
        for entry in &demo.directory {
            if entry.entry_type == 0 {
                signon_frames.extend(entry.frames.iter().cloned());
            } else {
                raw_timeline.extend(entry.frames.iter().cloned());
            }
        }
        if raw_timeline.is_empty() && !signon_frames.is_empty() {
            raw_timeline = std::mem::take(&mut signon_frames);
        }

        let base_t = raw_timeline
            .iter()
            .filter(|f| !matches!(f.data, DemoFrameData::NextSection))
            .map(|f| f64::from(f.time))
            .filter(|t| t.is_finite())
            .min_by(|a, b| a.total_cmp(b))
            .unwrap_or(0.0);

        let mut timeline = Vec::with_capacity(raw_timeline.len());
        let mut max_t = 0.0f64;
        for f in raw_timeline {
            let t = if f.time.is_finite() {
                (f64::from(f.time) - base_t).max(0.0)
            } else {
                max_t
            };
            max_t = max_t.max(t);
            timeline.push(TimelineFrame { time: t, frame: f });
        }

        let mut player = Self {
            demo,
            timeline,
            state: DemoSnapshotState::default(),
            keyframes: Vec::new(),
            fx_world: Box::new(NoWorld),
            paused: false,
            speed: 1.0,
            total_duration: max_t,
            console_lines: VecDeque::new(),
            pending_view_angles: Vec::new(),
            timedemo_mode: false,
            timedemo_frames: 0,
            timedemo_elapsed: 0.0,
        };

        for f in &signon_frames {
            if !matches!(f.data, DemoFrameData::NextSection) {
                player.apply_demo_frame(f, 0.0, false);
            }
        }
        player.state.finished = false;
        player.keyframes.push(player.state.clone());
        Ok(player)
    }

    pub fn set_fx_world(&mut self, world: Box<dyn FxWorld>) {
        self.fx_world = world;
    }

    pub fn header(&self) -> &DemoHeader {
        &self.demo.header
    }

    pub fn map_name(&self) -> &str {
        if let Some(info) = &self.state.server_info {
            &info.map_path
        } else {
            &self.demo.header.map_name
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    pub fn set_speed(&mut self, speed: f32) {
        if speed.is_finite() {
            self.speed = speed.clamp(0.1, 8.0);
        }
    }

    pub fn current_time(&self) -> f64 {
        self.state.playback_time
    }

    pub fn total_duration(&self) -> f64 {
        self.total_duration
    }

    pub fn cursor(&self) -> usize {
        self.state.cursor
    }

    pub fn total_frames(&self) -> usize {
        self.timeline.len()
    }

    pub fn is_finished(&self) -> bool {
        self.state.finished
    }

    pub fn start_timedemo(&mut self) {
        self.timedemo_mode = true;
        self.timedemo_frames = 0;
        self.timedemo_elapsed = 0.0;
        self.paused = false;
    }

    pub fn is_timedemo(&self) -> bool {
        self.timedemo_mode
    }

    /// Consumes the remainder of the demo at unthrottled speed (1 frame per tick) and returns
    /// timing metrics.
    pub fn run_timedemo_all(&mut self, synthetic_dt_per_frame: f64) -> TimeDemoReport {
        self.start_timedemo();
        let step = if synthetic_dt_per_frame.is_finite() && synthetic_dt_per_frame > 0.0 {
            synthetic_dt_per_frame
        } else {
            1.0 / 60.0
        };
        while !self.state.finished && self.state.cursor < self.timeline.len() {
            self.step_frame();
            self.timedemo_frames = self.timedemo_frames.saturating_add(1);
            self.timedemo_elapsed += step;
        }
        let elapsed = self.timedemo_elapsed.max(f64::EPSILON);
        TimeDemoReport {
            frames: self.timedemo_frames,
            elapsed_secs: self.timedemo_elapsed,
            avg_fps: (self.timedemo_frames as f64) / elapsed,
        }
    }

    /// Advances demo playback by `dt` real seconds (scaled by `speed`, no-op when `paused`).
    pub fn advance(&mut self, dt: f64) -> Vec<FiredEvent> {
        if self.paused || self.state.finished || !dt.is_finite() || dt <= 0.0 {
            return Vec::new();
        }
        if self.timedemo_mode {
            let events = self.step_frame();
            self.timedemo_frames = self.timedemo_frames.saturating_add(1);
            self.timedemo_elapsed += dt;
            return events;
        }
        let scaled = dt * f64::from(self.speed);
        let target = (self.state.playback_time + scaled).min(self.total_duration + 0.001);
        while self.state.cursor < self.timeline.len() {
            let frame_time = self.timeline[self.state.cursor].time;
            if frame_time > target {
                break;
            }
            let frame = self.timeline[self.state.cursor].frame.clone();
            self.state.playback_time = frame_time;
            self.apply_demo_frame(&frame, frame_time, true);
            self.state.cursor += 1;
            self.maybe_store_keyframe();
        }
        self.state.playback_time = target;
        if self.state.cursor >= self.timeline.len() {
            self.state.finished = true;
        }
        self.state.frames.advance(scaled);
        self.update_effects(target)
    }

    /// Advances playback by exactly one demo frame.
    pub fn step_frame(&mut self) -> Vec<FiredEvent> {
        if self.state.cursor >= self.timeline.len() {
            self.state.finished = true;
            return Vec::new();
        }
        let entry = self.timeline[self.state.cursor].clone();
        let dt = (entry.time - self.state.playback_time).max(0.0);
        self.state.playback_time = entry.time;
        self.apply_demo_frame(&entry.frame, entry.time, true);
        self.state.cursor += 1;
        if self.state.cursor >= self.timeline.len() {
            self.state.finished = true;
        }
        self.maybe_store_keyframe();
        self.state.frames.advance(dt.max(1.0 / 100.0));
        self.update_effects(entry.time)
    }

    /// Seeks forward or backward to `target_secs` using the nearest preceding keyframe.
    pub fn seek(&mut self, target_secs: f64) {
        let clamped = if target_secs.is_finite() {
            target_secs.clamp(0.0, self.total_duration)
        } else {
            0.0
        };
        let kf = self
            .keyframes
            .iter()
            .rfind(|k| k.playback_time <= clamped)
            .cloned()
            .unwrap_or_else(|| self.keyframes[0].clone());
        self.state = kf;
        while self.state.cursor < self.timeline.len() {
            let ft = self.timeline[self.state.cursor].time;
            if ft > clamped {
                break;
            }
            let frame = self.timeline[self.state.cursor].frame.clone();
            self.state.playback_time = ft;
            self.apply_demo_frame(&frame, ft, false);
            self.state.cursor += 1;
            self.maybe_store_keyframe();
        }
        self.state.playback_time = clamped;
        self.state.finished = self.state.cursor >= self.timeline.len();
    }

    fn maybe_store_keyframe(&mut self) {
        let last_t = self.keyframes.last().map(|k| k.playback_time).unwrap_or(-10.0);
        if self.state.playback_time - last_t >= KEYFRAME_INTERVAL_SECS {
            self.keyframes.push(self.state.clone());
        }
    }

    fn update_effects(&mut self, now: f64) -> Vec<FiredEvent> {
        let gravity = self
            .state
            .movevars
            .as_ref()
            .map(|m| m.gravity)
            .unwrap_or(800.0);
        let env = make_fx_env(
            &self.state.precache,
            &self.state.latest_entities,
            self.state.server_info.as_ref(),
            self.state.client_data.as_ref(),
            self.fx_world.as_ref(),
            now,
        );
        self.state.fx.update(&env, gravity)
    }

    fn apply_demo_frame(&mut self, frame: &DemoFrame, now: f64, record_lines: bool) {
        match &frame.data {
            DemoFrameData::NetMsg(nm) => {
                self.state.view_angles = Some(nm.info.ref_params.viewangles);
                self.state.latest_cmd = Some(nm.info.cmd.clone());
                self.state.ctx.last_incoming_seq = nm.incoming_sequence as u8;
                let (msgs, _err) = parse_svc_stream_partial(&nm.msg, &mut self.state.ctx);
                let seq = u32::try_from(nm.incoming_sequence).unwrap_or(0);
                for msg in msgs {
                    self.handle_svc(msg, seq, now, record_lines);
                }
            }
            DemoFrameData::ConsoleCommand(cmd) => {
                if record_lines {
                    self.push_console_line(format!("] {cmd}"));
                }
            }
            DemoFrameData::ClientData(cd) => {
                self.state.demo_client_data = Some(*cd);
                self.state.view_angles = Some(cd.viewangles);
            }
            DemoFrameData::WeaponAnim { anim, body } => {
                let msg = SvcMessage::WeaponAnim {
                    anim: (*anim).clamp(0, 255) as u8,
                    body: (*body).clamp(0, 255) as u8,
                };
                self.handle_svc(msg, self.state.frames.parsecount, now, record_lines);
            }
            DemoFrameData::Sound {
                channel,
                sample,
                attenuation,
                volume,
                pitch,
                ..
            } => {
                if record_lines {
                    let name = String::from_utf8_lossy(
                        sample.split(|&b| b == 0).next().unwrap_or(sample),
                    )
                    .into_owned();
                    let origin = self
                        .state
                        .client_data
                        .as_ref()
                        .map(|c| c.origin)
                        .unwrap_or(Vec3::ZERO);
                    self.state.fx.sounds.push(SoundCommand::Start(
                        SoundStart::by_name(
                            &name,
                            0,
                            (*channel).clamp(0, 255) as u8,
                            origin,
                            volume.clamp(0.0, 1.0),
                            *attenuation,
                            (*pitch).clamp(2, 255) as u8,
                        ),
                    ));
                }
            }
            DemoFrameData::Event(_) | DemoFrameData::DemoStart | DemoFrameData::DemoBuffer(_) => {}
            DemoFrameData::NextSection => {
                self.state.finished = true;
            }
        }
    }

    fn handle_svc(&mut self, msg: SvcMessage, incoming_sequence: u32, now: f64, record_lines: bool) {
        let _ = self.state.frames.handle(
            &msg,
            &self.state.ctx,
            &self.state.precache,
            incoming_sequence,
            now,
        );
        if matches!(msg, SvcMessage::PacketEntities(_))
            && let Some(frame) = self.state.frames.current_frame()
        {
            self.state.latest_entities.clear();
            for fe in &frame.entities {
                self.state.latest_entities.insert(fe.state.number, fe.state.clone());
            }
        }

        let env = make_fx_env(
            &self.state.precache,
            &self.state.latest_entities,
            self.state.server_info.as_ref(),
            self.state.client_data.as_ref(),
            self.fx_world.as_ref(),
            now,
        );
        self.state.fx.handle(&msg, &env);

        match msg {
            SvcMessage::ServerInfo(info) => {
                self.state.precache.clear();
                self.state.userinfos.clear();
                self.state.latest_entities.clear();
                self.state.server_info = Some(*info);
            }
            SvcMessage::NewMoveVars(mv) => {
                self.state.movevars = Some(*mv);
            }
            SvcMessage::ClientData { parsed, .. } => {
                self.state.client_data = Some(parsed);
            }
            SvcMessage::UpdateUserInfo { slot, userinfo, .. } => {
                if userinfo.is_empty() {
                    self.state.userinfos.remove(&slot);
                } else {
                    self.state.userinfos.insert(slot, userinfo);
                }
            }
            SvcMessage::ResourceList { resources, .. } => {
                self.state.precache.add(&resources);
            }
            SvcMessage::SetAngle(ang) => {
                self.state.view_angles = Some(ang);
                if record_lines {
                    self.pending_view_angles.push(ang);
                }
            }
            SvcMessage::Print(text) if record_lines => {
                self.push_console_line(text);
            }
            _ => {}
        }
    }

    fn push_console_line(&mut self, line: String) {
        while self.console_lines.len() >= MAX_DEMO_CONSOLE_LINES {
            self.console_lines.pop_front();
        }
        self.console_lines.push_back(line);
    }

    pub fn hud(&self) -> &HudState {
        &self.state.fx.hud
    }

    pub fn hud_mut(&mut self) -> &mut HudState {
        &mut self.state.fx.hud
    }

    pub fn frames(&self) -> &ClientFrames {
        &self.state.frames
    }

    pub fn frames_mut(&mut self) -> &mut ClientFrames {
        &mut self.state.frames
    }

    pub fn precache(&self) -> &PrecacheTables {
        &self.state.precache
    }

    pub fn fx(&self) -> &ClientEffects {
        &self.state.fx
    }

    pub fn fx_mut(&mut self) -> &mut ClientEffects {
        &mut self.state.fx
    }

    pub fn server_info(&self) -> Option<&ServerInfoMsg> {
        self.state.server_info.as_ref()
    }

    pub fn movevars(&self) -> Option<&MoveVarsMsg> {
        self.state.movevars.as_ref()
    }

    pub fn client_data(&self) -> Option<&GoldSrcClientData> {
        self.state.client_data.as_ref()
    }

    pub fn weapon_data(&self) -> &BTreeMap<u8, GoldSrcWeaponData> {
        &self.state.weapon_data
    }

    pub fn userinfos(&self) -> &BTreeMap<u8, String> {
        &self.state.userinfos
    }

    pub fn view_angles(&self) -> Option<Vec3> {
        self.state.view_angles
    }

    pub fn demo_client_data(&self) -> Option<DemoClientData> {
        self.state.demo_client_data
    }

    pub fn take_console_lines(&mut self) -> Vec<String> {
        self.console_lines.drain(..).collect()
    }

    pub fn take_sounds(&mut self) -> Vec<SoundCommand> {
        self.state.fx.sounds.drain()
    }

    pub fn take_view_angles(&mut self) -> Vec<Vec3> {
        std::mem::take(&mut self.pending_view_angles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::bitbuf::MsgWriter;
    use cs_proto48::messages::{
        write_svc_print, write_svc_serverinfo, write_svc_setangle, write_svc_time,
    };

    #[test]
    fn recorder_and_player_support_playback_timedemo_and_rewind_seek() {
        let mut rec = DemoRecorder::new("de_dust2", "cstrike", 0xABCD_1234, 48);

        let mut sw = MsgWriter::new();
        write_svc_serverinfo(
            &mut sw,
            &ServerInfoMsg {
                protocol: 48,
                spawn_count: 1,
                map_crc: 0xABCD_1234,
                client_dll_md5: [0; 16],
                max_clients: 16,
                player_slot: 0,
                is_deathmatch: true,
                game_dir: "cstrike".into(),
                hostname: "DemoTest".into(),
                map_path: "maps/de_dust2.bsp".into(),
                map_cycle: String::new(),
                vac_secure: false,
            },
        );
        rec.record_svc_packet(10.0, true, sw.as_slice(), DemoNetMsgInfo::default(), [1; 7]);
        rec.mark_signon_complete(10.05);

        for i in 0..=6 {
            let t = 11.0 + (i as f64) * 1.0;
            let mut pw = MsgWriter::new();
            write_svc_time(&mut pw, (i + 1) as f32);
            write_svc_setangle(&mut pw, Vec3::new(0.0, (i as f32) * 30.0, 0.0));
            write_svc_print(&mut pw, &format!("tick {i}\n"));
            let mut info = DemoNetMsgInfo::default();
            info.ref_params.viewangles = Vec3::new(0.0, (i as f32) * 30.0, 0.0);
            rec.record_svc_packet(t, false, pw.as_slice(), info, [i + 2; 7]);
        }

        let dem_bytes = rec.finish().unwrap();
        let mut player = DemoPlayer::from_bytes(&dem_bytes).unwrap();
        assert_eq!(player.map_name(), "maps/de_dust2.bsp");
        assert!(player.total_duration() >= 5.9);

        player.advance(3.2);
        let t_mid = player.current_time();
        assert!((3.1..=3.5).contains(&t_mid));
        assert!((player.view_angles().unwrap().y - 90.0).abs() < 0.01);

        // Seek backward to 1.2s -> must rewind viewangles back to ~30 deg (16-bit wire angle).
        player.seek(1.2);
        assert!((player.current_time() - 1.2).abs() < 1e-6);
        assert!((player.view_angles().unwrap().y - 30.0).abs() < 0.01);

        // TimeDemo runs remainder unthrottled to finish.
        let report = player.run_timedemo_all(1.0 / 100.0);
        assert!(report.frames >= 4);
        assert!(report.avg_fps > 10.0);
        assert!(player.is_finished());
    }
}
