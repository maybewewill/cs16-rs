//! Teams and the way into the game: the joining states (`m_iJoiningState`), the team and
//! appearance menus and their handlers (`HandleMenu_ChooseTeam`, `HandleMenu_ChooseAppearance`),
//! the `jointeam`, `joinclass` and `chooseteam` commands, the team limits (`TeamFull`,
//! `TeamStacked`, `SelectDefaultTeam`) and `TeamInfo`. Ported from ReGameDLL_CS (`client.cpp`,
//! `player.cpp`, `multiplay_gamerules.cpp`, `client.h`, `player.h`, read 2026-11-22), the vanilla
//! `mp.dll` behaviour (not the `REGAMEDLL_FIXES`/`REGAMEDLL_ADD` builds, with the deviations
//! listed below).
//!
//! A player enters [`JoinState::ShowLText`], goes on to [`JoinState::ShowTeamSelect`] (the
//! level text is not shown), gets the team menu and waits in [`JoinState::PickingTeam`] until it
//! has picked a team and an appearance; then [`JoinState::GetIntoGame`] spawns it, or, when the
//! rules do not let it spawn yet ([`rules::can_respawn`]), leaves it watching. [`pre_think`] runs the
//! per-player part of `PlayerThink` and `JoiningThink` from the player's commands, as the
//! original does from `PlayerPreThink`: the engine only runs commands of a client that is in
//! the game, so no menu is sent to a client still signing on.
//!
//! Deviations from vanilla:
//! - A random team (`jointeam 5`) when both teams are full is refused (the `REGAMEDLL_FIXES`
//!   behaviour); vanilla lets the player through with no team (UNASSIGNED) and the menu closed.
//! - The team counts (`m_iNumCT`, `m_iNumTerrorist`) are taken from the players when asked,
//!   not from the cache `CheckWinConditions` refreshes.
//!
//! Left for later tasks: the VIP (`jointeam 3` is refused: no map has a VIP safety zone yet), the items a player
//! drops or loses on a team change, the `Location` message, the career mode and the bots. `mp_teamlist` is not part of Counter-Strike's game library.

use cs_engine::consts::{
    FL_CLIENT, FL_FAKECLIENT, FL_PROXY, FL_SPECTATOR, MOVETYPE_NOCLIP, SOLID_NOT,
};
use cs_engine::{EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::menus::{
    self, MENU_KEY_0, MENU_KEY_1, MENU_KEY_2, MENU_KEY_3, MENU_KEY_4, MENU_KEY_5, MENU_KEY_6, Menu,
    VGUI_MENU_CLASS_CT, VGUI_MENU_CLASS_T, VGUI_MENU_TEAM, atoi,
};
use crate::messages;
use crate::money;
use crate::observer;
use crate::player::{DEAD_DEAD, EF_NODRAW, EF_NOINTERP, Team, apply_model, kill, spawn};
use crate::rules;
use crate::world::{self, CT_SPAWN_CLASS, T_SPAWN_CLASS};

/// `ChooseTeamMenuSlot` (`client.h`): what `jointeam` and the team menu's keys mean.
pub const MENU_SLOT_TEAM_TERRORIST: i32 = 1;
pub const MENU_SLOT_TEAM_CT: i32 = 2;
pub const MENU_SLOT_TEAM_VIP: i32 = 3;
pub const MENU_SLOT_TEAM_RANDOM: i32 = 5;
pub const MENU_SLOT_TEAM_SPECT: i32 = 6;

/// `CS_NUM_SKIN`: the appearances of each team.
const NUM_SKINS: i32 = 4;

/// `JoinState` (`player.h`): where a player is on its way into the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JoinState {
    /// In the game (`JOINED`).
    Joined,
    /// Just entered the server (`SHOWLTEXT`).
    #[default]
    ShowLText,
    /// Reading the level text (`READINGLTEXT`); never entered, the text is not shown.
    ReadingLText,
    /// About to be shown the team menu (`SHOWTEAMSELECT`).
    ShowTeamSelect,
    /// Has the team menu (or the appearance menu) open (`PICKINGTEAM`).
    PickingTeam,
    /// Chose a team and an appearance; spawns on the next [`frame`] (`GETINTOGAME`).
    GetIntoGame,
}

