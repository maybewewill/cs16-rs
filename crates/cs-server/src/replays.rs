//! Built-in per-round HLDEMO v5 (`.dem`) replay store and live match statistics tracker
//! for `cs-server`.
//!
//! Zero `unsafe` code; bounded ring-buffer memory budgets prevent unbounded growth across
//! long-running dedicated server sessions.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

use cs_clientcore::DemoRecorder;
use cs_proto48::demo::{DEMO_MAGIC, DemoClientData, DemoNetMsgInfo};

/// Default maximum number of completed round `.dem` files retained in memory.
pub const DEFAULT_MAX_REPLAY_ROUNDS: usize = 32;
/// Hard ceiling for a single recorded round `.dem` payload (16 MiB).
pub const MAX_SINGLE_REPLAY_BYTES: usize = 16 * 1024 * 1024;
/// Default total memory cap across all buffered round `.dem` files (128 MiB).
pub const DEFAULT_MAX_REPLAY_STORE_BYTES: usize = 128 * 1024 * 1024;
/// Maximum number of round summaries retained in `MatchStatsTracker`.
pub const MAX_ROUND_HISTORY: usize = 256;

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
}

struct ActiveRoundRecording {
    round_number: u32,
    map_name: String,
    map_crc: u32,
    started_at_secs: f64,
    seq_counter: i32,
    recorder: DemoRecorder,
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
        });
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
        self.push_finished_demo(meta.clone(), bytes)?;
        Ok(Some(meta))
    }

    /// Inserts an already-encoded HLDEMO v5 `.dem` buffer, enforcing ID and size guards.
    pub fn push_finished_demo(
        &mut self,
        mut meta: RoundReplayMeta,
        demo_bytes: Vec<u8>,
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
        self.rounds.push_back(StoredRoundReplay { meta, demo_bytes });
        Ok(())
    }

    /// Returns stored replay metadata ordered newest-first.
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

    /// Looks up replay metadata by ID.
    pub fn get_meta(&self, id: &str) -> Option<&RoundReplayMeta> {
        if !valid_replay_id(id) {
            return None;
        }
        self.rounds.iter().find(|r| r.meta.id == id).map(|r| &r.meta)
    }

    pub fn len(&self) -> usize {
        self.rounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rounds.is_empty()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn is_recording(&self) -> bool {
        self.active.is_some()
    }

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
            let mvp_json = match m.mvp_player.as_deref() {
                Some(mvp) => escape_json_str(mvp),
                None => "null".to_string(),
            };
            let _ = write!(
                out,
                "{{\"id\":{},\"round_number\":{},\"map\":{},\"map_crc\":{},\"winner\":{},\"reason\":{},\"duration_secs\":{},\"frame_count\":{},\"byte_len\":{},\"recorded_at_secs\":{},\"score_ct\":{},\"score_t\":{},\"mvp_player\":{},\"download_url\":{}}}",
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

    pub fn player(&self, key: &str) -> Option<&PlayerMatchStats> {
        self.players.get(key)
    }

    pub fn scores(&self) -> (u32, u32) {
        (self.score_ct, self.score_t)
    }

    pub fn current_round(&self) -> u32 {
        self.current_round
    }

    pub fn map_name(&self) -> &str {
        &self.map_name
    }

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
}
