//! The round rules, `CHalfLifeMultiplay` of ReGameDLL_CS (`multiplay_gamerules.cpp`,
//! `gamerules.h`, `game.cpp`, `player.cpp`, read 2026-11-22), the vanilla `mp.dll` behaviour (not
//! the `REGAMEDLL_FIXES`/`REGAMEDLL_ADD` builds).
//!
//! The state of the round ([`Rules`]: the freeze period, the round clock, the win status, the
//! round counters, the intermission) lives in [`GameState::rules`]. [`think`] is the rules'
//! `Think`, run every server frame: it checks the end of the match (`mp_timelimit`,
//! `mp_maxrounds`, `mp_winlimit`), the end of the freeze period and of the round, a round
//! restart that is due (`sv_restart`, `sv_restartround`) and then restarts the round
//! ([`restart_round`]). [`check_win_conditions`] is called from where a player's state changes
//! (a player spawns, dies, leaves or picks a team): it starts the game when both teams have a
//! player ("Game Commencing"), and ends a round when a team has been exterminated. A match that
//! ends (`GoToIntermission`) shows the scoreboard for `mp_chattime` and then asks the engine for
//! the next map of `mapcycle.txt` ([`EngineApi::change_level`]).
//!
//! Deviations from vanilla:
//! - The match clock and the other cvar checks that `Think` runs once a second are read every
//!   second as in the source; the `sv_accelerate`, `sv_friction` and `sv_stopspeed` hard locks
//!   are not applied (the registry's defaults differ from the values the original forces and
//!   the movement variables are only sent to a client when it signs on).
//! - A map cycle that cannot be used (no `mapcycle.txt`, no valid map in it) changes to the
//!   current map again; vanilla asks for `hldm1`, which does not exist in Counter-Strike (the
//!   change fails and is retried every frame). A failed change is retried once a second here.
//! - `mapcyclefile` is not read: the file is always `mapcycle.txt` ([`MapCycle::load`]), loaded
//!   once when the server starts. The `minplayers`/`maxplayers` of a line are honoured; the
//!   other rules of a line (commands run after the change) are not executed.
//! - The players leaving the server are counted the way vanilla counts them for the check made
//!   when they leave: dead, on their team ([`on_disconnect`]).
//! - `m_iCareer*`, bots, tutor, `m_flForceCameraValue`/`AllowSpec` change messages, the idle
//!   kick and the voice manager are not ported.
//!
//! Left for later tasks: the economy of a round (`m_iAccountCT`, `m_iAccountTerrorist`, the
//! loser bonus and its streaks: Task 6; a complete reset only gives back the starting money),
//! the radio call of the round start (Task 11), the map objectives (2E): the bomb, the
//! hostages, the VIP and the escape zones. Their branches in [`check_win_conditions`] and
//! [`check_round_time_expired`] are stubs: no entity of those classes exists yet, so a map has
//! no objective, and a round that runs out of time on such a map simply goes on, as
//! `CheckRoundTimeExpired` does (it pushes the round start 60 seconds forward). `CleanUpMap`
//! (re-creating the map's breakables and doors) has nothing to clean yet.

use cs_engine::{EngineApi, MsgDest};
use cs_proto48::messages::svc;
use cs_proto48::usermsg::CsUserMsg;

use crate::GameState;
use crate::menus::{self, Menu};
use crate::messages;
use crate::player::{self, Team};
use crate::teams::{self, JoinState};

/// `ROUND_RESPAWN_TIME` (`gamerules.h`): seconds into a round after which a joining player waits
/// for the next round.
pub const ROUND_RESPAWN_TIME: f64 = 20.0;
/// `ROUND_BEGIN_DELAY`: seconds between the end of a round and the next one.
pub const ROUND_BEGIN_DELAY: f64 = 5.0;
/// Seconds between the first player of each team being in the game and the first round.
pub const GAME_COMMENCE_DELAY: f64 = 3.0;
/// `MAX_INTERMISSION_TIME`: the longest the scoreboard stays up, in seconds.
pub const MAX_INTERMISSION_TIME: i32 = 120;
/// Seconds between two asks for a map change that did not happen.
const CHANGE_LEVEL_RETRY: f64 = 1.0;
/// The most players one team balance moves.
const MAX_BALANCE_SWAPS: usize = 4;
/// The buttons that end the intermission: `IN_DUCK | IN_ATTACK | IN_ATTACK2 | IN_USE | IN_JUMP`.
const INTERMISSION_BUTTONS: u16 = 4 | 1 | 2048 | 32 | 2;
/// `PITCH_NORM`.
const PITCH_NORM: i16 = 100;

/// `m_iRoundWinStatus` (`WINSTATUS_*`): who won the round, none while it is played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WinStatus {
    #[default]
    None,
    Cts,
    Terrorists,
    Draw,
}