/// The team name `TeamInfo` carries (`GetTeamName`).
pub fn team_name(team: Team) -> &'static str {
    match team {
        Team::Unassigned => "UNASSIGNED",
        Team::Terrorist => "TERRORIST",
        Team::Ct => "CT",
        Team::Spectator => "SPECTATOR",
    }
}

/// The player's name for the notices (`netname`), or what the original prints for none.
pub(crate) fn player_name(eng: &EngineApi<'_>, slot: u8) -> String {
    match eng.client_key(slot, "name") {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "<unconnected>".to_string(),
    }
}

/// The number of entities of `class` (`UTIL_CountEntities`).
fn count_entities(eng: &EngineApi<'_>, class: &str) -> usize {
    let mut n = 0;
    let mut after = None;
    while let Some(id) = eng.find_by_classname(after, class) {
        n += 1;
        after = Some(id);
    }
    n
}

/// The players of `team` that are in the game (`m_iNumCT`, `m_iNumTerrorist`).
pub(crate) fn count_team_players(g: &GameState, team: Team) -> usize {
    g.players
        .iter()
        .flatten()
        .filter(|p| p.team == team)
        .count()
}

/// `TeamFull`: a team holds as many players as it has spawn points.
fn team_full(g: &GameState, eng: &EngineApi<'_>, team: Team) -> bool {
    let spawn_points = match team {
        Team::Terrorist => count_entities(eng, T_SPAWN_CLASS),
        Team::Ct => count_entities(eng, CT_SPAWN_CLASS),
        _ => return false,
    };
    count_team_players(g, team) >= spawn_points
}

/// `TeamStacked`: whether `new` would hold `mp_limitteams` more players than the other team
/// once the player (now on `current`) joined it. The limit is 0 to 20 as `ReadMultiplayCvars`
/// clamps it; 0 is no limit.
fn team_stacked(g: &GameState, eng: &EngineApi<'_>, new: Team, current: Team) -> bool {
    // Players are allowed to change to their own team.
    if new == current {
        return false;
    }
    let limit = (eng.cvar_f32("mp_limitteams") as i32).clamp(0, 20) as usize;
    if limit == 0 {
        return false;
    }
    let (new_count, other_count) = match new {
        Team::Terrorist => (
            count_team_players(g, Team::Terrorist),
            count_team_players(g, Team::Ct),
        ),
        Team::Ct => (
            count_team_players(g, Team::Ct),
            count_team_players(g, Team::Terrorist),
        ),
        _ => return false,
    };
    let leaving_a_team = !matches!(current, Team::Unassigned | Team::Spectator);
    // A player that comes from the other team leaves it, which is one less over there.
    let allowed = if leaving_a_team { limit - 1 } else { limit };
    new_count + 1 > other_count + allowed
}

/// `CHalfLifeMultiplay::SelectDefaultTeam` (ReGameDLL `multiplay_gamerules.cpp`, the vanilla
/// non-`REGAMEDLL_ADD` branch, read 2026-11-22): the team with fewer players; when they are even
/// the team that has won fewer rounds; when those are even too a random one (`RANDOM_LONG(0, 1)`:
/// 0 is the counter-terrorists); then the other team when that one is full; `None` when both are.
fn select_default_team(g: &mut GameState, eng: &EngineApi<'_>) -> Option<Team> {
    let (terrorists, cts) = (
        count_team_players(g, Team::Terrorist),
        count_team_players(g, Team::Ct),
    );
    let (t_wins, ct_wins) = (g.num_terrorist_wins, g.num_ct_wins);
    let team = if terrorists < cts {
        Team::Terrorist
    } else if terrorists > cts {
        Team::Ct
    } else if t_wins < ct_wins {
        // Choose the team that is losing.
        Team::Terrorist
    } else if ct_wins < t_wins {
        Team::Ct
    } else if g.random_range(0, 1) == 0 {
        // Teams and scores are equal, pick a random team.
        Team::Ct
    } else {
        Team::Terrorist
    };
    if !team_full(g, eng, team) {
        return Some(team);
    }
    let other = if team == Team::Terrorist {
        Team::Ct
    } else {
        Team::Terrorist
    };
    (!team_full(g, eng, other)).then_some(other)
}

