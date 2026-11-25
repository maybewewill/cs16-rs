//! Built-in per-round HLDEMO v5 (`.dem`) replay store and live match statistics tracker
//! for `cs-server`.
//!
//! Zero `unsafe` code; bounded ring-buffer memory budgets prevent unbounded growth across
//! long-running dedicated server sessions.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt::Write as _;

use cs_assets::scheme::load_overview_image_rgba;
use cs_assets::vgui_scripts::OverviewSpec;
use cs_clientcore::DemoRecorder;
use cs_proto48::demo::{DEMO_MAGIC, DemoClientData, DemoNetMsgInfo};
use cs_vfs::Vfs;
use image::ImageEncoder;
use image::codecs::png::PngEncoder;

/// Default maximum number of completed round `.dem` files retained in memory.
pub const DEFAULT_MAX_REPLAY_ROUNDS: usize = 32;
/// Hard ceiling for a single recorded round `.dem` payload (16 MiB).
pub const MAX_SINGLE_REPLAY_BYTES: usize = 16 * 1024 * 1024;
/// Default total memory cap across all buffered round `.dem` files (128 MiB).
pub const DEFAULT_MAX_REPLAY_STORE_BYTES: usize = 128 * 1024 * 1024;
/// Maximum number of round summaries retained in `MatchStatsTracker`.
pub const MAX_ROUND_HISTORY: usize = 256;
/// Sampling frequency for `Round2dTimeline` player state snapshots (10 Hz).
pub const TIMELINE_SAMPLE_HZ: u32 = 10;
/// Sampling interval in seconds corresponding to `TIMELINE_SAMPLE_HZ`.
pub const TIMELINE_SAMPLE_INTERVAL_SECS: f64 = 0.1;
/// Hard serialized JSON size cap per round 2D timeline (512 KiB).
pub const MAX_TIMELINE_BYTES: usize = 512 * 1024;
/// Maximum number of 10 Hz frames buffered during a single round before throttling (6 minutes).
const MAX_TIMELINE_FRAMES_RAW: usize = 3600;
/// Maximum discrete shot events buffered in a single round timeline.
const MAX_TIMELINE_SHOTS_RAW: usize = 2048;
/// Maximum grenade trajectories buffered in a single round timeline.
const MAX_TIMELINE_GRENADES_RAW: usize = 256;

const DEFAULT_GAME_DIR: &str = "cstrike";
const PROTO48_NET_PROTOCOL: i32 = 48;
const ASSIST_DAMAGE_THRESHOLD: u64 = 40;

/// Strict JSON string escaper (escapes quotes, backslashes, and all ASCII control chars).
pub(crate) fn escape_json_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7F}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn fmt_finite_f64(value: f64, decimals: usize) -> String {
    let safe = if value.is_finite() { value } else { 0.0 };
    format!("{safe:.decimals$}")
}

/// Validates a replay identifier (`[A-Za-z0-9_-]`, 1..=64 bytes) to block path traversal.
pub fn valid_replay_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn sanitize_id_segment(input: &str) -> String {
    let mut out: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(32)
        .collect();
    if out.is_empty() {
        out.push_str("map");
    }
    out
}

/// Winning side for a completed round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundWinner {
    Terrorist,
    CounterTerrorist,
    Draw,
}

impl RoundWinner {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terrorist => "T",
            Self::CounterTerrorist => "CT",
            Self::Draw => "Draw",
        }
    }
}

/// Team side used by `MatchStatsTracker`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamSide {
    Terrorist,
    CounterTerrorist,
    Spectator,
    Unassigned,
}

impl TeamSide {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terrorist => "T",
            Self::CounterTerrorist => "CT",
            Self::Spectator => "SPEC",
            Self::Unassigned => "UNASSIGNED",
        }
    }
}

/// Validates a map name (`[A-Za-z0-9_-]`, 1..=64 bytes, no traversal) for overview routes.
pub fn valid_map_overview_name(name: &str) -> bool {
    let clean = name.strip_suffix(".bsp").unwrap_or(name);
    !clean.is_empty()
        && clean.len() <= 64
        && clean
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn fmt_finite_f32(value: f32, decimals: usize) -> String {
    let safe = if value.is_finite() { value } else { 0.0 };
    format!("{safe:.decimals$}")
}

fn write_vec3_json(out: &mut String, v: [f32; 3], decimals: usize) {
    out.push('[');
    out.push_str(&fmt_finite_f32(v[0], decimals));
    out.push(',');
    out.push_str(&fmt_finite_f32(v[1], decimals));
    out.push(',');
    out.push_str(&fmt_finite_f32(v[2], decimals));
    out.push(']');
}

/// Compact per-player state snapshot recorded at 10 Hz for HLTV-style 2D round replay.
/// Serializes as `[slot, team, alive, hp, x, y, z, yaw_deg, active_weapon_id, has_c4, flashed_alpha]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelinePlayerSample {
    pub slot: u8,
    pub team: u8,
    pub alive: bool,
    pub hp: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw_deg: f32,
    pub active_weapon_id: u8,
    pub has_c4: bool,
    pub flashed_alpha: u8,
}

impl TimelinePlayerSample {
    fn write_compact_json(&self, out: &mut String) {
        let alive_bit = u8::from(self.alive);
        let c4_bit = u8::from(self.has_c4);
        let hp = self.hp.clamp(0, 999);
        let _ = write!(
            out,
            "[{},{},{},{},{},{},{},{},{},{},{}]",
            self.slot,
            self.team,
            alive_bit,
            hp,
            fmt_finite_f32(self.x, 1),
            fmt_finite_f32(self.y, 1),
            fmt_finite_f32(self.z, 1),
            fmt_finite_f32(self.yaw_deg, 1),
            self.active_weapon_id,
            c4_bit,
            self.flashed_alpha,
        );
    }
}

/// Player roster entry included in [`Round2dTimeline`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelinePlayerMeta {
    pub slot: u8,
    pub name: String,
    pub team: u8,
    pub is_bot: bool,
}

/// One 10 Hz tick of player snapshots in [`Round2dTimeline`].
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTickFrame {
    pub t: f64,
    pub players: Vec<TimelinePlayerSample>,
}

/// Discrete shot tracer event for 2D replay visualization.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineShotEvent {
    pub tick: u32,
    pub slot: u8,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub weapon: u8,
}

/// Grenade category for 2D replay rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineGrenadeKind {
    He,
    Flash,
    Smoke,
}

impl TimelineGrenadeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::He => "he",
            Self::Flash => "flash",
            Self::Smoke => "smoke",
        }
    }
}

/// Thrown grenade flight trajectory and detonation point.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineGrenadeEvent {
    pub id: u32,
    pub kind: TimelineGrenadeKind,
    pub start_tick: u32,
    pub trajectory: Vec<[f32; 3]>,
    pub detonate_tick: Option<u32>,
    pub detonate_pos: Option<[f32; 3]>,
}

/// C4 state transition in a round timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineBombState {
    Dropped,
    Pickup,
    Planted,
    Defused,
    Exploded,
}

impl TimelineBombState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dropped => "dropped",
            Self::Pickup => "pickup",
            Self::Planted => "planted",
            Self::Defused => "defused",
            Self::Exploded => "exploded",
        }
    }
}

/// C4 state event in [`Round2dTimeline`].
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineBombEvent {
    pub tick: u32,
    pub state: TimelineBombState,
    pub pos: [f32; 3],
}

/// Killfeed event in [`Round2dTimeline`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineKillEvent {
    pub tick: u32,
    pub killer: Option<u8>,
    pub victim: u8,
    pub weapon: String,
    pub headshot: bool,
}

/// Complete 10 Hz 2D round replay timeline served by `GET /api/replays/<id>/timeline`.
#[derive(Debug, Clone, PartialEq)]
pub struct Round2dTimeline {
    pub id: String,
    pub round_number: u32,
    pub map_name: String,
    pub winner: RoundWinner,
    pub reason: String,
    pub duration_secs: f64,
    pub sample_rate_hz: u32,
    pub score_ct: u32,
    pub score_t: u32,
    pub players: Vec<TimelinePlayerMeta>,
    pub frames: Vec<TimelineTickFrame>,
    pub shots: Vec<TimelineShotEvent>,
    pub grenades: Vec<TimelineGrenadeEvent>,
    pub bombs: Vec<TimelineBombEvent>,
    pub kills: Vec<TimelineKillEvent>,
}

