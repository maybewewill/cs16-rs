//! Built-in KZ/bhop mode (`sv_kz_mode`, checkpoints, `func_button` map timers,
//! top-10 in-memory runs, non-lethal fall damage) and server-side Kreedz JumpStats (`sv_ljstats`).

use cs_engine::consts::{
    EF_NOINTERP, FL_DUCKING, FL_ONGROUND, MOVETYPE_NOCLIP, MOVETYPE_WALK,
};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::ljstats::{JumpFrameInput, JumpStats, JumpTracker};
use cs_sim::pm::PlayerMove;
use cs_sim::pm::consts::{IN_JUMP, PLAYER_HULL_MAXS, PLAYER_HULL_MINS};
use glam::Vec3;

use crate::GameState;
use crate::menus;
use crate::player::{MAX_PLAYERS, player_slot};

const HUD_PRINTTALK: u8 = 3;

/// Maximum checkpoints stored per player in KZ mode.
pub const MAX_KZ_CHECKPOINTS: usize = 64;

/// Maximum runs kept in the in-memory map leaderboard.
pub const MAX_KZ_TOP_RUNS: usize = 10;

const VEC_HULL_MIN: Vec3 = PLAYER_HULL_MINS[0];
const VEC_HULL_MAX: Vec3 = PLAYER_HULL_MAXS[0];
const VEC_DUCK_HULL_MIN: Vec3 = PLAYER_HULL_MINS[1];
const VEC_DUCK_HULL_MAX: Vec3 = Vec3::new(16.0, 16.0, 32.0);

/// Saved KZ checkpoint pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KzCheckpoint {
    pub origin: Vec3,
    pub angles: Vec3,
    pub ducking: bool,
}

/// Completed KZ map run stored in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct KzRunRecord {
    pub player_name: String,
    pub time: f64,
    pub checkpoints: u32,
    pub teleports: u32,
}

/// Per-player KZ checkpoint, timer, and JumpStats state.
#[derive(Debug, Clone, Default)]
pub struct KzPlayerState {
    pub checkpoints: Vec<KzCheckpoint>,
    pub start_pos: Option<KzCheckpoint>,
    pub timer_started_at: Option<f64>,
    pub cp_count: u32,
    pub tp_count: u32,
    pub lj_tracker: JumpTracker,
    pub last_ljstats: Option<JumpStats>,
}

/// Game-wide KZ mode state (`GameState::kz`).
#[derive(Debug, Clone)]
pub struct KzState {
    pub players: [KzPlayerState; MAX_PLAYERS],
    pub top_runs: Vec<KzRunRecord>,
}

impl Default for KzState {
    fn default() -> Self {
        Self {
            players: std::array::from_fn(|_| KzPlayerState::default()),
            top_runs: Vec::new(),
        }
    }
}

impl KzState {
    pub fn reset_map(&mut self) {
        self.players = std::array::from_fn(|_| KzPlayerState::default());
        self.top_runs.clear();
    }

    pub fn reset_player(&mut self, slot: u8) {
        if let Some(p) = self.players.get_mut(usize::from(slot)) {
            *p = KzPlayerState::default();
        }
    }
}

fn format_kz_time(secs: f64) -> String {
    let total_cs = (secs.max(0.0) * 100.0).round() as u64;
    let mins = total_cs / 6000;
    let rem_s = (total_cs % 6000) / 100;
    let cs = total_cs % 100;
    format!("{mins:02}:{rem_s:02}.{cs:02}")
}

fn send_kz_line(eng: &mut EngineApi<'_>, slot: u8, line: &str) {
    eng.client_print(slot, line);
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::TextMsg {
            dest: HUD_PRINTTALK,
            message: line.to_string(),
            params: Vec::new(),
        },
    );
}

