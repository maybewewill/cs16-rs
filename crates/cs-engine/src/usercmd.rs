//! Running client user commands: the real-time budget that bounds how much movement time a
//! client may spend, the choice of commands to run after packet loss, and the token bucket
//! that limits string commands. The server glue for `clc_move`, `setinfo` and bad moves is the
//! `impl Server` block below.
//!
//! The loss replay rule is a working assumption (recalled HLDS behaviour), not verified against a
//! real server. The `random_seed` handed to the game is ReHLDS's (`SV_ParseMove`,
//! `SV_RunDroppedCmds`, `SV_RunCmd`, `rehlds/engine/sv_user.cpp`, read 2026-11-22), see
//! [`select_cmds_with_seeds`].

use cs_proto48::delta::GoldSrcUserCmd;

use crate::client::ClientState;
use crate::game::{FakeMove, GameDll};
use crate::lifecycle::NAME_CHANGE_MIN_INTERVAL_SECS;
use crate::oob::{MAX_INFO_STRING, sanitize_name};
use crate::server::Server;
use crate::world::consts::{FL_FROZEN, SOLID_NOT};

/// Fake-client moves `Server::run_fake_moves` runs in one drain; what a game queues beyond that
/// while they run (a think that moves a bot again, and again) is dropped.
pub(crate) const MAX_FAKE_MOVES_PER_DRAIN: usize = 4096;
/// Movement time, in milliseconds, a client may bank ahead of the commands it sends.
pub const MAX_BANKED_MS: f64 = 500.0;
/// From this many dropped packets on, lost commands are no longer replayed.
pub const MAX_DROPPED_REPLAY: u32 = 20;
/// Consecutive unparsable payloads from an active client after which it is dropped.
pub const MAX_CONSECUTIVE_BAD_MOVES: u32 = 32;
/// String commands (other than the sign-on ones) a client may send per second, sustained.
pub const STRINGCMD_RATE_PER_SEC: f64 = 80.0;
/// String commands a client may send in one burst.
pub const STRINGCMD_BURST: f64 = 400.0;

/// Real time a client has earned and not yet spent on user commands.
#[derive(Debug, Clone, Default)]
pub struct MsecBudget {
    banked_ms: f64,
}

/// Slack for floating-point drift when whole milliseconds are taken from the bank, so that
/// ten accruals of 0.001 s grant 10 ms.
const BANK_EPSILON_MS: f64 = 1e-6;

impl MsecBudget {
    /// An empty budget.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `real_dt_secs` of real time, keeping at most [`MAX_BANKED_MS`]. Negative or
    /// non-finite time adds nothing.
    pub fn accrue(&mut self, real_dt_secs: f64) {
        if real_dt_secs > 0.0 && real_dt_secs.is_finite() {
            self.banked_ms = (self.banked_ms + real_dt_secs * 1000.0).min(MAX_BANKED_MS);
        }
    }

    /// Grants `min(msec, whole banked milliseconds)` and spends it.
    pub fn take(&mut self, msec: u8) -> u8 {
        let whole = (self.banked_ms + BANK_EPSILON_MS).floor();
        let granted = f64::from(msec).min(whole).max(0.0);
        self.banked_ms = (self.banked_ms - granted).max(0.0);
        granted as u8
    }
}

/// The commands to run for one `clc_move`, oldest first. `cmds_newest_first` holds the new
/// commands followed by `num_backup` backups; `dropped` is the number of packets lost before
/// this one and `last` the last command run for this client.
///
/// Without loss only the new commands run. After `0 < dropped < MAX_DROPPED_REPLAY` lost
/// packets, `last` is repeated for the losses the backups do not cover, then the freshest
/// `min(dropped, num_backup)` backups run, then the new commands. After heavier loss only the
/// new commands run.
pub fn select_cmds_to_run(
    cmds_newest_first: &[GoldSrcUserCmd],
    num_backup: u8,
    dropped: u32,
    last: &GoldSrcUserCmd,
) -> Vec<GoldSrcUserCmd> {
    select_cmds_with_seeds(cmds_newest_first, num_backup, dropped, last, 0)
        .into_iter()
        .map(|(cmd, _)| cmd)
        .collect()
}