impl Round2dTimeline {
    /// Serializes the timeline into strict JSON, guaranteed not to exceed `max_bytes`
    /// (default [`MAX_TIMELINE_BYTES`] = 512 KiB).
    pub fn to_bounded_json(&self, max_bytes: usize) -> String {
        let mut working = self.clone();
        let mut json = working.encode_json_unbounded();
        if json.len() <= max_bytes {
            return json;
        }

        // First thin dense intermediate frames while keeping endpoints and key tick counts aligned
        // only by dropping every other frame in a cloned copy? Instead of shifting indices,
        // keep frame indices stable by clearing player payloads on thinned frames or truncating
        // excess trailing shots/frames cleanly. Clearing oldest/alternating frames' snapshots
        // preserves tick indices 1:1 with events, or truncating trailing frames when very long.
        let mut stride = 2usize;
        while json.len() > max_bytes && stride <= 16 {
            let mut thinned = self.clone();
            for (idx, frame) in thinned.frames.iter_mut().enumerate() {
                if idx != 0
                    && idx + 1 != self.frames.len()
                    && idx % stride != 0
                {
                    frame.players.clear();
                }
            }
            // Drop trailing empty-player ticks only if needed; better keep empty-player ticks or
            // drop half of shots if still over budget.
            if stride >= 4 && thinned.shots.len() > 256 {
                let keep = thinned.shots.len() / 2;
                thinned.shots.drain(..thinned.shots.len() - keep);
            }
            json = thinned.encode_json_unbounded();
            working = thinned;
            stride *= 2;
        }

        while json.len() > max_bytes && !working.shots.is_empty() {
            let drop = (working.shots.len() / 2).max(1);
            working.shots.drain(..drop);
            json = working.encode_json_unbounded();
        }
        while json.len() > max_bytes && !working.grenades.is_empty() {
            working.grenades.remove(0);
            json = working.encode_json_unbounded();
        }
        while json.len() > max_bytes && working.frames.len() > 1 {
            let new_len = (working.frames.len() / 2).max(1);
            working.frames.truncate(new_len);
            json = working.encode_json_unbounded();
        }
        json
    }

    fn encode_json_unbounded(&self) -> String {
        let mut out = String::with_capacity(1024 + self.frames.len() * 192);
        let _ = write!(
            out,
            "{{\"id\":{},\"round_number\":{},\"map\":{},\"winner\":{},\"reason\":{},\"duration_secs\":{},\"sample_rate_hz\":{},\"score_ct\":{},\"score_t\":{},\"players\":[",
            escape_json_str(&self.id),
            self.round_number,
            escape_json_str(&self.map_name),
            escape_json_str(self.winner.as_str()),
            escape_json_str(&self.reason),
            fmt_finite_f64(self.duration_secs, 2),
            self.sample_rate_hz,
            self.score_ct,
            self.score_t,
        );
        for (idx, p) in self.players.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"slot\":{},\"name\":{},\"team\":{},\"is_bot\":{}}}",
                p.slot,
                escape_json_str(&p.name),
                p.team,
                p.is_bot,
            );
        }
        out.push_str("],\"frames\":[");
        for (idx, f) in self.frames.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"t\":{},\"p\":[", fmt_finite_f64(f.t, 2));
            for (pidx, sample) in f.players.iter().enumerate() {
                if pidx > 0 {
                    out.push(',');
                }
                sample.write_compact_json(&mut out);
            }
            out.push_str("]}");
        }
        out.push_str("],\"shots\":[");
        for (idx, s) in self.shots.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"tick\":{},\"slot\":{},\"from\":", s.tick, s.slot);
            write_vec3_json(&mut out, s.from, 1);
            out.push_str(",\"to\":");
            write_vec3_json(&mut out, s.to, 1);
            let _ = write!(out, ",\"weapon\":{}}}", s.weapon);
        }
        out.push_str("],\"grenades\":[");
        for (idx, g) in self.grenades.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"id\":{},\"kind\":{},\"start_tick\":{},\"trajectory\":[",
                g.id,
                escape_json_str(g.kind.as_str()),
                g.start_tick,
            );
            for (pidx, pt) in g.trajectory.iter().enumerate() {
                if pidx > 0 {
                    out.push(',');
                }
                write_vec3_json(&mut out, *pt, 1);
            }
            out.push_str("],\"detonate_tick\":");
            match g.detonate_tick {
                Some(dt) => {
                    let _ = write!(out, "{dt}");
                }
                None => out.push_str("null"),
            }
            out.push_str(",\"detonate_pos\":");
            match g.detonate_pos {
                Some(pos) => write_vec3_json(&mut out, pos, 1),
                None => out.push_str("null"),
            }
            out.push('}');
        }
        out.push_str("],\"bombs\":[");
        for (idx, b) in self.bombs.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"tick\":{},\"state\":{},\"pos\":",
                b.tick,
                escape_json_str(b.state.as_str()),
            );
            write_vec3_json(&mut out, b.pos, 1);
            out.push('}');
        }
        out.push_str("],\"kills\":[");
        for (idx, k) in self.kills.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let killer_json = match k.killer {
                Some(id) => id.to_string(),
                None => "null".to_string(),
            };
            let _ = write!(
                out,
                "{{\"tick\":{},\"killer\":{},\"victim\":{},\"weapon\":{},\"headshot\":{}}}",
                k.tick,
                killer_json,
                k.victim,
                escape_json_str(&k.weapon),
                k.headshot,
            );
        }
        out.push_str("]}");
        out
    }
}

/// Incremental 10 Hz timeline recorder paired with an active round recording.
#[derive(Debug, Clone)]
pub struct Round2dTimelineRecorder {
    started_at_secs: f64,
    last_sample_secs: Option<f64>,
    next_grenade_id: u32,
    grenade_index_by_key: HashMap<u32, usize>,
    players: BTreeMap<u8, TimelinePlayerMeta>,
    frames: Vec<TimelineTickFrame>,
    shots: Vec<TimelineShotEvent>,
    grenades: Vec<TimelineGrenadeEvent>,
    bombs: Vec<TimelineBombEvent>,
    kills: Vec<TimelineKillEvent>,
}

impl Round2dTimelineRecorder {
    pub fn new(started_at_secs: f64) -> Self {
        Self {
            started_at_secs,
            last_sample_secs: None,
            next_grenade_id: 1,
            grenade_index_by_key: HashMap::new(),
            players: BTreeMap::new(),
            frames: Vec::new(),
            shots: Vec::new(),
            grenades: Vec::new(),
            bombs: Vec::new(),
            kills: Vec::new(),
        }
    }

    pub fn tick_for_time(&self, now_secs: f64) -> u32 {
        let elapsed = (now_secs - self.started_at_secs).max(0.0);
        let raw = (elapsed * f64::from(TIMELINE_SAMPLE_HZ)).round();
        if raw.is_finite() && raw >= 0.0 {
            (raw as u64).min(u64::from(u32::MAX)) as u32
        } else {
            0
        }
    }

    pub fn set_player_info(&mut self, slot: u8, name: &str, team: u8, is_bot: bool) {
        self.players.insert(
            slot,
            TimelinePlayerMeta {
                slot,
                name: name.to_string(),
                team,
                is_bot,
            },
        );
    }

    /// Samples a 10 Hz frame if `TIMELINE_SAMPLE_INTERVAL_SECS` has elapsed since the last sample.
    pub fn sample_if_due(&mut self, now_secs: f64, players: &[TimelinePlayerSample]) -> bool {
        if self.frames.len() >= MAX_TIMELINE_FRAMES_RAW {
            return false;
        }
        if let Some(last) = self.last_sample_secs
            && now_secs - last < TIMELINE_SAMPLE_INTERVAL_SECS - 1e-6
        {
            return false;
        }
        self.last_sample_secs = Some(now_secs);
        let t = (now_secs - self.started_at_secs).max(0.0);
        self.frames.push(TimelineTickFrame {
            t,
            players: players.to_vec(),
        });
        true
    }

    pub fn record_shot(
        &mut self,
        now_secs: f64,
        slot: u8,
        from: [f32; 3],
        to: [f32; 3],
        weapon: u8,
    ) {
        if self.shots.len() >= MAX_TIMELINE_SHOTS_RAW {
            return;
        }
        self.shots.push(TimelineShotEvent {
            tick: self.tick_for_time(now_secs),
            slot,
            from,
            to,
            weapon,
        });
    }

    pub fn record_grenade_pos(
        &mut self,
        now_secs: f64,
        entity_key: u32,
        kind: TimelineGrenadeKind,
        pos: [f32; 3],
    ) {
        let tick = self.tick_for_time(now_secs);
        let idx = if let Some(&existing) = self.grenade_index_by_key.get(&entity_key) {
            existing
        } else {
            if self.grenades.len() >= MAX_TIMELINE_GRENADES_RAW {
                return;
            }
            let id = self.next_grenade_id;
            self.next_grenade_id = self.next_grenade_id.saturating_add(1);
            let idx = self.grenades.len();
            self.grenades.push(TimelineGrenadeEvent {
                id,
                kind,
                start_tick: tick,
                trajectory: Vec::new(),
                detonate_tick: None,
                detonate_pos: None,
            });
            self.grenade_index_by_key.insert(entity_key, idx);
            idx
        };
        if let Some(g) = self.grenades.get_mut(idx) {
            let should_push = g.trajectory.last().is_none_or(|prev| {
                let dx = prev[0] - pos[0];
                let dy = prev[1] - pos[1];
                let dz = prev[2] - pos[2];
                dx * dx + dy * dy + dz * dz >= 4.0
            });
            if should_push && g.trajectory.len() < 128 {
                g.trajectory.push(pos);
            }
        }
    }