/// The state of the rules (the data members of `CHalfLifeMultiplay`).
#[derive(Debug, Clone)]
pub struct Rules {
    /// `m_bFreezePeriod`: the round has started but nobody may move yet.
    freeze_period: bool,
    /// `m_fRoundStartTime`: server time the round clock counts from (0: not set yet).
    round_start_time: f64,
    /// `m_iRoundTimeSecs`: seconds the round clock counts down from (the freeze time during the
    /// freeze period, then the round time).
    round_time_secs: i32,
    /// `m_iRoundTime`: seconds of a round (`mp_roundtime`).
    round_time: i32,
    /// `m_iIntroRoundTime`: seconds of the freeze period (`mp_freezetime`).
    intro_round_time: i32,
    /// `m_flRestartRoundTime`: server time the round restarts at (0: none due).
    restart_round_time: f64,
    /// `m_iRoundWinStatus`.
    round_win_status: WinStatus,
    /// `m_bCompleteReset`: the next restart starts a new match (scores and accounts reset).
    complete_reset: bool,
    /// `m_bGameStarted`: both teams have had a player in the game.
    game_started: bool,
    /// `m_bNeededPlayers`: the game waits for a player on each team (round wins do not count).
    needed_players: bool,
    /// `m_tmNextPeriodicThink`.
    next_periodic_think: f64,
    /// `m_iTotalRoundsPlayed`.
    total_rounds_played: i32,
    /// `m_iMaxRounds` (`mp_maxrounds`).
    max_rounds: i32,
    /// `m_iMaxRoundsWon` (`mp_winlimit`).
    max_rounds_won: i32,
    /// `m_iRoundWinDifference` (`mp_windifference`).
    win_difference: i32,
    /// `m_bGameOver`: the match is over and the scoreboard is up.
    game_over: bool,
    /// `m_flIntermissionStartTime`.
    intermission_start_time: f64,
    /// `m_flIntermissionEndTime`.
    intermission_end_time: f64,
    /// `m_iEndIntermissionButtonHit`: a player pressed a key during the intermission.
    end_intermission_button_hit: bool,
    /// `m_iUnBalancedRounds`: rounds in a row the teams were two or more players apart.
    unbalanced_rounds: i32,
    /// `m_iNumCT` and `m_iNumTerrorist`: the players of each team (at the last count).
    num_ct: usize,
    num_terrorist: usize,
    /// `m_iNumSpawnableCT` and `m_iNumSpawnableTerrorist`: those not in the appearance menu.
    num_spawnable_ct: usize,
    num_spawnable_terrorist: usize,
    /// `m_flGameStartTime`: server time the match started (a complete reset).
    game_start_time: f64,
    /// Server time of the last ask for a map change.
    change_level_asked_at: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            freeze_period: true,
            round_start_time: 0.0,
            round_time_secs: 0,
            round_time: 0,
            intro_round_time: 0,
            restart_round_time: 0.0,
            round_win_status: WinStatus::None,
            complete_reset: false,
            game_started: false,
            needed_players: false,
            next_periodic_think: 0.0,
            total_rounds_played: 0,
            max_rounds: 0,
            max_rounds_won: 0,
            win_difference: 1,
            game_over: false,
            intermission_start_time: 0.0,
            intermission_end_time: 0.0,
            end_intermission_button_hit: false,
            unbalanced_rounds: 0,
            num_ct: 0,
            num_terrorist: 0,
            num_spawnable_ct: 0,
            num_spawnable_terrorist: 0,
            game_start_time: 0.0,
            change_level_asked_at: f64::NEG_INFINITY,
        }
    }
}

impl Rules {
    /// The rules of a map that has just started (`CHalfLifeMultiplay::CHalfLifeMultiplay`): the
    /// freeze period, which lasts `mp_freezetime` and two seconds more, with no round started.
    pub(crate) fn new(eng: &mut EngineApi<'_>) -> Self {
        let mut r = Self {
            max_rounds: non_negative_cvar(eng, "mp_maxrounds"),
            max_rounds_won: non_negative_cvar(eng, "mp_winlimit"),
            ..Self::default()
        };
        r.read_multiplay_cvars(eng);
        r.intro_round_time += 2;
        r.round_time_secs = r.intro_round_time;
        r.win_difference = (eng.cvar_f32("mp_windifference") as i32).max(1);
        if eng.cvar_f32("mp_windifference") < 1.0 {
            eng.set_cvar("mp_windifference", "1");
        }
        r
    }

    /// `ReadMultiplayCvars` (the vanilla bounds): the round time is 1 to 9 minutes and the freeze
    /// time 0 to 60 seconds; a value outside is written back to the cvar. `mp_limitteams` is
    /// bounded the same way (0 to 20).
    fn read_multiplay_cvars(&mut self, eng: &mut EngineApi<'_>) {
        let mut round_time = (eng.cvar_f32("mp_roundtime") * 60.0) as i32;
        if round_time > 540 {
            eng.set_cvar("mp_roundtime", "9");
            round_time = 540;
        } else if round_time < 60 {
            eng.set_cvar("mp_roundtime", "1");
            round_time = 60;
        }
        self.round_time = round_time;
        let mut intro = eng.cvar_f32("mp_freezetime") as i32;
        if intro > 60 {
            eng.set_cvar("mp_freezetime", "60");
            intro = 60;
        } else if intro < 0 {
            eng.set_cvar("mp_freezetime", "0");
            intro = 0;
        }
        self.intro_round_time = intro;
        let limit = eng.cvar_f32("mp_limitteams") as i32;
        if limit > 20 {
            eng.set_cvar("mp_limitteams", "20");
        } else if limit < 0 {
            eng.set_cvar("mp_limitteams", "0");
        }
    }

    /// `IsFreezePeriod`: the round has started but nobody may move yet.
    pub fn is_freeze_period(&self) -> bool {
        self.freeze_period
    }

    /// `GetRoundRemainingTime` at server time `now`: seconds left on the round clock (the freeze
    /// time while frozen). Negative once it has run out.
    pub fn round_remaining(&self, now: f64) -> f64 {
        f64::from(self.round_time_secs) - now + self.round_start_time
    }

    /// `m_iRoundWinStatus`.
    pub fn win_status(&self) -> WinStatus {
        self.round_win_status
    }

    /// `m_bGameStarted`: both teams have had a player in the game.
    pub fn game_started(&self) -> bool {
        self.game_started
    }

    /// `m_bGameOver`: the match is over and the next map is awaited.
    pub fn game_over(&self) -> bool {
        self.game_over
    }

