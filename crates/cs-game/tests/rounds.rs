//! The round rules (`CHalfLifeMultiplay`): the game that starts when both teams have a player,
//! the freeze period, the end of a round by elimination or by players leaving, the restart
//! cvars, the end of the match and the map change it asks for. Time is counted in server
//! frames of 10 ms; the probe is in slot 0, the other slots are players the test drives
//! directly (they receive no messages, so what they would see is read from the game's state).

mod common;

use common::{Bench, command, commands_of, enter, join, spawns};
use cs_engine::{EngineApi, GameDll, ServerMap};
use cs_game::CsGame;
use cs_game::player::{DEAD_NO, Player, Team};
use cs_game::rules::{Rules, WinStatus};
use cs_game::teams::JoinState;
use cs_game::weapons::{WeaponId, ammo_index, new_weapon};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use cs_vfs::Vfs;

/// Frames of 10 ms in `secs` seconds.
fn frames(secs: f64) -> usize {
    (secs * 100.0).round() as usize
}

fn rules(b: &Bench) -> &Rules {
    &b.server.game().game.state().rules
}

fn player(b: &Bench, slot: usize) -> Player {
    b.server.game().game.state().players[slot]
        .clone()
        .expect("a player in the slot")
}

fn wins(b: &Bench) -> (i32, i32) {
    let s = b.server.game().game.state();
    (s.num_terrorist_wins, s.num_ct_wins)
}

/// A server of `slots` slots on a stub map with plenty of spawn points, the probe at slot 0 and
/// the freeze time of a round at two seconds.
fn bench(slots: u8) -> Bench {
    let mut b = Bench::stub_slots(spawns(8, 8), slots);
    b.idle_cmds = true;
    b.step(6);
    // Room in the probe's message log (it holds 1024 and is full after the sign-on).
    b.probe.clear_messages();
    // The round time is set here, not left to a default (the servers differ).
    b.run(|_, eng| {
        eng.set_cvar("mp_freezetime", "2");
        eng.set_cvar("mp_roundtime", "2");
    });
    b
}

/// Puts the players of `slots` in the game on `team_choice` (1 terrorists, 2 counter-terrorists)
/// with appearance 1.
fn join_all(b: &mut Bench, slots: &[u8], team_choice: u8) {
    for &slot in slots {
        if slot != 0 {
            enter(b, slot);
        }
        join(b, slot, team_choice, 1);
    }
}

/// A match under way: the terrorists of `ts` and the counter-terrorists of `cts` have joined,
/// the game has commenced and the first round has restarted (in its freeze period).
fn match_under_way(slots: u8, ts: &[u8], cts: &[u8]) -> Bench {
    let mut b = bench(slots);
    join_all(&mut b, ts, 1);
    join_all(&mut b, cts, 2);
    b.step(frames(3.3));
    assert!(rules(&b).game_started());
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert!(rules(&b).is_freeze_period());
    b
}

fn texts(b: &Bench, dest: u8, text: &str) -> Vec<Vec<String>> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg {
                dest: d,
                message,
                params,
            } if d == dest && message == text => Some(params),
            _ => None,
        })
        .collect()
}

fn said(b: &Bench, dest: u8, text: &str) -> bool {
    !texts(b, dest, text).is_empty()
}

fn audio(b: &Bench, code: &str) -> bool {
    b.messages().iter().any(
        |m| matches!(m, CsUserMsg::SendAudio { audio_code, .. } if audio_code == code),
    )
}

/// Seconds left on the round clock.
fn remaining(b: &mut Bench) -> f64 {
    b.run(|game, eng| game.state().rules.round_remaining(eng.time()))
}

fn kill_player(b: &mut Bench, slot: u8) {
    b.run(move |game, eng| game.damage_player(eng, slot, None, 500.0, 0));
}

fn max_speed(b: &mut Bench, slot: u8) -> f32 {
    b.run(move |_, eng| eng.ent(eng.client_edict(slot)).unwrap().max_speed)
}

fn origin_of(b: &mut Bench, slot: u8) -> glam::Vec3 {
    b.run(move |_, eng| eng.ent(eng.client_edict(slot)).unwrap().origin)
}

fn forward(slot: u8, n: usize) -> impl FnOnce(&mut CsGame, &mut EngineApi<'_>) {
    move |game, eng| {
        for _ in 0..n {
            let cmd = GoldSrcUserCmd {
                msec: 10,
                forwardmove: 400.0,
                ..Default::default()
            };
            common::run_cmd(game, eng, slot, &cmd, 0);
        }
    }
}