    pub fn detonate_grenade(&mut self, now_secs: f64, entity_key: u32, pos: Option<[f32; 3]>) {
        let tick = self.tick_for_time(now_secs);
        if let Some(idx) = self.grenade_index_by_key.remove(&entity_key)
            && let Some(g) = self.grenades.get_mut(idx)
        {
            g.detonate_tick = Some(tick);
            g.detonate_pos = pos.or_else(|| g.trajectory.last().copied());
        }
    }

    pub fn record_bomb(&mut self, now_secs: f64, state: TimelineBombState, pos: [f32; 3]) {
        if self.bombs.len() >= 128 {
            return;
        }
        self.bombs.push(TimelineBombEvent {
            tick: self.tick_for_time(now_secs),
            state,
            pos,
        });
    }

    pub fn record_kill(
        &mut self,
        now_secs: f64,
        killer: Option<u8>,
        victim: u8,
        weapon: &str,
        headshot: bool,
    ) {
        if self.kills.len() >= 256 {
            return;
        }
        self.kills.push(TimelineKillEvent {
            tick: self.tick_for_time(now_secs),
            killer,
            victim,
            weapon: weapon.to_string(),
            headshot,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        self,
        id: String,
        round_number: u32,
        map_name: String,
        winner: RoundWinner,
        reason: String,
        now_secs: f64,
        score_ct: u32,
        score_t: u32,
    ) -> (Round2dTimeline, String) {
        let duration_secs = (now_secs - self.started_at_secs).max(0.0);
        let timeline = Round2dTimeline {
            id,
            round_number,
            map_name,
            winner,
            reason,
            duration_secs,
            sample_rate_hz: TIMELINE_SAMPLE_HZ,
            score_ct,
            score_t,
            players: self.players.into_values().collect(),
            frames: self.frames,
            shots: self.shots,
            grenades: self.grenades,
            bombs: self.bombs,
            kills: self.kills,
        };
        let bounded_json = timeline.to_bounded_json(MAX_TIMELINE_BYTES);
        (timeline, bounded_json)
    }
}

/// Overview metadata and optional PNG image bytes for `GET /api/map-overview/<map>{,.png}`.
#[derive(Debug, Clone, PartialEq)]
pub struct MapOverviewBundle {
    pub map: String,
    pub has_overview: bool,
    pub zoom: f32,
    pub origin: [f32; 3],
    pub rotated: bool,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub png_bytes: Option<Vec<u8>>,
}

impl MapOverviewBundle {
    pub fn to_json(&self) -> String {
        let image_url = if self.has_overview {
            escape_json_str(&format!("/api/map-overview/{}.png", self.map))
        } else {
            "null".to_string()
        };
        let mut out = String::with_capacity(256);
        let _ = write!(
            out,
            "{{\"map\":{},\"has_overview\":{},\"zoom\":{},\"origin\":",
            escape_json_str(&self.map),
            self.has_overview,
            fmt_finite_f32(self.zoom, 4),
        );
        write_vec3_json(&mut out, self.origin, 2);
        let _ = write!(out, ",\"rotated\":{},\"mins\":", self.rotated);
        write_vec3_json(&mut out, self.mins, 2);
        out.push_str(",\"maxs\":");
        write_vec3_json(&mut out, self.maxs, 2);
        let _ = write!(out, ",\"image_url\":{image_url}}}");
        out
    }
}

/// Reads world model (`models[0]`) `mins`/`maxs` from GoldSrc BSP v30 `LUMP_MODELS` without
/// decoding textures or lightmaps.
pub fn read_bsp_world_bounds(bsp_bytes: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    const BSP_V30: i32 = 30;
    const LUMP_MODELS: usize = 14;
    if bsp_bytes.len() < 4 + 15 * 8 {
        return None;
    }
    let version = i32::from_le_bytes(bsp_bytes[0..4].try_into().ok()?);
    if version != BSP_V30 {
        return None;
    }
    let lump_off = 4 + LUMP_MODELS * 8;
    let off = i32::from_le_bytes(bsp_bytes[lump_off..lump_off + 4].try_into().ok()?);
    let len = i32::from_le_bytes(bsp_bytes[lump_off + 4..lump_off + 8].try_into().ok()?);
    if off < 0 || len < 24 {
        return None;
    }
    let start = off as usize;
    let end = start.checked_add(24)?;
    let slice = bsp_bytes.get(start..end)?;
    let f = |i: usize| -> Option<f32> {
        let v = f32::from_le_bytes(slice[i..i + 4].try_into().ok()?);
        v.is_finite().then_some(v)
    };
    let mins = [f(0)?, f(4)?, f(8)?];
    let maxs = [f(12)?, f(16)?, f(20)?];
    Some((mins, maxs))
}

/// Encodes an RGBA8 image buffer into PNG bytes.
pub fn encode_rgba_to_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "overview image dimensions overflow".to_string())?;
    if rgba.len() != expected {
        return Err(format!(
            "overview RGBA length {} != expected {}",
            rgba.len(),
            expected
        ));
    }
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("failed to encode overview PNG: {e}"))?;
    Ok(out)
}

/// Converts a GoldSrc `overviews/<map>.bmp` (or TGA/PNG) byte buffer to RGBA8 PNG with green
/// `(0, 255, 0)` / palette index `255` (when green-keyed or explicitly transparent) chromakeying.
pub fn convert_overview_bmp_to_png(vfs: &Vfs, image_path: &str) -> Result<Vec<u8>, String> {
    let mut img = load_overview_image_rgba(vfs, image_path)
        .map_err(|e| format!("failed to decode overview `{image_path}`: {e}"))?;
    // Also honor explicit 8-bit uncompressed BMP palette index 255 if its palette color is
    // pure GoldSrc chromakey green or magenta/cyan key, while never punching holes in maps
    // whose index 255 holds wall/sky shades.
    if image_path.to_ascii_lowercase().ends_with(".bmp")
        && let Ok(raw) = vfs.read_bytes(image_path)
    {
        apply_bmp_index_255_chromakey(&raw, img.width, img.height, &mut img.rgba);
    }
    encode_rgba_to_png(img.width, img.height, &img.rgba)
}

