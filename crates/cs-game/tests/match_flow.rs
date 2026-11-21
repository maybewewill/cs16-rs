//! A whole round from the first connection to the next one (Phase 2C, Task 12): the probe (a real
//! client over loopback, slot 0) enters, is shown the team menu, joins the terrorists with
//! `jointeam`/`joinclass` sent as client strings, plays the freeze period and the round, and
//! kills itself with `kill` as the last living terrorist; the counter-terrorists win, are paid,
//! the scoreboard follows and the next round respawns everybody. The other three players are
//! driven directly (they receive no messages, so what they would see is read from the game's
//! state). The map is `de_dust2` when the game is installed (`CS16_GAMEDIR`), a stub map with
//! spawn points otherwise. Time is counted in server frames of 10 ms, never in wall-clock time.

mod common;

use common::{Bench, command, discover, dust2, enter, join, spawns};
use cs_engine::ServerMap;
use cs_game::menus::Menu;
use cs_game::player::{Player, Team};
use cs_game::rules::{Rules, WinStatus};
use cs_proto48::usermsg::CsUserMsg;

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

fn money(b: &Bench, slot: usize) -> i32 {
    player(b, slot).money
}

fn max_speed(b: &mut Bench, slot: u8) -> f32 {
    b.run(move |_, eng| eng.ent(eng.client_edict(slot)).unwrap().max_speed)
}

fn said(b: &Bench, dest: u8, text: &str) -> bool {
    b.messages().iter().any(
        |m| matches!(m, CsUserMsg::TextMsg { dest: d, message, .. } if *d == dest && message == text),
    )
}

fn audio(b: &Bench, code: &str) -> bool {
    b.messages()
        .iter()
        .any(|m| matches!(m, CsUserMsg::SendAudio { audio_code, .. } if audio_code == code))
}

/// A server of four slots with the probe in slot 0, the freeze time and the round time at two
/// seconds and two minutes' worth of room in the probe's log.
fn bench() -> Bench {
    let map = match discover() {
        Some(vfs) => dust2(&vfs),
        None => ServerMap::stub_with_entities("test", spawns(8, 8)),
    };
    let mut b = Bench::with_slots(map, 4);
    b.idle_cmds = true;
    b.step(6);
    b.run(|_, eng| {
        eng.set_cvar("mp_freezetime", "2");
        eng.set_cvar("mp_roundtime", "2");
    });
    b
}

#[test]
fn two_teams_play_a_round() {
    let mut b = bench();

    // Entering: the probe is in the game with the team menu in front of it, not yet on a team.
    let menu_seen = b.messages().iter().any(|m| {
        matches!(m, CsUserMsg::VguiMenu { menu_id: 2, .. })
            || matches!(m, CsUserMsg::ShowMenu { text, .. } if text == "#Team_Select")
    });
    assert!(menu_seen, "the team menu reached the probe");
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu, p.alive), (Team::Unassigned, Menu::ChooseTeam, false));
    assert!(!rules(&b).game_started());
    b.probe.clear_messages();

    // `jointeam 1` and `joinclass 1`, sent by the client as strings: the terrorists.
    b.probe.send_stringcmd("jointeam 1");
    b.step(5);
    assert_eq!(player(&b, 0).team, Team::Terrorist);
    b.probe.send_stringcmd("joinclass 1");
    b.step(5);
    let p = player(&b, 0);
    assert_eq!((p.team, p.class, p.menu, p.alive), (Team::Terrorist, 1, Menu::Off, true));
    // The rest of the players: a second terrorist and two counter-terrorists.
    enter(&mut b, 1);
    join(&mut b, 1, 1, 1);
    for slot in [2u8, 3] {
        enter(&mut b, slot);
        join(&mut b, slot, 2, 1);
    }

    // Both teams have a player: the game commences and the round restarts into its freeze
    // period; in it nobody moves and everybody has the starting money.
    b.step(frames(3.3));
    assert!(rules(&b).game_started());
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert!(rules(&b).is_freeze_period());
    for slot in 0..4u8 {
        let p = player(&b, usize::from(slot));
        assert!(p.alive, "slot {slot} spawned");
        assert_eq!(money(&b, usize::from(slot)), 800);
        assert_eq!(max_speed(&mut b, slot), 1.0);
    }
    assert!(b.messages().iter().any(|m| matches!(m, CsUserMsg::RoundTime(t) if (1..=2).contains(t))));

    // The freeze period ends: the round runs for `mp_roundtime` and the players can move.
    b.probe.clear_messages();
    b.step(frames(2.3));
    assert!(!rules(&b).is_freeze_period());
    assert_eq!(max_speed(&mut b, 0), 240.0);
    b.step(3);
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

    // One terrorist is out: the round goes on.
    command(&mut b, 1, "kill");
    b.step(3);
    assert!(!player(&b, 1).alive);
    assert_eq!(rules(&b).win_status(), WinStatus::None);

    // The probe, the last living terrorist, kills itself over the wire.
    b.probe.clear_messages();
    b.probe.send_stringcmd("kill");
    b.step(5);
    assert!(!player(&b, 0).alive);
    assert_eq!(rules(&b).win_status(), WinStatus::Cts);
    assert!(said(&b, 4, "#CTs_Win"));
    assert!(audio(&b, "%!MRAD_ctwin"));
    let s = b.server.game().game.state();
    assert_eq!((s.num_terrorist_wins, s.num_ct_wins), (0, 1));
    let msgs = b.messages();
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamScore { team_name, score: 1 } if team_name == "CT"
    )));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamScore { team_name, score: 0 } if team_name == "TERRORIST"
    )));
    // The scoreboard: the suicide as a death notice with the player as its own killer, and the
    // line of the probe with its lost frag and its death (slot 0 is player id 1).
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::DeathMsg { killer_id: 1, victim_id: 1, .. }
    )));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::ScoreInfo { player_id: 1, frags: -1, deaths: 1, team_id: 1, .. }
    )));
    let p = player(&b, 0);
    assert_eq!((p.frags, p.deaths), (-1, 1));

    // Nobody is paid before the next round: the round's reward arrives when it starts.
    assert_eq!((money(&b, 0), money(&b, 2)), (800, 800));
    b.probe.clear_messages();
    b.step(frames(5.3));

    // The next round: everybody alive again in a new freeze period, the score stays, the
    // winners got 3000 (the source's reward) and the losers the loser bonus of 1400.
    assert_eq!(rules(&b).win_status(), WinStatus::None);
    assert!(rules(&b).is_freeze_period());
    for slot in 0..4usize {
        assert!(player(&b, slot).alive, "slot {slot} respawned");
    }
    assert_eq!((money(&b, 2), money(&b, 3)), (3800, 3800));
    assert_eq!((money(&b, 0), money(&b, 1)), (2200, 2200));
    assert_eq!(rules(&b).total_rounds_played(), 1);
    assert!(
        b.messages()
            .iter()
            .any(|m| matches!(m, CsUserMsg::Money { amount: 2200, flash: true }))
    );
    assert_eq!(max_speed(&mut b, 0), 1.0);
    let s = b.server.game().game.state();
    assert_eq!((s.num_terrorist_wins, s.num_ct_wins), (0, 1));
}
