//! Money: the account of a player (`m_iAccount`) and what feeds it, from ReGameDLL_CS
//! (`player.cpp` `CBasePlayer::AddAccount`, `client.cpp` `CheckStartMoney`/`BlinkAccount`,
//! `multiplay_gamerules.cpp` `Round_Cts`/`Round_Ts`/`RestartRound`/`PlayerKilled`,
//! `gamerules.h` `RewardAccount`, read 2026-11-22), vanilla behaviour (not
//! `REGAMEDLL_FIXES`/`REGAMEDLL_ADD`: no `mp_maxmoney`, the cap is 16000).
//!
//! - [`add_account`] changes an account, clamped to `0..=16000`, and tells the player's HUD
//!   (`Money`, the flag is `bTrackChange`: the client flashes the change).
//! - [`check_start_money`] keeps `mp_startmoney` within 800..=16000, as the original, which writes
//!   the clamped value back to the cvar.
//! - [`Economy`] is what `CHalfLifeMultiplay` keeps for the rounds: the sums the teams are paid
//!   when the next round starts (`m_iAccountCT`, `m_iAccountTerrorist`), the losing streaks and
//!   the loser bonus that grows with them. A round won adds the win reward ([`Economy::round_won`]);
//!   `RestartRound` settles the streaks ([`Economy::settle`]) and pays every player of a team
//!   that is not marked as receiving nothing; a complete reset pays nothing and starts the
//!   streaks over.
//! - [`killed_by`] is the money of `PlayerKilled`: 300 for an enemy, -3300 for a teammate, which
//!   also counts the team kills (`mp_autokick` kicks at the third) and marks the player for
//!   `mp_tkpunish` ([`punish_team_kill`], from `RoundRespawn`).
//!
//! Deviations and what waits for later tasks:
//! - The rewards of the map objectives (bomb planted/defused/exploded, hostages, VIP, escape) and
//!   their loser-bonus branches are 2E; this build has maps with no objective, so the win reward
//!   is the plain `RR_CTS_WIN`/`RR_TERRORISTS_WIN` of 3000 (the original pays 3250 on a map with
//!   a bomb target). `MarkLivingPlayersOnTeamAsNotReceivingMoneyNextRound` is only called by
//!   those objective branches of the round timer, so it comes with them; the flag it sets
//!   ([`crate::player::Player::receives_no_money_next_round`]) is honoured here.
//! - `HintMessage` (`#Hint_careful_around_teammates`, `#Hint_cannot_play_because_tk`) has no
//!   hint queue behind it yet and is not sent.
//! - The blink of the freeze period (`MONEY_BLINK_AMOUNT`, 30 in `player.h`) belongs to the tutor
//!   (`TheTutor`), which does not exist, so the constant is not defined; [`blink_account`] is the
//!   buy menu's.