fn capture_pose(eng: &EngineApi<'_>, id: EdictId) -> Option<KzCheckpoint> {
    let v = eng.ent(id)?;
    Some(KzCheckpoint {
        origin: v.origin,
        angles: v.v_angle,
        ducking: v.flags & FL_DUCKING != 0,
    })
}

fn teleport_to_pose(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, cp: KzCheckpoint) {
    let id = eng.client_edict(slot);
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.fall_velocity = 0.0;
    }
    if let Some(kzp) = g.kz.players.get_mut(usize::from(slot)) {
        kzp.lj_tracker.invalidate();
    }
    if cp.ducking {
        eng.set_size(id, VEC_DUCK_HULL_MIN, VEC_DUCK_HULL_MAX);
    } else {
        eng.set_size(id, VEC_HULL_MIN, VEC_HULL_MAX);
    }
    eng.set_origin(id, cp.origin);
    if let Some(v) = eng.ent_mut(id) {
        if cp.ducking {
            v.flags |= FL_DUCKING;
        } else {
            v.flags &= !FL_DUCKING;
        }
        v.velocity = Vec3::ZERO;
        v.basevelocity = Vec3::ZERO;
        v.angles = cp.angles;
        v.v_angle = cp.angles;
        v.fixangle = 1;
        v.effects |= EF_NOINTERP;
    }
}

/// Records the player's initial KZ `/start` pose on spawn and resets active jump tracking.
pub(crate) fn on_player_spawn(g: &mut GameState, eng: &EngineApi<'_>, slot: u8) {
    let idx = usize::from(slot);
    if idx >= MAX_PLAYERS {
        return;
    }
    let id = eng.client_edict(slot);
    g.kz.players[idx].lj_tracker.invalidate();
    if g.kz.players[idx].start_pos.is_none() {
        g.kz.players[idx].start_pos = capture_pose(eng, id);
    }
}

