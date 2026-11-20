//! Running client user commands: the real-time budget that bounds how much movement time a
//! client may spend, the choice of commands to run after packet loss, and the token bucket
//! that limits string commands. The server glue for `clc_move`, `setinfo` and bad moves is the
//! `impl Server` block below.
//!
//! The loss replay rule and the `random_seed` handed to the game are working assumptions
//! (recalled HLDS behaviour), not verified against a real server.

use cs_proto48::delta::GoldSrcUserCmd;

use crate::client::ClientState;
use crate::game::GameDll;
use crate::oob::MAX_INFO_STRING;
use crate::server::Server;

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
    let backups = usize::from(num_backup).min(cmds_newest_first.len());
    let (new, backup) = cmds_newest_first.split_at(cmds_newest_first.len() - backups);
    let mut run =
        Vec::with_capacity(cmds_newest_first.len() + dropped.min(MAX_DROPPED_REPLAY) as usize);
    if dropped > 0 && dropped < MAX_DROPPED_REPLAY {
        let replayed = (dropped as usize).min(backups);
        run.extend(std::iter::repeat_n(
            last.clone(),
            dropped as usize - replayed,
        ));
        run.extend(backup[..replayed].iter().rev().cloned());
    }
    run.extend(new.iter().rev().cloned());
    run
}

/// Rate limit for a client's string commands: [`STRINGCMD_BURST`] tokens, refilled at
/// [`STRINGCMD_RATE_PER_SEC`]. It starts full.
#[derive(Debug, Clone)]
pub(crate) struct StringCmdBucket {
    tokens: f64,
}

impl Default for StringCmdBucket {
    fn default() -> Self {
        Self {
            tokens: STRINGCMD_BURST,
        }
    }
}

impl StringCmdBucket {
    pub(crate) fn refill(&mut self, real_dt_secs: f64) {
        if real_dt_secs > 0.0 && real_dt_secs.is_finite() {
            self.tokens =
                (self.tokens + real_dt_secs * STRINGCMD_RATE_PER_SEC).min(STRINGCMD_BURST);
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
    /// and to the string command bucket of every connected one.
    pub(crate) fn accrue_client_time(&mut self, real_dt_secs: f64) {
        for client in self.clients.iter_mut().filter(|c| !c.is_free()) {
            client.stringcmds.refill(real_dt_secs);
            if client.state == ClientState::Active {
                client.budget.accrue(real_dt_secs);
            }
        }
    }

    /// Runs the commands of one `clc_move` from `slot`, which arrived in netchan packet
    /// `sequence` after `dropped` lost packets. Ignored unless the client is `Active`.
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
        let run = select_cmds_to_run(
            cmds_newest_first,
            num_backup,
            dropped,
            &self.clients[index].last_cmd,
        );
        let count = run.len();
        for (i, cmd) in run.iter().enumerate() {
            let msec = self.clients[index].budget.take(cmd.msec);
            if msec == 0 {
                continue;
            }
            let granted = GoldSrcUserCmd {
                msec,
                ..cmd.clone()
            };
            // Working assumption: the packet's sequence minus the index from the newest.
            let random_seed = sequence.wrapping_sub((count - 1 - i) as u32);
            self.call_game(|game, eng| game.run_usercmd(eng, slot, &granted, random_seed));
        }
        let client = &mut self.clients[index];
        if let Some(last) = run.into_iter().last() {
            client.last_cmd = last;
        }
        client.bad_moves = 0;
        self.drain_outbox();
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
    /// change that would make the userinfo longer than [`MAX_INFO_STRING`] are ignored. The
    /// caller drains the outbox.
    pub(crate) fn client_setinfo(&mut self, slot: u8, argv: &[String]) {
        let Some(key) = argv.get(1) else {
            return;
        };
        let value = argv.get(2).map_or("", String::as_str);
        let forbidden = |s: &str| s.contains(['\\', '"']);
        if key.is_empty() || key.starts_with('*') || forbidden(key) || forbidden(value) {
            return;
        }
        let view = &mut self.views[usize::from(slot)];
        let mut info = view.userinfo.clone();
        info.set(key.as_str(), value);
        if info.to_wire_string().len() > MAX_INFO_STRING {
            return;
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

    #[test]
    fn backup_count_beyond_the_list_is_clamped() {
        // Both commands are backups; one lost packet replays the newer one.
        let cmds = [cmd(2.0), cmd(1.0)];
        let run = select_cmds_to_run(&cmds, 5, 1, &cmd(9.0));
        assert_eq!(run, vec![cmd(2.0)]);
        assert_eq!(select_cmds_to_run(&[], 2, 3, &cmd(9.0)), vec![cmd(9.0); 3]);
    }
}