/// `TeamInfo` of the player of `slot` to everyone, then its `ScoreAttrib` to every player
/// (`CBasePlayer::TeamChangeUpdate`, [`messages::set_scoreboard_attributes`]; only the dead flag:
/// the bomb and the VIP come later).
pub(crate) fn team_change_update(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    eng.send_usermsg(
        MsgDest::All,
        &CsUserMsg::TeamInfo {
            player_id: slot + 1,
            team_name: team_name(p.team).to_string(),
        },
    );
    if p.team == Team::Unassigned {
        return;
    }
    messages::set_scoreboard_attributes(g, eng, usize::from(slot));
}

/// What everyone is told when the player of `slot` leaves (`CHalfLifeMultiplay::ClientDisconnected`):
/// an empty scoreboard line and no team.
pub(crate) fn announce_departure(eng: &mut EngineApi<'_>, slot: u8) {
    let player_id = slot + 1;
    eng.send_usermsg(
        MsgDest::All,
        &CsUserMsg::ScoreInfo {
            player_id,
            frags: 0,
            deaths: 0,
            class_id: 0,
            team_id: 0,
        },
    );
    eng.send_usermsg(
        MsgDest::All,
        &CsUserMsg::TeamInfo {
            player_id,
            team_name: "UNASSIGNED".to_string(),
        },
    );
}

/// The team menu again, as `jointeam` and `menuselect` show it after a refusal: the in-game
/// version for a player that has joined.
pub(crate) fn show_team_menu_again(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.menu = Menu::ChooseTeam;
    let (joined, vgui) = (p.joining == JoinState::Joined, p.vgui_menus);
    if joined {
        menus::show_vgui_menu(
            eng,
            slot,
            vgui,
            VGUI_MENU_TEAM,
            MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5 | MENU_KEY_0,
            "#IG_Team_Select",
        );
    } else {
        menus::show_vgui_menu(
            eng,
            slot,
            vgui,
            VGUI_MENU_TEAM,
            MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5,
            "#Team_Select",
        );
    }
}

/// The per-player logic of the way into the game, run before each command the player's client
/// sends (`CBasePlayer::PreThink`, so only once the client is in the game and able to receive
/// the menus): the team-menu part of `CHalfLifeMultiplay::PlayerThink`, then `JoiningThink`.
///
/// Once the match is over `PreThink` returns after `PlayerThink` (player.cpp:4741, read
/// 2026-11-22): the team menu is still offered but `JoiningThink` does not run, so nobody enters
/// the game during the intermission.
pub(crate) fn pre_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    player_think(g, eng, slot);
    if !g.rules.game_over() {
        joining_think(g, eng, slot);
    }
}

/// The team-menu part of `CHalfLifeMultiplay::PlayerThink`: a player about to pick a team gets
/// the menu, unless `humans_join_team` says which team humans join: then it joins it.
fn player_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if eng.is_fake_client(slot) {
        return;
    }
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if p.menu == Menu::ChooseTeam || p.joining != JoinState::ShowTeamSelect {
        return;
    }
    let humans = eng.cvar_str("humans_join_team").unwrap_or_default();
    let auto = if humans.eq_ignore_ascii_case("T") {
        Some(MENU_SLOT_TEAM_TERRORIST)
    } else if humans.eq_ignore_ascii_case("CT") {
        Some(MENU_SLOT_TEAM_CT)
    } else {
        None
    };
    let vgui = p.vgui_menus;
    if auto.is_none() {
        if eng.cvar_f32("allow_spectators") == 0.0 {
            menus::show_vgui_menu(
                eng,
                slot,
                vgui,
                VGUI_MENU_TEAM,
                MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5,
                "#Team_Select",
            );
        } else {
            menus::show_vgui_menu(
                eng,
                slot,
                vgui,
                VGUI_MENU_TEAM,
                MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5 | MENU_KEY_6,
                "#Team_Select_Spect",
            );
        }
    }
    p.menu = Menu::ChooseTeam;
    p.joining = JoinState::PickingTeam;
    if let Some(choice) = auto {
        handle_menu_choose_team(g, eng, slot, choice);
    }
}