    /// `m_flRestartRoundTime`: server time the round restarts at, 0 when no restart is due.
    pub fn restart_round_time(&self) -> f64 {
        self.restart_round_time
    }

    /// `m_iTotalRoundsPlayed`.
    pub fn total_rounds_played(&self) -> i32 {
        self.total_rounds_played
    }

    /// `m_flIntermissionStartTime`.
    pub fn intermission_start_time(&self) -> f64 {
        self.intermission_start_time
    }
}

/// A cvar that must not be negative (`mp_maxrounds`, `mp_winlimit`): a negative value is written
/// back as 0.
fn non_negative_cvar(eng: &mut EngineApi<'_>, name: &str) -> i32 {
    let value = eng.cvar_f32(name) as i32;
    if value < 0 {
        eng.set_cvar(name, "0");
        return 0;
    }
    value
}

/// The number of players on each team that are alive and dead (`InitializePlayerCounts`).
struct Counts {
    alive_terrorist: usize,
    alive_ct: usize,
    dead_terrorist: usize,
    dead_ct: usize,
}

// ---- the messages of the rules ----

/// `CHalfLifeMultiplay::UpdateTeamScores`: both teams' round wins to everyone.
fn update_team_scores(g: &GameState, eng: &mut EngineApi<'_>) {
    for (team, wins) in [
        (Team::Ct, g.num_ct_wins),
        (Team::Terrorist, g.num_terrorist_wins),
    ] {
        eng.send_usermsg(
            MsgDest::All,
            &CsUserMsg::TeamScore {
                team_name: teams::team_name(team).to_string(),
                score: wins.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            },
        );
    }
}

/// `Broadcast`: the radio sentence `%!MRAD_<sentence>` to everyone (`SendAudio`).
fn broadcast(eng: &mut EngineApi<'_>, sentence: &str) {
    eng.send_usermsg(
        MsgDest::Broadcast,
        &CsUserMsg::SendAudio {
            sender_id: 0,
            audio_code: format!("%!MRAD_{sentence}"),
            pitch: PITCH_NORM,
        },
    );
}

/// `EndRoundMessage` (the part that reaches the clients): the text of the end of the round in
/// the middle of everyone's screen. The log lines are not written.
fn end_round_message(eng: &mut EngineApi<'_>, sentence: &str) {
    if !sentence.is_empty() {
        menus::print_center_all(eng, sentence);
    }
}

// ---- the state of the players ----

/// `InitializePlayerCounts`: counts the players of each team, alive and dead, and those that may
/// spawn (not in the appearance menu).
fn initialize_player_counts(g: &mut GameState) -> Counts {
    let mut c = Counts {
        alive_terrorist: 0,
        alive_ct: 0,
        dead_terrorist: 0,
        dead_ct: 0,
    };
    let r = &mut g.rules;
    r.num_terrorist = 0;
    r.num_ct = 0;
    r.num_spawnable_terrorist = 0;
    r.num_spawnable_ct = 0;
    for p in g.players.iter().flatten() {
        let spawnable = p.menu != Menu::ChooseAppearance;
        match p.team {
            Team::Ct => {
                r.num_ct += 1;
                r.num_spawnable_ct += usize::from(spawnable);
                if p.alive {
                    c.alive_ct += 1;
                } else {
                    c.dead_ct += 1;
                }
            }
            Team::Terrorist => {
                r.num_terrorist += 1;
                r.num_spawnable_terrorist += usize::from(spawnable);
                if p.alive {
                    c.alive_terrorist += 1;
                } else {
                    c.dead_terrorist += 1;
                }
            }
            _ => {}
        }
    }
    c
}

/// `CHalfLifeMultiplay::FPlayerCanRespawn`, the rule that decides whether a player that has just
/// picked its appearance spawns now or watches until the next round: a player that has spawned
/// in this round (`m_iNumSpawns`) or that joins a round more than [`ROUND_RESPAWN_TIME`] seconds
/// old while both teams have players waits for the next round, and one in the appearance menu
/// does not spawn.
pub(crate) fn can_respawn(g: &GameState, eng: &EngineApi<'_>, slot: u8) -> bool {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    // A player cannot respawn twice in a round.
    if p.num_spawns > 0 {
        return false;
    }
    // A player cannot respawn until the next round if more than 20 seconds in. (The screen fade
    // of `mp_fadetoblack 1` is not sent: the observer task owns it.)
    if teams::count_team_players(g, Team::Terrorist) > 0
        && teams::count_team_players(g, Team::Ct) > 0
        && eng.time() > g.rules.round_start_time + ROUND_RESPAWN_TIME
    {
        return false;
    }
    // A player cannot respawn while in the Choose Appearance menu.
    p.menu != Menu::ChooseAppearance
}

// ---- the end of a round ----

/// `TerminateRound`: the round is over with `status`; the next one starts after `delay`.
fn terminate_round(g: &mut GameState, eng: &EngineApi<'_>, delay: f64, status: WinStatus) {
    g.rules.round_win_status = status;
    g.rules.restart_round_time = eng.time() + delay;
}

/// `Round_Cts`: the counter-terrorists win the round. (The account of the round belongs to the
/// economy.)
fn round_cts(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!(ct_wins = g.num_ct_wins + 1, "round won by the counter-terrorists");
    broadcast(eng, "ctwin");
    if !g.rules.needed_players {
        g.num_ct_wins += 1;
        update_team_scores(g, eng);
    }
    end_round_message(eng, "#CTs_Win");
    terminate_round(g, eng, delay, WinStatus::Cts);
}

/// `Round_Ts`: the terrorists win the round.
fn round_ts(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!(t_wins = g.num_terrorist_wins + 1, "round won by the terrorists");
    broadcast(eng, "terwin");
    if !g.rules.needed_players {
        g.num_terrorist_wins += 1;
        update_team_scores(g, eng);
    }
    end_round_message(eng, "#Terrorists_Win");
    terminate_round(g, eng, delay, WinStatus::Terrorists);
}