/// [`select_cmds_to_run`] with the `random_seed` `pfnCmdStart` is given for each command, as
/// `SV_ParseMove` and `SV_RunDroppedCmds` (ReHLDS `rehlds/engine/sv_user.cpp`, read 2026-11-22)
/// pass it to `SV_RunCmd`, where `sequence` is the packet's `netchan.incoming_sequence`:
/// - a new command is run with `sequence - i`, `i` being its index in `cmds_newest_first` (the
///   newest is 0, so the seeds of successive packets' newest commands are successive);
/// - a backup command replayed for a lost packet with `sequence - cmdnum`, `cmdnum` being its
///   index in `cmds_newest_first` as well (it follows the new commands);
/// - the last command repeated for a loss the backups do not cover with 0 (`SV_RunCmd(&lastcmd,
///   0, FALSE)`).
///
/// The seed is an `int` in C; here its wrapping `u32` (what `m_random_seed` is). Both halves of a
/// command that `SV_RunCmd` splits (`msec > 50`) get the seed of the command, see
/// [`split_usercmd`].
pub fn select_cmds_with_seeds(
    cmds_newest_first: &[GoldSrcUserCmd],
    num_backup: u8,
    dropped: u32,
    last: &GoldSrcUserCmd,
    sequence: u32,
) -> Vec<(GoldSrcUserCmd, u32)> {
    let backups = usize::from(num_backup).min(cmds_newest_first.len());
    let (new, backup) = cmds_newest_first.split_at(cmds_newest_first.len() - backups);
    let seed_of = |index: usize| sequence.wrapping_sub(index as u32);
    let mut run =
        Vec::with_capacity(cmds_newest_first.len() + dropped.min(MAX_DROPPED_REPLAY) as usize);
    if dropped > 0 && dropped < MAX_DROPPED_REPLAY {
        let replayed = (dropped as usize).min(backups);
        run.extend(std::iter::repeat_n(
            (last.clone(), 0),
            dropped as usize - replayed,
        ));
        run.extend(
            backup[..replayed]
                .iter()
                .enumerate()
                .rev()
                .map(|(j, cmd)| (cmd.clone(), seed_of(new.len() + j))),
        );
    }
    run.extend(
        new.iter()
            .enumerate()
            .rev()
            .map(|(i, cmd)| (cmd.clone(), seed_of(i))),
    );
    run
}

/// `SV_ClearMoveCmds` (ReHLDS `sv_user.cpp`, read 2026-11-22), for a frozen player: the first
/// `numcmds` commands lose their `msec`, moves and buttons and the `impulse` (the player is
/// frozen), and look where `v_angle` looks.
fn clear_move_cmds(cmds: &[GoldSrcUserCmd], numcmds: usize, v_angle: glam::Vec3) -> Vec<GoldSrcUserCmd> {
    let mut out = cmds.to_vec();
    for cmd in out.iter_mut().take(numcmds) {
        cmd.msec = 0;
        cmd.forwardmove = 0.0;
        cmd.sidemove = 0.0;
        cmd.upmove = 0.0;
        cmd.buttons = 0;
        cmd.impulse = 0;
        cmd.viewangles = v_angle;
    }
    out
}

/// Longest step, in milliseconds, one user command may give the game; longer ones are split.
pub const MAX_USERCMD_STEP_MS: u8 = 50;

/// `cmd` as the steps the game runs: unchanged when its `msec` is at most
/// [`MAX_USERCMD_STEP_MS`], otherwise run as two commands of `msec / 2` each (integer
/// division, so an odd millisecond is not simulated), each split the same way. Buttons, angles
/// and moves are the same in every step; the `impulse` is cleared in the second half, so only
/// the first step keeps it. This is what `SV_RunCmd` in ReHLDS (`rehlds/engine/sv_user.cpp`,
/// `master`) does, read on 2026-11-21. The time budget is charged the full `msec` before the
/// split, so the simulated time can be a few milliseconds less than what was charged.
pub fn split_usercmd(cmd: &GoldSrcUserCmd) -> Vec<GoldSrcUserCmd> {
    fn push_steps(cmd: GoldSrcUserCmd, steps: &mut Vec<GoldSrcUserCmd>) {
        if cmd.msec <= MAX_USERCMD_STEP_MS {
            steps.push(cmd);
            return;
        }
        let half = cmd.msec / 2;
        let second = GoldSrcUserCmd {
            msec: half,
            impulse: 0,
            ..cmd.clone()
        };
        let first = GoldSrcUserCmd { msec: half, ..cmd };
        push_steps(first, steps);
        push_steps(second, steps);
    }
    let mut steps = Vec::new();
    push_steps(cmd.clone(), &mut steps);
    steps
}