/// Handles both chat (`/cp`, `/tp`, `/gocheck`, `/prevcp`, `/stuck`, `/start`, `/reset`) and
/// console (`cp`, `tp`, `gocheck`, `prevcp`, `stuck`, `start`, `reset`) KZ commands.
pub(crate) fn handle_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    raw_cmd: &str,
    from_chat: bool,
) -> bool {
    let trimmed = raw_cmd.trim();
    if from_chat && !trimmed.starts_with('/') {
        return false;
    }
    let cmd = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let cmd_lower = cmd.to_ascii_lowercase();
    let is_kz_cmd = matches!(
        cmd_lower.as_str(),
        "cp" | "tp" | "gocheck" | "prevcp" | "stuck" | "start" | "reset"
    );
    if !is_kz_cmd {
        return false;
    }
    if eng.cvar_f32("sv_kz_mode") == 0.0 {
        send_kz_line(
            eng,
            slot,
            "[KZ] Enable sv_kz_mode 1 to use KZ checkpoints and timers.\n",
        );
        return true;
    }
    let idx = usize::from(slot);
    if idx >= MAX_PLAYERS {
        return true;
    }
    let alive = g.players[idx].as_ref().is_some_and(|p| p.alive);
    if !alive {
        send_kz_line(eng, slot, "[KZ] You must be alive to use KZ commands.\n");
        return true;
    }
    let id = eng.client_edict(slot);
    match cmd_lower.as_str() {
        "cp" => {
            let Some(v) = eng.ent(id) else {
                return true;
            };
            if v.flags & FL_ONGROUND == 0 || v.movetype != MOVETYPE_WALK {
                send_kz_line(
                    eng,
                    slot,
                    "[KZ] Must be standing on solid ground to save a checkpoint.\n",
                );
                return true;
            }
            let Some(cp) = capture_pose(eng, id) else {
                return true;
            };
            let kzp = &mut g.kz.players[idx];
            if kzp.checkpoints.len() >= MAX_KZ_CHECKPOINTS {
                kzp.checkpoints.remove(0);
            }
            kzp.checkpoints.push(cp);
            kzp.cp_count = kzp.cp_count.saturating_add(1);
            let n = kzp.cp_count;
            let stored = kzp.checkpoints.len();
            send_kz_line(
                eng,
                slot,
                &format!("[KZ] Checkpoint #{n} saved ({stored}/{MAX_KZ_CHECKPOINTS}).\n"),
            );
        }
        "tp" | "gocheck" => {
            let Some(cp) = g.kz.players[idx].checkpoints.last().copied() else {
                send_kz_line(eng, slot, "[KZ] No checkpoints saved.\n");
                return true;
            };
            g.kz.players[idx].tp_count = g.kz.players[idx].tp_count.saturating_add(1);
            let tp = g.kz.players[idx].tp_count;
            teleport_to_pose(g, eng, slot, cp);
            send_kz_line(
                eng,
                slot,
                &format!("[KZ] Teleported to checkpoint (TPs: {tp}).\n"),
            );
        }
        "prevcp" | "stuck" => {
            let kzp = &mut g.kz.players[idx];
            if kzp.checkpoints.len() >= 2 {
                kzp.checkpoints.pop();
            }
            let Some(cp) = kzp.checkpoints.last().copied() else {
                send_kz_line(eng, slot, "[KZ] No previous checkpoint saved.\n");
                return true;
            };
            kzp.tp_count = kzp.tp_count.saturating_add(1);
            let tp = kzp.tp_count;
            teleport_to_pose(g, eng, slot, cp);
            send_kz_line(
                eng,
                slot,
                &format!("[KZ] Returned to previous checkpoint (TPs: {tp}).\n"),
            );
        }
        "start" => {
            let start_pose = g.kz.players[idx].start_pos;
            let kzp = &mut g.kz.players[idx];
            kzp.timer_started_at = None;
            kzp.cp_count = 0;
            kzp.tp_count = 0;
            kzp.checkpoints.clear();
            kzp.lj_tracker.invalidate();
            if let Some(cp) = start_pose {
                teleport_to_pose(g, eng, slot, cp);
            }
            send_kz_line(eng, slot, "[KZ] Returned to start.\n");
        }
        "reset" => {
            let kzp = &mut g.kz.players[idx];
            kzp.timer_started_at = None;
            kzp.cp_count = 0;
            kzp.tp_count = 0;
            kzp.checkpoints.clear();
            kzp.lj_tracker.invalidate();
            send_kz_line(eng, slot, "[KZ] Timer and checkpoints reset.\n");
        }
        _ => {}
    }
    true
}

fn is_kz_start_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "counter_start" | "clockstartbutton" | "firsttimerelay"
    )
}

fn is_kz_stop_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "counter_off" | "clockstopbutton" | "but_stop"
    )
}

