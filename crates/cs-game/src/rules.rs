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
//! - `mapcyclefile` is not read: the file is always `mapcycle.txt` ([`crate::mapcycle::MapCycle::load`]), loaded
//!   once when the server starts. The `minplayers`/`maxplayers` of a line are honoured; the
//!   other rules of a line (commands run after the change) are not executed.
//! - A player that leaves is `FL_DORMANT` before `ClientDisconnected`, so the counts of the check
//!   made when it leaves do not include it ([`on_disconnect`]); nothing of this runs once the
//!   match is over.
//! - `m_bRoundTerminating` is written by `TerminateRound`/`RestartRound` but never read in
//!   vanilla, so it is not ported. `ClearBodyQue` and `CleanUpMap` are skipped: nothing creates
//!   corpse entities or breakables yet (the corpse is a client-side `ClCorpse`); whether the
//!   corpses of the last round are gone after a restart needs a live check with a real client.
//! - `votemap`/`vote` (map and kick votes, `mp_mapvoteratio`, `DisplayMaps` with a vote) are not
//!   ported: phase 7 (votes). `listmaps` lists the cycle with 0 votes each.
//! - `mp_fraglimit` has no effect: in vanilla `CheckFragLimit` is empty outside `REGAMEDLL_ADD`.
//! - `CheckGameOver`'s "nobody may change the map" test is `!UTIL_HumansInGame()` in vanilla
//!   (only bots: change at once); here it is an empty server (`players` all empty), which is the
//!   same while there are no bots (assumption: `UTIL_HumansInGame` was not read).
//! - A map change the engine refuses is asked again once a second ([`CHANGE_LEVEL_RETRY`]) and
//!   each retry advances the map cycle, where vanilla `ChangeLevel` checks each item with
//!   `IS_MAP_VALID` before it takes it (assumption, the body of `ChangeLevel` was not read for
//!   this note); the engine's validity check is not reachable from the game.
//! - `m_iCareer*`, bots, tutor, `m_flForceCameraValue`/`AllowSpec` change messages, the idle
//!   kick and the voice manager are not ported.
//!
//! The money of a round (`m_iAccountCT`, `m_iAccountTerrorist`, the loser bonus and its streaks)
//! is [`crate::money`]'s; `RestartRound` settles it here.
//!
//! Left for later tasks: the map objectives (2E): the bomb, the
//! hostages, the VIP and the escape zones. Their branches in [`check_win_conditions`] and
//! [`check_round_time_expired`] are stubs: no entity of those classes exists yet, so a map has
//! no objective, and a round that runs out of time on such a map simply goes on, as
//! `CheckRoundTimeExpired` does (it pushes the round start 60 seconds forward). `CleanUpMap`
//! (re-creating the map's breakables and doors) has nothing to clean yet.

use cs_engine::{EngineApi, MsgDest};
use cs_proto48::messages::svc;
use cs_proto48::usermsg::CsUserMsg;