/// `CBasePlayer::JoiningThink` (ReGameDLL `player.cpp:3814`, the non-`REGAMEDLL_FIXES` branch,
/// read 2026-11-22). Whatever the state it is not `Joined` in, and unless the player was taken
/// into the game by this call, the intro camera may move on ([`crate::intro::joining_think`]):
/// the `switch` of the original has no case for the team menu states, which fall out of it.
fn joining_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    match p.joining {
        JoinState::Joined => return,
        JoinState::ShowTeamSelect | JoinState::PickingTeam => {}
        JoinState::ShowLText => {
            // `ResetMenu`, then (vanilla, not `REGAMEDLL_FIXES`) the defuser status icon is
            // hidden (the player has no defuser: `m_bHasDefuser = false`) and `SendItemStatus`.
            p.joining = JoinState::ShowTeamSelect;
            menus::reset_menu_message(eng, slot);
            eng.send_usermsg(
                MsgDest::One(slot),
                &CsUserMsg::StatusIcon {
                    status: 0,
                    sprite_name: "defuser".to_string(),
                    color: None,
                },
            );
            crate::buy::send_item_status(eng, slot, p.has_night_vision, p.has_defuser);
        }
        // There is no level text to read.
        JoinState::ReadingLText => p.joining = JoinState::ShowTeamSelect,
        JoinState::GetIntoGame => {
            get_into_game(g, eng, slot);
            return;
        }
    }
    crate::intro::joining_think(g, eng, slot);
}

/// `CBasePlayer::GetIntoGame`: the player is in the game; it spawns when the rules allow
/// ([`rules::can_respawn`]), else it watches until the next round.
fn get_into_game(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.not_killed = false;
    p.team_kills = 0;
    // `GetIntoGame` (ReGameDLL player.cpp:10746) resets `m_iIgnoreGlobalChat`.
    p.chat.ignore = crate::chat::IgnoreMsg::None;
    p.joining = JoinState::Joined;
    let (team, class) = (p.team, p.class);
    if rules::can_respawn(g, eng, slot) {
        spawn(g, eng, slot, team, class);
        rules::check_win_conditions(g, eng);
        return;
    }
    // Watching until the next round.
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.deadflag = crate::player::DEAD_RESPAWNABLE;
        v.flags &= FL_PROXY | FL_FAKECLIENT;
        v.flags |= FL_SPECTATOR | FL_CLIENT;
    }
    start_observer(g, eng, slot, false);
    rules::check_win_conditions(g, eng);
    eng.send_usermsg(
        MsgDest::All,
        &CsUserMsg::TeamInfo {
            player_id: slot + 1,
            team_name: team_name(team).to_string(),
        },
    );
    if let Some(p) = g.players[usize::from(slot)].as_ref() {
        messages::score_info(eng, u16::from(slot) + 1, p);
    }
    // The end of the branch (player.cpp:10819, read 2026-11-22): how to change the view, once.
    if crate::hint::first_time(g, slot, crate::hint::dhf::SPEC_DUCK) {
        crate::hint::hint_message(g, eng, slot, "#Spec_Duck", true, true);
    }
}