/// Hooks `func_button` presses in `sv_kz_mode 1` for standard KZ map start/stop timers.
pub(crate) fn on_func_button_press(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    button_id: EdictId,
    activator: Option<EdictId>,
) {
    if eng.cvar_f32("sv_kz_mode") == 0.0 {
        return;
    }
    let Some(act) = activator else {
        return;
    };
    let Some(slot_idx) = player_slot(g, eng, act) else {
        return;
    };
    let slot = slot_idx as u8;
    let Some((targetname, target)) = eng
        .ent(button_id)
        .map(|v| (v.targetname.clone(), v.target.clone()))
    else {
        return;
    };

    if is_kz_start_name(&targetname) || is_kz_start_name(&target) {
        let now = eng.time();
        let pose = capture_pose(eng, act);
        let kzp = &mut g.kz.players[slot_idx];
        kzp.timer_started_at = Some(now);
        kzp.cp_count = 0;
        kzp.tp_count = 0;
        if pose.is_some() {
            kzp.start_pos = pose;
        }
        let msg = "[KZ] Timer started!\n";
        send_kz_line(eng, slot, msg);
        return;
    }

    if is_kz_stop_name(&targetname) || is_kz_stop_name(&target) {
        let Some(started) = g.kz.players[slot_idx].timer_started_at.take() else {
            return;
        };
        let elapsed = (eng.time() - started).max(0.0);
        let cp = g.kz.players[slot_idx].cp_count;
        let tp = g.kz.players[slot_idx].tp_count;
        let player_name = eng
            .client_key(slot, "name")
            .unwrap_or("Player")
            .to_string();
        let record = KzRunRecord {
            player_name: player_name.clone(),
            time: elapsed,
            checkpoints: cp,
            teleports: tp,
        };
        g.kz.top_runs.push(record.clone());
        g.kz.top_runs
            .sort_by(|a, b| a.time.total_cmp(&b.time));
        g.kz.top_runs.truncate(MAX_KZ_TOP_RUNS);
        let rank = g
            .kz
            .top_runs
            .iter()
            .position(|r| r.player_name == record.player_name && (r.time - record.time).abs() < 1e-9)
            .map_or(MAX_KZ_TOP_RUNS + 1, |i| i + 1);
        let formatted = format_kz_time(elapsed);
        let rank_tag = if rank <= MAX_KZ_TOP_RUNS {
            format!(" [#{rank}]")
        } else {
            String::new()
        };
        let msg = format!(
            "[KZ] {player_name} finished in {formatted} (CP: {cp}, TP: {tp}){rank_tag}\n"
        );
        menus::print_all(eng, HUD_PRINTTALK, &msg, &[]);
        eng.client_print(slot, &msg);
    }
}

/// Invalidates JumpStats when a player touches `trigger_teleport`.
pub(crate) fn on_player_teleport(g: &mut GameState, eng: &EngineApi<'_>, other: EdictId) {
    if let Some(idx) = player_slot(g, eng, other) {
        g.kz.players[idx].lj_tracker.invalidate();
    }
}

/// Invalidates JumpStats when a player is pushed by `trigger_push`.
pub(crate) fn on_player_push(g: &mut GameState, eng: &EngineApi<'_>, other: EdictId) {
    if let Some(idx) = player_slot(g, eng, other) {
        g.kz.players[idx].lj_tracker.mark_pushed();
    }
}

/// Observes a player movement step for server-side `sv_ljstats`.
pub(crate) fn on_pmove_step(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    cmd: &GoldSrcUserCmd,
    pm: &PlayerMove,
    is_highjump_edge: bool,
) {
    let idx = usize::from(slot);
    if idx >= MAX_PLAYERS {
        return;
    }
    if eng.cvar_f32("sv_ljstats") == 0.0 {
        g.kz.players[idx].lj_tracker.invalidate();
        return;
    }
    let min_cvar = eng.cvar_f32("cl_ljstats_min");
    let min_distance = if min_cvar > 0.0 { min_cvar } else { 215.0 };
        let input = JumpFrameInput {
        dt: pm.frametime,
        origin: pm.origin,
        velocity: pm.velocity,
        on_ground: pm.onground != -1,
        ducking: pm.flags & (FL_DUCKING as i32) != 0,
        waterlevel: pm.waterlevel,
        on_ladder: pm.movetype == i32::from(cs_engine::consts::MOVETYPE_FLY),
        noclip: pm.movetype == i32::from(MOVETYPE_NOCLIP),
        jump_pressed: i32::from(cmd.buttons) & IN_JUMP != 0,
        sidemove: cmd.sidemove,
        forwardmove: cmd.forwardmove,
        gravity: pm.movevars.gravity,
        airaccelerate: pm.movevars.airaccelerate,
        maxspeed: pm.maxspeed,
        is_highjump_edge,
        min_distance,
    };
    if let Some(stats) = g.kz.players[idx].lj_tracker.observe(input) {
        let line = format!("{}\n", stats.format_summary());
        g.kz.players[idx].last_ljstats = Some(stats);
        send_kz_line(eng, slot, &line);
    }
}