#[test]
fn game_commencing_on_first_players() {
    let mut b = bench(3);
    // A map that starts waits in its freeze period, with the clock set for the freeze time and
    // two seconds more (at most eight seconds from the first think).
    assert!(rules(&b).is_freeze_period());
    assert!(!rules(&b).game_started());

    // One team alone does not start the game: it is told the score does not count yet.
    b.probe.clear_messages();
    join_all(&mut b, &[0], 1);
    assert!(player(&b, 0).alive);
    assert!(said(&b, 2, "#Game_scoring"));
    assert!(!said(&b, 4, "#Game_Commencing"));
    assert!(!rules(&b).game_started());

    // The first counter-terrorist starts it: "Game Commencing", the freeze period is lifted and
    // the first round restarts three seconds later with a complete reset.
    b.run(|game, _| {
        let p = game.state_mut().players[0].as_mut().unwrap();
        p.frags = 5;
        p.deaths = 2;
    });
    b.probe.clear_messages();
    join_all(&mut b, &[1], 2);
    assert!(said(&b, 4, "#Game_Commencing"));
    assert!(rules(&b).game_started());
    assert_eq!(rules(&b).win_status(), WinStatus::Draw);
    assert!(!rules(&b).is_freeze_period());
    let due = rules(&b).restart_round_time();
    assert!(due > 0.0);
    // No win is counted for the commencing "round".
    assert_eq!(wins(&b), (0, 0));
    assert_eq!(player(&b, 0).num_spawns, 1);

    b.step(frames(3.3));
    let (t, ct) = (player(&b, 0), player(&b, 1));
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert_eq!(rules(&b).restart_round_time(), 0.0);
    assert!(rules(&b).is_freeze_period());
    // A complete reset: the scoreboard starts over, everyone spawns again.
    assert_eq!((t.frags, t.deaths), (0, 0));
    assert_eq!((t.num_spawns, ct.num_spawns), (1, 1));
    assert!(t.alive && ct.alive);
    assert_eq!(rules(&b).total_rounds_played(), 0);
    // Joining a team again does not commence the game twice.
    b.probe.clear_messages();
    join_all(&mut b, &[2], 2);
    assert!(!said(&b, 4, "#Game_Commencing"));
}

#[test]
fn freeze_time_holds_players_then_round_starts() {
    let mut b = match_under_way(3, &[0], &[1]);
    // The round restart spawned both in the freeze period: `ResetMaxSpeed` gives 1.
    assert_eq!(max_speed(&mut b, 0), 1.0);
    assert_eq!(max_speed(&mut b, 1), 1.0);
    // A client sending "forward" goes nowhere (the stub map has no floor, so it only falls).
    let start = origin_of(&mut b, 1);
    b.run(forward(1, 50));
    let moved = origin_of(&mut b, 1);
    assert!(
        (moved.x - start.x).abs() < 1.0 && (moved.y - start.y).abs() < 1.0,
        "{start:?} {moved:?}"
    );

    // The RoundTime of the freeze period is the freeze time.
    let timers: Vec<u16> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::RoundTime(t) => Some(t),
            _ => None,
        })
        .collect();
    assert!(
        timers.last().is_some_and(|t| (1..=2).contains(t)),
        "{timers:?}"
    );

    // Two seconds after the restart the round starts.
    b.probe.clear_messages();
    b.step(frames(2.3));
    assert!(!rules(&b).is_freeze_period());
    assert_eq!(max_speed(&mut b, 0), 250.0);
    assert_eq!(max_speed(&mut b, 1), 250.0);
    b.step(3);
    // The new clock is the round time (`mp_roundtime`, 2 minutes).
    let timer = b
        .messages()
        .into_iter()
        .rev()
        .find_map(|m| match m {
            CsUserMsg::RoundTime(t) => Some(t),
            _ => None,
        })
        .expect("a RoundTime at the start of the round");
    assert!((118..=120).contains(&timer), "{timer}");
    // They move now.
    let start = origin_of(&mut b, 1);
    b.run(forward(1, 100));
    let moved = origin_of(&mut b, 1);
    assert!(
        (moved.x - start.x).abs() + (moved.y - start.y).abs() > 2.0,
        "{start:?} {moved:?}"
    );
}