use cs_engine::{EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;

use crate::GameState;
use crate::menus::{HUD_PRINTCENTER, HUD_PRINTCONSOLE};
use crate::player::{Team, player_slot};
use crate::rules::WinStatus;

/// The most money a player can have (`AddAccount`).
pub const MAX_ACCOUNT: i32 = 16000;
/// The least `mp_startmoney` (`CheckStartMoney`).
pub const MIN_START_MONEY: i32 = 800;
/// `REWARD_CTS_WIN`: what the counter-terrorists are paid for a round won (no bomb target).
pub const REWARD_CTS_WIN: i32 = 3000;
/// `REWARD_TERRORISTS_WIN`: what the terrorists are paid for a round won (no bomb target).
pub const REWARD_TERRORISTS_WIN: i32 = 3000;
/// `REWARD_LOSER_BONUS_DEFAULT`: the loser bonus of a new match.
pub const REWARD_LOSER_BONUS_DEFAULT: i32 = 1400;
/// `REWARD_LOSER_BONUS_MIN`: the loser bonus after a losing streak was broken.
pub const REWARD_LOSER_BONUS_MIN: i32 = 1500;
/// `REWARD_LOSER_BONUS_MAX`: the loser bonus stops growing once it has reached this.
pub const REWARD_LOSER_BONUS_MAX: i32 = 3000;
/// `REWARD_LOSER_BONUS_ADD`: what the loser bonus grows by for each round lost in a streak.
pub const REWARD_LOSER_BONUS_ADD: i32 = 500;
/// `REWARD_KILLED_ENEMY`.
pub const REWARD_KILLED_ENEMY: i32 = 300;
/// `PAYBACK_FOR_KILLED_TEAMMATES`.
pub const PAYBACK_FOR_KILLED_TEAMMATES: i32 = -3300;
/// The team kill that `mp_autokick` kicks for (`killer->m_iTeamKills == 3`).
const AUTOKICK_TEAM_KILLS: i32 = 3;

/// The round economy of `CHalfLifeMultiplay`.
#[derive(Debug, Clone)]
pub struct Economy {
    /// `m_iAccountCT`: what each counter-terrorist is paid when the next round starts.
    account_ct: i32,
    /// `m_iAccountTerrorist`.
    account_terrorist: i32,
    /// `m_iNumConsecutiveCTLoses`: rounds the counter-terrorists have lost in a row.
    consecutive_ct_loses: i32,
    /// `m_iNumConsecutiveTerroristLoses`.
    consecutive_terrorist_loses: i32,
    /// `m_iLoserBonus`: what the team that lost the round is paid.
    loser_bonus: i32,
}

impl Default for Economy {
    /// The constructor of `CHalfLifeMultiplay`: nothing owed, no streaks, the default bonus.
    fn default() -> Self {
        Self {
            account_ct: 0,
            account_terrorist: 0,
            consecutive_ct_loses: 0,
            consecutive_terrorist_loses: 0,
            loser_bonus: REWARD_LOSER_BONUS_DEFAULT,
        }
    }
}

impl Economy {
    /// `m_iAccountCT`.
    pub fn account_ct(&self) -> i32 {
        self.account_ct
    }

    /// `m_iAccountTerrorist`.
    pub fn account_terrorist(&self) -> i32 {
        self.account_terrorist
    }

    /// `m_iNumConsecutiveCTLoses`.
    pub fn consecutive_ct_loses(&self) -> i32 {
        self.consecutive_ct_loses
    }

    /// `m_iNumConsecutiveTerroristLoses`.
    pub fn consecutive_terrorist_loses(&self) -> i32 {
        self.consecutive_terrorist_loses
    }

    /// `m_iLoserBonus`.
    pub fn loser_bonus(&self) -> i32 {
        self.loser_bonus
    }

    /// `Round_Cts` and `Round_Ts`: the winners earn the win reward (the objective rewards of the
    /// bomb maps are 2E).
    pub(crate) fn round_won(&mut self, team: Team) {
        match team {
            Team::Ct => self.account_ct += REWARD_CTS_WIN,
            Team::Terrorist => self.account_terrorist += REWARD_TERRORISTS_WIN,
            Team::Unassigned | Team::Spectator => {}
        }
    }

    /// The part of `RestartRound` that scales the loser bonus and assigns it. The team that won
    /// the round ends its own losing streak (a bonus that a streak had grown falls back to the
    /// minimum) and adds a lost round to the other team's. While a team has lost twice in a row
    /// the bonus grows by 500 each time this runs, as long as it is below the maximum (a bonus of
    /// 2900 still grows to 3400, as in the original; a draw does not touch the streaks but still
    /// grows the bonus). The loser of the round is paid the bonus.
    pub(crate) fn settle(&mut self, status: WinStatus) {
        // Scale up the loser bonus when teams fall into losing streaks.
        match status {
            WinStatus::Terrorists => {
                // Check to see if they just broke a losing streak.
                if self.consecutive_terrorist_loses > 1 {
                    self.loser_bonus = REWARD_LOSER_BONUS_MIN;
                }
                self.consecutive_terrorist_loses = 0;
                self.consecutive_ct_loses += 1;
            }
            WinStatus::Cts => {
                if self.consecutive_ct_loses > 1 {
                    self.loser_bonus = REWARD_LOSER_BONUS_MIN;
                }
                self.consecutive_ct_loses = 0;
                self.consecutive_terrorist_loses += 1;
            }
            WinStatus::None | WinStatus::Draw => {}
        }
        // Check if the losing team is in a losing streak and the loser bonus has not maxed out.
        if (self.consecutive_terrorist_loses > 1 || self.consecutive_ct_loses > 1)
            && self.loser_bonus < REWARD_LOSER_BONUS_MAX
        {
            self.loser_bonus += REWARD_LOSER_BONUS_ADD;
        }
        // Assign the losing bonus.
        match status {
            WinStatus::Terrorists => self.account_ct += self.loser_bonus,
            WinStatus::Cts => self.account_terrorist += self.loser_bonus,
            WinStatus::None | WinStatus::Draw => {}
        }
    }

    /// `m_bCompleteReset` of `RestartRound`: a new match pays nothing extra and starts the
    /// streaks over.
    pub(crate) fn reset_match(&mut self) {
        *self = Self::default();
    }

    /// What each player of `team` is paid at the start of the round.
    pub(crate) fn payout(&self, team: Team) -> Option<i32> {
        match team {
            Team::Ct => Some(self.account_ct),
            Team::Terrorist => Some(self.account_terrorist),
            Team::Unassigned | Team::Spectator => None,
        }
    }

    /// The end of `RestartRound`: the round's sums were paid.
    pub(crate) fn clear_accounts(&mut self) {
        self.account_ct = 0;
        self.account_terrorist = 0;
    }
}

/// `CheckStartMoney` (`client.cpp`): `mp_startmoney` clamped to 800..=16000, the clamped value
/// written back to the cvar. Returns the starting money.
pub(crate) fn check_start_money(eng: &mut EngineApi<'_>) -> i32 {
    let money = eng.cvar_f32("mp_startmoney") as i32;
    if money > MAX_ACCOUNT {
        eng.set_cvar("mp_startmoney", &MAX_ACCOUNT.to_string());
        MAX_ACCOUNT
    } else if money < MIN_START_MONEY {
        eng.set_cvar("mp_startmoney", &MIN_START_MONEY.to_string());
        MIN_START_MONEY
    } else {
        money
    }
}

/// `gmsgMoney` to `slot`: the account, and whether the HUD flashes the change.
pub(crate) fn send_money(eng: &mut EngineApi<'_>, slot: u8, amount: i32, track_change: bool) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::Money {
            amount,
            flash: track_change,
        },
    );
}