/// `CBasePlayer::StartObserver` ([`observer::start_observer`]) at a spawn point of
/// `GetPlayerSpawnSpot`. A player that joins the spectators (`HandleMenu_ChooseTeam`,
/// `client.cpp`) is observed at the point's origin and angles; a player that waits for the next
/// round (`GetIntoGame`, `player.cpp`) at its own origin, which `GetPlayerSpawnSpot` has just
/// put one unit above the point, with the point's angles.
fn start_observer(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, spectator: bool) {
    let (spot_origin, spot_angles) = world::get_player_spawn_spot(g, eng, slot);
    let origin = if spectator {
        spot_origin
    } else {
        eng.ent(eng.client_edict(slot))
            .map_or(spot_origin, |v| v.origin)
    };
    observer::start_observer(g, eng, slot, origin, spot_angles);
}

/// `HandleMenu_ChooseTeam` (`client.cpp`): the player of `slot` chose `choice` in the team menu.
/// Returns whether the choice was handled and the menu can close; `false` shows the menu again.
pub(crate) fn handle_menu_choose_team(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    choice: i32,
) -> bool {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    let current = p.team;
    let team = match choice {
        MENU_SLOT_TEAM_TERRORIST => Team::Terrorist,
        MENU_SLOT_TEAM_CT => Team::Ct,
        // Becoming the VIP needs a map with a VIP safety zone.
        MENU_SLOT_TEAM_VIP => return false,
        MENU_SLOT_TEAM_RANDOM => match select_default_team(g, eng) {
            Some(team) => team,
            None => return false,
        },
        MENU_SLOT_TEAM_SPECT => return choose_spectator(g, eng, slot),
        _ => return false,
    };
    let is_terrorist = team == Team::Terrorist;

    // The team is full.
    if team_full(g, eng, team) {
        let text = if is_terrorist {
            "#Terrorists_Full"
        } else {
            "#CTs_Full"
        };
        menus::print_center(eng, slot, text);
        return false;
    }
    // Players are allowed to change to their own team, to choose a new appearance.
    if team_stacked(g, eng, team, current) {
        let text = if is_terrorist {
            "#Too_Many_Terrorists"
        } else {
            "#Too_Many_CTs"
        };
        menus::print_center(eng, slot, text);
        return false;
    }
    // `humans_join_team` names the only team humans may join.
    let humans = eng.cvar_str("humans_join_team").unwrap_or_default();
    let human_team = if humans.eq_ignore_ascii_case("CT") {
        Some(Team::Ct)
    } else if humans.eq_ignore_ascii_case("T") {
        Some(Team::Terrorist)
    } else {
        None
    };
    if !eng.is_fake_client(slot) && human_team.is_some_and(|t| t != team) {
        let text = if is_terrorist {
            "#Humans_Join_Team_CT"
        } else {
            "#Humans_Join_Team_T"
        };
        menus::print_center(eng, slot, text);
        return false;
    }
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return false;
    };
    // A player that died and changed teams once may not do it again.
    if p.team_changed && !p.alive {
        menus::print_center(eng, slot, "#Only_1_Team_Change");
        return false;
    }
    if p.team == Team::Spectator {
        // Coming out of the spectators: a fresh start.
        p.not_killed = true;
        p.chat.ignore = crate::chat::IgnoreMsg::None;
        p.money = money::check_start_money(eng);
        p.alive = false;
        p.takes_damage = false;
        p.joining = JoinState::GetIntoGame;
        let id = eng.client_edict(slot);
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NOCLIP;
            v.effects = EF_NODRAW | EF_NOINTERP;
            v.deadflag = DEAD_DEAD;
            v.punchangle = Vec3::ZERO;
            v.velocity = Vec3::ZERO;
            v.max_speed = 1.0;
        }
        // `m_bHasNightVision = false; SendItemStatus()`.
        p.has_night_vision = false;
        crate::buy::send_item_status(eng, slot, false, p.has_defuser);
        apply_model(eng, slot, Team::Unassigned, 0);
    }

    // The appearance menu; the player in the game dies for the change.
    let vgui = p.vgui_menus;
    let alive = p.alive;
    p.menu = Menu::ChooseAppearance;
    if !eng.is_fake_client(slot) {
        if is_terrorist {
            menus::show_vgui_menu(
                eng,
                slot,
                vgui,
                VGUI_MENU_CLASS_T,
                MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_5,
                "#Terrorist_Select",
            );
        } else {
            menus::show_vgui_menu(
                eng,
                slot,
                vgui,
                VGUI_MENU_CLASS_CT,
                MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_5,
                "#CT_Select",
            );
        }
    }
    if alive {
        // A suicide: it costs a frag and counts as a death, and the others see it under the
        // team the player is leaving.
        kill(g, eng, slot);
    }

    // Switch the team.
    let name = player_name(eng, slot);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return false;
    };
    p.team_changed = true;
    p.team = team;
    team_change_update(g, eng, slot);
    let text = if is_terrorist {
        "#Game_join_terrorist"
    } else {
        "#Game_join_ct"
    };
    menus::print_all_notify(eng, text, &name);
    true
}