#[test]
fn elimination_ends_the_round_for_the_other_team() {
    let mut b = match_under_way(3, &[0], &[1]);
    b.step(frames(2.3));
    assert!(!rules(&b).is_freeze_period());

    // The counter-terrorist dies: the terrorists win the round.
    b.probe.clear_messages();
    kill_player(&mut b, 1);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::Terrorists);
    assert_eq!(wins(&b), (1, 0));
    assert!(said(&b, 4, "#Terrorists_Win"));
    assert!(audio(&b, "%!MRAD_terwin"));
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::TeamScore { team_name, score: 1 } if team_name == "TERRORIST"
    )));
    // The next round starts five seconds later: everyone is alive again, the score stays.
    assert!(rules(&b).restart_round_time() > 0.0);
    b.step(frames(2.0));
    assert!(!player(&b, 1).alive);
    b.step(frames(3.3));
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert!(rules(&b).is_freeze_period());
    assert!(player(&b, 0).alive && player(&b, 1).alive);
    assert_eq!(wins(&b), (1, 0));
    assert_eq!(rules(&b).total_rounds_played(), 1);

    // And the other way round, with the terrorist dying: the counter-terrorists win.
    b.step(frames(2.3));
    b.probe.clear_messages();
    kill_player(&mut b, 0);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::Cts);
    assert_eq!(wins(&b), (1, 1));
    assert!(said(&b, 4, "#CTs_Win"));
    assert!(audio(&b, "%!MRAD_ctwin"));

    // Nobody wins twice: a death while the next round is awaited changes nothing.
    kill_player(&mut b, 1);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::Cts);
    assert_eq!(wins(&b), (1, 1));
}

#[test]
fn a_team_with_players_left_plays_on() {
    // Two on each side: one death does not end the round.
    let mut b = match_under_way(5, &[0, 1], &[2, 3]);
    b.step(frames(2.3));
    kill_player(&mut b, 2);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    kill_player(&mut b, 3);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::Terrorists);
}

#[test]
fn round_time_expiry_as_regamedll() {
    let mut b = bench(3);
    // A round of one minute with no freeze time, from the next restart.
    b.run(|_, eng| {
        eng.set_cvar("mp_roundtime", "1");
        eng.set_cvar("mp_freezetime", "0");
    });
    join_all(&mut b, &[0], 1);
    join_all(&mut b, &[1], 2);
    b.step(frames(3.3));
    assert!(rules(&b).game_started());
    assert!(!rules(&b).is_freeze_period());
    let before = remaining(&mut b);
    assert!((58.0..=60.0).contains(&before), "{before}");

    // The clock runs out. The map has no objective (bomb, hostages, escape, VIP), so as in
    // `CheckRoundTimeExpired` nobody wins: the clock gets 60 more seconds.
    b.step(frames(61.0));
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert_eq!(rules(&b).restart_round_time(), 0.0);
    assert!(player(&b, 0).alive && player(&b, 1).alive);
    let after = remaining(&mut b);
    assert!((115.0..=120.0).contains(&after), "{after}");
    assert_eq!(wins(&b), (0, 0));
    // And the round still ends by elimination.
    kill_player(&mut b, 0);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::Cts);
}

#[test]
fn restart_round_cvar() {
    let mut b = match_under_way(3, &[0], &[1]);
    b.step(frames(2.3));
    // A scored round, a player that changed team (not yet cleared) and some frags.
    kill_player(&mut b, 1);
    b.step(frames(5.3));
    b.run(|game, _| {
        let s = game.state_mut();
        s.players[0].as_mut().unwrap().frags = 4;
        s.players[0].as_mut().unwrap().team_changed = true;
        s.players[1].as_mut().unwrap().deaths = 3;
    });
    assert_eq!(wins(&b), (1, 0));

    b.probe.clear_messages();
    b.run(|_, eng| eng.set_cvar("sv_restart", "3"));
    b.step(frames(1.2));
    // Announced (at most a second late, as the cvars are looked at once a second) and reset.
    assert_eq!(
        texts(&b, 4, "#Game_will_restart_in"),
        [vec!["3".to_string(), "SECONDS".to_string()]]
    );
    assert_eq!(
        texts(&b, 2, "#Game_will_restart_in_console"),
        [vec!["3".to_string(), "SECONDS".to_string()]]
    );
    assert_eq!(b.run(|_, eng| eng.cvar_f32("sv_restart")), 0.0);
    assert_eq!(wins(&b), (1, 0));
    assert!(player(&b, 0).team_changed);

    b.probe.clear_messages();
    b.step(frames(3.0));
    // A new match: no wins, a clean scoreboard, the starting money, everyone spawned again and
    // free to change teams again (`m_bTeamChanged`).
    let (t, ct) = (player(&b, 0), player(&b, 1));
    assert_eq!(wins(&b), (0, 0));
    assert_eq!((t.frags, ct.deaths), (0, 0));
    assert_eq!((t.money, ct.money), (800, 800));
    assert_eq!((t.num_spawns, ct.num_spawns), (1, 1));
    assert!(!t.team_changed && !ct.team_changed);
    assert!(t.alive && ct.alive);
    assert!(rules(&b).is_freeze_period());
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::TeamScore { team_name, score: 0 } if team_name == "CT"
    )));

    // `sv_restartround` does the same, and wins over `sv_restart`; more than a minute is a
    // minute.
    b.probe.clear_messages();
    b.run(|_, eng| {
        eng.set_cvar("sv_restartround", "100");
        eng.set_cvar("sv_restart", "2");
    });
    b.step(frames(1.2));
    assert_eq!(
        texts(&b, 4, "#Game_will_restart_in"),
        [vec!["60".to_string(), "SECONDS".to_string()]]
    );
    // One second is "SECOND".
    b.run(|_, eng| eng.set_cvar("sv_restart", "1"));
    b.probe.clear_messages();
    b.step(frames(1.2));
    // `sv_restartround` was cleared by the first, so the new `sv_restart` counts.
    assert_eq!(
        texts(&b, 4, "#Game_will_restart_in"),
        [vec!["1".to_string(), "SECOND".to_string()]]
    );
}