fn apply_bmp_index_255_chromakey(raw: &[u8], width: u32, height: u32, rgba: &mut [u8]) {
    if raw.len() < 54 + 256 * 4 || &raw[0..2] != b"BM" {
        return;
    }
    let dib_size = u32::from_le_bytes([raw[14], raw[15], raw[16], raw[17]]) as usize;
    let bpp = u16::from_le_bytes([raw[28], raw[29]]);
    let compression = u32::from_le_bytes([raw[30], raw[31], raw[32], raw[33]]);
    if bpp != 8 || compression != 0 || dib_size < 40 {
        return;
    }
    let pal_off = 14 + dib_size;
    let Some(p255) = raw.get(pal_off + 255 * 4..pal_off + 256 * 4) else {
        return;
    };
    let (b, g, r) = (p255[0], p255[1], p255[2]);
    let is_key_color = (r <= 12 && g >= 242 && b <= 12) || (r >= 245 && g <= 12 && b >= 245);
    if !is_key_color {
        return;
    }
    let pixel_offset = u32::from_le_bytes([raw[10], raw[11], raw[12], raw[13]]) as usize;
    let raw_h = i32::from_le_bytes([raw[22], raw[23], raw[24], raw[25]]);
    let top_down = raw_h < 0;
    let w = width as usize;
    let h = height as usize;
    let row_stride = (w + 3) & !3;
    for y in 0..h {
        let src_row = if top_down { y } else { h - 1 - y };
        let row_start = pixel_offset.saturating_add(src_row.saturating_mul(row_stride));
        let Some(src) = raw.get(row_start..row_start + w) else {
            break;
        };
        for (x, &idx) in src.iter().enumerate() {
            if idx == 255 {
                let dst = (y * w + x) * 4;
                if let Some(px) = rgba.get_mut(dst..dst + 4) {
                    px.copy_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
    }
}

/// Loads overview spec, PNG bytes (if `overviews/<map>.txt` + image exist), and BSP world bounds
/// for `map_name` from `vfs`.
pub fn load_map_overview_from_vfs(
    vfs: &Vfs,
    map_name: &str,
    fallback_bounds: Option<([f32; 3], [f32; 3])>,
) -> Option<MapOverviewBundle> {
    let clean = map_name.strip_suffix(".bsp").unwrap_or(map_name);
    if !valid_map_overview_name(clean) {
        return None;
    }

    let bsp_path = format!("maps/{clean}.bsp");
    let bsp_bounds = vfs
        .read_bytes(&bsp_path)
        .ok()
        .and_then(|bytes| read_bsp_world_bounds(&bytes));
    let has_bsp = bsp_bounds.is_some() || vfs.exists(&bsp_path);
    let (mins, maxs) = bsp_bounds
        .or(fallback_bounds)
        .unwrap_or(([-2048.0, -2048.0, -512.0], [2048.0, 2048.0, 512.0]));

    if let Some(spec) = OverviewSpec::load(vfs, clean) {
        let candidates = [
            spec.image.as_str(),
            &format!("overviews/{clean}.bmp"),
            &format!("overviews/{clean}.tga"),
            &format!("overviews/{clean}.png"),
        ];
        for cand in candidates {
            if cand.is_empty() {
                continue;
            }
            if let Ok(png) = convert_overview_bmp_to_png(vfs, cand) {
                return Some(MapOverviewBundle {
                    map: clean.to_string(),
                    has_overview: true,
                    zoom: spec.zoom,
                    origin: spec.origin,
                    rotated: spec.rotated,
                    mins,
                    maxs,
                    png_bytes: Some(png),
                });
            }
        }
    }

    if has_bsp || fallback_bounds.is_some() {
        return Some(MapOverviewBundle {
            map: clean.to_string(),
            has_overview: false,
            zoom: 1.0,
            origin: [0.0; 3],
            rotated: false,
            mins,
            maxs,
            png_bytes: None,
        });
    }

    None
}

/// Metadata describing one completed round `.dem` replay.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundReplayMeta {
    pub id: String,
    pub round_number: u32,
    pub map_name: String,
    pub map_crc: u32,
    pub winner: RoundWinner,
    pub reason: String,
    pub duration_secs: f64,
    pub frame_count: usize,
    pub byte_len: usize,
    pub recorded_at_secs: f64,
    pub score_ct: u32,
    pub score_t: u32,
    pub mvp_player: Option<String>,
}

#[derive(Debug, Clone)]
struct StoredRoundReplay {
    meta: RoundReplayMeta,
    demo_bytes: Vec<u8>,
    timeline_json: Option<String>,
}

struct ActiveRoundRecording {
    round_number: u32,
    map_name: String,
    map_crc: u32,
    started_at_secs: f64,
    seq_counter: i32,
    recorder: DemoRecorder,
    timeline: Round2dTimelineRecorder,
}

/// Bounded ring-buffer store of per-round GoldSrc HLDEMO v5 (`.dem`) recordings.
pub struct RoundReplayStore {
    max_rounds: usize,
    max_total_bytes: usize,
    total_bytes: usize,
    next_round: u32,
    rounds: VecDeque<StoredRoundReplay>,
    active: Option<ActiveRoundRecording>,
}

impl Default for RoundReplayStore {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_REPLAY_ROUNDS)
    }
}

impl RoundReplayStore {
    /// Creates a replay store holding at most `max_rounds` (clamped to `>= 1`) within the
    /// default total byte budget.
    pub fn new(max_rounds: usize) -> Self {
        Self::with_limits(max_rounds, DEFAULT_MAX_REPLAY_STORE_BYTES)
    }

    /// Creates a replay store with explicit round count and total byte limits.
    pub fn with_limits(max_rounds: usize, max_total_bytes: usize) -> Self {
        Self {
            max_rounds: max_rounds.max(1),
            max_total_bytes: max_total_bytes.max(1024),
            total_bytes: 0,
            next_round: 1,
            rounds: VecDeque::new(),
            active: None,
        }
    }

    /// Starts (or replaces) the active round `.dem` recording using `DemoRecorder`.
    pub fn begin_round(&mut self, map_name: &str, map_crc: u32, now_secs: f64) {
        let round_number = self.next_round;
        self.next_round = self.next_round.saturating_add(1);
        let recorder =
            DemoRecorder::new(map_name, DEFAULT_GAME_DIR, map_crc, PROTO48_NET_PROTOCOL);
        self.active = Some(ActiveRoundRecording {
            round_number,
            map_name: map_name.to_string(),
            map_crc,
            started_at_secs: now_secs,
            seq_counter: 0,
            recorder,
            timeline: Round2dTimelineRecorder::new(now_secs),
        });
    }

    /// Registers or updates player metadata on the active round's 2D timeline.
    pub fn set_timeline_player(&mut self, slot: u8, name: &str, team: u8, is_bot: bool) {
        if let Some(active) = &mut self.active {
            active.timeline.set_player_info(slot, name, team, is_bot);
        }
    }

    /// Samples a 10 Hz frame of player states into the active round's 2D timeline.
    pub fn sample_timeline_if_due(
        &mut self,
        now_secs: f64,
        players: &[TimelinePlayerSample],
    ) -> bool {
        if let Some(active) = &mut self.active {
            active.timeline.sample_if_due(now_secs, players)
        } else {
            false
        }
    }

    /// Records a shot tracer into the active round's 2D timeline.
    pub fn record_timeline_shot(
        &mut self,
        now_secs: f64,
        slot: u8,
        from: [f32; 3],
        to: [f32; 3],
        weapon: u8,
    ) {
        if let Some(active) = &mut self.active {
            active.timeline.record_shot(now_secs, slot, from, to, weapon);
        }
    }

    /// Records an active grenade's position into the active round's 2D timeline.
    pub fn record_timeline_grenade_pos(
        &mut self,
        now_secs: f64,
        entity_key: u32,
        kind: TimelineGrenadeKind,
        pos: [f32; 3],
    ) {
        if let Some(active) = &mut self.active {
            active
                .timeline
                .record_grenade_pos(now_secs, entity_key, kind, pos);
        }
    }

    /// Marks an active grenade as detonated in the active round's 2D timeline.
    pub fn detonate_timeline_grenade(
        &mut self,
        now_secs: f64,
        entity_key: u32,
        pos: Option<[f32; 3]>,
    ) {
        if let Some(active) = &mut self.active {
            active.timeline.detonate_grenade(now_secs, entity_key, pos);
        }
    }

    /// Records a C4 state transition in the active round's 2D timeline.
    pub fn record_timeline_bomb(
        &mut self,
        now_secs: f64,
        state: TimelineBombState,
        pos: [f32; 3],
    ) {
        if let Some(active) = &mut self.active {
            active.timeline.record_bomb(now_secs, state, pos);
        }
    }

    /// Records a killfeed event in the active round's 2D timeline.
    pub fn record_timeline_kill(
        &mut self,
        now_secs: f64,
        killer: Option<u8>,
        victim: u8,
        weapon: &str,
        headshot: bool,
    ) {
        if let Some(active) = &mut self.active {
            active
                .timeline
                .record_kill(now_secs, killer, victim, weapon, headshot);
        }
    }

    /// Marks signon complete on the active round recording.
    pub fn mark_signon_complete(&mut self, now_secs: f64) {
        if let Some(active) = &mut self.active {
            active.recorder.mark_signon_complete(now_secs);
        }
    }

    /// Records an `svc_*` stream packet into the active round `.dem`.
    pub fn record_svc_packet(&mut self, host_time: f64, is_signon: bool, payload: &[u8]) {
        if let Some(active) = &mut self.active {
            active.seq_counter = active.seq_counter.saturating_add(1);
            let seq = active.seq_counter;
            active.recorder.record_svc_packet(
                host_time,
                is_signon,
                payload,
                DemoNetMsgInfo::default(),
                [seq; 7],
            );
        }
    }

    /// Records a console command frame into the active round `.dem`.
    pub fn record_console_command(&mut self, host_time: f64, cmd: &str) {
        if let Some(active) = &mut self.active {
            active.recorder.record_console_command(host_time, cmd);
        }
    }

    /// Records a `DemoClientData` frame into the active round `.dem`.
    #[allow(dead_code)]
    pub fn record_client_data(&mut self, host_time: f64, client_data: DemoClientData) {
        if let Some(active) = &mut self.active {
            active.recorder.record_client_data(host_time, client_data);
        }
    }

    /// Finalizes the active round `.dem` recording and inserts it into the ring buffer.
    pub fn finish_round(
        &mut self,
        winner: RoundWinner,
        reason: &str,
        now_secs: f64,
        score_ct: u32,
        score_t: u32,
        mvp_player: Option<String>,
    ) -> Result<Option<RoundReplayMeta>, String> {
        let Some(active) = self.active.take() else {
            return Ok(None);
        };
        let frame_count = active.recorder.frame_count();
        let bytes = active
            .recorder
            .finish()
            .map_err(|e| format!("failed to finalize HLDEMO replay: {e}"))?;

        let id = format!(
            "r{:04}-{}",
            active.round_number,
            sanitize_id_segment(&active.map_name)
        );
        let duration_secs = (now_secs - active.started_at_secs).max(0.0);
        let (_timeline, timeline_json) = active.timeline.finish(
            id.clone(),
            active.round_number,
            active.map_name.clone(),
            winner,
            reason.to_string(),
            now_secs,
            score_ct,
            score_t,
        );
        let meta = RoundReplayMeta {
            id,
            round_number: active.round_number,
            map_name: active.map_name,
            map_crc: active.map_crc,
            winner,
            reason: reason.to_string(),
            duration_secs,
            frame_count,
            byte_len: bytes.len(),
            recorded_at_secs: now_secs,
            score_ct,
            score_t,
            mvp_player,
        };
        self.push_finished_demo_with_timeline(meta.clone(), bytes, Some(timeline_json))?;
        Ok(Some(meta))
    }