/// `CBasePlayer::AddAccount`: `amount` (negative to take money) to the account of `slot`, kept
/// between 0 and 16000, and the new account to the player's HUD. `false` when nobody is in the
/// slot.
pub(crate) fn add_account(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    amount: i32,
    track_change: bool,
) -> bool {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return false;
    };
    p.money = p.money.saturating_add(amount).clamp(0, MAX_ACCOUNT);
    let money = p.money;
    send_money(eng, slot, money, track_change);
    true
}

/// `BlinkAccount`: the account of `slot` blinks `times` times on its HUD.
pub fn blink_account(eng: &mut EngineApi<'_>, slot: u8, times: u8) {
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::BlinkAcct(times));
}

/// `ClientPrint(pev, dest, text, param)` to one player.
fn print_to(eng: &mut EngineApi<'_>, slot: u8, dest: u8, text: &str, params: &[&str]) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::TextMsg {
            dest,
            message: text.to_string(),
            params: params.iter().map(|p| (*p).to_string()).collect(),
        },
    );
}

/// The money of `CHalfLifeMultiplay::PlayerKilled` for the player of `victim` killed by the
/// attacker `killer` (an edict; `None` for the world): nothing for a suicide or for the world;
/// -3300 for killing a teammate, with the team kill counted, the kill command of `mp_autokick`
/// at the third and the warning on the killer's screen; 300 for killing an enemy.
pub(crate) fn killed_by(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    victim: u8,
    killer: Option<cs_engine::EdictId>,
) {
    let Some(killer) = killer.and_then(|k| player_slot(g, eng, k)) else {
        return;
    };
    // The player killed itself.
    if killer == usize::from(victim) {
        return;
    }
    let victim_team = g
        .players
        .get(usize::from(victim))
        .and_then(Option::as_ref)
        .map(|p| p.team);
    let Some(kp) = g.players[killer].as_mut() else {
        return;
    };
    let killer_slot = killer as u8;
    if victim_team == Some(kp.team) {
        kp.team_kills += 1;
        kp.just_killed_teammate = true;
        let kills = kp.team_kills;
        add_account(g, eng, killer_slot, PAYBACK_FOR_KILLED_TEAMMATES, true);
        print_to(eng, killer_slot, HUD_PRINTCENTER, "#Killed_Teammate", &[]);
        print_to(
            eng,
            killer_slot,
            HUD_PRINTCONSOLE,
            "#Game_teammate_kills",
            &[&kills.to_string()],
        );
        if eng.cvar_f32("mp_autokick") != 0.0 && kills == AUTOKICK_TEAM_KILLS {
            print_to(
                eng,
                killer_slot,
                HUD_PRINTCONSOLE,
                "#Banned_For_Killing_Teamates",
                &[],
            );
            // The original runs `kick # <userid>` with no reason (non-FIXES), so the player sees
            // the console kick's default text.
            eng.kick(killer_slot, "Kicked by server");
        }
    } else {
        add_account(g, eng, killer_slot, REWARD_KILLED_ENEMY, true);
    }
}