/// Whether the view angles and the forward, side and up moves of `cmd` are all finite. A
/// command failing this never reaches the game.
pub fn usercmd_is_finite(cmd: &GoldSrcUserCmd) -> bool {
    cmd.viewangles.is_finite()
        && cmd.forwardmove.is_finite()
        && cmd.sidemove.is_finite()
        && cmd.upmove.is_finite()
}

/// A token bucket: `burst` tokens, refilled at `rate_per_sec`. It starts full.
#[derive(Debug, Clone)]
pub(crate) struct TokenBucket {
    tokens: f64,
    rate_per_sec: f64,
    burst: f64,
}

impl TokenBucket {
    pub(crate) fn new(rate_per_sec: f64, burst: f64) -> Self {
        Self {
            tokens: burst,
            rate_per_sec,
            burst,
        }
    }

    /// The limit on string commands: [`STRINGCMD_BURST`] tokens, refilled at
    /// [`STRINGCMD_RATE_PER_SEC`].
    pub(crate) fn stringcmds() -> Self {
        Self::new(STRINGCMD_RATE_PER_SEC, STRINGCMD_BURST)
    }

    pub(crate) fn refill(&mut self, real_dt_secs: f64) {
        if real_dt_secs > 0.0 && real_dt_secs.is_finite() {
            self.tokens = (self.tokens + real_dt_secs * self.rate_per_sec).min(self.burst);
        }
    }