    /// Inserts an already-encoded HLDEMO v5 `.dem` buffer, enforcing ID and size guards.
    #[allow(dead_code)]
    pub fn push_finished_demo(
        &mut self,
        meta: RoundReplayMeta,
        demo_bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.push_finished_demo_with_timeline(meta, demo_bytes, None)
    }

    fn push_finished_demo_with_timeline(
        &mut self,
        mut meta: RoundReplayMeta,
        demo_bytes: Vec<u8>,
        timeline_json: Option<String>,
    ) -> Result<(), String> {
        if !valid_replay_id(&meta.id) {
            return Err(format!("invalid replay id `{}`", meta.id));
        }
        if !demo_bytes.starts_with(&DEMO_MAGIC) {
            return Err("replay bytes do not begin with HLDEMO magic".to_string());
        }
        if demo_bytes.len() > MAX_SINGLE_REPLAY_BYTES || demo_bytes.len() > self.max_total_bytes {
            return Err(format!(
                "replay size {} exceeds limit",
                demo_bytes.len()
            ));
        }
        meta.byte_len = demo_bytes.len();

        // Replace any existing entry with the same ID before enforcing ring limits.
        if let Some(pos) = self.rounds.iter().position(|r| r.meta.id == meta.id)
            && let Some(old) = self.rounds.remove(pos)
        {
            self.total_bytes = self.total_bytes.saturating_sub(old.demo_bytes.len());
        }

        while self.rounds.len() >= self.max_rounds
            || self.total_bytes.saturating_add(demo_bytes.len()) > self.max_total_bytes
        {
            let Some(oldest) = self.rounds.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(oldest.demo_bytes.len());
        }

        self.total_bytes = self.total_bytes.saturating_add(demo_bytes.len());
        self.rounds.push_back(StoredRoundReplay {
            meta,
            demo_bytes,
            timeline_json,
        });
        Ok(())
    }

    /// Returns stored replay metadata ordered newest-first.
    #[allow(dead_code)]
    pub fn list(&self) -> Vec<&RoundReplayMeta> {
        self.rounds.iter().rev().map(|r| &r.meta).collect()
    }

    /// Looks up raw `.dem` bytes by replay ID.
    pub fn get_demo(&self, id: &str) -> Option<&[u8]> {
        if !valid_replay_id(id) {
            return None;
        }
        self.rounds
            .iter()
            .find(|r| r.meta.id == id)
            .map(|r| r.demo_bytes.as_slice())
    }

    /// Looks up bounded `Round2dTimeline` JSON by replay ID.
    pub fn get_timeline_json(&self, id: &str) -> Option<&str> {
        if !valid_replay_id(id) {
            return None;
        }
        self.rounds
            .iter()
            .find(|r| r.meta.id == id)
            .and_then(|r| r.timeline_json.as_deref())
    }

    /// Looks up replay metadata by ID.
    #[allow(dead_code)]
    pub fn get_meta(&self, id: &str) -> Option<&RoundReplayMeta> {
        if !valid_replay_id(id) {
            return None;
        }
        self.rounds.iter().find(|r| r.meta.id == id).map(|r| &r.meta)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.rounds.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.rounds.is_empty()
    }

    #[allow(dead_code)]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.rounds.clear();
        self.total_bytes = 0;
        self.active = None;
    }

    /// Serializes the replay index as a strict JSON object (`rounds` newest-first).
    pub fn to_json_index(&self) -> String {
        let mut out = String::with_capacity(256 + self.rounds.len() * 192);
        let _ = write!(
            out,
            "{{\"count\":{},\"max_rounds\":{},\"total_bytes\":{},\"recording\":{},\"rounds\":[",
            self.rounds.len(),
            self.max_rounds,
            self.total_bytes,
            self.active.is_some()
        );
        for (idx, entry) in self.rounds.iter().rev().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let m = &entry.meta;
            let download_url = format!("/api/replays/{}.dem", m.id);
            let has_timeline = entry.timeline_json.is_some();
            let timeline_url_json = if has_timeline {
                escape_json_str(&format!("/api/replays/{}/timeline", m.id))
            } else {
                "null".to_string()
            };
            let mvp_json = match m.mvp_player.as_deref() {
                Some(mvp) => escape_json_str(mvp),
                None => "null".to_string(),
            };
            let _ = write!(
                out,
                "{{\"id\":{},\"round_number\":{},\"map\":{},\"map_crc\":{},\"winner\":{},\"reason\":{},\"duration_secs\":{},\"frame_count\":{},\"byte_len\":{},\"recorded_at_secs\":{},\"score_ct\":{},\"score_t\":{},\"mvp_player\":{},\"download_url\":{},\"has_timeline\":{},\"timeline_url\":{}}}",
                escape_json_str(&m.id),
                m.round_number,
                escape_json_str(&m.map_name),
                m.map_crc,
                escape_json_str(m.winner.as_str()),
                escape_json_str(&m.reason),
                fmt_finite_f64(m.duration_secs, 2),
                m.frame_count,
                m.byte_len,
                fmt_finite_f64(m.recorded_at_secs, 2),
                m.score_ct,
                m.score_t,
                mvp_json,
                escape_json_str(&download_url),
                has_timeline,
                timeline_url_json,
            );
        }
        out.push_str("]}");
        out
    }
}

/// Aggregated per-player statistics across the current match.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerMatchStats {
    pub key: String,
    pub name: String,
    pub team: TeamSide,
    pub is_bot: bool,
    pub kills: u32,
    pub deaths: u32,
    pub headshots: u32,
    pub assists: u32,
    pub damage_dealt: u64,
    pub rounds_played: u32,
    pub plants: u32,
    pub defuses: u32,
    pub hostage_rescues: u32,
    pub mvps: u32,
}

impl PlayerMatchStats {
    fn new(key: String, name: String, team: TeamSide, is_bot: bool) -> Self {
        Self {
            key,
            name,
            team,
            is_bot,
            kills: 0,
            deaths: 0,
            headshots: 0,
            assists: 0,
            damage_dealt: 0,
            rounds_played: 0,
            plants: 0,
            defuses: 0,
            hostage_rescues: 0,
            mvps: 0,
        }
    }

    /// Average Damage per Round (`damage_dealt / rounds_played`).
    pub fn adr(&self) -> f64 {
        if self.rounds_played == 0 {
            0.0
        } else {
            self.damage_dealt as f64 / f64::from(self.rounds_played)
        }
    }

    /// Headshot percentage (`0.0..=100.0`).
    pub fn hs_percent(&self) -> f64 {
        if self.kills == 0 {
            0.0
        } else {
            (f64::from(self.headshots) / f64::from(self.kills)) * 100.0
        }
    }

    /// Kill-to-death ratio.
    pub fn kd_ratio(&self) -> f64 {
        if self.deaths == 0 {
            f64::from(self.kills)
        } else {
            f64::from(self.kills) / f64::from(self.deaths)
        }
    }
}

/// Summary of one completed round tracked by [`MatchStatsTracker`].
#[derive(Debug, Clone, PartialEq)]
pub struct RoundSummaryRecord {
    pub round_number: u32,
    pub map_name: String,
    pub winner: RoundWinner,
    pub reason: String,
    pub duration_secs: f64,
    pub mvp_key: Option<String>,
    pub mvp_name: Option<String>,
    pub score_ct: u32,
    pub score_t: u32,
    pub total_kills: u32,
    pub total_damage: u64,
}

/// Tracks player kills, deaths, headshots, assists, damage/ADR, objectives, MVPs, and
/// per-round history for live scoreboard / web admin display.
pub struct MatchStatsTracker {
    map_name: String,
    current_round: u32,
    score_ct: u32,
    score_t: u32,
    players: BTreeMap<String, PlayerMatchStats>,
    round_kills: BTreeMap<String, u32>,
    round_damage: BTreeMap<String, u64>,
    round_objectives: BTreeMap<String, u32>,
    round_participants: BTreeSet<String>,
    damage_to_victim: BTreeMap<String, BTreeMap<String, u64>>,
    round_history: VecDeque<RoundSummaryRecord>,
}

impl Default for MatchStatsTracker {
    fn default() -> Self {
        Self::new("de_dust2")
    }
}