/// The team-kill part of `CBasePlayer::RoundRespawn`: with `mp_tkpunish` on, a player that
/// killed a teammate in the last round is told to `kill` itself (the client sends the command
/// after it respawned) and is marked as punished until it dies.
pub(crate) fn punish_team_kill(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let punish = eng.cvar_f32("mp_tkpunish") != 0.0;
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if p.just_killed_teammate && punish {
        p.punished_for_tk = true;
        p.just_killed_teammate = false;
        eng.stufftext(slot, "kill\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draw_pays_nothing_and_keeps_the_streaks() {
        let mut e = Economy::default();
        e.settle(WinStatus::Draw);
        assert_eq!((e.account_ct(), e.account_terrorist()), (0, 0));
        assert_eq!(e.consecutive_ct_loses() + e.consecutive_terrorist_loses(), 0);
        assert_eq!(e.loser_bonus(), REWARD_LOSER_BONUS_DEFAULT);
        // A streak in progress keeps growing the bonus through a draw, as the original's check
        // does not look at the result of the round.
        e.settle(WinStatus::Terrorists);
        e.settle(WinStatus::Terrorists);
        assert_eq!(e.loser_bonus(), 1900);
        e.settle(WinStatus::Draw);
        assert_eq!(e.loser_bonus(), 2400);
    }

    #[test]
    fn the_win_reward_goes_to_the_winners() {
        let mut e = Economy::default();
        e.round_won(Team::Ct);
        assert_eq!((e.account_ct(), e.account_terrorist()), (REWARD_CTS_WIN, 0));
        e.round_won(Team::Terrorist);
        assert_eq!(e.account_terrorist(), REWARD_TERRORISTS_WIN);
        e.reset_match();
        assert_eq!((e.account_ct(), e.account_terrorist()), (0, 0));
        assert_eq!(e.payout(Team::Spectator), None);
    }
}