/// The `MENU_SLOT_TEAM_SPECT` branch of `HandleMenu_ChooseTeam`: a player may watch when
/// `allow_spectators` is on and the player is dead or the round is in its freeze period.
fn choose_spectator(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    if eng.cvar_f32("allow_spectators") == 0.0 {
        menus::print_center(eng, slot, "#Cannot_Be_Spectator");
        eng.stufftext(slot, "slot10\n");
        return false;
    }
    let freeze = g.rules.is_freeze_period();
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    // Already a spectator.
    if p.team == Team::Spectator {
        return true;
    }
    // Only in the freeze period or when dead.
    if !(freeze || !p.alive) {
        menus::print_center(eng, slot, "#Cannot_Be_Spectator");
        eng.stufftext(slot, "slot10\n");
        return false;
    }
    if p.team != Team::Unassigned && p.alive && kill(g, eng, slot) {
        // One frag back: the one the suicide cost.
        if let Some(p) = g.players[usize::from(slot)].as_mut() {
            p.frags += 1;
        }
    }
    // `pPlayer->RemoveAllItems(TRUE)` (client.cpp 1938, read 2026-11-22): a spectator keeps
    // no weapon, no ammo and no suit. (`m_bHasC4 = false` after it is already part of it.)
    crate::inventory::remove_all_items(g, eng, slot, true);
    let Some(p) = g.players[usize::from(slot)].as_mut() else {
        return false;
    };
    p.team = Team::Spectator;
    p.joining = JoinState::Joined;
    // The account is emptied.
    p.money = 0;
    let (frags, deaths) = (p.frags, p.deaths);
    messages::money(eng, slot, 0);
    // Sent with no team and no class, as the original does: `MSG_BROADCAST`, unreliable
    // (`REGAMEDLL_FIXES` would use `MSG_ALL`; client.cpp:1962, read 2026-11-22).
    eng.send_usermsg(
        MsgDest::Broadcast,
        &CsUserMsg::ScoreInfo {
            player_id: slot + 1,
            frags: frags.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            deaths: deaths.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            class_id: 0,
            team_id: 0,
        },
    );
    // `m_pIntroCamera = nullptr; m_bTeamChanged = true` (client.cpp:1981, read 2026-11-22).
    if let Some(p) = g.players[usize::from(slot)].as_mut() {
        p.intro_camera = None;
        p.team_changed = true;
    }
    team_change_update(g, eng, slot);
    start_observer(g, eng, slot, true);
    // The original sends the `Spectator` message again, after the one `StartObserver` sent.
    messages::send_raw(
        eng,
        MsgDest::All,
        "Spectator",
        &[(u16::from(slot) + 1) as u8, 1],
    );
    // With `mp_fadetoblack 1` the screen is faded back in: `UTIL_ScreenFade(pPlayer,
    // Vector(0, 0, 0), 0.001, 0, 0, FFADE_IN)`.
    if eng.cvar_f32("mp_fadetoblack") as i32 == observer::FADETOBLACK_STAY {
        observer::screen_fade(eng, slot, 0.001, 0.0, 0, observer::FFADE_IN);
    }
    true
}