/// A cycle of the given maps, as the server loads it at start-up.
fn cycle(maps: &[&str]) -> Vfs {
    let mut files = vec![(
        "mapcycle.txt".to_string(),
        maps.iter().map(|m| format!("{m}\n")).collect::<String>().into_bytes(),
    )];
    files.extend(maps.iter().map(|m| (format!("maps/{m}.bsp"), Vec::new())));
    Vfs::from_memory(files)
}

#[test]
fn win_limit_changes_the_map() {
    let mut b = match_under_way(3, &[0], &[1]);
    b.run(|game, _| game.load_map_cycle(&cycle(&["de_one", "de_two"])));
    b.run(|_, eng| {
        eng.set_cvar("mp_winlimit", "2");
        eng.set_cvar("mp_chattime", "3");
    });
    b.step(frames(2.3));

    // One round won is not enough.
    kill_player(&mut b, 1);
    b.step(frames(5.5));
    assert_eq!(wins(&b), (1, 0));
    assert!(!rules(&b).game_over());
    b.step(frames(2.3));

    // The second round won ends the match: the scoreboard (intermission) comes up.
    b.probe.clear_messages();
    kill_player(&mut b, 1);
    b.step(frames(0.3));
    assert_eq!(wins(&b), (2, 0));
    assert!(rules(&b).game_over());
    assert!(
        b.probe
            .messages()
            .iter()
            .any(|m| matches!(m, cs_proto48::messages::SvcMessage::Intermission))
    );
    let started = rules(&b).intermission_start_time();
    assert!(started > 0.0);
    // The players are not respawned for another round meanwhile.
    b.step(frames(5.5));
    assert!(!player(&b, 1).alive);
    assert_eq!(rules(&b).win_status(), WinStatus::Terrorists);
    // Nothing asked for yet: `mp_chattime` is over but nobody pressed a key.
    assert_eq!(b.server.take_pending_change_level(), None);

    // A player presses a key (jump): the map changes to the first of the cycle.
    b.run(|game, eng| {
        let cmd = GoldSrcUserCmd {
            msec: 10,
            buttons: 2,
            ..Default::default()
        };
        common::run_cmd(game, eng, 1, &cmd, 0);
    });
    b.step(3);
    assert_eq!(
        b.server.take_pending_change_level(),
        Some("de_one".to_string())
    );
    // The server owner changes the map; the game starts over on the new one but remembers
    // where the cycle was.
    b.server
        .change_map(ServerMap::stub_with_entities("de_one", spawns(2, 2)));
    {
        let s = b.server.game().game.state();
        assert_eq!((s.num_terrorist_wins, s.num_ct_wins), (0, 0));
        assert!(s.rules.is_freeze_period());
        assert!(!s.rules.game_over() && !s.rules.game_started());
        assert!(s.players.iter().all(Option::is_none));
    }
    // The next match asks for the next map of the cycle.
    assert_eq!(
        b.server.game().game.state().map_cycle.next_map(),
        Some("de_two")
    );
}