/// `Round_Draw`: nobody wins the round.
fn round_draw(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!("round is a draw");
    end_round_message(eng, "#Round_Draw");
    broadcast(eng, "rounddraw");
    terminate_round(g, eng, delay, WinStatus::Draw);
}

/// `NeededPlayersCheck(float)` (`ROUND_GAME_COMMENCE`): the first round starts as soon as both
/// teams have a player, after `delay`, with a complete reset.
fn game_commence(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    // Make sure we are not on the freeze period.
    g.rules.freeze_period = false;
    g.rules.complete_reset = true;
    tracing::info!("game commencing");
    end_round_message(eng, "#Game_Commencing");
    terminate_round(g, eng, delay, WinStatus::Draw);
    g.rules.game_started = true;
}

/// `NeededPlayersCheck()`: whether the game has to wait for players, or can start now. `true`
/// when it started the game (the round was ended with a "Game Commencing").
fn needed_players_check(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    // We needed players to start scoring. Do we have them now?
    if g.rules.num_spawnable_terrorist == 0 || g.rules.num_spawnable_ct == 0 {
        menus::print_console_all(eng, "#Game_scoring");
        g.rules.needed_players = true;
        g.rules.game_started = false;
    }
    if !g.rules.game_started
        && g.rules.num_spawnable_terrorist != 0
        && g.rules.num_spawnable_ct != 0
    {
        game_commence(g, eng, GAME_COMMENCE_DELAY);
        return true;
    }
    false
}

/// `TeamExterminationCheck`: a team with no player alive loses the round; with no player alive
/// on either side the round is a draw. (The terrorists' win is held back in vanilla while a
/// planted bomb ticks: no bomb exists yet.)
fn team_extermination_check(g: &mut GameState, eng: &mut EngineApi<'_>, c: &Counts) -> bool {
    let r = &g.rules;
    let both_play = r.num_ct > 0
        && r.num_spawnable_ct > 0
        && r.num_terrorist > 0
        && r.num_spawnable_terrorist > 0;
    if both_play {
        if c.alive_terrorist == 0 && c.dead_terrorist != 0 && c.alive_ct > 0 {
            round_cts(g, eng, ROUND_BEGIN_DELAY);
            return true;
        } else if c.alive_ct == 0 && c.dead_ct != 0 {
            round_ts(g, eng, ROUND_BEGIN_DELAY);
            return true;
        }
    } else if c.alive_ct == 0 && c.alive_terrorist == 0 {
        round_draw(g, eng, ROUND_BEGIN_DELAY);
        return true;
    }
    false
}

/// `CheckWinConditions`: called whenever a player's state changes. Starts the game once both
/// teams have a player and ends the round when a team has been exterminated. Does nothing once
/// the round has been won. The VIP, prison escape, bomb and hostage rescue checks are the map
/// objectives (2E): no map has any yet.
pub(crate) fn check_win_conditions(g: &mut GameState, eng: &mut EngineApi<'_>) {
    // If a winner has already been determined and the game has started, get out of here.
    if g.rules.game_started && g.rules.round_win_status != WinStatus::None {
        return;
    }
    let counts = initialize_player_counts(g);
    g.rules.needed_players = false;
    // Other players' check.
    if needed_players_check(g, eng) {
        return;
    }
    // VIP/assassination, prison escape, bomb: 2E.
    // Team extermination. The hostage rescue check that follows it (2E) has nothing to check
    // yet, so whether the round ended is of no more interest: if not, it is still in progress.
    let _ended = team_extermination_check(g, eng, &counts);
}

/// What `ClientDisconnected` does before the player is gone: the player counts as dead on its
/// team in the check that follows.
pub(crate) fn on_disconnect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        p.alive = false;
    }
    check_win_conditions(g, eng);
}

// ---- the clock of the round ----

/// `CheckFreezePeriodExpired`.
fn check_freeze_period_expired(g: &mut GameState, eng: &mut EngineApi<'_>) {
    if g.rules.round_remaining(eng.time()) > 0.0 {
        return;
    }
    on_round_freeze_end(g, eng);
}

/// `OnRoundFreezeEnd`: the round starts. Nobody is frozen any more and the round clock starts
/// counting down the round time. (The radio call of the round start is Task 11's.)
fn on_round_freeze_end(g: &mut GameState, eng: &mut EngineApi<'_>) {
    // Freeze period expired: kill the flag.
    g.rules.freeze_period = false;
    // Reset the round time.
    g.rules.round_start_time = eng.time();
    g.rules.round_time_secs = g.rules.round_time;
    for slot in 0..g.players.len() {
        let slot = slot as u8;
        let Some(p) = g.players[usize::from(slot)].as_ref() else {
            continue;
        };
        if p.joining == JoinState::Joined && p.team != Team::Spectator {
            player::reset_max_speed(g, eng, slot);
        }
        messages::sync_round_timer(g, eng, slot);
    }
}

/// `CheckRoundTimeExpired`: when the round clock runs out and nobody has won, the objective of
/// the map decides who does (the bomb saved, the hostages not rescued, the escape or the VIP
/// failed: 2E, no map has one yet). On a map with none the round goes on and the clock restarts
/// with 60 more seconds.
fn check_round_time_expired(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let now = eng.time();
    // Not expired, or somebody has won already (the restart is due). The bomb planted, the
    // other reason the round goes on, needs the bomb (2E).
    if g.rules.round_remaining(now) > 0.0 || g.rules.round_win_status != WinStatus::None {
        return;
    }
    // Bomb target / hostages / escape zone / VIP safety zone: 2E.
    // This is done so that the portion of code has enough time to do its thing.
    g.rules.round_start_time = now + 60.0;
}