use crate::GameState;
use crate::chat;
use crate::hud_misc;
use crate::menus::{self, Menu};
use crate::messages;
use crate::money::{self, Economy};
use crate::observer;
use crate::player::{self, PITCH_NORM, Team};
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
    pub(crate) num_terrorist: usize,
    /// `m_iTotalGunCount`, `m_iTotalGrenadeCount`, `m_iTotalArmourCount`: what the armoury
    /// racks of the map hold in all (never reset but by the rules being made).
    pub(crate) total_gun_count: i32,
    pub(crate) total_grenade_count: i32,
    pub(crate) total_armour_count: i32,
    /// `m_iNumSpawnableCT` and `m_iNumSpawnableTerrorist`: those not in the appearance menu.
    num_spawnable_ct: usize,
    num_spawnable_terrorist: usize,
    /// `m_flGameStartTime`: server time the match started (a complete reset).
    game_start_time: f64,
    /// Server time of the last ask for a map change.
    change_level_asked_at: f64,
    /// The round economy: what the teams are paid, the losing streaks, the loser bonus.
    economy: Economy,
    /// `m_bMapHasBuyZone`: the map has a `func_buyzone` (else the players buy around the spawn
    /// points).
    map_has_buy_zone: bool,
    /// `m_bMapHasBombTarget`: the map has a bomb target (a `func_bomb_target` or the legacy
    /// `info_bomb_target`).
    map_has_bomb_target: bool,
    /// `m_bMapHasBombZone`: the map has a `func_bomb_target` (not only the legacy point).
    map_has_bomb_zone: bool,
    /// `m_bMapHasRescueZone`: the map has a `func_hostage_rescue`.
    map_has_rescue_zone: bool,
    /// `m_bMapHasEscapeZone`: the map has a `func_escapezone`.
    map_has_escape_zone: bool,
    /// `m_bMapHasVIPSafetyZone`: the map has a `func_vip_safetyzone`.
    map_has_vip_safety_zone: bool,
    /// `m_bCTCantBuy`: the counter-terrorists may not buy (`info_map_parameters`).
    ct_cant_buy: bool,
    /// `m_bTCantBuy`: the terrorists may not buy (`info_map_parameters`).
    t_cant_buy: bool,
    /// `m_flBombRadius`: 500 unless `info_map_parameters` says otherwise. The bomb's explosion
    /// takes it as its damage and 3.5 times it as its radius (`CBaseMonster::RadiusDamage`).
    bomb_radius: f32,
    /// `m_bBombDropped`: the C4 is lying dropped on the ground.
    pub bomb_dropped: bool,
    /// `m_bTargetBombed`: the planted C4 exploded this round.
    pub target_bombed: bool,
    /// `m_bBombDefused`: the planted C4 was defused this round.
    pub bomb_defused: bool,
    /// `m_iC4Timer`: seconds from plant to detonation (`mp_c4timer`, clamped 10..=90).
    pub c4_timer: i32,
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
            total_gun_count: 0,
            total_grenade_count: 0,
            total_armour_count: 0,
            num_spawnable_ct: 0,
            num_spawnable_terrorist: 0,
            game_start_time: 0.0,
            change_level_asked_at: f64::NEG_INFINITY,
            economy: Economy::default(),
            map_has_buy_zone: false,
            map_has_bomb_target: false,
            map_has_bomb_zone: false,
            map_has_rescue_zone: false,
            map_has_escape_zone: false,
            map_has_vip_safety_zone: false,
            ct_cant_buy: false,
            t_cant_buy: false,
            bomb_radius: 500.0,
            bomb_dropped: false,
            target_bombed: false,
            bomb_defused: false,
            c4_timer: 45,
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
        r.check_map_conditions(eng);
        r.intro_round_time += 2;
        r.round_time_secs = r.intro_round_time;
        r.win_difference = (eng.cvar_f32("mp_windifference") as i32).max(1);
        if eng.cvar_f32("mp_windifference") < 1.0 {
            eng.set_cvar("mp_windifference", "1");
        }
        r
    }

    /// `CHalfLifeMultiplay::CheckMapConditions` (ReGameDLL `multiplay_gamerules.cpp` 1640-1672,
    /// read 2026-11-23): what the objective entities of the map are. A `func_bomb_target` makes
    /// both bomb flags true; the legacy `info_bomb_target` alone only the target one. Run at the
    /// start of the map and by every `RestartRound`.
    pub(crate) fn check_map_conditions(&mut self, eng: &EngineApi<'_>) {
        let has = |class: &str| eng.find_by_classname(None, class).is_some();
        if has("func_bomb_target") {
            self.map_has_bomb_target = true;
            self.map_has_bomb_zone = true;
        } else if has("info_bomb_target") {
            self.map_has_bomb_target = true;
            self.map_has_bomb_zone = false;
        } else {
            self.map_has_bomb_target = false;
            self.map_has_bomb_zone = false;
        }
        self.map_has_rescue_zone = has("func_hostage_rescue");
        self.map_has_buy_zone = has("func_buyzone");
        self.map_has_escape_zone = has("func_escapezone");
        self.map_has_vip_safety_zone = has("func_vip_safetyzone");
    }

    /// The tail of `CMapInfo::CheckMapInfo` (`mapinfo.cpp` 60-67): the bomb radius and the
    /// teams that may not buy, written into the rules.
    pub(crate) fn set_map_parameters(&mut self, bomb_radius: f32, ct_cant_buy: bool, t_cant_buy: bool) {
        self.bomb_radius = bomb_radius;
        self.ct_cant_buy = ct_cant_buy;
        self.t_cant_buy = t_cant_buy;
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
        let mut c4_timer = eng.cvar_f32("mp_c4timer") as i32;
        if c4_timer > 90 {
            eng.set_cvar("mp_c4timer", "90");
            c4_timer = 90;
        } else if c4_timer < 10 {
            eng.set_cvar("mp_c4timer", "10");
            c4_timer = 10;
        }
        self.c4_timer = c4_timer;
    }

    /// The round economy (`m_iAccountCT`, `m_iNumConsecutiveCTLoses`, `m_iLoserBonus`, ...).
    pub fn economy(&self) -> &Economy {
        &self.economy
    }

    /// `IsFreezePeriod`: the round has started but nobody may move yet.
    pub fn is_freeze_period(&self) -> bool {
        self.freeze_period
    }

    /// Sets `m_bGameOver`, without the intermission. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn force_game_over(&mut self, over: bool) {
        self.game_over = over;
    }

    /// `m_bMapHasBuyZone`.
    pub fn map_has_buy_zone(&self) -> bool {
        self.map_has_buy_zone
    }

    /// `m_bMapHasBombTarget`.
    pub fn map_has_bomb_target(&self) -> bool {
        self.map_has_bomb_target
    }

    /// `m_bMapHasBombZone`.
    pub fn map_has_bomb_zone(&self) -> bool {
        self.map_has_bomb_zone
    }

    /// `m_bMapHasRescueZone`.
    pub fn map_has_rescue_zone(&self) -> bool {
        self.map_has_rescue_zone
    }

    /// `m_bMapHasEscapeZone`.
    pub fn map_has_escape_zone(&self) -> bool {
        self.map_has_escape_zone
    }

    /// `m_bMapHasVIPSafetyZone`.
    pub fn map_has_vip_safety_zone(&self) -> bool {
        self.map_has_vip_safety_zone
    }

    /// `m_bCTCantBuy`.
    pub fn ct_cant_buy(&self) -> bool {
        self.ct_cant_buy
    }

    /// `m_bTCantBuy`.
    pub fn t_cant_buy(&self) -> bool {
        self.t_cant_buy
    }

    /// `m_flBombRadius`.
    pub fn bomb_radius(&self) -> f32 {
        self.bomb_radius
    }

    /// `m_fRoundStartTime`: server time the round clock counts from.
    pub fn round_start_time(&self) -> f64 {
        self.round_start_time
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
pub(crate) fn broadcast(eng: &mut EngineApi<'_>, sentence: &str) {
    eng.send_usermsg(
        MsgDest::Broadcast,
        &CsUserMsg::SendAudio {
            sender_id: 0,
            audio_code: format!("%!MRAD_{sentence}"),
            pitch: i16::from(PITCH_NORM),
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
pub(crate) fn can_respawn(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    // A player cannot respawn twice in a round.
    if p.num_spawns > 0 {
        return false;
    }
    // A player cannot respawn until the next round if more than 20 seconds in. If it just
    // connected and `mp_fadetoblack` is 1, "maybe the server admin doesn't want him peeking
    // around": its screen goes black and stays (`UTIL_ScreenFade(pPlayer, Vector(0, 0, 0), 3, 3,
    // 255, FFADE_OUT | FFADE_STAYOUT)`, the only place a late joiner is faded; `Killed` fades the
    // dead and [`player::spawn`] clears it).
    if teams::count_team_players(g, Team::Terrorist) > 0
        && teams::count_team_players(g, Team::Ct) > 0
        && eng.time() > g.rules.round_start_time + ROUND_RESPAWN_TIME
    {
        if eng.cvar_f32("mp_fadetoblack") == observer::FADETOBLACK_STAY as f32 {
            observer::screen_fade(
                eng,
                slot,
                3.0,
                3.0,
                255,
                observer::FFADE_OUT | observer::FFADE_STAYOUT,
            );
        }
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

/// `Round_Cts`: the counter-terrorists win the round (and are paid for it when the next one
/// starts).
fn round_cts(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!(needed_players = g.rules.needed_players, "round won by the counter-terrorists");
    broadcast(eng, "ctwin");
    g.rules.economy.round_won(Team::Ct);
    if !g.rules.needed_players {
        g.num_ct_wins += 1;
        update_team_scores(g, eng);
    }
    end_round_message(eng, "#CTs_Win");
    terminate_round(g, eng, delay, WinStatus::Cts);
}

/// `Round_Ts`: the terrorists win the round.
fn round_ts(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!(needed_players = g.rules.needed_players, "round won by the terrorists");
    broadcast(eng, "terwin");
    g.rules.economy.round_won(Team::Terrorist);
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

/// `CHalfLifeMultiplay::ClientDisconnected` (ReGameDLL `multiplay_gamerules.cpp:3580`, read
/// 2026-11-22), called by `ClientDisconnect` after it made the player dormant: the player is
/// `DEAD_DEAD` and `SetScoreboardAttributes()` tells everyone; then (the items, the VIP and the
/// votes have nothing to release yet) its scoreboard line is emptied (`ScoreInfo` with nothing,
/// `TeamInfo` `UNASSIGNED`), the observers that followed it move on ([`observer::on_target_left`])
/// and `CheckWinConditions` runs last. The player is dormant, so `InitializePlayerCounts` no longer
/// counts it: here it is removed from [`GameState::players`] before the check. (The empty
/// `Location` message and `game_playerleave` are not ported.)
pub(crate) fn on_disconnect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        p.alive = false;
    }
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.deadflag = player::DEAD_DEAD;
    }
    messages::set_scoreboard_attributes(g, eng, usize::from(slot));
    if let Some(entry) = g.players.get_mut(usize::from(slot)) {
        *entry = None;
    }
    teams::announce_departure(eng, slot);
    observer::on_target_left(g, eng, slot);
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

/// `OnRoundFreezeEnd` (`multiplay_gamerules.cpp`, read 2026-11-22): the round starts. Nobody is
/// frozen any more, the round clock starts counting down the round time, and the first joined
/// CT and the first joined T call out a random start sentence over the radio
/// (`RANDOM_LONG(0, 3)`: MOVEOUT, LETSGO, LOCKNLOAD or GO, the same for both teams). The map
/// objectives that change the sentences (`m_bMapHasEscapeZone`: ELIM/GETOUT,
/// `m_bMapHasVIPSafetyZone`: VIP/LOCKNLOAD) are 2E. The call is `Radio(sentence)` without a
/// verbose text, so the teammates get the `SendAudio` and the icon, no `TextMsg`. Vanilla's
/// `pev->flags == FL_DORMANT` test (an equality, never true for a joined player) is dropped.
pub(crate) fn on_round_freeze_end(g: &mut GameState, eng: &mut EngineApi<'_>) {
    // Freeze period expired: kill the flag.
    g.rules.freeze_period = false;
    let sentence = match g.random_range(0, 3) {
        0 => "%!MRAD_MOVEOUT",
        1 => "%!MRAD_LETSGO",
        2 => "%!MRAD_LOCKNLOAD",
        _ => "%!MRAD_GO",
    };
    // Reset the round time.
    g.rules.round_start_time = eng.time();
    g.rules.round_time_secs = g.rules.round_time;
    let (mut ct_played, mut t_played) = (false, false);
    for slot in 0..g.players.len() {
        let slot = slot as u8;
        let Some(p) = g.players[usize::from(slot)].as_ref() else {
            continue;
        };
        if p.joining == JoinState::Joined {
            if p.team == Team::Ct && !ct_played {
                ct_played = true;
                hud_misc::radio(g, eng, slot, sentence, "");
            } else if p.team == Team::Terrorist && !t_played {
                t_played = true;
                hud_misc::radio(g, eng, slot, sentence, "");
            }
            if g.players[usize::from(slot)]
                .as_ref()
                .is_some_and(|p| p.team != Team::Spectator)
            {
                player::reset_max_speed(g, eng, slot);
                // `OnRoundFreezeEnd` (multiplay_gamerules.cpp 2861): shooting is allowed.
                if let Some(p) = g.players[usize::from(slot)].as_mut() {
                    p.can_shoot = true;
                }
            }
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
/// money (the account is emptied and told to the client, then `AddAccount` gives the
/// `mp_startmoney` that `CheckStartMoney` allows).
fn reset_player(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let start = money::check_start_money(eng);
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
    p.money = 0;
    messages::money(eng, slot, 0);
    money::add_account(g, eng, slot, start, true);
    if let Some(p) = g.players[usize::from(slot)].as_ref() {
        messages::score_info(eng, u16::from(slot) + 1, p);
    }
}

/// `CBasePlayer::RoundRespawn`: the player of `slot` spawns again, unless it is choosing its
/// appearance. A player that killed a teammate may be punished first ([`money::punish_team_kill`]).
fn round_respawn(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let choosing = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_none_or(|p| p.menu == Menu::ChooseAppearance);
    money::punish_team_kill(g, eng, slot);
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

    // Hardlock the player accelaration to 5.0 (`RestartRound`, multiplay_gamerules.cpp
    // 1697-1700, read 2026-11-22): set whatever they are.
    for (name, value) in HARD_LOCKED_CVARS {
        eng.set_cvar(name, value);
    }

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

    // Check to see if there's a mapping info paramater entity, then what the map has
    // (`RestartRound`, multiplay_gamerules.cpp 1853-1857, read 2026-11-23). The escape and VIP
    // rounds that follow in the source are not ported yet (see the module documentation).
    crate::zones::check_map_info(g);
    g.rules.check_map_conditions(eng);
    // The hostages, the VIP and the escape rounds are not ported (see the module documentation).
    // Scale up the loser bonus when teams fall into losing streaks, and assign it.
    let status = g.rules.round_win_status;
    g.rules.economy.settle(status);
    // The round time stamp must be set before the players are spawned.
    g.rules.round_start_time = now;
    if g.rules.complete_reset {
        // No cash for anyone at first rounds (only the starting money), and it is like no one
        // has ever won or lost.
        g.rules.economy.reset_match();
        g.num_terrorist_wins = 0;
        g.num_ct_wins = 0;
    }

    for slot in 0..g.players.len() {
        let Some(p) = g.players[slot].as_mut() else {
            continue;
        };
        p.num_spawns = 0;
        p.team_changed = false;
        let (team, paid) = (p.team, !p.receives_no_money_next_round);
        // The players of the teams are paid for the last round.
        if let Some(sum) = g.rules.economy.payout(team)
            && paid
        {
            money::add_account(g, eng, slot as u8, sum, true);
        }
        if team != Team::Unassigned && team != Team::Spectator {
            round_respawn(g, eng, slot as u8);
        }
    }

    // `CleanUpMap` (vanilla runs it after the players are respawned): the armoury starts over,
    // the grenades and every weapon box on the ground go.
    crate::inventory::clean_up_map(g, eng);

    // Reset game variables.
    let rules = &mut g.rules;
    rules.intermission_end_time = 0.0;
    rules.intermission_start_time = 0.0;
    rules.restart_round_time = 0.0;
    rules.economy.clear_accounts();
    rules.round_win_status = WinStatus::None;
    rules.bomb_dropped = false;
    rules.target_bombed = false;
    rules.bomb_defused = false;
    rules.complete_reset = false;
}

/// Hook called when the C4 is planted (`CGrenade::ShootSatchelCharge` / `CC4::PrimaryAttack`).
pub(crate) fn on_bomb_planted(g: &mut GameState, _eng: &mut EngineApi<'_>) {
    g.rules.bomb_dropped = false;
}

/// Hook called when the planted C4 explodes (`CGrenade::Explode2`).
pub(crate) fn on_bomb_exploded(g: &mut GameState, eng: &mut EngineApi<'_>) {
    g.rules.target_bombed = true;
    check_win_conditions(g, eng);
}

/// Hook called when the planted C4 is defused (`CGrenade::DefuseBombEnd`).
pub(crate) fn on_bomb_defused(g: &mut GameState, eng: &mut EngineApi<'_>) {
    g.rules.bomb_defused = true;
    check_win_conditions(g, eng);
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

/// The movement cvars the game forces (`sv_accelerate`, `sv_friction`, `sv_stopspeed`): the
/// values of vanilla `RestartRound` and `Think`. The registry defaults of `sv_accelerate` and
/// `sv_stopspeed` differ (5 and 100: accepted, see `docs/parity/cvars.toml`).
const HARD_LOCKED_CVARS: [(&str, &str); 3] = [
    ("sv_accelerate", "5"),
    ("sv_friction", "4"),
    ("sv_stopspeed", "75"),
];

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
        // `Think` (multiplay_gamerules.cpp 2567-2580, read 2026-11-22): once a second, a
        // movement cvar that is not its locked value is set back.
        for (name, value) in HARD_LOCKED_CVARS {
            if eng.cvar_f32(name) != value.parse::<f32>().unwrap_or(0.0) {
                eng.set_cvar(name, value);
            }
        }
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
    // `buttonsChanged = m_afButtonLast ^ pev->button; m_afButtonPressed = buttonsChanged &
    // pev->button; m_afButtonReleased = buttonsChanged & ~pev->button`.
    let changed = p.last_buttons ^ buttons;
    let pressed = changed & buttons;
    p.last_buttons = buttons;
    p.buttons_pressed = pressed;
    p.buttons_released = changed & !buttons;
    if g.rules.game_over && pressed & INTERMISSION_BUTTONS != 0 {
        g.rules.end_intermission_button_hit = true;
    }
}

/// The `timeleft` command (`ClientCommand_`, ReGameDLL `client.cpp:2851`, read 2026-11-22): at
/// most once a second per player, the chat line `#Game_no_timelimit` when `mp_timelimit` is 0,
/// else `#Game_timelimit` with the minutes and the two-digit seconds left (`GetTimeLeft()`, never
/// below 0, truncated to whole seconds: the match started at [`Rules::game_start_time`]).
pub(crate) fn timeleft_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if now <= f64::from(p.time_check_allowed) {
        return;
    }
    p.time_check_allowed = (now + 1.0) as i32;
    let limit = eng.cvar_f32("mp_timelimit");
    if limit == 0.0 {
        chat::client_print(eng, slot, chat::HUD_PRINTTALK, "#Game_no_timelimit", &[]);
        return;
    }
    let left = (g.rules.game_start_time + f64::from(limit * 60.0) - now) as i32;
    let left = left.max(0);
    let (minutes, seconds) = (left / 60, left % 60);
    chat::client_print(
        eng,
        slot,
        chat::HUD_PRINTTALK,
        "#Game_timelimit",
        &[&minutes.to_string(), &format!("{seconds:02}")],
    );
}

/// The `listmaps` command (`ClientCommand_` `client.cpp:2766`, `CHalfLifeMultiplay::DisplayMaps`
/// with no vote, read 2026-11-22): at most every 0.3 seconds per player, every map of the cycle
/// as a console line `#Votes` with its number, its name and its votes. Voting (`votemap`, `vote`,
/// `mp_mapvoteratio`) is phase 7's, so every map has 0 votes.
pub(crate) fn listmaps_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if now < p.next_listmaps {
        return;
    }
    p.next_listmaps = now + 0.3;
    for (i, item) in g.map_cycle.items.iter().enumerate() {
        chat::client_print(
            eng,
            slot,
            chat::HUD_PRINTCONSOLE,
            "#Votes",
            &[&(i + 1).to_string(), &item.map, "0"],
        );
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
    if !eng.change_level(&map) {
        tracing::warn!(map = %map, "the engine refused the map change");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