#[test]
fn the_match_ends_without_a_key_after_two_minutes() {
    let mut b = match_under_way(3, &[0], &[1]);
    b.run(|_, eng| {
        eng.set_cvar("mp_maxrounds", "1");
        eng.set_cvar("mp_chattime", "1");
    });
    // The limit is read when a match starts and every second.
    b.step(frames(1.2));
    b.step(frames(2.3));
    kill_player(&mut b, 1);
    // The round ends and restarts, which is the first round played: the match is over.
    b.step(frames(5.5));
    assert!(rules(&b).game_over());
    // No cycle was loaded: the current map is asked for again, but only after the intermission
    // (two minutes without a key).
    b.step(frames(100.0));
    assert_eq!(b.server.take_pending_change_level(), None);
    // The request comes within the next 21 seconds (frame by frame, so it is taken as it is made).
    let mut asked = None;
    for _ in 0..2100 {
        b.step(1);
        asked = b.server.take_pending_change_level();
        if asked.is_some() {
            break;
        }
    }
    assert_eq!(asked, Some("test".to_string()));
    // An unanswered request is repeated, once a second at most.
    b.step(frames(0.5));
    assert_eq!(b.server.take_pending_change_level(), None);
    b.step(frames(0.6));
    assert_eq!(
        b.server.take_pending_change_level(),
        Some("test".to_string())
    );
}

#[test]
fn the_time_limit_ends_the_match() {
    let mut b = match_under_way(3, &[0], &[1]);
    b.run(|_, eng| eng.set_cvar("mp_timelimit", "1"));
    // The match started when the game commenced, a few seconds ago.
    b.step(frames(50.0));
    assert!(!rules(&b).game_over());
    b.step(frames(12.0));
    assert!(rules(&b).game_over());
    // A negative limit is no limit and is written back as 0.
    let mut b = match_under_way(3, &[0], &[1]);
    b.run(|_, eng| eng.set_cvar("mp_timelimit", "-5"));
    b.step(5);
    assert!(!rules(&b).game_over());
    assert_eq!(b.run(|_, eng| eng.cvar_f32("mp_timelimit")), 0.0);
}

/// The index of the first message of `b` that `is`, and panics if there is none.
fn position(b: &Bench, what: &str, is: impl Fn(&CsUserMsg) -> bool) -> usize {
    b.messages()
        .iter()
        .position(is)
        .unwrap_or_else(|| panic!("no {what} message"))
}

#[test]
fn leaving_players_end_the_round_as_regamedll() {
    // Two terrorists and a counter-terrorist.
    let mut b = match_under_way(4, &[0, 1], &[2]);
    b.step(frames(2.3));
    // One terrorist is dead and the other leaves: the dead one's team has nobody alive (the
    // leaver is dormant, so it is not counted at all), so the counter-terrorists win the round.
    kill_player(&mut b, 0);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    b.probe.clear_messages();
    b.run(|game, eng| game.client_disconnect(eng, 1));
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::Cts);
    assert_eq!(wins(&b), (0, 1));
    assert!(said(&b, 4, "#CTs_Win"));
    // `ClientDisconnect`/`ClientDisconnected` order: the notice, the leaver's `ScoreAttrib`
    // (dead), the emptied scoreboard line and team, and only then `CheckWinConditions`, whose
    // messages (the win's audio and text) come last. The `ScoreAttrib` goes to each client alone
    // (`MSG_ONE`) and the rest to everyone (`MSG_ALL`), which the engine delivers at the end of
    // the frame, after what went to single clients (`SV_UpdateToReliableMessages`): so the
    // `ScoreAttrib` is the first of them.
    let notice = position(&b, "#Game_disconnected", |m| {
        matches!(m, CsUserMsg::TextMsg { message, params, .. }
            if message == "#Game_disconnected" && params.len() == 1)
    });
    let attrib = position(&b, "leaver ScoreAttrib", |m| {
        matches!(m, CsUserMsg::ScoreAttrib { player_id: 2, flags: 1 })
    });
    let info = position(&b, "leaver ScoreInfo", |m| {
        matches!(m, CsUserMsg::ScoreInfo { player_id: 2, frags: 0, deaths: 0, class_id: 0, team_id: 0 })
    });
    let team = position(&b, "leaver TeamInfo", |m| {
        matches!(m, CsUserMsg::TeamInfo { player_id: 2, team_name } if team_name == "UNASSIGNED")
    });
    let win_audio = position(&b, "ctwin", |m| {
        matches!(m, CsUserMsg::SendAudio { audio_code, .. } if audio_code == "%!MRAD_ctwin")
    });
    let win_text = position(&b, "#CTs_Win", |m| {
        matches!(m, CsUserMsg::TextMsg { message, .. } if message == "#CTs_Win")
    });
    assert!(attrib < notice && notice < info && info < team, "{notice} {attrib} {info} {team}");
    assert!(team < win_audio && team < win_text, "{team} {win_audio} {win_text}");

    // The last counter-terrorist leaves in the middle of a round: it is dormant, so the
    // terrorists do not win anything; `NeededPlayersCheck` finds nobody on the other side, the
    // game goes back to waiting (`#Game_scoring`) and no round is won.
    let mut b = match_under_way(4, &[0, 1], &[2]);
    b.step(frames(2.3));
    b.probe.clear_messages();
    b.run(|game, eng| game.client_disconnect(eng, 2));
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert_eq!(wins(&b), (0, 0));
    assert!(!said(&b, 4, "#Terrorists_Win"));
    assert!(!rules(&b).game_started());
    assert!(said(&b, 2, "#Game_scoring"));
    assert!(player(&b, 0).alive && player(&b, 1).alive);
}