/// `CheckRestartRound`: `sv_restartround` or `sv_restart` set to a number of seconds restarts
/// the round (and the match) after that long, at most 60.
fn check_restart_round(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let mut delay = eng.cvar_f32("sv_restartround") as i32;
    if delay == 0 {
        delay = eng.cvar_f32("sv_restart") as i32;
    }
    if delay > 0 {
        restart_round_check(g, eng, f64::from(delay.min(60)));
    }
}

/// `RestartRoundCheck` (`ROUND_GAME_RESTART`): announces a restart in `delay` seconds and
/// clears the cvars that asked for it.
fn restart_round_check(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    let (number, unit) = (
        format!("{}", delay as i32),
        if delay == 1.0 { "SECOND" } else { "SECONDS" },
    );
    menus::print_all(
        eng,
        menus::HUD_PRINTCENTER,
        "#Game_will_restart_in",
        &[&number, unit],
    );
    menus::print_all(
        eng,
        menus::HUD_PRINTCONSOLE,
        "#Game_will_restart_in_console",
        &[&number, unit],
    );
    g.rules.restart_round_time = eng.time() + delay;
    g.rules.complete_reset = true;
    eng.set_cvar("sv_restartround", "0");
    eng.set_cvar("sv_restart", "0");
}

// ---- the start of a round ----

/// `CBasePlayer::Reset`: a new match gives the player a clean scoreboard line and the starting
/// money (`AddAccount` is the economy's: the account is just set).
fn reset_player(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let money = player::start_money(eng);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.frags = 0;
    p.deaths = 0;
    p.not_killed = false;
    p.money = money;
    messages::money(eng, slot, money);
    messages::score_info(eng, u16::from(slot) + 1, p);
}

/// `CBasePlayer::RoundRespawn`: the player of `slot` spawns again, unless it is choosing its
/// appearance. (The team-kill punishment is the economy's.)
fn round_respawn(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let choosing = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_none_or(|p| p.menu == Menu::ChooseAppearance);
    if choosing {
        return;
    }
    player::respawn_player(g, eng, slot);
}

/// The appearance a player has after `SwitchTeam` moves it to the other team: urban and leet,
/// gign and guerilla, sas and arctic, gsg9 and terror are each other's counterpart.
fn switched_class(class: u8) -> u8 {
    match class {
        1 => 2,
        2 => 1,
        c => c,
    }
}

/// `CBasePlayer::SwitchTeam`: the player of `slot` goes to the other team, wearing the model
/// that stands for its own. Everyone is told, as for a team picked by the player.
fn switch_team(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let new_team = match p.team {
        Team::Ct => Team::Terrorist,
        Team::Terrorist => Team::Ct,
        _ => return,
    };
    p.team = new_team;
    p.class = switched_class(p.class);
    let class = p.class;
    player::apply_model(eng, slot, new_team, class);
    teams::team_change_update(g, eng, slot);
    let text = if new_team == Team::Terrorist {
        "#Game_join_terrorist_auto"
    } else {
        "#Game_join_ct_auto"
    };
    let name = teams::player_name(eng, slot);
    menus::print_all_notify(eng, text, &name);
}

/// `CBasePlayer::CanSwitchTeam`: a player of the team that must give players away (no VIP
/// exists yet).
fn can_switch_team(g: &GameState, slot: usize, from: Team) -> bool {
    g.players[slot].as_ref().is_some_and(|p| p.team == from)
}

/// `BalanceTeams`: moves the most recently joined players of the larger team to the other
/// until they differ by at most one, at most [`MAX_BALANCE_SWAPS`] of them. (The ratio of a
/// map with a VIP safety zone is 2E.)
fn balance_teams(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let (cts, ts) = (g.rules.num_ct, g.rules.num_terrorist);
    let (from, swaps) = if cts > ts {
        (Team::Ct, (cts - ts) / 2)
    } else if ts > cts {
        (Team::Terrorist, (ts - cts) / 2)
    } else {
        // Teams are even: get out of here.
        return;
    };
    // Don't swap more than 4 players at a time.
    for _ in 0..swaps.min(MAX_BALANCE_SWAPS) {
        // The player that joined the server last (the highest user id).
        let candidate = (0..g.players.len())
            .filter(|&i| can_switch_team(g, i, from))
            .max_by_key(|&i| g.players[i].as_ref().map_or(0, |p| p.join_order));
        if let Some(slot) = candidate {
            switch_team(g, eng, slot as u8);
        }
    }
}