impl MatchStatsTracker {
    pub fn new(map_name: impl Into<String>) -> Self {
        Self {
            map_name: map_name.into(),
            current_round: 0,
            score_ct: 0,
            score_t: 0,
            players: BTreeMap::new(),
            round_kills: BTreeMap::new(),
            round_damage: BTreeMap::new(),
            round_objectives: BTreeMap::new(),
            round_participants: BTreeSet::new(),
            damage_to_victim: BTreeMap::new(),
            round_history: VecDeque::new(),
        }
    }

    pub fn reset_match(&mut self, map_name: impl Into<String>) {
        self.map_name = map_name.into();
        self.current_round = 0;
        self.score_ct = 0;
        self.score_t = 0;
        self.players.clear();
        self.clear_round_accumulators();
        self.round_history.clear();
    }

    fn clear_round_accumulators(&mut self) {
        self.round_kills.clear();
        self.round_damage.clear();
        self.round_objectives.clear();
        self.round_participants.clear();
        self.damage_to_victim.clear();
    }

    fn ensure_player_mut(&mut self, key: &str) -> &mut PlayerMatchStats {
        self.players.entry(key.to_string()).or_insert_with(|| {
            PlayerMatchStats::new(key.to_string(), key.to_string(), TeamSide::Unassigned, false)
        })
    }

    /// Registers or updates a player's display name, team, and bot flag.
    pub fn register_player(&mut self, key: &str, name: &str, team: TeamSide, is_bot: bool) {
        let entry = self.players.entry(key.to_string()).or_insert_with(|| {
            PlayerMatchStats::new(key.to_string(), name.to_string(), team, is_bot)
        });
        entry.name = name.to_string();
        entry.team = team;
        entry.is_bot = is_bot;
        if matches!(team, TeamSide::Terrorist | TeamSide::CounterTerrorist) {
            self.round_participants.insert(key.to_string());
        }
    }

    /// Starts tracking a new round and clears per-round accumulators.
    pub fn begin_round(&mut self, round_number: u32) {
        self.current_round = round_number;
        self.clear_round_accumulators();
        for (key, p) in &self.players {
            if matches!(p.team, TeamSide::Terrorist | TeamSide::CounterTerrorist) {
                self.round_participants.insert(key.clone());
            }
        }
    }

    /// Records damage dealt by `attacker_key` to `victim_key` (clamped positive damage).
    pub fn record_damage(&mut self, attacker_key: Option<&str>, victim_key: &str, damage: u32) {
        self.round_participants.insert(victim_key.to_string());
        if let Some(attacker) = attacker_key {
            self.round_participants.insert(attacker.to_string());
            if attacker != victim_key && damage > 0 {
                let dmg = u64::from(damage);
                self.ensure_player_mut(attacker).damage_dealt += dmg;
                *self.round_damage.entry(attacker.to_string()).or_insert(0) += dmg;
                *self
                    .damage_to_victim
                    .entry(victim_key.to_string())
                    .or_default()
                    .entry(attacker.to_string())
                    .or_insert(0) += dmg;
            }
        }
    }

    /// Records a kill event, updating kills/deaths/headshots and attributing at most one assist.
    pub fn record_kill(
        &mut self,
        killer_key: Option<&str>,
        victim_key: &str,
        explicit_assist_key: Option<&str>,
        headshot: bool,
        friendly_fire: bool,
    ) {
        self.round_participants.insert(victim_key.to_string());
        self.ensure_player_mut(victim_key).deaths += 1;

        let chosen_assist = match explicit_assist_key {
            Some(a) if Some(a) != killer_key && a != victim_key => Some(a.to_string()),
            _ => self
                .damage_to_victim
                .get(victim_key)
                .and_then(|contribs| {
                    contribs
                        .iter()
                        .filter(|(k, dmg)| {
                            Some(k.as_str()) != killer_key
                                && k.as_str() != victim_key
                                && **dmg >= ASSIST_DAMAGE_THRESHOLD
                        })
                        .max_by(|(ka, da), (kb, db)| da.cmp(db).then_with(|| kb.cmp(ka)))
                        .map(|(k, _)| k.clone())
                }),
        };
        self.damage_to_victim.remove(victim_key);

        if let Some(assist) = chosen_assist {
            self.round_participants.insert(assist.clone());
            self.ensure_player_mut(&assist).assists += 1;
        }

        if let Some(killer) = killer_key {
            self.round_participants.insert(killer.to_string());
            if killer != victim_key && !friendly_fire {
                let p = self.ensure_player_mut(killer);
                p.kills += 1;
                if headshot {
                    p.headshots += 1;
                }
                *self.round_kills.entry(killer.to_string()).or_insert(0) += 1;
            }
        }
    }

    pub fn record_bomb_plant(&mut self, player_key: &str) {
        self.round_participants.insert(player_key.to_string());
        self.ensure_player_mut(player_key).plants += 1;
        *self
            .round_objectives
            .entry(player_key.to_string())
            .or_insert(0) += 1;
    }

    pub fn record_bomb_defuse(&mut self, player_key: &str) {
        self.round_participants.insert(player_key.to_string());
        self.ensure_player_mut(player_key).defuses += 1;
        *self
            .round_objectives
            .entry(player_key.to_string())
            .or_insert(0) += 1;
    }

    pub fn record_hostage_rescue(&mut self, player_key: &str) {
        self.round_participants.insert(player_key.to_string());
        self.ensure_player_mut(player_key).hostage_rescues += 1;
        *self
            .round_objectives
            .entry(player_key.to_string())
            .or_insert(0) += 1;
    }

    /// Finalizes the round, updates team scores, computes/credits MVP, and appends round history.
    pub fn end_round(
        &mut self,
        winner: RoundWinner,
        reason: &str,
        duration_secs: f64,
        explicit_mvp_key: Option<&str>,
    ) -> RoundSummaryRecord {
        if self.current_round == 0 {
            self.current_round = 1;
        }
        match winner {
            RoundWinner::CounterTerrorist => self.score_ct = self.score_ct.saturating_add(1),
            RoundWinner::Terrorist => self.score_t = self.score_t.saturating_add(1),
            RoundWinner::Draw => {}
        }

        let participants: Vec<String> = self.round_participants.iter().cloned().collect();
        for key in &participants {
            self.ensure_player_mut(key).rounds_played += 1;
        }

        let mvp_key = explicit_mvp_key
            .map(ToString::to_string)
            .or_else(|| self.select_round_mvp(winner));
        let mvp_name = if let Some(ref k) = mvp_key {
            let p = self.ensure_player_mut(k);
            p.mvps += 1;
            Some(p.name.clone())
        } else {
            None
        };

        let total_kills = self.round_kills.values().copied().sum();
        let total_damage = self.round_damage.values().copied().sum();
        let record = RoundSummaryRecord {
            round_number: self.current_round,
            map_name: self.map_name.clone(),
            winner,
            reason: reason.to_string(),
            duration_secs: duration_secs.max(0.0),
            mvp_key,
            mvp_name,
            score_ct: self.score_ct,
            score_t: self.score_t,
            total_kills,
            total_damage,
        };

        if self.round_history.len() >= MAX_ROUND_HISTORY {
            self.round_history.pop_front();
        }
        self.round_history.push_back(record.clone());
        self.clear_round_accumulators();
        record
    }

    fn select_round_mvp(&self, winner: RoundWinner) -> Option<String> {
        let winning_team = match winner {
            RoundWinner::CounterTerrorist => Some(TeamSide::CounterTerrorist),
            RoundWinner::Terrorist => Some(TeamSide::Terrorist),
            RoundWinner::Draw => None,
        };
        let mut best: Option<(u64, &str)> = None;
        for (key, p) in &self.players {
            if let Some(team) = winning_team
                && p.team != team
            {
                continue;
            }
            let kills = u64::from(self.round_kills.get(key).copied().unwrap_or(0));
            let objs = u64::from(self.round_objectives.get(key).copied().unwrap_or(0));
            let dmg = self.round_damage.get(key).copied().unwrap_or(0);
            let score = objs * 300 + kills * 200 + dmg;
            if score == 0 {
                continue;
            }
            match best {
                None => best = Some((score, key.as_str())),
                Some((best_score, best_key))
                    if score > best_score || (score == best_score && key.as_str() < best_key) =>
                {
                    best = Some((score, key.as_str()));
                }
                _ => {}
            }
        }
        best.map(|(_, k)| k.to_string())
    }

    #[allow(dead_code)]
    pub fn player(&self, key: &str) -> Option<&PlayerMatchStats> {
        self.players.get(key)
    }

    pub fn scores(&self) -> (u32, u32) {
        (self.score_ct, self.score_t)
    }

    pub fn current_round(&self) -> u32 {
        self.current_round
    }

    #[allow(dead_code)]
    pub fn map_name(&self) -> &str {
        &self.map_name
    }