#[test]
fn the_last_ct_leaving_a_one_on_one_wins_nothing() {
    // The probe (slot 0) is the terrorist, slot 1 the counter-terrorist.
    let mut b = match_under_way(3, &[0], &[1]);
    b.step(frames(2.3));
    b.probe.clear_messages();
    b.run(|game, eng| game.client_disconnect(eng, 1));
    b.step(3);
    // No round is won, no win counted: the leaver was not counted, `NeededPlayersCheck` says
    // the game is waiting for players.
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert_eq!(wins(&b), (0, 0));
    assert!(!said(&b, 4, "#Terrorists_Win"));
    assert!(!audio(&b, "%!MRAD_terwin"));
    assert!(!rules(&b).game_started());
    // `CheckWinConditions` is the last thing `ClientDisconnected` does: the scoreboard line and
    // the team went out before `#Game_scoring`.
    let info = position(&b, "leaver ScoreInfo", |m| {
        matches!(m, CsUserMsg::ScoreInfo { player_id: 2, frags: 0, deaths: 0, class_id: 0, team_id: 0 })
    });
    let team = position(&b, "leaver TeamInfo", |m| {
        matches!(m, CsUserMsg::TeamInfo { player_id: 2, team_name } if team_name == "UNASSIGNED")
    });
    let scoring = position(&b, "#Game_scoring", |m| {
        matches!(m, CsUserMsg::TextMsg { dest: 2, message, .. } if message == "#Game_scoring")
    });
    assert!(info < team && team < scoring, "{info} {team} {scoring}");
}

#[test]
fn nothing_is_announced_when_a_player_leaves_during_the_intermission() {
    let mut b = match_under_way(3, &[0], &[1]);
    b.run(|_, eng| {
        eng.set_cvar("mp_maxrounds", "1");
        eng.set_cvar("mp_chattime", "10");
    });
    b.step(frames(1.2));
    kill_player(&mut b, 1);
    b.step(frames(5.5));
    assert!(rules(&b).game_over());
    b.probe.clear_messages();
    b.run(|game, eng| game.client_disconnect(eng, 1));
    b.step(3);
    // `ClientDisconnect` does nothing while `IsGameOver()`: no notice, no emptied line.
    assert!(!said(&b, 1, "#Game_disconnected"));
    assert!(
        !b.messages()
            .iter()
            .any(|m| matches!(m, CsUserMsg::TeamInfo { player_id: 2, .. }))
    );
    assert!(b.server.game().game.state().players[1].is_none());
}

#[test]
fn a_late_joiner_waits_for_the_next_round() {
    let mut b = match_under_way(4, &[0], &[1]);
    // More than 20 seconds into the round, a player that picks a team may not spawn.
    b.step(frames(22.0));
    enter(&mut b, 2);
    join(&mut b, 2, 1, 1);
    let p = player(&b, 2);
    assert_eq!(p.team, Team::Terrorist);
    assert!(!p.alive);
    assert_eq!(p.num_spawns, 0);
    let deadflag = b.run(|_, eng| eng.ent(eng.client_edict(2)).unwrap().deadflag);
    assert_ne!(deadflag, DEAD_NO);

    // The next round brings it in.
    b.run(|_, eng| eng.set_cvar("sv_restartround", "1"));
    b.step(frames(3.5));
    let p = player(&b, 2);
    assert!(p.alive);
    assert_eq!(p.num_spawns, 1);
}

