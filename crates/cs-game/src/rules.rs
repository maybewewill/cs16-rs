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
//! Objective win conditions (`VIP`, `Prison`, `Bomb`, `TeamExtermination`, `HostageRescue`),
//! `PickNextVIP`, VIP rotation/balancing, `CheckRoundTimeExpired`, objective rewards, `GiveC4`
//! rotation, and hostage `RePosition` follow vanilla ReGameDLL_CS (`multiplay_gamerules.cpp`).

use cs_engine::consts::SOLID_NOT;
use cs_engine::{EngineApi, GameStatEvent, MsgDest};
use cs_proto48::messages::svc;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::chat::{self, HUD_PRINTTALK};
use crate::entity::GameEntity;
use crate::hud_misc;
use crate::menus::{self, Menu};
use crate::messages;
use crate::money::{self, Economy};
use crate::observer;
use crate::player::{self, DEAD_DEAD, DEAD_NO, DEAD_RESPAWNABLE, PITCH_NORM, Team};
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

/// Competitive `lo3` / `mp_live_on_three` 3-restart state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lo3State {
    #[default]
    Idle,
    Restart1,
    Restart2,
    Restart3,
}

/// Competitive `mp_knife_round` state machine (`.stay` / `.switch` -> `lo3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KnifeRoundState {
    #[default]
    Inactive,
    Active,
    AwaitingChoice { winner: Team },
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
    pub(crate) num_ct: usize,
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
    /// `m_iHostagesRescued`: hostages rescued in the current round.
    pub hostages_rescued: i32,
    /// `m_iHostagesTouched`: hostages first touched by CTs in the current round.
    pub hostages_touched: i32,
    /// `m_iC4Guy`: 1-based rotating index of the terrorist who receives the C4.
    pub(crate) c4_guy: i32,
    /// `m_pVIP`: player slot currently designated as the VIP on `as_` maps.
    pub(crate) vip_slot: Option<u8>,
    /// `m_pVIPQueue[MAX_VIP_QUEUES]`: player slots queued to become the next VIP.
    pub(crate) vip_queue: [Option<u8>; 5],
    /// `m_iConsecutiveVIP`: rounds the current VIP has served consecutively.
    pub(crate) consecutive_vip: i32,
    /// `m_iLastPick`: 1-based index of the CT `PickNextVIP` takes next (starts at 1).
    pub(crate) last_vip_pick: i32,
    /// `m_flRequiredEscapeRatio`: fraction of terrorists that must escape (`0.5`).
    pub(crate) required_escape_ratio: f32,
    /// `m_iNumEscapers`: total terrorists participating in this escape round.
    pub(crate) num_escapers: i32,
    /// `m_iHaveEscaped`: terrorists who have reached `func_escapezone` this round.
    pub(crate) have_escaped: i32,
    /// `m_flEscapeRatio`: computed `have_escaped / num_escapers` ratio.
    pub(crate) escape_ratio: f32,
    /// `m_iNumEscapeRounds`: escape rounds played since the last team swap.
    pub(crate) num_escape_rounds: i32,
    /// `lo3` / `mp_live_on_three` 3-restart state machine.
    pub(crate) lo3_state: Lo3State,
    /// `mp_knife_round` state machine.
    pub(crate) knife_round: KnifeRoundState,
    /// Next server time `sv_showhitboxes` emits debug hitbox boxes.
    pub(crate) next_hitbox_draw: f64,
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
            hostages_rescued: 0,
            hostages_touched: 0,
            c4_guy: 0,
            vip_slot: None,
            vip_queue: [None; 5],
            consecutive_vip: 0,
            last_vip_pick: 1,
            required_escape_ratio: 0.5,
            num_escapers: 0,
            have_escaped: 0,
            escape_ratio: 0.0,
            num_escape_rounds: 0,
            lo3_state: Lo3State::Idle,
            knife_round: KnifeRoundState::Inactive,
            next_hitbox_draw: 0.0,
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

    pub(crate) fn is_restarting(&self) -> bool {
        self.restart_round_time > 0.0
    }

    pub(crate) fn add_ct_account(&mut self, amount: i32) {
        self.economy.add_ct_account(amount);
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

    pub fn lo3_state(&self) -> Lo3State {
        self.lo3_state
    }

    pub fn knife_round(&self) -> KnifeRoundState {
        self.knife_round
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

    /// `m_pVIP`: current VIP slot, if any.
    pub fn vip_slot(&self) -> Option<u8> {
        self.vip_slot
    }

    /// `m_iConsecutiveVIP`: rounds the current VIP has served (10 after the VIP killed itself).
    pub fn consecutive_vip(&self) -> i32 {
        self.consecutive_vip
    }

    /// `m_iNumEscapers`.
    pub fn num_escapers(&self) -> i32 {
        self.num_escapers
    }

    /// `m_iHaveEscaped`.
    pub fn have_escaped(&self) -> i32 {
        self.have_escaped
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
    r.have_escaped = 0;
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
                if p.escaped {
                    r.have_escaped += 1;
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
    if eng.cvar_f32("mp_warmup_mode") != 0.0 {
        return p.menu != Menu::ChooseAppearance;
    }
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
fn terminate_round_with(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    delay: f64,
    status: WinStatus,
    sentence: &str,
    bonus: i32,
) {
    let winner = match status {
        WinStatus::Terrorists => 1,
        WinStatus::Cts => 2,
        WinStatus::None | WinStatus::Draw => 0,
    };
    let mut end_ctx = cs_mod_api::RoundEndCtx {
        winner,
        reason: crate::mod_hooks::round_end_reason_code(sentence),
        bonus,
    };
    if crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
        sink.on_round_end(g, eng, cs_mod_api::HookPhase::Pre, &mut end_ctx)
    }) == cs_mod_api::HookAction::Supercede
    {
        return;
    }
    g.rules.round_win_status = status;
    g.rules.restart_round_time = eng.time() + delay;
    if eng.cvar_f32("sv_damage_print") != 0.0 {
        player::print_all_remaining_round_damage(g, eng);
    }
    let _ = crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
        sink.on_round_end(g, eng, cs_mod_api::HookPhase::Post, &mut end_ctx)
    });
}