    #[allow(dead_code)]
    pub fn round_history(&self) -> &VecDeque<RoundSummaryRecord> {
        &self.round_history
    }

    /// Returns players ordered by kills, MVPs, ADR, fewest deaths, then key.
    pub fn leaderboard(&self) -> Vec<&PlayerMatchStats> {
        let mut list: Vec<&PlayerMatchStats> = self.players.values().collect();
        list.sort_by(|a, b| {
            b.kills
                .cmp(&a.kills)
                .then_with(|| b.mvps.cmp(&a.mvps))
                .then_with(|| b.adr().total_cmp(&a.adr()))
                .then_with(|| a.deaths.cmp(&b.deaths))
                .then_with(|| a.key.cmp(&b.key))
        });
        list
    }

    /// Exports match summary, sorted player leaderboard, and per-round history as JSON.
    pub fn to_json(&self) -> String {
        let players = self.leaderboard();
        let mut out = String::with_capacity(512 + players.len() * 220 + self.round_history.len() * 140);
        let _ = write!(
            out,
            "{{\"map\":{},\"current_round\":{},\"score_ct\":{},\"score_t\":{},\"players\":[",
            escape_json_str(&self.map_name),
            self.current_round,
            self.score_ct,
            self.score_t,
        );
        for (idx, p) in players.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"key\":{},\"name\":{},\"team\":{},\"is_bot\":{},\"kills\":{},\"deaths\":{},\"headshots\":{},\"hs_percent\":{},\"assists\":{},\"damage\":{},\"rounds_played\":{},\"adr\":{},\"kd_ratio\":{},\"plants\":{},\"defuses\":{},\"hostage_rescues\":{},\"mvps\":{}}}",
                escape_json_str(&p.key),
                escape_json_str(&p.name),
                escape_json_str(p.team.as_str()),
                p.is_bot,
                p.kills,
                p.deaths,
                p.headshots,
                fmt_finite_f64(p.hs_percent(), 1),
                p.assists,
                p.damage_dealt,
                p.rounds_played,
                fmt_finite_f64(p.adr(), 1),
                fmt_finite_f64(p.kd_ratio(), 2),
                p.plants,
                p.defuses,
                p.hostage_rescues,
                p.mvps,
            );
        }
        out.push_str("],\"rounds\":[");
        for (idx, r) in self.round_history.iter().rev().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let mvp_name_json = match r.mvp_name.as_deref() {
                Some(n) => escape_json_str(n),
                None => "null".to_string(),
            };
            let _ = write!(
                out,
                "{{\"round_number\":{},\"map\":{},\"winner\":{},\"reason\":{},\"duration_secs\":{},\"mvp_name\":{},\"score_ct\":{},\"score_t\":{},\"total_kills\":{},\"total_damage\":{}}}",
                r.round_number,
                escape_json_str(&r.map_name),
                escape_json_str(r.winner.as_str()),
                escape_json_str(&r.reason),
                fmt_finite_f64(r.duration_secs, 2),
                mvp_name_json,
                r.score_ct,
                r.score_t,
                r.total_kills,
                r.total_damage,
            );
        }
        out.push_str("]}");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::bitbuf::MsgWriter;
    use cs_proto48::demo::{DEMO_PROTOCOL, parse_demo};
    use cs_proto48::messages::{
        ServerInfoMsg, write_svc_print, write_svc_serverinfo, write_svc_time,
    };

    fn sample_signon_packet() -> Vec<u8> {
        let mut w = MsgWriter::new();
        write_svc_serverinfo(
            &mut w,
            &ServerInfoMsg {
                protocol: 48,
                spawn_count: 1,
                map_crc: 0x1234_5678,
                client_dll_md5: [0u8; 16],
                max_clients: 16,
                player_slot: 0,
                is_deathmatch: true,
                game_dir: "cstrike".into(),
                hostname: "Test Server".into(),
                map_path: "maps/de_dust2.bsp".into(),
                map_cycle: String::new(),
                vac_secure: false,
            },
        );
        w.into_bytes()
    }

    fn sample_tick_packet() -> Vec<u8> {
        let mut w = MsgWriter::new();
        write_svc_time(&mut w, 1.25);
        write_svc_print(&mut w, "Round live\n");
        w.into_bytes()
    }

    #[test]
    fn records_valid_hldemo_v5_per_round_and_enforces_ring_buffer() {
        let mut store = RoundReplayStore::new(2);
        let signon_pkt = sample_signon_packet();
        let tick_pkt = sample_tick_packet();
        let zero_vec = DemoNetMsgInfo::default().ref_params.vieworg;

        for round in 1..=3 {
            let t0 = f64::from(round) * 10.0;
            store.begin_round("de_dust2", 0xABCD_0000 + round, t0);
            store.record_svc_packet(t0 + 0.05, true, &signon_pkt);
            store.mark_signon_complete(t0 + 0.1);
            store.record_svc_packet(t0 + 0.5, false, &tick_pkt);
            store.record_console_command(t0 + 1.0, "say glhf");
            store.record_client_data(
                t0 + 1.5,
                DemoClientData {
                    origin: zero_vec,
                    viewangles: zero_vec,
                    weapon_bits: 0,
                    fov: 90.0,
                },
            );

            let meta = store
                .finish_round(
                    RoundWinner::CounterTerrorist,
                    "Target Defused",
                    t0 + 25.0,
                    round,
                    0,
                    Some("Alice".to_string()),
                )
                .unwrap()
                .expect("round metadata");
            assert_eq!(meta.round_number, round);
            assert!(meta.byte_len > 544);
        }

        // Ring buffer capacity = 2: round 1 evicted, rounds 2 and 3 remain.
        assert_eq!(store.len(), 2);
        assert!(store.get_demo("r0001-de_dust2").is_none());
        let r3_bytes = store.get_demo("r0003-de_dust2").expect("round 3 demo");
        let parsed = parse_demo(r3_bytes).expect("valid HLDEMO v5");
        assert_eq!(parsed.header.demo_protocol, DEMO_PROTOCOL);
        assert_eq!(parsed.header.net_protocol, 48);
        assert_eq!(parsed.header.map_name, "de_dust2");

        // Traversal IDs are rejected.
        assert!(store.get_demo("../r0003-de_dust2").is_none());
        assert!(!valid_replay_id("bad/id"));

        let json = store.to_json_index();
        assert!(json.contains("\"count\":2"));
        assert!(json.contains("\"/api/replays/r0003-de_dust2.dem\""));
        assert!(json.contains("\"/api/replays/r0002-de_dust2.dem\""));
    }

    #[test]
    fn match_stats_tracker_computes_adr_hs_assists_objectives_mvp_and_json() {
        let mut tracker = MatchStatsTracker::new("de_inferno");
        tracker.register_player("p1", "Alice\"<x>", TeamSide::CounterTerrorist, false);
        tracker.register_player("p2", "Bob", TeamSide::CounterTerrorist, false);
        tracker.register_player("t1", "Trudy", TeamSide::Terrorist, true);

        tracker.begin_round(1);
        tracker.record_damage(Some("p2"), "t1", 55);
        tracker.record_damage(Some("p1"), "t1", 45);
        tracker.record_kill(Some("p1"), "t1", None, true, false);
        tracker.record_bomb_defuse("p1");
        let summary = tracker.end_round(
            RoundWinner::CounterTerrorist,
            "Bomb Defused",
            42.5,
            None,
        );

        assert_eq!(summary.score_ct, 1);
        assert_eq!(summary.score_t, 0);
        assert_eq!(summary.mvp_key.as_deref(), Some("p1"));

        let p1 = tracker.player("p1").unwrap();
        assert_eq!(p1.kills, 1);
        assert_eq!(p1.headshots, 1);
        assert_eq!(p1.defuses, 1);
        assert_eq!(p1.mvps, 1);
        assert_eq!(p1.rounds_played, 1);
        assert!((p1.adr() - 45.0).abs() < 1e-6);
        assert!((p1.hs_percent() - 100.0).abs() < 1e-6);

        // Bob dealt 55 damage to Trudy before Alice got the kill -> automatic assist.
        let p2 = tracker.player("p2").unwrap();
        assert_eq!(p2.assists, 1);
        assert!((p2.adr() - 55.0).abs() < 1e-6);

        let json = tracker.to_json();
        assert!(json.contains("\"name\":\"Alice\\\"\\u003cx\\u003e\""));
        assert!(json.contains("\"defuses\":1"));
        assert!(json.contains("\"assists\":1"));
        assert!(json.contains("\"rounds\":[{\"round_number\":1"));
    }

    fn build_8bit_overview_bmp_with_index_255_green() -> Vec<u8> {
        let w = 2u32;
        let h = 2u32;
        let row_stride = 4usize; // 2 bytes padded to 4-byte boundary
        let pixel_off = 54u32 + 256 * 4;
        let file_size = pixel_off + (h * row_stride as u32);
        let mut bmp = vec![0u8; file_size as usize];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&file_size.to_le_bytes());
        bmp[10..14].copy_from_slice(&pixel_off.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&(w as i32).to_le_bytes());
        bmp[22..26].copy_from_slice(&(h as i32).to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&8u16.to_le_bytes());
        // Palette entry 1 = bright red (BGR = 0, 0, 255).
        bmp[54 + 4..54 + 8].copy_from_slice(&[0, 0, 255, 0]);
        // Palette entry 255 = pure GoldSrc chromakey green (BGR = 0, 255, 0).
        bmp[54 + 255 * 4..54 + 256 * 4].copy_from_slice(&[0, 255, 0, 0]);
        // Bottom row (row 0): [255, 1], top row (row 1): [1, 255]
        let po = pixel_off as usize;
        bmp[po] = 255;
        bmp[po + 1] = 1;
        bmp[po + row_stride] = 1;
        bmp[po + row_stride + 1] = 255;
        bmp
    }

    fn build_minimal_bsp_v30_with_bounds(mins: [f32; 3], maxs: [f32; 3]) -> Vec<u8> {
        let header_len = 4 + 15 * 8;
        let mut bsp = vec![0u8; header_len + 64];
        bsp[0..4].copy_from_slice(&30i32.to_le_bytes());
        let lump14_hdr = 4 + 14 * 8;
        bsp[lump14_hdr..lump14_hdr + 4].copy_from_slice(&(header_len as i32).to_le_bytes());
        bsp[lump14_hdr + 4..lump14_hdr + 8].copy_from_slice(&64i32.to_le_bytes());
        for (idx, v) in mins.iter().chain(maxs.iter()).enumerate() {
            let off = header_len + idx * 4;
            bsp[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
        bsp
    }

    #[test]
    fn round_2d_timeline_samples_at_10hz_and_caps_at_512_kib() {
        let mut store = RoundReplayStore::new(4);
        store.begin_round("de_dust2", 0x1122_3344, 100.0);
        store.set_timeline_player(1, "Alice", 2, false);
        store.set_timeline_player(2, "Bot_Bob", 1, true);

        let sample_a = TimelinePlayerSample {
            slot: 1,
            team: 2,
            alive: true,
            hp: 100,
            x: -223.0,
            y: 1097.0,
            z: -192.0,
            yaw_deg: 90.0,
            active_weapon_id: 22,
            has_c4: false,
            flashed_alpha: 0,
        };
        let sample_b = TimelinePlayerSample {
            slot: 2,
            team: 1,
            alive: true,
            hp: 85,
            x: 120.5,
            y: 640.0,
            z: -120.0,
            yaw_deg: -45.0,
            active_weapon_id: 28,
            has_c4: true,
            flashed_alpha: 128,
        };

        // Feeding at 100 Hz (0.01s) must coalesce to 10 Hz (0.1s).
        for step in 0..=25 {
            let t = 100.0 + f64::from(step) * 0.01;
            store.sample_timeline_if_due(t, &[sample_a, sample_b]);
        }
        store.record_timeline_shot(100.12, 1, [-223.0, 1097.0, -175.0], [120.5, 640.0, -105.0], 22);
        store.record_timeline_grenade_pos(100.15, 77, TimelineGrenadeKind::Smoke, [-200.0, 1000.0, -170.0]);
        store.record_timeline_grenade_pos(100.25, 77, TimelineGrenadeKind::Smoke, [0.0, 800.0, -180.0]);
        store.detonate_timeline_grenade(100.25, 77, Some([0.0, 800.0, -180.0]));
        store.record_timeline_bomb(100.20, TimelineBombState::Planted, [100.0, 650.0, -120.0]);
        store.record_timeline_kill(100.22, Some(1), 2, "m4a1", true);

        let meta = store
            .finish_round(RoundWinner::CounterTerrorist, "Bomb Defused", 100.3, 1, 0, Some("Alice".into()))
            .unwrap()
            .unwrap();
        let tl_json = store.get_timeline_json(&meta.id).expect("timeline JSON stored");
        assert!(tl_json.len() <= MAX_TIMELINE_BYTES);
        assert!(tl_json.contains("\"sample_rate_hz\":10"));
        assert!(tl_json.contains("\"frames\":["));
        // 0.00, 0.10, 0.20 => 3 sampled frames across 26 100 Hz steps.
        assert_eq!(tl_json.matches("\"t\":").count(), 3);
        assert!(tl_json.contains("\"kind\":\"smoke\""));
        assert!(tl_json.contains("\"state\":\"planted\""));
        assert!(tl_json.contains("\"headshot\":true"));
        assert!(store.to_json_index().contains("\"has_timeline\":true"));

        // Hard 512 KiB bound even with a huge synthetic 32-player 3-minute round.
        let mut rec = Round2dTimelineRecorder::new(0.0);
        let roster: Vec<TimelinePlayerSample> = (1..=32)
            .map(|s| TimelinePlayerSample {
                slot: s,
                team: if s <= 16 { 2 } else { 1 },
                alive: true,
                hp: 100,
                x: f32::from(s) * 123.4,
                y: -f32::from(s) * 234.5,
                z: -128.0,
                yaw_deg: 179.5,
                active_weapon_id: 28,
                has_c4: s == 17,
                flashed_alpha: 200,
            })
            .collect();
        for tick in 0..1800 {
            let t = f64::from(tick) * 0.1;
            rec.sample_if_due(t, &roster);
            rec.record_shot(t, 1, [10.0, 20.0, 30.0], [500.0, 600.0, 40.0], 28);
        }
        let (_tl, bounded) = rec.finish(
            "r0099-de_dust2".into(),
            99,
            "de_dust2".into(),
            RoundWinner::Terrorist,
            "Target Bombed".into(),
            180.0,
            5,
            6,
        );
        assert!(
            bounded.len() <= MAX_TIMELINE_BYTES,
            "bounded len {} exceeded 512 KiB ({MAX_TIMELINE_BYTES})",
            bounded.len()
        );
        assert!(bounded.starts_with('{') && bounded.ends_with('}'));
    }

    #[test]
    fn overview_bmp_png_conversion_chromakeys_index_255_and_falls_back_to_bsp_bounds() {
        let bmp = build_8bit_overview_bmp_with_index_255_green();
        let bsp = build_minimal_bsp_v30_with_bounds([-1500.0, -1200.0, -256.0], [1800.0, 2100.0, 512.0]);
        let overview_txt = b"global { ZOOM 1.50 ORIGIN -223 1097 -192 ROTATED 0 }\nlayer { IMAGE \"overviews/de_test.bmp\" HEIGHT -192 }\n".to_vec();

        let vfs = Vfs::from_memory([
            ("overviews/de_test.txt".into(), overview_txt),
            ("overviews/de_test.bmp".into(), bmp),
            ("maps/de_test.bsp".into(), bsp.clone()),
            ("maps/fy_wireframe.bsp".into(), bsp),
        ]);

        let bundle = load_map_overview_from_vfs(&vfs, "de_test", None).expect("de_test overview");
        assert!(bundle.has_overview);
        assert!((bundle.zoom - 1.50).abs() < 1e-4);
        assert_eq!(bundle.origin, [-223.0, 1097.0, -192.0]);
        assert!(!bundle.rotated);
        assert_eq!(bundle.mins, [-1500.0, -1200.0, -256.0]);
        assert_eq!(bundle.maxs, [1800.0, 2100.0, 512.0]);
        let png = bundle.png_bytes.as_ref().expect("PNG bytes");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

        let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .expect("valid PNG")
            .to_rgba8();
        // Top-left (0,0) in image was top row col 0 = index 1 (red, opaque);
        // top-right (1,0) was index 255 (green, transparent).
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(1, 0).0[3], 0);
        assert_eq!(decoded.get_pixel(0, 1).0[3], 0);
        assert_eq!(decoded.get_pixel(1, 1).0, [255, 0, 0, 255]);

        let ov_json = bundle.to_json();
        assert!(ov_json.contains("\"has_overview\":true"));
        assert!(ov_json.contains("\"image_url\":\"/api/map-overview/de_test.png\""));

        // Map without overview BMP/TXT falls back to BSP world bounds for wireframe radar.
        let fallback = load_map_overview_from_vfs(&vfs, "fy_wireframe", None).expect("wireframe fallback");
        assert!(!fallback.has_overview);
        assert!(fallback.png_bytes.is_none());
        assert_eq!(fallback.mins, [-1500.0, -1200.0, -256.0]);
        assert_eq!(fallback.maxs, [1800.0, 2100.0, 512.0]);
        let fb_json = fallback.to_json();
        assert!(fb_json.contains("\"has_overview\":false"));
        assert!(fb_json.contains("\"image_url\":null"));

        assert!(!valid_map_overview_name("../secret"));
        assert!(valid_map_overview_name("fy_pool_day"));
    }
}