/// `RestartRound`: a new round. The teams are balanced when they have been uneven for a round,
/// a new match resets the scores, and every player on a team spawns again, frozen for
/// `mp_freezetime`.
pub(crate) fn restart_round(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let now = eng.time();
    tracing::info!(complete_reset = g.rules.complete_reset, "restarting the round");
    g.rules.total_rounds_played += 1;

    // Tabulate the number of players on each team.
    g.rules.num_ct = teams::count_team_players(g, Team::Ct);
    g.rules.num_terrorist = teams::count_team_players(g, Team::Terrorist);

    let balance = eng.cvar_f32("mp_autoteambalance") > 0.0;
    if balance && g.rules.unbalanced_rounds >= 1 {
        balance_teams(g, eng);
    }
    let (cts, ts) = (g.rules.num_ct, g.rules.num_terrorist);
    if cts.abs_diff(ts) >= 2 {
        g.rules.unbalanced_rounds += 1;
    } else {
        g.rules.unbalanced_rounds = 0;
    }
    // Warn the players of an impending auto-balance next round...
    if balance && g.rules.unbalanced_rounds == 1 {
        menus::print_center_all(eng, "#Auto_Team_Balance_Next_Round");
    }

    if g.rules.complete_reset {
        // A new match.
        if eng.cvar_f32("mp_timelimit") < 0.0 {
            eng.set_cvar("mp_timelimit", "0");
        }
        g.rules.game_start_time = now;
        // Reset total number of rounds played.
        g.rules.total_rounds_played = 0;
        g.rules.max_rounds = non_negative_cvar(eng, "mp_maxrounds");
        g.rules.max_rounds_won = non_negative_cvar(eng, "mp_winlimit");
        // Reset score info.
        g.num_terrorist_wins = 0;
        g.num_ct_wins = 0;
        update_team_scores(g, eng);
        // Reset the player stats.
        for slot in 0..g.players.len() {
            if g.players[slot].is_some() {
                reset_player(g, eng, slot as u8);
            }
        }
    }

    g.rules.freeze_period = true;
    g.rules.read_multiplay_cvars(eng);
    // This makes the round timer function as the intro timer on the client side.
    g.rules.round_time_secs = g.rules.intro_round_time;

    // The accounts, the hostages, the VIP and the escape rounds are not ported (see the module
    // documentation). The round time stamp must be set before the players are spawned.
    g.rules.round_start_time = now;
    if g.rules.complete_reset {
        // We are starting fresh: it is like no one has ever won or lost.
        g.num_terrorist_wins = 0;
        g.num_ct_wins = 0;
    }

    for slot in 0..g.players.len() {
        let Some(p) = g.players[slot].as_mut() else {
            continue;
        };
        p.num_spawns = 0;
        p.team_changed = false;
        if p.team != Team::Unassigned && p.team != Team::Spectator {
            round_respawn(g, eng, slot as u8);
        }
    }

    // Reset game variables.
    let rules = &mut g.rules;
    rules.intermission_end_time = 0.0;
    rules.intermission_start_time = 0.0;
    rules.restart_round_time = 0.0;
    rules.round_win_status = WinStatus::None;
    rules.complete_reset = false;
}

// ---- the end of the match ----

/// `GoToIntermission`: the match is over; the scoreboard is shown for `mp_chattime` seconds and
/// then the next map is asked for.
fn go_to_intermission(g: &mut GameState, eng: &mut EngineApi<'_>) {
    if g.rules.game_over {
        // Intermission has already been triggered, so ignore.
        return;
    }
    eng.send(MsgDest::All, &[svc::INTERMISSION]);
    let chat_time = clamp_chat_time(eng);
    let now = eng.time();
    g.rules.intermission_end_time = now + f64::from(chat_time);
    g.rules.intermission_start_time = now;
    g.rules.game_over = true;
    g.rules.end_intermission_button_hit = false;
}

/// `mp_chattime` as an integer of 1 to [`MAX_INTERMISSION_TIME`] seconds; a value outside is
/// written back.
fn clamp_chat_time(eng: &mut EngineApi<'_>) -> i32 {
    let time = eng.cvar_f32("mp_chattime") as i32;
    if time < 1 {
        eng.set_cvar("mp_chattime", "1");
        1
    } else if time > MAX_INTERMISSION_TIME {
        eng.set_cvar("mp_chattime", &MAX_INTERMISSION_TIME.to_string());
        MAX_INTERMISSION_TIME
    } else {
        time
    }
}

/// `CheckGameOver`: while the scoreboard is up, waits out `mp_chattime` and a key press of a
/// player (or [`MAX_INTERMISSION_TIME`]), then changes the map.
fn check_game_over(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    if !g.rules.game_over {
        return false;
    }
    let chat_time = clamp_chat_time(eng);
    let now = eng.time();
    let r = &mut g.rules;
    r.intermission_end_time = r.intermission_start_time + f64::from(chat_time);
    let nobody = g.players.iter().all(Option::is_none);
    if r.intermission_end_time < now
        && (nobody
            || r.end_intermission_button_hit
            || r.intermission_start_time + f64::from(MAX_INTERMISSION_TIME) < now)
    {
        // Intermission is over.
        change_level(g, eng);
    }
    true
}

/// `CheckTimeLimit`: the match ends `mp_timelimit` minutes after it started.
fn check_time_limit(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    let limit = eng.cvar_f32("mp_timelimit");
    if limit < 0.0 {
        eng.set_cvar("mp_timelimit", "0");
        return false;
    }
    if limit != 0.0 {
        let end = g.rules.game_start_time + f64::from(limit * 60.0);
        if eng.time() >= end {
            tracing::info!("changing maps because the time limit has been met");
            go_to_intermission(g, eng);
            return true;
        }
    }
    false
}

/// `CheckMaxRounds`: the match ends after `mp_maxrounds` rounds.
fn check_max_rounds(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    if g.rules.max_rounds != 0 && g.rules.total_rounds_played >= g.rules.max_rounds {
        tracing::info!("changing maps because the maximum rounds have been met");
        go_to_intermission(g, eng);
        return true;
    }
    false
}

/// `CheckWinLimit`: the match ends when a team has won `mp_winlimit` rounds, `mp_windifference`
/// more than the other.
fn check_win_limit(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    let (ct, t) = (g.num_ct_wins, g.num_terrorist_wins);
    let r = &g.rules;
    if r.max_rounds_won != 0
        && (ct >= r.max_rounds_won || t >= r.max_rounds_won)
        && (ct - t >= r.win_difference || t - ct >= r.win_difference)
    {
        tracing::info!("changing maps because one team has won the specified number of rounds");
        go_to_intermission(g, eng);
        return true;
    }
    false
}

// ---- the rules' Think ----