    /// Spends one token; `false` when none is left.
    pub(crate) fn try_take(&mut self) -> bool {
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl<G: GameDll> Server<G> {
    /// Credits `real_dt_secs` of frame time: to the movement budget of every `Active` client
    /// and to the string command and `setinfo` buckets of every connected one.
    pub(crate) fn accrue_client_time(&mut self, real_dt_secs: f64) {
        for client in self.clients.iter_mut().filter(|c| !c.is_free()) {
            client.stringcmds.refill(real_dt_secs);
            client.setinfos.refill(real_dt_secs);
            if client.state == ClientState::Active {
                client.budget.accrue(real_dt_secs);
            }
        }
    }

    /// Runs the commands of one `clc_move` from `slot`, which arrived in netchan packet
    /// `sequence` after `dropped` lost packets. Ignored unless the client is `Active`. A command
    /// failing [`usercmd_is_finite`] is skipped; the others get the time the budget grants and
    /// reach the game as the steps of [`split_usercmd`].
    pub(crate) fn handle_move(
        &mut self,
        slot: u8,
        cmds_newest_first: &[GoldSrcUserCmd],
        num_backup: u8,
        dropped: u32,
        sequence: u32,
    ) {
        let index = usize::from(slot);
        if self.clients[index].state != ClientState::Active {
            return;
        }
        // `SV_ParseMove` (ReHLDS `sv_user.cpp`, read 2026-11-22): a frozen player gets no
        // movement. `SV_ClearMoveCmds` strips the new commands (not the backups) and keeps the
        // view where it is, and `net_drop = 0` replays nothing. (The server pause and the
        // singleplayer console of the same condition do not exist here.)
        let frozen = self
            .edicts
            .get(self.edicts.client(slot))
            .filter(|e| e.vars.flags & FL_FROZEN != 0)
            .map(|e| e.vars.v_angle);
        let cleared;
        let (cmds_newest_first, dropped) = if let Some(v_angle) = frozen {
            let new = cmds_newest_first.len() - usize::from(num_backup).min(cmds_newest_first.len());
            cleared = clear_move_cmds(cmds_newest_first, new, v_angle);
            (&cleared[..], 0)
        } else {
            (cmds_newest_first, dropped)
        };
        let seeded = select_cmds_with_seeds(
            cmds_newest_first,
            num_backup,
            dropped,
            &self.clients[index].last_cmd,
            sequence,
        );
        let (run, seeds): (Vec<GoldSrcUserCmd>, Vec<u32>) = seeded.into_iter().unzip();
        let mut svtimebase = self.establish_time_base(&run);
        for (cmd, &random_seed) in run.iter().zip(&seeds) {
            // Dropped before it costs any time; it is not a bad move.
            if !usercmd_is_finite(cmd) {
                continue;
            }
            let msec = self.clients[index].budget.take(cmd.msec);
            // A command that asked for time and got none is the budget's refusal (the speedhack
            // bound) and does not run. A command of 0 ms is not special to `SV_RunCmd` (ReHLDS
            // `rehlds/engine/sv_user.cpp`, read 2026-11-22): it reaches the game with a frame
            // time of 0 (the client sends one every frame it has no movement to send), and the
            // budget gives it nothing and takes nothing.
            if msec == 0 && cmd.msec != 0 {
                continue;
            }
            let granted = GoldSrcUserCmd {
                msec,
                ..cmd.clone()
            };
            // The seed of the command (`select_cmds_with_seeds`): both halves of a split
            // command share it.
            for step in split_usercmd(&granted) {
                svtimebase += f64::from(step.msec) * 0.001;
                self.run_player_cmd(slot, &step, random_seed, svtimebase);
                let kicked = self.outbox.kicks.iter().any(|(kicked, _)| *kicked == slot);
                if kicked || self.clients[index].state != ClientState::Active {
                    // Kicked by the game (a kick queued during the step is not yet carried out:
                    // HLDS stops the move at once), or freed by what a touch did.
                    self.drain_outbox();
                    return;
                }
            }
        }
        let client = &mut self.clients[index];
        if let Some(last) = run.into_iter().rfind(usercmd_is_finite) {
            client.last_cmd = last;
        }
        // The move left the client in the middle of its last command: half of it is not part
        // of the ping of the frame it acknowledged (`SV_ParseMove`, ReHLDS
        // `rehlds/engine/sv_user.cpp`, read 2026-11-21).
        let half_last = f64::from(client.last_cmd.msec) * 0.5 / 1000.0;
        client
            .frames
            .trim_ping(client.chan.incoming_acknowledged, half_last);
        client.bad_moves = 0;
        self.drain_outbox();
    }

    /// `SV_EstablishTimeBase` (ReHLDS `rehlds/engine/sv_user.cpp`, read 2026-11-21): the server
    /// time the first command of a move starts at, the end of the frame minus the duration of
    /// every command that will run (the new ones and the replayed ones; `run` holds both, by the
    /// `msec` they arrived with, before the time budget and the split).
    fn establish_time_base(&self, run: &[GoldSrcUserCmd]) -> f64 {
        let total: f64 = run.iter().map(|c| f64::from(c.msec) / 1000.0).sum();
        self.frametime + self.sv_time - total
    }

    /// One step of `SV_RunCmd`: the game's `run_usercmd` at the client's `svtimebase` (the time
    /// at the end of this step) with the step's own frame time, `EngineApi::time` and
    /// `EngineApi::frametime` of the call; then `SV_ProcessImpacts`, see
    /// [`finish_player_cmd`](Self::finish_player_cmd), then the game's `post_usercmd`.
    fn run_player_cmd(&mut self, slot: u8, step: &GoldSrcUserCmd, random_seed: u32, time: f64) {
        let host_frametime = std::mem::replace(&mut self.frametime, f64::from(step.msec) * 0.001);
        // Touches reported outside a command are not this command's.
        self.world.player_impacts.clear();
        self.sync_views();
        // `SV_RunCmd` (ReHLDS `rehlds/engine/sv_user.cpp`, read 2026-11-22), before the game
        // runs: `v.button` is the command's buttons, then `SV_CheckMovingGround` with the
        // command's frame time, `(float)(ucmd->msec * 0.001)`: a conveyor under the player's
        // feet sets its `basevelocity`, and a base velocity nothing renewed is folded into the
        // `velocity`. Not ported here: `clbasevelocity` (no `EntVars` field for it), the
        // `impulse 204` full update of the original, and the `v_angle` / `pmove->oldangles`
        // update, which the game does itself (the engine has no `pmove` to hand the old angles
        // to: cs-game owns `SV_SetupPMove`, and Task 8 of the pm_shared plan takes the angles).
        // The `impulse` is stored before the game's `CmdStart` here, after it in the original.
        let player = self.edicts.client(slot);
        if let Some(v) = self.edicts.get_mut(player).map(|e| &mut e.vars) {
            v.button = i32::from(step.buttons);
            // `if (ucmd->impulse) sv_player->v.impulse = ucmd->impulse;` (`SV_RunCmd`, read
            // 2026-11-22): a zero impulse leaves the last one for the game to clear.
            if step.impulse != 0 {
                v.impulse = i32::from(step.impulse);
            }
        }
        self.check_moving_ground(player, (f64::from(step.msec) * 0.001) as f32);
        // `SV_RunCmd`: the other players are where this client saw them for the command, and
        // are put back after `SV_ProcessImpacts`. Not for a fake client (`if
        // (!host_client->fakeclient) SV_SetupMove(host_client)`, `sv_user.cpp`, read 2026-11-23).
        let fake = self.clients[usize::from(slot)].fake;
        if !fake {
            self.setup_move(slot);
        }
        self.call_game_synced_at(time, |game, eng| {
            game.run_usercmd(eng, slot, step, random_seed)
        });
        // After `pfnPM_Move`, before `SV_FinishPMove` (`SV_RunCmd`): the movevars the client
        // holds against the server's; a fake client has none (`!host_client->fakeclient`).
        if !fake {
            self.sync_client_movevars(slot);
        }
        self.finish_player_cmd(slot, time);
        // `PlayerPostThink` and `CmdEnd` come after `SV_ProcessImpacts` (the relink and the
        // touches), still inside the lag-compensation window, with the command's time and frame
        // time (`gGlobalVariables.frametime = frametime`).
        self.call_game_synced_at(time, |game, eng| game.post_usercmd(eng, slot, step));
        self.frametime = host_frametime;
        if !fake {
            self.restore_move();
        }
    }

    /// Runs the moves `EngineApi::run_player_move` queued, after the game call that queued them
    /// returned (`PF_RunPlayerMove_I`, ReHLDS `pr_cmds.cpp` 2035-2066, read 2026-11-23), each
    /// followed by what a client's move is followed by: the game's queued messages and kicks, and
    /// the touches. A move made while these run (by the game's thinks inside them) joins the
    /// queue and is run by the same loop, at most [`MAX_FAKE_MOVES_PER_DRAIN`] in all; the rest
    /// is dropped.
    pub(crate) fn run_fake_moves(&mut self) {
        if self.in_fake_moves || self.outbox.fake_moves.is_empty() {
            return;
        }
        self.in_fake_moves = true;
        let mut budget = MAX_FAKE_MOVES_PER_DRAIN;
        while budget > 0 && !self.outbox.fake_moves.is_empty() {
            let batch = std::mem::take(&mut self.outbox.fake_moves);
            let kept = batch.len().min(budget);
            self.net_stats.fake_moves_dropped += (batch.len() - kept) as u64;
            for FakeMove { slot, cmd } in batch.into_iter().take(kept) {
                budget -= 1;
                self.run_fake_move(slot, &cmd);
                self.drain_messages();
                self.dispatch_touches();
            }
        }
        // Over the budget: the rest is dropped and counted.
        self.net_stats.fake_moves_dropped += self.outbox.fake_moves.len() as u64;
        self.outbox.fake_moves.clear();
        self.in_fake_moves = false;
    }

    /// The body of `PF_RunPlayerMove_I` after the command is built: `svtimebase` is
    /// `host_frametime + sv.time - msec / 1000`, `SV_PreRunCmd` is empty, `SV_RunCmd(&cmd, 0,
    /// FALSE)` (the random seed 0) runs the command in halves while it is longer than 50 ms
    /// (see [`split_usercmd`]), and `lastcmd` becomes the command. A bot that was kicked or
    /// freed by what a step did gets no further step.
    fn run_fake_move(&mut self, slot: u8, cmd: &GoldSrcUserCmd) {
        let index = usize::from(slot);
        if !self.clients[index].fake || self.clients[index].state != ClientState::Active {
            return;
        }
        let mut svtimebase = self.frametime + self.sv_time - f64::from(cmd.msec) / 1000.0;
        for step in split_usercmd(cmd) {
            svtimebase += f64::from(step.msec) * 0.001;
            self.run_player_cmd(slot, &step, 0, svtimebase);
            let kicked = self.outbox.kicks.iter().any(|(kicked, _)| *kicked == slot);
            if kicked || !self.clients[index].fake {
                return;
            }
        }
        self.clients[index].last_cmd = cmd.clone();
    }

    /// `SV_ProcessImpacts` (`sv_user.cpp`): unless the player's edict is `SOLID_NOT`, links it
    /// again, which queues the triggers its box now meets, then queues the impacts the movement
    /// reported (`EngineApi::player_touched`) as `SV_Impact(other, player)`; the touches are
    /// delivered at once, after the call that found them. The touches of a non-solid player are
    /// dropped, and so are those reported for another player than `slot`'s.
    /// The touches are delivered at the command's `time` (its `svtimebase`, which ReHLDS leaves in
    /// `gpGlobals->time` through `SV_ProcessImpacts`).
    fn finish_player_cmd(&mut self, slot: u8, time: f64) {
        let player = self.edicts.client(slot);
        let touched = std::mem::take(&mut self.world.player_impacts);
        let solid = self.edicts.get(player).is_some_and(|e| e.vars.solid != SOLID_NOT);
        if solid {
            self.with_engine(|eng| eng.link_entity(player, true));
            for (_, other) in touched.into_iter().filter(|&(p, _)| p == player) {
                self.impact(other, player);
            }
        }
        self.world.touch_time = Some(time);
        self.dispatch_touches();
        self.world.touch_time = None;
    }

    /// Counts an unparsable payload from `slot` as a bad move; an `Active` client is dropped
    /// after [`MAX_CONSECUTIVE_BAD_MOVES`] of them in a row.
    pub(crate) fn count_bad_move(&mut self, slot: u8) {
        let client = &mut self.clients[usize::from(slot)];
        if client.state != ClientState::Active {
            return;
        }
        client.bad_moves += 1;
        if client.bad_moves >= MAX_CONSECUTIVE_BAD_MOVES {
            self.free_slot(slot);
        }
    }

    /// `setinfo <key> <value>`: stores the pair in the client's userinfo and tells the game.
    /// Keys starting with `*`, a missing key, a key or value containing `\` or `"`, and a
    /// change that would make the userinfo longer than [`MAX_INFO_STRING`] are ignored. A
    /// `name` is sanitised and made unique as at connect; an empty one, and one arriving less
    /// than [`NAME_CHANGE_MIN_INTERVAL_SECS`] after the last accepted name change, are
    /// ignored. The caller drains the outbox.
    pub(crate) fn client_setinfo(&mut self, slot: u8, argv: &[String]) {
        let Some(key) = argv.get(1) else {
            return;
        };
        let value = argv.get(2).map_or("", String::as_str);
        let forbidden = |s: &str| s.contains(['\\', '"']);
        if key.is_empty() || key.starts_with('*') || forbidden(key) || forbidden(value) {
            return;
        }
        let is_name = key.eq_ignore_ascii_case("name");
        let value = if is_name {
            let last = self.clients[usize::from(slot)].last_name_change;
            let too_soon =
                last.is_some_and(|t| self.real_time - t < NAME_CHANGE_MIN_INTERVAL_SECS);
            if value.is_empty() || too_soon {
                return;
            }
            self.unique_name(&sanitize_name(value), Some(slot))
        } else {
            value.to_string()
        };
        let view = &mut self.views[usize::from(slot)];
        let mut info = view.userinfo.clone();
        info.set(key.as_str(), value);
        if info.to_wire_string().len() > MAX_INFO_STRING {
            return;
        }
        if is_name {
            self.clients[usize::from(slot)].last_name_change = Some(self.real_time);
        }
        view.userinfo = info;
        view.userinfo_dirty = true;
        self.call_game(|game, eng| game.client_userinfo_changed(eng, slot));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(forwardmove: f32) -> GoldSrcUserCmd {
        GoldSrcUserCmd {
            msec: 10,
            forwardmove,
            ..Default::default()
        }
    }

    #[test]
    fn budget_grants_only_accrued_time() {
        let mut b = MsecBudget::new();
        b.accrue(0.010);
        assert_eq!(b.take(10), 10);
        assert_eq!(b.take(10), 0);
    }

    #[test]
    fn budget_bank_is_capped() {
        let mut b = MsecBudget::new();
        b.accrue(5.0);
        let granted: u32 = std::iter::from_fn(|| Some(u32::from(b.take(200))))
            .take_while(|&g| g > 0)
            .sum();
        assert_eq!(granted, MAX_BANKED_MS as u32);
    }

    #[test]
    fn budget_grants_partial_time_and_keeps_fractions() {
        let mut b = MsecBudget::new();
        b.accrue(0.0045);
        assert_eq!(b.take(10), 4);
        b.accrue(0.0055);
        assert_eq!(b.take(10), 6);
        // Negative or non-finite time earns nothing.
        b.accrue(-1.0);
        b.accrue(f64::NAN);
        assert_eq!(b.take(10), 0);
    }

    #[test]
    fn token_bucket_starts_full_and_refills_at_its_rate() {
        use crate::lifecycle::{SETINFO_BURST, SETINFO_RATE_PER_SEC};
        let mut b = TokenBucket::new(SETINFO_RATE_PER_SEC, SETINFO_BURST);
        let taken = std::iter::from_fn(|| Some(b.try_take()))
            .take_while(|&t| t)
            .count();
        assert_eq!(taken as f64, SETINFO_BURST);
        b.refill(0.5);
        assert!(b.try_take());
        assert!(!b.try_take());
        // Refills stop at the burst; bad time adds nothing.
        b.refill(f64::NAN);
        b.refill(-3.0);
        assert!(!b.try_take());
        b.refill(1000.0);
        let taken = std::iter::from_fn(|| Some(b.try_take()))
            .take_while(|&t| t)
            .count();
        assert_eq!(taken as f64, SETINFO_BURST);
    }

    #[test]
    fn no_loss_runs_only_new_cmds() {
        // Newest first: two new commands, then two backups.
        let cmds = [cmd(4.0), cmd(3.0), cmd(2.0), cmd(1.0)];
        let run = select_cmds_to_run(&cmds, 2, 0, &cmd(0.0));
        assert_eq!(run, vec![cmd(3.0), cmd(4.0)]);
    }

    #[test]
    fn loss_within_backup_replays_backups() {
        let (new, b_new, b_old) = (cmd(3.0), cmd(2.0), cmd(1.0));
        let cmds = [new.clone(), b_new.clone(), b_old.clone()];
        let run = select_cmds_to_run(&cmds, 2, 2, &cmd(0.0));
        assert_eq!(run, vec![b_old, b_new, new]);

        // One packet lost: only the newest backup is replayed.
        let run = select_cmds_to_run(&cmds, 2, 1, &cmd(0.0));
        assert_eq!(run, vec![cmd(2.0), cmd(3.0)]);
    }

    #[test]
    fn loss_beyond_backup_repeats_last() {
        let last = cmd(9.0);
        let cmds = [cmd(3.0), cmd(2.0), cmd(1.0)];
        let run = select_cmds_to_run(&cmds, 2, 5, &last);
        assert_eq!(
            run,
            vec![
                last.clone(),
                last.clone(),
                last,
                cmd(1.0),
                cmd(2.0),
                cmd(3.0)
            ]
        );
    }

    #[test]
    fn huge_loss_runs_only_new() {
        let cmds = [cmd(3.0), cmd(2.0), cmd(1.0)];
        let run = select_cmds_to_run(&cmds, 2, MAX_DROPPED_REPLAY, &cmd(9.0));
        assert_eq!(run, vec![cmd(3.0)]);
    }

    fn msecs(steps: &[GoldSrcUserCmd]) -> Vec<u8> {
        steps.iter().map(|c| c.msec).collect()
    }

    #[test]
    fn short_cmd_is_not_split() {
        let c = GoldSrcUserCmd {
            msec: MAX_USERCMD_STEP_MS,
            impulse: 100,
            ..cmd(1.0)
        };
        assert_eq!(split_usercmd(&c), vec![c]);
    }

    #[test]
    fn long_cmd_is_split_in_halves_recursively() {
        let c = GoldSrcUserCmd {
            msec: 200,
            buttons: 6,
            impulse: 201,
            viewangles: glam::Vec3::new(10.0, 20.0, 0.0),
            ..cmd(250.0)
        };
        let steps = split_usercmd(&c);
        assert_eq!(msecs(&steps), vec![50, 50, 50, 50]);
        // Buttons, angles and moves stay; only the first step keeps the impulse.
        for (i, step) in steps.iter().enumerate() {
            let expected_impulse = if i == 0 { 201 } else { 0 };
            assert_eq!(
                *step,
                GoldSrcUserCmd {
                    msec: 50,
                    impulse: expected_impulse,
                    ..c.clone()
                }
            );
        }
    }

    /// Time simulated for a command of `msec`: halved, rounding down, until a half is at most
    /// [`MAX_USERCMD_STEP_MS`].
    fn halved_total(msec: u8) -> u32 {
        if msec <= MAX_USERCMD_STEP_MS {
            u32::from(msec)
        } else {
            2 * halved_total(msec / 2)
        }
    }

    #[test]
    fn odd_millisecond_is_not_simulated() {
        assert_eq!(msecs(&split_usercmd(&cmd_ms(51))), vec![25, 25]);
        // 101 halves to 50 and 50.
        assert_eq!(msecs(&split_usercmd(&cmd_ms(101))), vec![50, 50]);
        // 255 halves to 127, 63, then 31: eight steps, 248 ms.
        assert_eq!(msecs(&split_usercmd(&cmd_ms(255))), vec![31; 8]);
        for msec in 51..=255u8 {
            let steps = msecs(&split_usercmd(&cmd_ms(msec)));
            assert!(
                steps.iter().all(|&s| s <= MAX_USERCMD_STEP_MS),
                "{msec}: {steps:?}"
            );
            // Every level splits into equal halves, so all steps are equal and their count is
            // a power of two.
            assert!(steps.iter().all(|&s| s == steps[0]), "{msec}: {steps:?}");
            assert!(steps.len().is_power_of_two(), "{msec}: {steps:?}");
            let total: u32 = steps.iter().map(|&s| u32::from(s)).sum();
            assert_eq!(total, halved_total(msec), "{msec}: {steps:?}");
            assert!(total <= u32::from(msec));
        }
    }

    fn cmd_ms(msec: u8) -> GoldSrcUserCmd {
        GoldSrcUserCmd { msec, ..cmd(1.0) }
    }

    #[test]
    fn non_finite_moves_and_angles_are_detected() {
        assert!(usercmd_is_finite(&cmd(250.0)));
        let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        for v in bad {
            let cases = [
                GoldSrcUserCmd {
                    forwardmove: v,
                    ..cmd(0.0)
                },
                GoldSrcUserCmd {
                    sidemove: v,
                    ..cmd(0.0)
                },
                GoldSrcUserCmd {
                    upmove: v,
                    ..cmd(0.0)
                },
                GoldSrcUserCmd {
                    viewangles: glam::Vec3::new(0.0, v, 0.0),
                    ..cmd(0.0)
                },
                GoldSrcUserCmd {
                    viewangles: glam::Vec3::new(v, 0.0, 0.0),
                    ..cmd(0.0)
                },
                GoldSrcUserCmd {
                    viewangles: glam::Vec3::new(0.0, 0.0, v),
                    ..cmd(0.0)
                },
            ];
            for c in cases {
                assert!(!usercmd_is_finite(&c), "{c:?}");
            }
        }
    }

    #[test]
    fn backup_count_beyond_the_list_is_clamped() {
        // Both commands are backups; one lost packet replays the newer one.
        let cmds = [cmd(2.0), cmd(1.0)];
        let run = select_cmds_to_run(&cmds, 5, 1, &cmd(9.0));
        assert_eq!(run, vec![cmd(2.0)]);
        assert_eq!(select_cmds_to_run(&[], 2, 3, &cmd(9.0)), vec![cmd(9.0); 3]);
    }
}