fn terminate_round(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64, status: WinStatus) {
    let sentence = match status {
        WinStatus::Cts => "#CTs_Win",
        WinStatus::Terrorists => "#Terrorists_Win",
        WinStatus::Draw | WinStatus::None => "#Round_Draw",
    };
    terminate_round_with(g, eng, delay, status, sentence, 0);
}

fn on_knife_round_winner(g: &mut GameState, eng: &mut EngineApi<'_>, winner: Team) -> bool {
    if g.rules.knife_round != KnifeRoundState::Active {
        return false;
    }
    g.rules.knife_round = KnifeRoundState::AwaitingChoice { winner };
    g.rules.restart_round_time = 0.0;
    let label = match winner {
        Team::Ct => "CT",
        Team::Terrorist => "Terrorists",
        _ => "Winners",
    };
    let msg = format!("[KNIFE] {label} won the knife round! Type .stay or .switch in chat.\n");
    menus::print_all(eng, HUD_PRINTTALK, &msg, &[]);
    menus::print_all(eng, menus::HUD_PRINTCENTER, &msg, &[]);
    true
}

/// Shared helper for winning round outcomes (`Target_Bombed`, `Target_Defused`, `Round_Cts`,
/// `Round_Ts`, `VIP_*`, `Prison_*`, `Hostage_*`, `Target_Saved`).
#[allow(clippy::too_many_arguments)]
fn win_round(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    winner: Team,
    audio: &str,
    reward: i32,
    sentence: &str,
    delay: f64,
    guard_wins: bool,
) {
    broadcast(eng, audio);
    let status = match winner {
        Team::Ct => {
            g.rules.economy.add_ct_account(reward);
            WinStatus::Cts
        }
        Team::Terrorist => {
            g.rules.economy.add_terrorist_account(reward);
            WinStatus::Terrorists
        }
        _ => WinStatus::Draw,
    };
    let inc_win = |g: &mut GameState| match winner {
        Team::Ct => g.num_ct_wins += 1,
        Team::Terrorist => g.num_terrorist_wins += 1,
        _ => {}
    };
    if guard_wins {
        if !g.rules.needed_players {
            inc_win(g);
            update_team_scores(g, eng);
        }
        end_round_message(eng, sentence);
        terminate_round_with(g, eng, delay, status, sentence, reward);
    } else {
        // Vanilla time-expiry outcomes update wins unconditionally and call `UpdateTeamScores`
        // after `TerminateRound` (`multiplay_gamerules.cpp` 2905-3014).
        inc_win(g);
        end_round_message(eng, sentence);
        terminate_round_with(g, eng, delay, status, sentence, reward);
        update_team_scores(g, eng);
    }
    let winner_code = match status {
        WinStatus::Terrorists => 1,
        WinStatus::Cts => 2,
        WinStatus::None | WinStatus::Draw => 0,
    };
    eng.emit_stat_event(GameStatEvent::RoundEnd {
        winner: winner_code,
        reason: sentence.to_string(),
    });
    let _ = on_knife_round_winner(g, eng, winner);
}

/// `Round_Cts`: the counter-terrorists win the round (and are paid for it when the next one
/// starts).
fn round_cts(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!(needed_players = g.rules.needed_players, "round won by the counter-terrorists");
    broadcast(eng, "ctwin");
    g.rules.economy.round_won(Team::Ct, g.rules.map_has_bomb_target);
    if !g.rules.needed_players {
        g.num_ct_wins += 1;
        update_team_scores(g, eng);
    }
    end_round_message(eng, "#CTs_Win");
    terminate_round(g, eng, delay, WinStatus::Cts);
    eng.emit_stat_event(GameStatEvent::RoundEnd {
        winner: 2,
        reason: "#CTs_Win".to_string(),
    });
    let _ = on_knife_round_winner(g, eng, Team::Ct);
}

/// `Round_Ts`: the terrorists win the round.
fn round_ts(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!(needed_players = g.rules.needed_players, "round won by the terrorists");
    broadcast(eng, "terwin");
    g.rules.economy.round_won(Team::Terrorist, g.rules.map_has_bomb_target);
    if !g.rules.needed_players {
        g.num_terrorist_wins += 1;
        update_team_scores(g, eng);
    }
    end_round_message(eng, "#Terrorists_Win");
    terminate_round(g, eng, delay, WinStatus::Terrorists);
    eng.emit_stat_event(GameStatEvent::RoundEnd {
        winner: 1,
        reason: "#Terrorists_Win".to_string(),
    });
    let _ = on_knife_round_winner(g, eng, Team::Terrorist);
}