/// `CHalfLifeMultiplay::Think`, run every server frame.
pub(crate) fn think(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let now = eng.time();
    if g.rules.round_start_time == 0.0 {
        // Initialize the timer time stamps; this happens once only.
        g.rules.round_start_time = now;
    }
    // Check game rules.
    if check_game_over(g, eng)
        || check_time_limit(g, eng)
        || check_max_rounds(g, eng)
        || check_win_limit(g, eng)
    {
        return;
    }
    // Check for the end of the round.
    if g.rules.freeze_period {
        check_freeze_period_expired(g, eng);
    } else {
        check_round_time_expired(g, eng);
    }
    if g.rules.restart_round_time > 0.0 && g.rules.restart_round_time <= now {
        restart_round(g, eng);
    }
    if now > g.rules.next_periodic_think {
        check_restart_round(g, eng);
        g.rules.next_periodic_think = now + 1.0;
        g.rules.max_rounds = non_negative_cvar(eng, "mp_maxrounds");
        g.rules.max_rounds_won = non_negative_cvar(eng, "mp_winlimit");
    }
}

/// `CHalfLifeMultiplay::PlayerThink`, the part about the intermission: a key pressed by a
/// player while the scoreboard is up lets the map change once `mp_chattime` is over.
pub(crate) fn player_think(g: &mut GameState, slot: u8, buttons: u16) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let pressed = buttons & !p.last_buttons;
    p.last_buttons = buttons;
    if g.rules.game_over && pressed & INTERMISSION_BUTTONS != 0 {
        g.rules.end_intermission_button_hit = true;
    }
}

// ---- the map cycle ----

/// One line of `mapcycle.txt` (`mapcycle_item_t`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct MapCycleItem {
    map: String,
    min_players: i32,
    max_players: i32,
}

/// The maps `mapcycle.txt` lists, in order, and the one the cycle is at (`mapcycle_t`). It
/// outlives the maps: [`GameState::map_cycle`] is not reset when the map changes.
#[derive(Debug, Clone, Default)]
pub struct MapCycle {
    items: Vec<MapCycleItem>,
    next: usize,
}

/// The tokens of a Half-Life text file (`SharedParse`): words split by white space, `"quoted"`
/// strings, `//` comments to the end of the line.
fn tokenize_line(line: &str) -> Vec<String> {
    let line = line.split("//").next().unwrap_or("");
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c.is_control() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut token = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                token.push(c);
            }
            tokens.push(token);
        } else {
            let mut token = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c.is_control() {
                    break;
                }
                token.push(c);
                chars.next();
            }
            tokens.push(token);
        }
    }
    tokens
}

/// The value of `key` in an info string `\key\value\key2\value2` (`GET_KEY_VALUE`), as a number
/// (`atoi`), 0 when it is missing.
fn info_number(info: &str, key: &str) -> i32 {
    let mut parts = info.split('\\').skip(1);
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k == key {
            return menus::atoi(v);
        }
    }
    0
}

impl MapCycle {
    /// `ReloadMapCycleFile` on `mapcycle.txt` (the `mapcyclefile` cvar is not read): every line
    /// names a map, optionally followed by `\minplayers\N\maxplayers\N`. A map that is not
    /// valid (`IS_MAP_VALID`: it has no `maps/<name>.bsp`) is skipped. The first map of the
    /// file is where the cycle starts. Nothing is listed for a file that is missing or empty.
    pub fn load(vfs: &cs_vfs::Vfs) -> Self {
        let text = match vfs.read_string_lossy("mapcycle.txt") {
            Ok(text) => text,
            Err(cs_vfs::VfsError::NotFound(_)) => String::new(),
            Err(err) => {
                tracing::warn!(%err, "mapcycle.txt could not be read");
                String::new()
            }
        };
        let mut items = Vec::new();
        for line in text.lines() {
            let tokens = tokenize_line(line);
            let Some(map) = tokens.first() else { continue };
            if !cs_engine::valid_map_name(map) || !vfs.exists(&format!("maps/{map}.bsp")) {
                tracing::info!(map = %map, "skipping a map of the map cycle: not a valid map");
                continue;
            }
            let rules = tokens.get(1).map_or("", String::as_str);
            items.push(MapCycleItem {
                map: map.clone(),
                min_players: info_number(rules, "minplayers").max(0),
                max_players: info_number(rules, "maxplayers").max(0),
            });
        }
        Self { items, next: 0 }
    }

    /// The map the cycle is at (where [`advance`](Self::advance) goes next when no player range
    /// holds it back), `None` for an empty cycle.
    pub fn next_map(&self) -> Option<&str> {
        self.items.get(self.next).map(|i| i.map.as_str())
    }

    /// The map the cycle goes to next with `players` players on the server, and moves on past it
    /// (the item walk of `ChangeLevel`): the first map from where the cycle stands whose player
    /// range holds `players`; the last map of the walk is never looked at, as in the original.
    /// `max_clients` caps the ranges. `None` for an empty cycle.
    fn advance(&mut self, players: i32, max_clients: i32) -> Option<String> {
        let n = self.items.len();
        if n == 0 {
            return None;
        }
        let mut found = false;
        let mut chosen = None;
        for k in 0..n - 1 {
            let i = (self.next + k) % n;
            let item = &self.items[i];
            let mut keep_looking = false;
            if item.min_players != 0 {
                if players >= item.min_players.min(max_clients) {
                    found = true;
                } else {
                    keep_looking = true;
                }
            }
            if item.max_players != 0 {
                if players <= item.max_players.min(max_clients) {
                    found = true;
                } else {
                    keep_looking = true;
                }
            }
            if keep_looking {
                continue;
            }
            found = true;
            chosen = Some(i);
            break;
        }
        let index = match (chosen, found) {
            (Some(i), _) => i,
            // The walk ended: the last item of it, or where the cycle stood if nothing fitted.
            (None, true) => (self.next + n - 1) % n,
            (None, false) => self.next,
        };
        self.next = (index + 1) % n;
        Some(self.items[index].map.clone())
    }
}