/// `HandleMenu_ChooseAppearance` (`client.cpp`): the player of `slot` chose appearance `choice`
/// (1 to 4; any other number picks one at random) for its team. A player on its way into the
/// game spawns with it ([`get_into_game`]); one already in the game wears it from its next
/// spawn.
pub(crate) fn handle_menu_choose_appearance(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    choice: i32,
) {
    let class = if (1..=NUM_SKINS).contains(&choice) {
        choice as u8
    } else {
        g.random_range(1, NUM_SKINS) as u8
    };
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.menu = Menu::Off;
    // Only a team that plays has appearances.
    if !p.team.plays() {
        return;
    }
    let joined = p.joining == JoinState::Joined;
    if p.joining == JoinState::PickingTeam {
        p.joining = JoinState::GetIntoGame;
    }
    p.class = class;
    let team = p.team;
    apply_model(eng, slot, team, class);
    // A player already in the game changes the count of the players that may spawn.
    if joined {
        rules::check_win_conditions(g, eng);
    }
}

/// The `jointeam` command: join the team of `argv[1]` (1 terrorists, 2 counter-terrorists, 5 a
/// team picked for the player, 6 the spectators).
pub(crate) fn join_team(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    if p.menu == Menu::ChooseAppearance {
        menus::print_center(eng, slot, "#Command_Not_Available");
        return;
    }
    let choice = atoi(argv.get(1).map_or("", String::as_str));
    if handle_menu_choose_team(g, eng, slot, choice) {
        // The spectators have no appearance to choose.
        let next = if choice == MENU_SLOT_TEAM_VIP || choice == MENU_SLOT_TEAM_SPECT {
            Menu::Off
        } else {
            Menu::ChooseAppearance
        };
        if let Some(p) = g.players[usize::from(slot)].as_mut() {
            p.menu = next;
        }
    } else {
        show_team_menu_again(g, eng, slot);
    }
}

/// The `joinclass` command: wear appearance `argv[1]`; only while the appearance menu is open.
pub(crate) fn join_class(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let choice = atoi(argv.get(1).map_or("", String::as_str));
    if p.menu != Menu::ChooseAppearance {
        menus::print_center(eng, slot, "#Command_Not_Available");
        return;
    }
    handle_menu_choose_appearance(g, eng, slot, choice);
}

/// The `chooseteam` command: the team menu of a player in the game. The spectators' key is
/// offered while the player is dead or in the freeze period.
pub(crate) fn choose_team(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let freeze = g.rules.is_freeze_period();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if p.menu == Menu::ChooseAppearance {
        return;
    }
    if p.team_changed && !p.alive {
        menus::print_center(eng, slot, "#Only_1_Team_Change");
        return;
    }
    p.menu = Menu::ChooseTeam;
    let vgui = p.vgui_menus;
    if freeze || !p.alive {
        menus::show_vgui_menu(
            eng,
            slot,
            vgui,
            VGUI_MENU_TEAM,
            MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5 | MENU_KEY_6 | MENU_KEY_0,
            "#IG_Team_Select_Spect",
        );
    } else {
        menus::show_vgui_menu(
            eng,
            slot,
            vgui,
            VGUI_MENU_TEAM,
            MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_5 | MENU_KEY_0,
            "#IG_Team_Select",
        );
    }
}

/// Puts a player in the game for the test hooks, as the team selection would have.
#[cfg(feature = "test-hooks")]
pub(crate) fn mark_joined(p: &mut crate::player::Player) {
    p.joining = JoinState::Joined;
    p.menu = Menu::Off;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_names_are_the_clients() {
        assert_eq!(team_name(Team::Terrorist), "TERRORIST");
        assert_eq!(team_name(Team::Ct), "CT");
        assert_eq!(team_name(Team::Spectator), "SPECTATOR");
        assert_eq!(team_name(Team::Unassigned), "UNASSIGNED");
    }

    #[test]
    fn a_new_player_starts_before_the_team_menu() {
        assert_eq!(JoinState::default(), JoinState::ShowLText);
        let p = crate::player::Player::new(800);
        assert_eq!((p.joining, p.menu), (JoinState::ShowLText, Menu::Off));
    }
}
