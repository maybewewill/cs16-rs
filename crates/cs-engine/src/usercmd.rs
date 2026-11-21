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
use crate::lifecycle::NAME_CHANGE_MIN_INTERVAL_SECS;
use crate::oob::{MAX_INFO_STRING, sanitize_name};
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
        let run = select_cmds_to_run(
            cmds_newest_first,
            num_backup,
            dropped,
            &self.clients[index].last_cmd,
        );
        let count = run.len();
        for (i, cmd) in run.iter().enumerate() {
            // Dropped before it costs any time; it is not a bad move.
            if !usercmd_is_finite(cmd) {
                continue;
            }
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
            for step in split_usercmd(&granted) {
                self.call_game(|game, eng| game.run_usercmd(eng, slot, &step, random_seed));
            }
        }
        let client = &mut self.clients[index];
        if let Some(last) = run.into_iter().rfind(usercmd_is_finite) {
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
            let too_soon = last.is_some_and(|t| self.real_time - t < NAME_CHANGE_MIN_INTERVAL_SECS);
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