/// `CHalfLifeMultiplay::ChangeLevel`: asks the engine for the next map of the cycle. A cycle
/// that cannot be used keeps the current map. Asked again after [`CHANGE_LEVEL_RETRY`] seconds
/// if the map did not change.
fn change_level(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let now = eng.time();
    if now - g.rules.change_level_asked_at < CHANGE_LEVEL_RETRY {
        return;
    }
    g.rules.change_level_asked_at = now;
    let players = g.players.iter().flatten().count() as i32;
    let max_clients = i32::from(eng.max_clients());
    let map = g
        .map_cycle
        .advance(players, max_clients)
        .unwrap_or_else(|| eng.map_name().to_string());
    tracing::info!(map = %map, "CHANGE LEVEL");
    eng.change_level(&map);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(lines: &[(&str, i32, i32)]) -> MapCycle {
        MapCycle {
            items: lines
                .iter()
                .map(|&(map, min_players, max_players)| MapCycleItem {
                    map: map.to_string(),
                    min_players,
                    max_players,
                })
                .collect(),
            next: 0,
        }
    }

    #[test]
    fn the_cycle_goes_round_in_order() {
        let mut c = cycle(&[("a", 0, 0), ("b", 0, 0), ("c", 0, 0)]);
        let got: Vec<_> = (0..5).filter_map(|_| c.advance(4, 32)).collect();
        assert_eq!(got, ["a", "b", "c", "a", "b"]);
        assert_eq!(MapCycle::default().advance(1, 32), None);
        // A cycle of one map goes to it again and again.
        let mut one = cycle(&[("only", 0, 0)]);
        assert_eq!(one.advance(1, 32).as_deref(), Some("only"));
        assert_eq!(one.advance(1, 32).as_deref(), Some("only"));
    }

    #[test]
    fn the_cycle_honours_the_player_range_of_a_line() {
        // `big` wants 8 players: with 2 the cycle skips it.
        let mut c = cycle(&[("a", 0, 0), ("big", 8, 0), ("c", 0, 0)]);
        assert_eq!(c.advance(2, 32).as_deref(), Some("a"));
        assert_eq!(c.advance(2, 32).as_deref(), Some("c"));
        assert_eq!(c.advance(2, 32).as_deref(), Some("a"));
        // With 9 it is taken.
        let mut c = cycle(&[("a", 0, 0), ("big", 8, 0), ("c", 0, 0)]);
        c.next = 1;
        assert_eq!(c.advance(9, 32).as_deref(), Some("big"));
        // `small` takes at most 4.
        let mut c = cycle(&[("small", 0, 4), ("c", 0, 0), ("d", 0, 0)]);
        assert_eq!(c.advance(6, 32).as_deref(), Some("c"));
        assert_eq!(c.advance(3, 32).as_deref(), Some("d"));
        // Nothing fits: the map the cycle stood at.
        let mut c = cycle(&[("x", 9, 0), ("y", 9, 0), ("z", 9, 0)]);
        assert_eq!(c.advance(1, 32).as_deref(), Some("x"));
    }

    #[test]
    fn map_cycle_lines_are_read_like_the_original() {
        assert_eq!(tokenize_line("de_dust2"), ["de_dust2"]);
        assert_eq!(
            tokenize_line("  cs_office  \\minplayers\\4\\maxplayers\\8 // comment"),
            ["cs_office", "\\minplayers\\4\\maxplayers\\8"]
        );
        assert_eq!(tokenize_line("\"de_nuke\""), ["de_nuke"]);
        assert!(tokenize_line("// only a comment").is_empty());
        assert_eq!(info_number("\\minplayers\\4\\maxplayers\\8", "maxplayers"), 8);
        assert_eq!(info_number("\\minplayers\\4", "maxplayers"), 0);
        assert_eq!(info_number("", "minplayers"), 0);
    }

    #[test]
    fn a_map_cycle_loads_the_valid_maps_only() {
        let vfs = cs_vfs::Vfs::from_memory([
            (
                "mapcycle.txt".to_string(),
                b"de_dust2\ncs_gone\n\\bad\nde_nuke \\minplayers\\2\n".to_vec(),
            ),
            ("maps/de_dust2.bsp".to_string(), Vec::new()),
            ("maps/de_nuke.bsp".to_string(), Vec::new()),
        ]);
        let c = MapCycle::load(&vfs);
        let maps: Vec<_> = c.items.iter().map(|i| i.map.as_str()).collect();
        assert_eq!(maps, ["de_dust2", "de_nuke"]);
        assert_eq!(c.items[1].min_players, 2);
        assert!(MapCycle::load(&cs_vfs::Vfs::from_memory([])).items.is_empty());
    }

    #[test]
    fn a_switched_team_wears_the_counterpart_model() {
        for class in 1..=4u8 {
            assert_eq!(switched_class(switched_class(class)), class);
        }
        // urban <-> leet, gsg9 <-> terror, sas <-> arctic, gign <-> guerilla.
        assert_eq!(switched_class(1), 2);
        assert_eq!(switched_class(3), 3);
        assert_eq!(switched_class(4), 4);
    }

    #[test]
    fn the_round_clock_counts_down_from_the_freeze_time() {
        let r = Rules {
            round_time_secs: 6,
            round_start_time: 100.0,
            ..Rules::default()
        };
        assert_eq!(r.round_remaining(100.0), 6.0);
        assert_eq!(r.round_remaining(104.5), 1.5);
        assert!(r.round_remaining(107.0) < 0.0);
    }
}