#[test]
fn spectating_in_the_freeze_period_costs_nothing() {
    // Two counter-terrorists, so the round goes on when one of them watches.
    let mut b = match_under_way(4, &[0], &[1, 2]);
    b.run(|game, _| game.state_mut().players[1].as_mut().unwrap().frags = 3);
    // Still frozen: the player may watch although it is alive. The suicide that makes it so
    // costs a frag, which the spectator branch gives back; it counts as a death.
    b.probe.clear_messages();
    command(&mut b, 1, "jointeam 6");
    let p = player(&b, 1);
    assert_eq!(p.team, Team::Spectator);
    assert!(!p.alive);
    assert_eq!((p.frags, p.deaths), (3, 1));
    assert!(p.team_changed);
    b.step(3);
    assert_eq!(rules(&b).win_status(), WinStatus::None);

    // The next round clears `team_changed`, so the spectator may change again.
    b.run(|_, eng| eng.set_cvar("sv_restartround", "1"));
    b.step(frames(3.5));
    let p = player(&b, 1);
    assert_eq!(p.team, Team::Spectator);
    assert!(!p.team_changed);
    assert!(!p.alive);
}

#[test]
fn auto_team_balance_moves_the_last_to_join() {
    // Three terrorists and one counter-terrorist (the third terrorist can only join once the
    // counter-terrorist is there: `mp_limitteams`).
    let mut b = match_under_way(5, &[0], &[3]);
    join_all(&mut b, &[1, 2], 1);
    assert_eq!(player(&b, 2).team, Team::Terrorist);
    // The next restart counts the teams two apart: players are warned and the balance comes
    // with the one after.
    b.probe.clear_messages();
    b.run(|_, eng| eng.set_cvar("sv_restartround", "1"));
    b.step(frames(3.5));
    assert!(said(&b, 4, "#Auto_Team_Balance_Next_Round"));
    assert_eq!(player(&b, 2).team, Team::Terrorist);

    // Still uneven at the next restart: the player that joined last goes over.
    b.probe.clear_messages();
    b.run(|_, eng| eng.set_cvar("sv_restartround", "1"));
    b.step(frames(3.5));
    let moved = player(&b, 2);
    assert_eq!(moved.team, Team::Ct);
    assert_eq!(moved.class, 2, "terror becomes gsg9");
    assert_eq!(player(&b, 1).team, Team::Terrorist);
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 3, team_name } if team_name == "CT"
    )));
    assert!(said(&b, 1, "#Game_join_ct_auto"));
    // It was respawned as a counter-terrorist.
    assert!(moved.alive);

    // With `mp_autoteambalance 0` nothing moves.
    let mut b = match_under_way(5, &[0], &[3]);
    join_all(&mut b, &[1, 2], 1);
    b.run(|_, eng| eng.set_cvar("mp_autoteambalance", "0"));
    for _ in 0..2 {
        b.run(|_, eng| eng.set_cvar("sv_restartround", "1"));
        b.step(frames(3.5));
    }
    assert_eq!(player(&b, 2).team, Team::Terrorist);
}

#[test]
fn nobody_gets_into_the_game_during_the_intermission() {
    // `PreThink` returns after `PlayerThink` while `IsGameOver()`: no `JoiningThink`, so a player
    // that connects during the intermission stays at `SHOWLTEXT` (no team menu, no spawn) however
    // many commands it sends, and one that picks a team and an appearance is not spawned.
    let mut b = match_under_way(4, &[0], &[1]);
    b.run(|_, eng| {
        eng.set_cvar("mp_maxrounds", "1");
        eng.set_cvar("mp_chattime", "10");
    });
    b.step(frames(1.2));
    kill_player(&mut b, 1);
    b.step(frames(5.5));
    assert!(rules(&b).game_over());
    enter(&mut b, 2);
    b.run(|game, eng| {
        game.client_command(eng, 2, &["jointeam".into(), "1".into()]);
        game.client_command(eng, 2, &["joinclass".into(), "1".into()]);
        commands_of(game, eng, 2, 20);
    });
    let p = player(&b, 2);
    assert_eq!((p.joining, p.alive, p.num_spawns), (JoinState::ShowLText, false, 0));
}

#[test]
fn timeleft_answers_as_regamedll() {
    let mut b = match_under_way(3, &[0], &[1]);
    // No limit: `#Game_no_timelimit` in the chat.
    b.probe.clear_messages();
    command(&mut b, 0, "timeleft");
    assert_eq!(texts(&b, 3, "#Game_no_timelimit"), [Vec::<String>::new()]);
    // Not oftener than once a second (`m_iTimeCheckAllowed`).
    command(&mut b, 0, "timeleft");
    assert_eq!(texts(&b, 3, "#Game_no_timelimit").len(), 1);

    // A limit of 10 minutes: 9 minutes and some seconds are left, the seconds in two digits.
    b.run(|_, eng| eng.set_cvar("mp_timelimit", "10"));
    b.step(frames(1.5));
    b.probe.clear_messages();
    command(&mut b, 0, "timeleft");
    let left = texts(&b, 3, "#Game_timelimit");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0][0], "9");
    assert_eq!(left[0][1].len(), 2);
    assert!(left[0][1].parse::<i32>().unwrap() < 60);

    // The limit is over (the match ends): 0 minutes and "00" seconds, never negative.
    b.run(|_, eng| eng.set_cvar("mp_timelimit", "0.01"));
    b.step(frames(1.5));
    b.probe.clear_messages();
    command(&mut b, 0, "timeleft");
    assert_eq!(
        texts(&b, 3, "#Game_timelimit"),
        [vec!["0".to_string(), "00".to_string()]]
    );
}