/// `Round_Draw`: nobody wins the round.
fn round_draw(g: &mut GameState, eng: &mut EngineApi<'_>, delay: f64) {
    tracing::info!("round is a draw");
    end_round_message(eng, "#Round_Draw");
    broadcast(eng, "rounddraw");
    terminate_round(g, eng, delay, WinStatus::Draw);
    eng.emit_stat_event(GameStatEvent::RoundEnd {
        winner: 0,
        reason: "#Round_Draw".to_string(),
    });
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
    eng.emit_stat_event(GameStatEvent::RoundEnd {
        winner: 0,
        reason: "#Game_Commencing".to_string(),
    });
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

/// `VIPRoundEndCheck` (`multiplay_gamerules.cpp` 1157-1174): VIP escaped or assassinated on maps
/// with a `func_vip_safetyzone`.
fn vip_round_end_check(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    let Some(vip) = g.rules.vip_slot else {
        return false;
    };
    if !g.rules.map_has_vip_safety_zone {
        return false;
    }
    let escaped = g
        .players
        .get(usize::from(vip))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.escaped);
    if escaped {
        win_round(
            g,
            eng,
            Team::Ct,
            "ctwin",
            money::REWARD_VIP_ESCAPED,
            "#VIP_Escaped",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    let dead = eng
        .ent(eng.client_edict(vip))
        .is_none_or(|v| v.deadflag != DEAD_NO);
    if dead {
        win_round(
            g,
            eng,
            Team::Terrorist,
            "terwin",
            money::REWARD_VIP_ASSASSINATED,
            "#VIP_Assassinated",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    false
}

/// `PrisonRoundEndCheck` (`multiplay_gamerules.cpp` 1247-1270): escape ratio check on `es_` maps.
fn prison_round_end_check(g: &mut GameState, eng: &mut EngineApi<'_>, c: &Counts) -> bool {
    if !g.rules.map_has_escape_zone {
        return false;
    }
    // Note: vanilla computes `real_t(m_iHaveEscaped) / real_t(m_iNumEscapers)`; 0/0 yields `NaN`,
    // making the `<` and `>=` comparisons false as in IEEE 754 C++.
    let ratio = (g.rules.have_escaped as f32) / (g.rules.num_escapers as f32);
    g.rules.escape_ratio = ratio;
    if ratio >= g.rules.required_escape_ratio {
        win_round(
            g,
            eng,
            Team::Terrorist,
            "terwin",
            money::REWARD_TERRORISTS_ESCAPED,
            "#Terrorists_Escaped",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    if c.alive_terrorist == 0 && ratio < g.rules.required_escape_ratio {
        let reward = ((1.0 - ratio) * money::REWARD_CTS_PREVENT_ESCAPE as f32) as i32;
        win_round(
            g,
            eng,
            Team::Ct,
            "ctwin",
            reward,
            "#CTs_PreventEscape",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    if c.alive_terrorist == 0 && c.dead_terrorist != 0 && g.rules.num_spawnable_ct > 0 {
        let reward = ((1.0 - ratio) * money::REWARD_ESCAPING_TERRORISTS_NEUTRALIZED as f32) as i32;
        win_round(
            g,
            eng,
            Team::Ct,
            "ctwin",
            reward,
            "#Escaping_Terrorists_Neutralized",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    false
}

/// `BombRoundEndCheck` (`multiplay_gamerules.cpp` 1327-1340): target bombed or bomb defused.
fn bomb_round_end_check(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    if g.rules.target_bombed && g.rules.map_has_bomb_target {
        win_round(
            g,
            eng,
            Team::Terrorist,
            "terwin",
            money::REWARD_TARGET_BOMB,
            "#Target_Bombed",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    if g.rules.bomb_defused && g.rules.map_has_bomb_target {
        g.rules.economy.add_terrorist_account(money::REWARD_BOMB_PLANTED);
        win_round(
            g,
            eng,
            Team::Ct,
            "ctwin",
            money::REWARD_BOMB_DEFUSED,
            "#Bomb_Defused",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    false
}

/// `TeamExterminationCheck`: a team with no player alive loses the round; with no player alive
/// on either side the round is a draw. While a planted live C4 ticks (`!m_bJustBlew`), CTs do not
/// win by eliminating all terrorists.
fn team_extermination_check(g: &mut GameState, eng: &mut EngineApi<'_>, c: &Counts) -> bool {
    if eng.cvar_f32("mp_warmup_mode") != 0.0 {
        return false;
    }
    let r = &g.rules;
    let both_play = r.num_ct > 0
        && r.num_spawnable_ct > 0
        && r.num_terrorist > 0
        && r.num_spawnable_terrorist > 0;
    if both_play {
        if c.alive_terrorist == 0 && c.dead_terrorist != 0 && c.alive_ct > 0 {
            let mut nowin = false;
            let mut cursor = None;
            while let Some(id) = eng.find_by_classname(cursor, "grenade") {
                cursor = Some(id);
                if eng
                    .private::<Box<dyn GameEntity>>(id)
                    .is_some_and(|e| e.is_planted_live_c4())
                {
                    nowin = true;
                }
            }
            if !nowin {
                round_cts(g, eng, ROUND_BEGIN_DELAY);
                return true;
            }
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

/// `HostageRescueRoundEndCheck` (`multiplay_gamerules.cpp` 1473-1510, vanilla): ends in CT win
/// when all hostages are either dead or rescued and at least half were rescued.
fn hostage_rescue_round_end_check(g: &mut GameState, eng: &mut EngineApi<'_>) -> bool {
    let mut hostages_count = 0i32;
    let mut any_alive = false;
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "hostage_entity") {
        cursor = Some(id);
        hostages_count += 1;
        if eng
            .private::<Box<dyn GameEntity>>(id)
            .is_some_and(|e| e.is_alive(eng, id))
        {
            any_alive = true;
        }
    }
    if !any_alive
        && hostages_count > 0
        && (g.rules.hostages_rescued as f32) >= (hostages_count as f32) * 0.5
    {
        win_round(
            g,
            eng,
            Team::Ct,
            "ctwin",
            money::REWARD_ALL_HOSTAGES_RESCUED,
            "#All_Hostages_Rescued",
            ROUND_BEGIN_DELAY,
            true,
        );
        return true;
    }
    false
}

/// `CheckWinConditions` (`multiplay_gamerules.cpp` 884-943, vanilla): called whenever a player's
/// or objective's state changes. Checks NeededPlayers, VIP, PrisonEscape, Bomb, TeamExtermination,
/// and HostageRescue in exact source order.
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
    // Assassination / VIP scenario check.
    if vip_round_end_check(g, eng) {
        return;
    }
    // Prison escape check.
    if prison_round_end_check(g, eng, &counts) {
        return;
    }
    // Bomb check.
    if bomb_round_end_check(g, eng) {
        return;
    }
    // Team extermination check.
    if team_extermination_check(g, eng, &counts) {
        return;
    }
    // Hostage rescue check.
    let _ = hostage_rescue_round_end_check(g, eng);
}

/// `CHalfLifeMultiplay::ClientDisconnected` (ReGameDLL `multiplay_gamerules.cpp:3580`, read
/// 2026-11-22), called by `ClientDisconnect` after it made the player dormant: the player is
/// `DEAD_DEAD` and `SetScoreboardAttributes()` tells everyone; then a carried C4 is dropped (the
/// defuser is not: vanilla finds nothing to drop), the VIP slot is released
/// ([`crate::vip::on_disconnect`]: only `m_pVIP = nullptr`; the votes are not ported), its
/// scoreboard line is emptied (`ScoreInfo` with nothing,
/// `TeamInfo` `UNASSIGNED`), the observers that followed it move on ([`observer::on_target_left`])
/// and `CheckWinConditions` runs last. The player is dormant, so `InitializePlayerCounts` no longer
/// counts it: here it is removed from [`GameState::players`] before the check. (The empty
/// `Location` message and `game_playerleave` are not ported.)
pub(crate) fn on_disconnect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let has_c4 = if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        p.alive = false;
        p.has_c4
    } else {
        false
    };
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.deadflag = player::DEAD_DEAD;
    }
    messages::set_scoreboard_attributes(g, eng, usize::from(slot));
    // Vanilla (`!REGAMEDLL_FIXES`): drops carried C4, then tries `DropPlayerItem("item_thighpack")`
    // which finds nothing (only `REGAMEDLL_FIXES` spawns a defuser on disconnect).
    if has_c4 {
        crate::inventory::drop_player_item(g, eng, slot, Some("weapon_c4"));
    }
    if let Some(entry) = g.players.get_mut(usize::from(slot)) {
        *entry = None;
    }
    if usize::from(slot) < g.map_votes.len() {
        g.map_votes[usize::from(slot)] = None;
        g.kick_votes[usize::from(slot)] = None;
    }
    for v in &mut g.kick_votes {
        if *v == Some(slot) {
            *v = None;
        }
    }
    crate::vip::on_disconnect(g, slot);
    g.kz.reset_player(slot);
    g.custom_weapons.clear_player(slot);
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

/// `OnRoundFreezeEnd` (`multiplay_gamerules.cpp` 2776-2881, read 2026-11-23): the round starts.
/// Nobody is frozen any more, the round clock starts counting down the round time, and the first
/// joined CT and first joined T call out their scenario start sentence over the radio.
pub(crate) fn on_round_freeze_end(g: &mut GameState, eng: &mut EngineApi<'_>) {
    if crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
        sink.on_freeze_time_end(g, eng, cs_mod_api::HookPhase::Pre)
    }) == cs_mod_api::HookAction::Supercede
    {
        return;
    }
    // Freeze period expired: kill the flag.
    g.rules.freeze_period = false;
    let default_sentence = match g.random_range(0, 3) {
        0 => "%!MRAD_MOVEOUT",
        1 => "%!MRAD_LETSGO",
        2 => "%!MRAD_LOCKNLOAD",
        _ => "%!MRAD_GO",
    };
    let (ct_sentence, t_sentence) = if g.rules.map_has_escape_zone {
        ("%!MRAD_ELIM", "%!MRAD_GETOUT")
    } else if g.rules.map_has_vip_safety_zone {
        ("%!MRAD_VIP", "%!MRAD_LOCKNLOAD")
    } else {
        (default_sentence, default_sentence)
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
                hud_misc::radio(g, eng, slot, ct_sentence, "");
            } else if p.team == Team::Terrorist && !t_played {
                t_played = true;
                hud_misc::radio(g, eng, slot, t_sentence, "");
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
    let _ = crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
        sink.on_freeze_time_end(g, eng, cs_mod_api::HookPhase::Post)
    });
}

/// `CHalfLifeMultiplay::IsBombPlanted` (`multiplay_gamerules.cpp` 3196-3212): true when the map
/// has a bomb target and a planted C4 (`grenade` with `m_bIsC4`) exists.
pub(crate) fn is_bomb_planted(g: &GameState, eng: &EngineApi<'_>) -> bool {
    if !g.rules.map_has_bomb_target {
        return false;
    }
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "grenade") {
        cursor = Some(id);
        if eng
            .private::<Box<dyn GameEntity>>(id)
            .is_some_and(|e| e.is_planted_c4())
        {
            return true;
        }
    }
    false
}

/// `CHalfLifeMultiplay::MarkLivingPlayersOnTeamAsNotReceivingMoneyNextRound` (`multiplay_gamerules.cpp`
/// 3214-3231): living players on `team` forfeit round bonus money at the next `RestartRound`.
fn mark_living_players_no_money(g: &mut GameState, eng: &EngineApi<'_>, team: Team) {
    for (slot, slot_p) in g.players.iter_mut().enumerate() {
        let Some(p) = slot_p.as_mut() else {
            continue;
        };
        if p.team != team {
            continue;
        }
        let id = eng.client_edict(slot as u8);
        if eng
            .ent(id)
            .is_some_and(|v| v.health > 0.0 && v.deadflag == DEAD_NO)
        {
            p.receives_no_money_next_round = true;
        }
    }
}

/// `CheckRoundTimeExpired` (`multiplay_gamerules.cpp` 3027-3112): when the round clock runs out
/// (and no planted C4 is holding the round open), the objective of the map decides who wins:
/// bomb saved -> hostages not rescued -> escape prevented -> VIP not escaped. On a map with no
/// objective the clock moves forward by 60 seconds.
fn check_round_time_expired(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let now = eng.time();
    if g.rules.round_remaining(now) > 0.0
        || g.rules.round_win_status != WinStatus::None
        || is_bomb_planted(g, eng)
    {
        return;
    }
    if g.rules.map_has_bomb_target {
        win_round(
            g,
            eng,
            Team::Ct,
            "ctwin",
            money::REWARD_TARGET_BOMB_SAVED,
            "#Target_Saved",
            ROUND_BEGIN_DELAY,
            false,
        );
        mark_living_players_no_money(g, eng, Team::Terrorist);
    } else if eng.find_by_classname(None, "hostage_entity").is_some() {
        win_round(
            g,
            eng,
            Team::Terrorist,
            "terwin",
            money::REWARD_HOSTAGE_NOT_RESCUED,
            "#Hostages_Not_Rescued",
            ROUND_BEGIN_DELAY,
            false,
        );
        mark_living_players_no_money(g, eng, Team::Ct);
    } else if g.rules.map_has_escape_zone {
        broadcast(eng, "ctwin");
        g.num_ct_wins += 1;
        end_round_message(eng, "#Terrorists_Not_Escaped");
        terminate_round(g, eng, ROUND_BEGIN_DELAY, WinStatus::Cts);
        update_team_scores(g, eng);
    } else if g.rules.map_has_vip_safety_zone {
        win_round(
            g,
            eng,
            Team::Terrorist,
            "terwin",
            money::REWARD_VIP_NOT_ESCAPED,
            "#VIP_Not_Escaped",
            ROUND_BEGIN_DELAY,
            false,
        );
    }
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
    if g.players[usize::from(slot)]
        .as_ref()
        .is_some_and(|p| p.has_defuser)
    {
        player::remove_defuser(g, eng, slot);
    }
}

/// Gives C4 to a specific living player slot (`CBasePlayer::MakeBomber`).
#[cfg(feature = "test-hooks")]
pub(crate) fn give_c4(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    player::make_bomber(g, eng, slot)
}

/// `CHalfLifeMultiplay::IsThereABomber` (`multiplay_gamerules.cpp` 2101-2123): true when any
/// non-CT player currently carries C4 (`m_bHasC4`).
pub(crate) fn is_there_a_bomber(g: &GameState) -> bool {
    g.players
        .iter()
        .flatten()
        .any(|p| p.team != Team::Ct && p.has_c4)
}

/// `CHalfLifeMultiplay::IsThereABomb` (`multiplay_gamerules.cpp` 2125-2143): true when a planted
/// C4 (`grenade` with `m_bIsC4`), a world `weapon_c4`, or a boxed dropped C4 exists.
pub(crate) fn is_there_a_bomb(eng: &EngineApi<'_>) -> bool {
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "grenade") {
        cursor = Some(id);
        if eng
            .private::<Box<dyn GameEntity>>(id)
            .is_some_and(|e| e.is_planted_c4())
        {
            return true;
        }
    }
    if eng.find_by_classname(None, "weapon_c4").is_some() {
        return true;
    }
    let mut box_cursor = None;
    while let Some(id) = eng.find_by_classname(box_cursor, "weaponbox") {
        box_cursor = Some(id);
        if eng
            .private::<Box<dyn GameEntity>>(id)
            .is_some_and(|e| e.is_dropped_bomb())
        {
            return true;
        }
    }
    false
}

/// `CHalfLifeMultiplay::GiveC4` (`multiplay_gamerules.cpp` 690-785, vanilla): hands C4 to the
/// next living terrorist in rotating index order (`m_iC4Guy`), falling back to the first living
/// terrorist if none received it.
pub(crate) fn give_c4_to_terrorists(g: &mut GameState, eng: &mut EngineApi<'_>) -> Option<u8> {
    let team_count = g.rules.num_terrorist as i32;
    g.rules.c4_guy += 1;
    if g.rules.c4_guy > team_count {
        g.rules.c4_guy = 1;
    }

    let mut temp = 0i32;
    let mut chosen = None;
    for slot in 0..g.players.len() {
        let slot_u8 = slot as u8;
        let living_t = g.players[slot]
            .as_ref()
            .is_some_and(|p| p.team == Team::Terrorist)
            && eng
                .ent(eng.client_edict(slot_u8))
                .is_some_and(|v| v.deadflag == DEAD_NO);
        if !living_t {
            continue;
        }
        temp += 1;
        if temp == g.rules.c4_guy && player::make_bomber(g, eng, slot_u8) {
            chosen = Some(slot_u8);
            break;
        }
    }

    if !is_there_a_bomber(g) {
        g.rules.c4_guy = 0;
        for slot in 0..g.players.len() {
            let slot_u8 = slot as u8;
            let living_t = g.players[slot]
                .as_ref()
                .is_some_and(|p| p.team == Team::Terrorist)
                && eng
                    .ent(eng.client_edict(slot_u8))
                    .is_some_and(|v| v.deadflag == DEAD_NO);
            if living_t && player::make_bomber(g, eng, slot_u8) {
                return Some(slot_u8);
            }
        }
    }
    chosen
}

/// `CHalfLifeMultiplay::SwapAllPlayers` (`multiplay_gamerules.cpp` 1512-1539): switches every
/// player's team and swaps CT/T team scores.
fn swap_all_players(g: &mut GameState, eng: &mut EngineApi<'_>) {
    for slot in 0..g.players.len() {
        if g.players[slot].is_some() {
            switch_team(g, eng, slot as u8);
        }
    }
    std::mem::swap(&mut g.num_terrorist_wins, &mut g.num_ct_wins);
    update_team_scores(g, eng);
}

/// `CBasePlayer::CanSwitchTeam`: a player of the team that must give players away (never the VIP).
fn can_switch_team(g: &GameState, slot: usize, from: Team) -> bool {
    g.players[slot]
        .as_ref()
        .is_some_and(|p| p.team == from && !p.is_vip)
}

/// `BalanceTeams`: moves the most recently joined players of the larger team to the other until
/// balanced (equal +/- 1 normally, or 0.55 CT target ratio on VIP maps), at most
/// [`MAX_BALANCE_SWAPS`] of them (`multiplay_gamerules.cpp` 1551-1595).
fn balance_teams(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let (cts, ts) = (g.rules.num_ct, g.rules.num_terrorist);
    let (from, swaps) = if g.rules.map_has_vip_safety_zone {
        let total = cts + ts;
        let desired_ct = if total % 2 != 0 {
            ((total as f32 * 0.55) as usize) + 1
        } else {
            total / 2
        };
        let desired_t = total.saturating_sub(desired_ct);
        if cts < desired_ct {
            (Team::Terrorist, desired_ct - cts)
        } else if ts < desired_t {
            (Team::Ct, desired_t - ts)
        } else {
            // Teams are balanced for a VIP map: get out of here.
            return;
        }
    } else if cts > ts {
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
    let mut round_ctx = cs_mod_api::RoundStartCtx {
        round_num: g.rules.total_rounds_played.max(0) as u32 + 1,
    };
    if crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
        sink.on_round_start(g, eng, cs_mod_api::HookPhase::Pre, &mut round_ctx)
    }) == cs_mod_api::HookAction::Supercede
    {
        return;
    }
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

    // Reset the dropped bomb on everyone's radar and show the round timer on bomb maps.
    if g.rules.map_has_bomb_target {
        eng.send_usermsg(MsgDest::All, &CsUserMsg::BombPickup);
        eng.send_usermsg(MsgDest::All, &CsUserMsg::ShowTimer);
    }
    g.rules.bomb_dropped = false;

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
    // (`RestartRound`, multiplay_gamerules.cpp 1853-1901, vanilla).
    crate::zones::check_map_info(g);
    g.rules.check_map_conditions(eng);

    if g.rules.map_has_escape_zone {
        g.rules.num_escapers = 0;
        g.rules.have_escaped = 0;
        if g.rules.num_escape_rounds >= 3 {
            swap_all_players(g, eng);
            g.rules.num_escape_rounds = 0;
        }
        g.rules.num_escape_rounds += 1;
    }

    if g.rules.map_has_vip_safety_zone {
        crate::vip::pick_next_vip(g, eng);
        g.rules.consecutive_vip += 1;
    }

    // Surviving hostage bonus (`mp:1880-1901`) and `CHostage::RePosition` (`UTIL_RestartOther`).
    let mut acct_tmp = 0i32;
    let mut hostage_ids = Vec::new();
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "hostage_entity") {
        hostage_ids.push(id);
        cursor = Some(id);
    }
    for id in hostage_ids {
        if acct_tmp >= 2000 {
            break;
        }
        if let Some(v) = eng.ent(id)
            && v.solid != SOLID_NOT
        {
            acct_tmp += money::REWARD_TOOK_HOSTAGE;
            if v.deadflag == DEAD_DEAD
                && let Some(vm) = eng.ent_mut(id)
            {
                vm.deadflag = DEAD_RESPAWNABLE;
            }
        }
        crate::entity::with_entity(g, eng, id, |entity, g, eng| {
            entity.restart(g, eng, id);
        });
    }

    // Scale up the loser bonus when teams fall into losing streaks, and assign it.
    let status = g.rules.round_win_status;
    let rescued_hostages = g.rules.hostages_rescued;
    let is_escape_map = g.rules.map_has_escape_zone;
    g.rules
        .economy
        .settle(status, acct_tmp, rescued_hostages, is_escape_map);
    // The round time stamp must be set before the players are spawned.
    g.rules.round_start_time = now;
    if g.rules.complete_reset {
        // No cash for anyone at first rounds (only the starting money), and it is like no one
        // has ever won or lost.
        g.rules.economy.reset_match();
        g.num_terrorist_wins = 0;
        g.num_ct_wins = 0;
    }
    let round_num = g.rules.total_rounds_played.max(0) as u32 + 1;
    eng.emit_stat_event(GameStatEvent::RoundStart {
        round_number: round_num,
    });

    for slot in 0..g.players.len() {
        let Some(p) = g.players[slot].as_mut() else {
            continue;
        };
        p.num_spawns = 0;
        p.team_changed = false;
        let (team, paid, has_c4) = (p.team, !p.receives_no_money_next_round, p.has_c4);
        if team == Team::Terrorist && is_escape_map {
            // `RestartRound` (`mp:2026-2030`): on escape maps, terrorists start each round with
            // default gear ("remove the Ts guns").
            p.not_killed = false;
        }
        if team == Team::Terrorist {
            g.rules.num_escapers += 1;
        }
        // The players of the teams are paid for the last round.
        if let Some(sum) = g.rules.economy.payout(team)
            && paid
        {
            money::add_account(g, eng, slot as u8, sum, true);
        }
        if team != Team::Unassigned && team != Team::Spectator {
            // Vanilla (`!REGAMEDLL_FIXES`, `multiplay_gamerules.cpp:2042`): drop carried C4
            // before respawning; `clean_up_map` removes the box immediately afterward.
            if has_c4 {
                crate::inventory::drop_player_item(g, eng, slot as u8, Some("weapon_c4"));
            }
            round_respawn(g, eng, slot as u8);
        }
    }

    // `CleanUpMap`: the armoury starts over and every grenade / weapon box / thighpack goes.
    crate::bots::on_round_restart(g);
    crate::inventory::clean_up_map(g, eng);

    if g.rules.map_has_bomb_target {
        give_c4_to_terrorists(g, eng);
    }

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
    rules.hostages_rescued = 0;
    rules.hostages_touched = 0;
    rules.complete_reset = false;
    g.round_damage = [[crate::RoundDamageEntry::default(); crate::player::MAX_PLAYERS];
        crate::player::MAX_PLAYERS];
    g.damage_printed = [false; crate::player::MAX_PLAYERS];
    advance_lo3_on_restart(g, eng);
    round_ctx.round_num = g.rules.total_rounds_played.max(0) as u32;
    let _ = crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
        sink.on_round_start(g, eng, cs_mod_api::HookPhase::Post, &mut round_ctx)
    });
}

/// Starts the competitive `lo3` / `mp_live_on_three` 3-restart scrim sequence.
pub(crate) fn start_lo3(g: &mut GameState, eng: &mut EngineApi<'_>) {
    g.rules.lo3_state = Lo3State::Restart1;
    let banner = "[LIVE] --- LIVE ON THREE RESTARTS ---\n";
    menus::print_all(eng, HUD_PRINTTALK, banner, &[]);
    menus::print_all(eng, menus::HUD_PRINTCENTER, banner, &[]);
    menus::print_all(eng, menus::HUD_PRINTCONSOLE, banner, &[]);
    restart_round_check(g, eng, 1.0);
}

fn advance_lo3_on_restart(g: &mut GameState, eng: &mut EngineApi<'_>) {
    match g.rules.lo3_state {
        Lo3State::Idle => {}
        Lo3State::Restart1 => {
            g.rules.lo3_state = Lo3State::Restart2;
            menus::print_all(eng, HUD_PRINTTALK, "[LIVE] Restart #1\n", &[]);
            restart_round_check(g, eng, 1.0);
        }
        Lo3State::Restart2 => {
            g.rules.lo3_state = Lo3State::Restart3;
            menus::print_all(eng, HUD_PRINTTALK, "[LIVE] Restart #2\n", &[]);
            restart_round_check(g, eng, 3.0);
        }
        Lo3State::Restart3 => {
            g.rules.lo3_state = Lo3State::Idle;
            let live = "[LIVE] GL & HF! MATCH IS LIVE!\n";
            menus::print_all(eng, HUD_PRINTTALK, live, &[]);
            menus::print_all(eng, menus::HUD_PRINTCENTER, live, &[]);
            menus::print_all(eng, menus::HUD_PRINTCONSOLE, live, &[]);
        }
    }
}

/// Handles `.stay` / `.switch` chat commands from the winning team after `mp_knife_round`.
pub(crate) fn handle_knife_round_chat(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    text: &str,
) -> bool {
    let KnifeRoundState::AwaitingChoice { winner } = g.rules.knife_round else {
        return false;
    };
    let Some(team) = g.players.get(usize::from(slot)).and_then(Option::as_ref).map(|p| p.team) else {
        return false;
    };
    if team != winner {
        return false;
    }
    let cmd = text.trim();
    if cmd.eq_ignore_ascii_case(".stay") {
        menus::print_all(
            eng,
            HUD_PRINTTALK,
            "[KNIFE] Winners chose .stay! Starting LO3...\n",
            &[],
        );
    } else if cmd.eq_ignore_ascii_case(".switch") {
        for s in 0..crate::player::MAX_PLAYERS {
            switch_team(g, eng, s as u8);
        }
        menus::print_all(
            eng,
            HUD_PRINTTALK,
            "[KNIFE] Winners chose .switch! Switching sides & starting LO3...\n",
            &[],
        );
    } else {
        return false;
    }
    g.rules.knife_round = KnifeRoundState::Inactive;
    eng.set_cvar("mp_knife_round", "0");
    start_lo3(g, eng);
    true
}

/// Hook called when a hostage is rescued (`CHostage::IdleThink`).
pub(crate) fn on_hostage_rescued(g: &mut GameState, eng: &mut EngineApi<'_>) {
    check_win_conditions(g, eng);
}

/// Hook called when a hostage is killed (`CHostage::TakeDamage`).
pub(crate) fn on_hostage_killed(g: &mut GameState, eng: &mut EngineApi<'_>) {
    check_win_conditions(g, eng);
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
pub(crate) fn go_to_intermission(g: &mut GameState, eng: &mut EngineApi<'_>) {
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
    if eng.cvar_f32("mp_knife_round") != 0.0 && g.rules.knife_round == KnifeRoundState::Inactive {
        g.rules.knife_round = KnifeRoundState::Active;
        g.rules.complete_reset = true;
        let _ = initialize_player_counts(g);
        if g.rules.num_spawnable_ct > 0 && g.rules.num_spawnable_terrorist > 0 {
            g.rules.game_started = true;
            g.rules.needed_players = false;
        }
        let msg = "[KNIFE] Knife round started! Winner chooses .stay or .switch\n";
        menus::print_all(eng, HUD_PRINTTALK, msg, &[]);
        menus::print_all(eng, menus::HUD_PRINTCENTER, msg, &[]);
        restart_round(g, eng);
    }
    // Check game rules.
    if check_game_over(g, eng)
        || check_time_limit(g, eng)
        || check_max_rounds(g, eng)
        || check_win_limit(g, eng)
    {
        return;
    }
    if eng.cvar_f32("mp_warmup_mode") != 0.0 {
        let to_respawn: Vec<u8> = g
            .players
            .iter()
            .enumerate()
            .filter_map(|(slot, p)| {
                let p = p.as_ref()?;
                (!p.alive
                    && !matches!(p.team, Team::Unassigned | Team::Spectator)
                    && p.menu != Menu::ChooseAppearance
                    && now >= p.dead_time + 0.5)
                    .then_some(slot as u8)
            })
            .collect();
        for slot in to_respawn {
            let _ = player::respawn_player(g, eng, slot);
        }
    }
    // Check for the end of the round.
    if g.rules.freeze_period {
        if eng.cvar_f32("mp_warmup_mode") != 0.0 {
            on_round_freeze_end(g, eng);
        } else {
            check_freeze_period_expired(g, eng);
        }
    } else {
        check_round_time_expired(g, eng);
    }
    if g.rules.restart_round_time > 0.0
        && g.rules.restart_round_time <= now
        && !matches!(g.rules.knife_round, KnifeRoundState::AwaitingChoice { .. })
    {
        restart_round(g, eng);
    }
    if eng.cvar_f32("sv_cheats") != 0.0
        && eng.cvar_f32("sv_showhitboxes") != 0.0
        && now >= g.rules.next_hitbox_draw
    {
        g.rules.next_hitbox_draw = now + 0.1;
        emit_hitbox_debug(g, eng);
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
        let timelimit = eng.cvar_f32("mp_timelimit");
        let time_left = if timelimit.is_finite() && timelimit > 0.0 {
            let total = f64::from(timelimit) * 60.0;
            (total - (now - g.rules.game_start_time)).max(0.0).trunc() as i32
        } else {
            0
        };
        eng.set_cvar("mp_timeleft", &time_left.to_string());
        let fraglimit = eng.cvar_f32("mp_fraglimit");
        let frags_left = if fraglimit.is_finite() && fraglimit > 0.0 {
            let max_frags = g
                .players
                .iter()
                .flatten()
                .map(|p| p.frags)
                .max()
                .unwrap_or(0);
            ((fraglimit as i32) - max_frags).max(0)
        } else {
            0
        };
        eng.set_cvar("mp_fragsleft", &frags_left.to_string());
    }
}

fn emit_hitbox_debug(g: &GameState, eng: &mut EngineApi<'_>) {
    let mut players_drawn = 0usize;
    for (slot, slot_player) in g.players.iter().enumerate().take(crate::player::MAX_PLAYERS) {
        if players_drawn >= 4 {
            break;
        }
        let Some(p) = slot_player.as_ref() else {
            continue;
        };
        if !p.alive {
            continue;
        }
        let id = eng.client_edict(slot as u8);
        if eng.ent(id).is_none_or(|v| v.deadflag != DEAD_NO) {
            continue;
        }
        if let Some(boxes) = eng.studio_hitbox_boxes(id) {
            players_drawn += 1;
            for (group, corners) in boxes {
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for c in corners {
                    min = min.min(c);
                    max = max.max(c);
                }
                let color = match group {
                    1 => [255, 40, 40],
                    2 | 3 => [255, 140, 30],
                    4 | 5 => [255, 240, 50],
                    6 | 7 => [60, 230, 80],
                    _ => [200, 200, 200],
                };
                let te = cs_proto48::tempent::box_fx(min, max, 1, color);
                eng.send_temp_entity(MsgDest::Broadcast, &te);
            }
        }
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
        let idx = i + 1;
        let votes = g.map_votes.iter().filter(|&&v| v == Some(idx)).count();
        chat::client_print(
            eng,
            slot,
            chat::HUD_PRINTCONSOLE,
            "#Votes",
            &[&idx.to_string(), &item.map, &votes.to_string()],
        );
    }
}

/// The `votemap <index|mapname>` command (`CHalfLifeMultiplay::ProcessClientCommand` + `CountMapVotes`).
pub(crate) fn votemap_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
) {
    let Some(arg) = argv.get(1) else {
        listmaps_command(g, eng, slot);
        return;
    };
    let ratio = eng.cvar_f32("mp_mapvoteratio");
    if ratio <= 0.0 {
        eng.client_print(slot, "Voting is disabled on this server.");
        return;
    }
    let choice = if let Ok(num) = arg.parse::<usize>()
        && (1..=g.map_cycle.items.len()).contains(&num)
    {
        Some(num)
    } else {
        g.map_cycle
            .items
            .iter()
            .position(|m| m.map.eq_ignore_ascii_case(arg))
            .map(|idx| idx + 1)
    };
    let Some(map_num) = choice else {
        eng.client_print(slot, "Map not found in mapcycle.");
        return;
    };
    g.map_votes[usize::from(slot)] = Some(map_num);
    let voters = g.players.iter().flatten().count();
    let needed = ((voters as f32 * ratio.clamp(0.0, 1.0)).ceil() as usize).max(1);
    let tally = g.map_votes.iter().filter(|&&v| v == Some(map_num)).count();
    let map_name = g.map_cycle.items[map_num - 1].map.clone();
    eng.client_print(
        slot,
        &format!("Voted for {map_name} ({tally}/{needed} needed)."),
    );
    if tally >= needed {
        eng.print_all(&format!("Map vote passed: changing to {map_name}."));
        g.map_votes = [None; player::MAX_PLAYERS];
        let _ = eng.change_level(&map_name);
    }
}

/// The `vote <slot>` kick vote command (teammate vote against `mp_kickpercent`).
pub(crate) fn vote_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
) {
    let Some(voter_team) = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| p.team)
    else {
        return;
    };
    let Some(arg) = argv.get(1) else {
        for (i, entry) in g.players.iter().enumerate() {
            if let Some(p) = entry
                && p.team == voter_team
            {
                let name = eng.client_key(i as u8, "name").unwrap_or("Player");
                eng.client_print(slot, &format!("{} : {name}", i + 1));
            }
        }
        return;
    };
    let Ok(one_based) = arg.parse::<usize>() else {
        return;
    };
    if one_based == 0 || one_based > eng.max_clients() as usize {
        return;
    }
    let target_slot = (one_based - 1) as u8;
    let Some(target) = g.players[usize::from(target_slot)].as_ref() else {
        return;
    };
    if target.team != voter_team {
        return;
    }
    let ratio = eng.cvar_f32("mp_kickpercent").clamp(0.0, 1.0);
    if ratio <= 0.0 {
        eng.client_print(slot, "Kick voting is disabled.");
        return;
    }
    g.kick_votes[usize::from(slot)] = Some(target_slot);
    let team_count = g
        .players
        .iter()
        .flatten()
        .filter(|p| p.team == voter_team)
        .count();
    let needed = ((team_count as f32 * ratio).ceil() as usize).max(1);
    let tally = g
        .kick_votes
        .iter()
        .filter(|&&v| v == Some(target_slot))
        .count();
    if tally >= needed {
        for v in &mut g.kick_votes {
            if *v == Some(target_slot) {
                *v = None;
            }
        }
        eng.kick(target_slot, "Kicked by team vote");
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