#[test]
fn listmaps_lists_the_cycle_with_no_votes() {
    let mut b = match_under_way(3, &[0], &[1]);
    b.run(|game, _| game.load_map_cycle(&cycle(&["de_one", "de_two", "de_three"])));
    b.probe.clear_messages();
    command(&mut b, 0, "listmaps");
    // One console line per map, `#Votes` with its number, its name and its votes.
    let lines: Vec<Vec<String>> = texts(&b, 2, "#Votes");
    assert_eq!(
        lines,
        [
            vec!["1".to_string(), "de_one".to_string(), "0".to_string()],
            vec!["2".to_string(), "de_two".to_string(), "0".to_string()],
            vec!["3".to_string(), "de_three".to_string(), "0".to_string()],
        ]
    );
    // Not twice within 0.3 seconds.
    command(&mut b, 0, "listmaps");
    assert_eq!(texts(&b, 2, "#Votes").len(), 3);
    b.step(frames(0.5));
    command(&mut b, 0, "listmaps");
    assert_eq!(texts(&b, 2, "#Votes").len(), 6);
}

#[test]
fn restart_round_resets_the_decals() {
    // `CleanUpMap` ends with `PLAYBACK_EVENT(FEV_GLOBAL | FEV_RELIABLE, 0, m_usResetDecals)`
    // (multiplay_gamerules.cpp 685): every client gets `events/decal_reset.sc` on its reliable
    // stream when a round restarts, once, and not before.
    let mut b = match_under_way(3, &[0], &[1]);
    let event = b.run(|_, eng| eng.precache_event("events/decal_reset.sc"));
    assert_ne!(event, 0, "ClientPrecache precaches the decal reset");
    let resets = |b: &Bench| {
        b.probe
            .messages()
            .iter()
            .filter(|m| matches!(m, SvcMessage::EventReliable { index, .. } if *index == event))
            .count()
    };
    b.probe.clear_messages();
    b.step(frames(1.0));
    assert_eq!(resets(&b), 0, "nothing restarts mid-round");

    b.run(|_, eng| eng.set_cvar("sv_restartround", "1"));
    b.step(frames(3.0));
    assert_eq!(resets(&b), 1, "the restart wipes the decals once");
}

#[test]
fn spectating_in_the_freeze_period_gives_up_the_inventory() {
    // `HandleMenu_ChooseTeam` (client.cpp 1925-1938): after the suicide, `RemoveAllItems(TRUE)`:
    // the weapons, the ammo and the suit go, and the client is told `CurWeapon 0 0 0`.
    let mut b = match_under_way(3, &[1], &[0]);
    b.run(|game, eng| {
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Ak47).unwrap(), 30, 90));
        assert!(game.select_weapon(eng, 0, WeaponId::Ak47));
    });
    assert!(b.server.game().game.carried_weapons(0).contains(&WeaponId::Ak47));
    let before = b.run(|game, eng| game.update_client_data(eng, 0).weapons);
    assert_ne!(before & (1 << WeaponId::Ak47 as u32), 0, "the AK is in `pev->weapons`");

    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 6");
    b.step(3);
    assert_eq!(player(&b, 0).team, Team::Spectator);
    assert!(b.server.game().game.carried_weapons(0).is_empty(), "no weapon is kept");
    let ammo = b.server.game().game.player_ammo(0, ammo_index("762Nato") as usize);
    assert_eq!(ammo, Some(0), "no ammo is kept");
    let after = b.run(|game, eng| game.update_client_data(eng, 0).weapons);
    assert_eq!(after, 0, "`pev->weapons` is empty, the suit's bit too");
    let weapon_messages: Vec<_> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::CurWeapon { is_active, weapon_id, clip_ammo } => {
                Some((is_active, weapon_id, clip_ammo))
            }
            _ => None,
        })
        .collect();
    assert!(
        weapon_messages.contains(&(false, 0, 0)),
        "`CurWeapon 0 0 0` of RemoveAllItems: {weapon_messages:?}"
    );
}
