//! Teams, the way into the game and the menus: the team menu a new player gets, `jointeam`,
//! `joinclass`, `chooseteam` and `menuselect`, the limits on the teams, switching teams while
//! alive and what hostile input does. Messages to the players of slots the test did not
//! connect (slot 1 and up) are not delivered, so what those players see is read from the game's
//! state; slot 0 has a probe that sees everything it is sent.

mod common;

use common::{Bench, command, enter, join, spawns};
use cs_engine::GameDll;
use cs_engine::consts::{MOVETYPE_WALK, SOLID_SLIDEBOX};
use cs_game::menus::Menu;
use cs_game::player::{DEAD_DYING, Player, Team};
use cs_game::teams::JoinState;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::usermsg::CsUserMsg;

fn player(b: &Bench, slot: usize) -> Player {
    b.server.game().game.state().players[slot]
        .clone()
        .expect("a player in the slot")
}

/// A bench the probe has reached the team menu on.
fn bench(map: Vec<cs_assets::bsp::Entity>, slots: u8) -> Bench {
    let mut b = Bench::stub_slots(map, slots);
    b.idle_cmds = true;
    b.step(6);
    b
}

/// Runs the map's first freeze period out (it lasts `mp_freezetime` and two seconds from the
/// start of the map), so a round is being played.
fn end_freeze(b: &mut Bench) {
    b.step(800);
    assert!(!b.server.game().game.state().rules.is_freeze_period());
}

fn is_team_menu(m: &CsUserMsg, keys: u16) -> bool {
    matches!(m, CsUserMsg::VguiMenu { menu_id: 2, keys_bitmask, time: -1, multipart: false, name }
        if *keys_bitmask == keys && name == " ")
}

fn is_text(m: &CsUserMsg, dest: u8, text: &str) -> bool {
    matches!(m, CsUserMsg::TextMsg { dest: d, message, .. } if *d == dest && message == text)
}

#[test]
fn new_player_sees_the_team_menu() {
    let mut b = bench(spawns(2, 2), 2);
    let msgs = b.messages();
    // Keys 1, 2, 5 and 6 (the spectators), as `allow_spectators 1` has it.
    let menu = msgs
        .iter()
        .position(|m| is_team_menu(m, 0b11_0011))
        .expect("the VGUI team menu");
    // The menu comes after the empty text menu that closes whatever the client had open.
    let reset = msgs
        .iter()
        .position(
            |m| matches!(m, CsUserMsg::ShowMenu { keys_bitmask: 0, text, .. } if text.is_empty()),
        )
        .expect("the menu reset");
    assert!(reset < menu);
    let p = player(&b, 0);
    assert_eq!(
        (p.team, p.joining, p.menu, p.alive),
        (
            Team::Unassigned,
            JoinState::PickingTeam,
            Menu::ChooseTeam,
            false
        )
    );

    // `allow_spectators 0` offers keys 1, 2 and 5 only.
    b.run(|game, eng| {
        eng.set_cvar("allow_spectators", "0");
        game.client_put_in_server(eng, 0);
    });
    b.step(6);
    assert!(b.messages().iter().any(|m| is_team_menu(m, 0b1_0011)));
}

#[test]
fn text_menus_for_clients_that_want_them() {
    let mut b = bench(spawns(2, 2), 2);
    let vgui_before = b
        .messages()
        .iter()
        .filter(|m| matches!(m, CsUserMsg::VguiMenu { .. }))
        .count();
    b.run(|game, eng| {
        eng.set_client_key(0, "_vgui_menus", "0");
        game.client_userinfo_changed(eng, 0);
        game.client_put_in_server(eng, 0);
    });
    b.step(6);
    let msgs = b.messages();
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::ShowMenu { keys_bitmask: 0b11_0011, display_time: -1, multipart: false, text }
            if text == "#Team_Select_Spect"
    )));
    assert_eq!(
        msgs.iter()
            .filter(|m| matches!(m, CsUserMsg::VguiMenu { .. }))
            .count(),
        vgui_before,
        "no VGUI menu for a client that asked for text ones"
    );

    // `menuselect` answers them: 1 is the terrorists, then 3 is the third appearance.
    command(&mut b, 0, "menuselect 1");
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu), (Team::Terrorist, Menu::ChooseAppearance));
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::ShowMenu { keys_bitmask: 0b1_1111, text, .. } if text == "#Terrorist_Select"
    )));
    command(&mut b, 0, "menuselect 3");
    b.step(2);
    let p = player(&b, 0);
    assert_eq!((p.class, p.menu, p.alive), (3, Menu::Off, true));
}

#[test]
fn jointeam_then_joinclass_spawns_on_the_team() {
    let mut b = bench(spawns(2, 2), 2);
    // The real path: the probe's `stringcmd` reaches the game as a client command.
    b.probe.send_stringcmd("jointeam 1");
    b.step(5);
    let p = player(&b, 0);
    assert_eq!(
        (p.team, p.menu, p.alive),
        (Team::Terrorist, Menu::ChooseAppearance, false)
    );
    let msgs = b.messages();
    // The terrorists' appearance menu: id 26, keys 1 to 5.
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::VguiMenu {
            menu_id: 26,
            keys_bitmask: 0b1_1111,
            time: -1,
            ..
        }
    )));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 1, team_name } if team_name == "TERRORIST"
    )));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::TextMsg { dest: 1, message, params }
            if message == "#Game_join_terrorist" && params == &["p".to_string()]
    )));

    b.probe.send_stringcmd("joinclass 3");
    b.step(5);
    let p = player(&b, 0);
    assert_eq!((p.team, p.class, p.menu), (Team::Terrorist, 3, Menu::Off));
    assert_eq!(p.joining, JoinState::Joined);
    assert!(p.alive && p.takes_damage);
    let (v, model) = b.run(|_, eng| {
        let id = eng.client_edict(0);
        (
            eng.ent(id).unwrap().clone(),
            eng.client_key(0, "model").map(str::to_string),
        )
    });
    assert_eq!(model.as_deref(), Some("arctic"));
    assert_eq!((v.solid, v.movetype), (SOLID_SLIDEBOX, MOVETYPE_WALK));
    // On a terrorist spawn point (`info_player_deathmatch`; the stub map has no floor, so it
    // has begun to fall).
    assert!(v.origin.x < 0.0 && v.origin.z <= 1.0, "{:?}", v.origin);
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::ScoreInfo {
            player_id: 1,
            team_id: 1,
            ..
        }
    )));

    // And it walks, once the freeze period (in which nobody moves) is over.
    end_freeze(&mut b);
    let start = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().origin);
    for _ in 0..100 {
        b.probe.queue_cmd(GoldSrcUserCmd {
            msec: 10,
            forwardmove: 400.0,
            ..Default::default()
        });
    }
    b.step(120);
    let moved = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().origin);
    // The stub map has no floor: it falls, and what it gains forward is what the air allows.
    assert!(
        moved.x > start.x + 2.0 && moved.z < start.z,
        "{start:?} {moved:?}"
    );
}

#[test]
fn a_random_appearance_is_one_of_the_teams() {
    let mut b = bench(spawns(2, 2), 2);
    command(&mut b, 0, "jointeam 2");
    // 0, 5 and anything else out of 1 to 4 pick one at random.
    command(&mut b, 0, "joinclass 9");
    b.step(2);
    let p = player(&b, 0);
    assert!((1..=4).contains(&p.class), "{}", p.class);
    assert!(p.alive);
    let model = b.run(|_, eng| eng.client_key(0, "model").map(str::to_string));
    assert!(["urban", "gsg9", "sas", "gign"].contains(&model.as_deref().unwrap()));
}

#[test]
fn team_limits_refuse_as_regamedll() {
    // A team holds as many players as it has spawn points: one terrorist point, one terrorist.
    let mut b = bench(spawns(3, 1), 3);
    enter(&mut b, 1);
    command(&mut b, 1, "jointeam 1");
    assert_eq!(player(&b, 1).team, Team::Terrorist);
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 1");
    b.step(3);
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu), (Team::Unassigned, Menu::ChooseTeam));
    let msgs = b.messages();
    assert!(msgs.iter().any(|m| is_text(m, 4, "#Terrorists_Full")));
    // The team menu comes up again.
    assert!(msgs.iter().any(|m| is_team_menu(m, 0b1_0011)));

    // `mp_limitteams 2`. Slots 1 and 2 are terrorists, slot 3 a counter-terrorist.
    let mut b = bench(spawns(5, 5), 4);
    for slot in 1..4 {
        enter(&mut b, slot);
    }
    command(&mut b, 1, "jointeam 1");
    command(&mut b, 2, "jointeam 1");
    command(&mut b, 3, "jointeam 2");
    assert_eq!(
        (player(&b, 1).team, player(&b, 2).team, player(&b, 3).team),
        (Team::Terrorist, Team::Terrorist, Team::Ct)
    );
    // A counter-terrorist that changes teams leaves one less behind: 2 + 1 > 1 + (2 - 1).
    command(&mut b, 3, "jointeam 1");
    assert_eq!(
        player(&b, 3).team,
        Team::Ct,
        "stacked for a player of the other team"
    );
    // A newcomer counts for the whole limit: 2 + 1 > 1 + 2 is false.
    command(&mut b, 0, "jointeam 1");
    assert_eq!(player(&b, 0).team, Team::Terrorist);
    // A player may always rechoose its own team, however many it is over.
    command(&mut b, 1, "jointeam 1");
    assert_eq!(player(&b, 1).team, Team::Terrorist);

    // The newcomer that is one too many is told so and shown the menu again.
    let mut b = bench(spawns(5, 5), 3);
    for slot in 1..3 {
        enter(&mut b, slot);
        command(&mut b, slot, "jointeam 1");
    }
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 1");
    assert_eq!(player(&b, 0).team, Team::Unassigned);
    let msgs = b.messages();
    assert!(msgs.iter().any(|m| is_text(m, 4, "#Too_Many_Terrorists")));
    assert!(msgs.iter().any(|m| is_team_menu(m, 0b1_0011)));
    // Without a limit it is let in.
    b.run(|_, eng| eng.set_cvar("mp_limitteams", "0"));
    command(&mut b, 0, "jointeam 1");
    assert_eq!(player(&b, 0).team, Team::Terrorist);
}

#[test]
fn auto_assign_balances() {
    // Slots 1 and 2 are terrorists; the player who asks for any team gets the smaller one.
    let mut b = bench(spawns(3, 3), 4);
    for slot in 1..3 {
        enter(&mut b, slot);
        command(&mut b, slot, "jointeam 1");
    }
    command(&mut b, 0, "jointeam 5");
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu), (Team::Ct, Menu::ChooseAppearance));
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::VguiMenu {
            menu_id: 27,
            keys_bitmask: 0b1_1111,
            ..
        }
    )));
    // With one counter-terrorist to two terrorists the next one is a counter-terrorist again.
    enter(&mut b, 3);
    command(&mut b, 3, "jointeam 5");
    assert_eq!(player(&b, 3).team, Team::Ct);

    // With even teams either does.
    let mut b = bench(spawns(3, 3), 2);
    command(&mut b, 0, "jointeam 5");
    let p = player(&b, 0);
    assert!(matches!(p.team, Team::Ct | Team::Terrorist));

    // Both teams full: refused (vanilla would put the player in the game with no team).
    let mut b = bench(spawns(1, 1), 3);
    enter(&mut b, 1);
    enter(&mut b, 2);
    command(&mut b, 1, "jointeam 1");
    command(&mut b, 2, "jointeam 2");
    command(&mut b, 0, "jointeam 5");
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu), (Team::Unassigned, Menu::ChooseTeam));
}

#[test]
fn switching_team_alive_kills_as_regamedll() {
    let mut b = bench(spawns(3, 3), 2);
    join(&mut b, 0, 1, 1);
    assert!(player(&b, 0).alive);
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().frags = 5);
    b.probe.clear_messages();

    command(&mut b, 0, "jointeam 2");
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu), (Team::Ct, Menu::ChooseAppearance));
    assert!(!p.alive && !p.takes_damage && p.team_changed);
    // A suicide: a frag less and a death more.
    assert_eq!((p.frags, p.deaths), (4, 1));
    let deadflag = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().deadflag);
    assert_eq!(deadflag, DEAD_DYING);
    b.step(2);
    let msgs = b.messages();
    let new = &msgs;
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::DeathMsg { killer_id: 1, victim_id: 1, weapon, .. } if weapon == "world"
    )));
    // The death is announced under the team the player is leaving.
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::ScoreInfo {
            player_id: 1,
            frags: 4,
            deaths: 1,
            team_id: 1,
            ..
        }
    )));
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 1, team_name } if team_name == "CT"
    )));
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::ScoreAttrib {
            player_id: 1,
            flags: 1
        }
    )));
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::VguiMenu {
            menu_id: 27,
            keys_bitmask: 0b1_1111,
            ..
        }
    )));
    // The class menu is open: `jointeam` is not available.
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 1");
    assert_eq!(player(&b, 0).team, Team::Ct);
    assert!(
        b.messages()
            .iter()
            .any(|m| is_text(m, 4, "#Command_Not_Available"))
    );

    // It wears the new appearance from its next spawn; it is not spawned in the middle of a round.
    command(&mut b, 0, "joinclass 2");
    b.step(3);
    let p = player(&b, 0);
    assert_eq!((p.class, p.menu, p.alive), (2, Menu::Off, false));

    // A dead player that changed teams once may not do it again.
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 1");
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu), (Team::Ct, Menu::ChooseTeam));
    assert!(
        b.messages()
            .iter()
            .any(|m| is_text(m, 4, "#Only_1_Team_Change"))
    );
}

#[test]
fn spectators() {
    let mut b = bench(spawns(3, 3), 2);
    // A player on its way into the game may watch.
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 6");
    let p = player(&b, 0);
    assert_eq!(
        (p.team, p.joining, p.menu),
        (Team::Spectator, JoinState::Joined, Menu::Off)
    );
    assert_eq!(p.money, 0);
    let msgs = b.messages();
    let new = &msgs;
    assert!(
        new.iter()
            .any(|m| matches!(m, CsUserMsg::Money { amount: 0, .. }))
    );
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 1, team_name } if team_name == "SPECTATOR"
    )));
    // The scoreboard line goes out with no team, as the original sends it.
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::ScoreInfo {
            player_id: 1,
            team_id: 0,
            ..
        }
    )));

    // Watching counts as the one change of teams a round allows, and a spectator is dead.
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 2");
    assert_eq!(player(&b, 0).team, Team::Spectator);
    assert!(
        b.messages()
            .iter()
            .any(|m| is_text(m, 4, "#Only_1_Team_Change"))
    );

    // A new round lets it change. It starts afresh and waits for the next round.
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().team_changed = false);
    command(&mut b, 0, "jointeam 2");
    b.step(3);
    let p = player(&b, 0);
    assert_eq!(
        (p.team, p.menu, p.money),
        (Team::Ct, Menu::ChooseAppearance, 800)
    );
    command(&mut b, 0, "joinclass 4");
    b.step(3);
    let p = player(&b, 0);
    assert_eq!(
        (p.class, p.menu, p.joining, p.alive),
        (4, Menu::Off, JoinState::Joined, false)
    );
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 1, team_name } if team_name == "CT"
    )));

    // A living player may not watch (outside the freeze period).
    let mut b = bench(spawns(3, 3), 2);
    join(&mut b, 0, 1, 1);
    end_freeze(&mut b);
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().frags = 3);
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 6");
    let p = player(&b, 0);
    assert_eq!(
        (p.team, p.alive, p.frags, p.deaths),
        (Team::Terrorist, true, 3, 0)
    );
    assert!(
        b.messages()
            .iter()
            .any(|m| is_text(m, 4, "#Cannot_Be_Spectator"))
    );

    // Nor may anyone when the server does not allow spectators.
    let mut b = bench(spawns(3, 3), 2);
    b.run(|_, eng| eng.set_cvar("allow_spectators", "0"));
    command(&mut b, 0, "jointeam 6");
    assert_eq!(player(&b, 0).team, Team::Unassigned);
}

#[test]
fn chooseteam_shows_the_in_game_menu() {
    let mut b = bench(spawns(3, 3), 2);
    join(&mut b, 0, 2, 1);
    end_freeze(&mut b);
    b.probe.clear_messages();
    command(&mut b, 0, "chooseteam");
    b.step(2);
    // Alive: keys 1, 2, 5 and 0.
    assert!(b.messages().iter().any(|m| is_team_menu(m, 0b10_0001_0011)));
    assert_eq!(player(&b, 0).menu, Menu::ChooseTeam);

    // Dead (or in the freeze period) the spectators' key is offered too. A new round let the
    // player change teams again.
    b.run(|game, eng| {
        game.damage_player(eng, 0, None, 500.0, 0);
        game.state_mut().players[0].as_mut().unwrap().team_changed = false;
    });
    b.probe.clear_messages();
    command(&mut b, 0, "chooseteam");
    b.step(2);
    assert!(b.messages().iter().any(|m| is_team_menu(m, 0b10_0011_0011)));

    // Not while the appearance menu is open.
    let mut b = bench(spawns(3, 3), 2);
    command(&mut b, 0, "jointeam 1");
    b.probe.clear_messages();
    command(&mut b, 0, "chooseteam");
    b.step(2);
    assert!(!b.messages().iter().any(|m| is_team_menu(m, 0b10_0001_0011)));
    assert_eq!(player(&b, 0).menu, Menu::ChooseAppearance);
}

#[test]
fn humans_join_team_sends_players_to_that_team() {
    let mut b = bench(spawns(3, 3), 2);
    b.run(|game, eng| {
        eng.set_cvar("humans_join_team", "CT");
        game.client_put_in_server(eng, 0);
    });
    b.step(6);
    let p = player(&b, 0);
    assert_eq!((p.team, p.menu), (Team::Ct, Menu::ChooseAppearance));
    // The other team is refused.
    command(&mut b, 0, "joinclass 1");
    b.step(2);
    b.probe.clear_messages();
    command(&mut b, 0, "chooseteam");
    command(&mut b, 0, "jointeam 1");
    assert_eq!(player(&b, 0).team, Team::Ct);
    assert!(
        b.messages()
            .iter()
            .any(|m| is_text(m, 4, "#Humans_Join_Team_CT"))
    );
}

#[test]
fn every_team_is_told_to_the_joiner() {
    let mut b = bench(spawns(3, 3), 2);
    enter(&mut b, 1);
    command(&mut b, 1, "jointeam 1");
    // The player of slot 0 enters again: it is told the team of everyone in the game.
    b.probe.clear_messages();
    b.run(|game, eng| game.client_put_in_server(eng, 0));
    b.step(2);
    let new = b.messages();
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 2, team_name } if team_name == "TERRORIST"
    )));
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 1, team_name } if team_name == "UNASSIGNED"
    )));
    // Everyone is told when somebody leaves.
    b.probe.clear_messages();
    b.run(|game, eng| game.client_disconnect(eng, 1));
    b.step(2);
    let new = b.messages();
    assert!(new.iter().any(|m| matches!(
        m,
        CsUserMsg::TeamInfo { player_id: 2, team_name } if team_name == "UNASSIGNED"
    )));
    assert!(
        new.iter()
            .any(|m| matches!(m, CsUserMsg::ScoreInfo { player_id: 2, .. }))
    );
}

/// What a player is, to see that nothing changed.
fn snapshot(b: &mut Bench) -> (Team, JoinState, bool, i32, i32, i32, u8) {
    let p = player(b, 0);
    (
        p.team, p.joining, p.alive, p.frags, p.deaths, p.money, p.class,
    )
}

#[test]
fn hostile_team_commands_are_ignored() {
    let mut b = bench(spawns(3, 3), 2);
    let long = "9".repeat(4096);
    let before = snapshot(&mut b);
    assert_eq!(before.0, Team::Unassigned);

    let run_argv = |b: &mut Bench, argv: Vec<String>| {
        b.run(move |game, eng| game.client_command(eng, 0, &argv));
    };
    let hostile_lines: Vec<Vec<String>> = vec![
        vec![],
        vec![String::new()],
        vec!["jointeam".into()],
        vec!["jointeam".into(), String::new()],
        vec!["jointeam".into(), "999".into()],
        vec!["jointeam".into(), "-1".into()],
        vec!["jointeam".into(), "0".into()],
        vec!["jointeam".into(), "3".into()],
        vec!["jointeam".into(), "abc".into()],
        vec!["jointeam".into(), "99999999999999999999".into()],
        vec!["jointeam".into(), long.clone()],
        vec!["JOINTEAM".into(), "1".into()],
        vec!["joinclass".into()],
        vec!["joinclass".into(), "1".into()],
        vec!["joinclass".into(), long.clone()],
        vec!["menuselect".into()],
        vec!["menuselect".into(), "0".into()],
        vec!["menuselect".into(), "10".into()],
        vec!["menuselect".into(), "abc".into()],
        vec!["menuselect".into(), "-7".into()],
        vec!["menuselect".into(), long.clone()],
        vec!["menuselect".into(), "1".into()],
        vec!["chooseteam".into(), long.clone()],
        vec![long.clone()],
    ];
    for argv in hostile_lines {
        run_argv(&mut b, argv);
        // The VGUI client answers the menu with `jointeam`: `menuselect` does nothing, and
        // nothing above puts the player on a team or in the game.
        assert_eq!(snapshot(&mut b), before);
    }

    // The same with a client that uses text menus, the answers `menuselect` takes (a key the
    // menu has none for only shows it again).
    b.run(|game, eng| {
        eng.set_client_key(0, "_vgui_menus", "0");
        game.client_userinfo_changed(eng, 0);
    });
    for key in ["0", "10", "abc", "-7", "99", "3", "4", ""] {
        let argv = vec!["menuselect".to_string(), key.to_string()];
        run_argv(&mut b, argv);
        assert_eq!(snapshot(&mut b), before, "menuselect {key:?}");
    }
    // Commands from a slot with no player at all.
    b.run(|game, eng| {
        game.client_command(eng, 1, &["jointeam".to_string(), "1".to_string()]);
        game.client_command(eng, 31, &["menuselect".to_string(), "1".to_string()]);
        game.client_command(eng, 200, &["joinclass".to_string(), "1".to_string()]);
    });
    assert!(b.server.game().game.state().players[1].is_none());
    assert_eq!(snapshot(&mut b), before);
    // And the real thing still works.
    command(&mut b, 0, "menuselect 2");
    assert_eq!(player(&b, 0).team, Team::Ct);
}

#[test]
fn commands_before_the_player_is_in_the_game_are_harmless() {
    // The slot has no player until the engine puts one in: no panic, no player.
    let mut b = bench(spawns(2, 2), 3);
    b.run(|game, eng| {
        for line in [
            "jointeam 1",
            "joinclass 1",
            "menuselect 1",
            "chooseteam",
            "kill",
        ] {
            let argv: Vec<String> = line.split(' ').map(str::to_string).collect();
            game.client_command(eng, 2, &argv);
        }
    });
    assert!(b.server.game().game.state().players[2].is_none());

    // Right after it entered (before any menu) `joinclass` is refused.
    b.run(|game, eng| {
        game.client_put_in_server(eng, 2);
        game.client_command(eng, 2, &["joinclass".to_string(), "1".to_string()]);
        game.client_command(eng, 2, &["jointeam".to_string(), "1".to_string()]);
    });
    let p = player(&b, 2);
    assert_eq!(p.team, Team::Terrorist);
    assert!(!p.alive);
}

fn menu_messages(b: &Bench) -> usize {
    b.messages()
        .iter()
        .filter(|m| matches!(m, CsUserMsg::VguiMenu { .. } | CsUserMsg::ShowMenu { .. }))
        .count()
}

#[test]
fn menus_only_reach_a_client_that_is_in_game() {
    // The engine puts the player in the server before the client has begun; the menus wait for
    // its first command (`PlayerPreThink`), however many frames pass. A client always sends a
    // move (a placeholder of 0 ms when it has no command), and the game runs it like any other,
    // so this client is one that sends none: the probe's `suppress_moves`.
    let mut b = Bench::stub_without_moves(spawns(2, 2));
    b.step(30);
    assert_eq!(menu_messages(&b), 0);
    let p = player(&b, 0);
    assert_eq!((p.joining, p.menu), (JoinState::ShowLText, Menu::Off));

    // The first commands bring the menu.
    b.probe.suppress_moves(false);
    for _ in 0..4 {
        b.probe.queue_cmd(GoldSrcUserCmd {
            msec: 10,
            ..Default::default()
        });
    }
    b.step(6);
    assert!(b.messages().iter().any(|m| is_team_menu(m, 0b11_0011)));
    let p = player(&b, 0);
    assert_eq!(
        (p.joining, p.menu),
        (JoinState::PickingTeam, Menu::ChooseTeam)
    );
}

#[test]
fn the_connect_userinfo_chooses_the_menu_kind() {
    // A client whose `connect` userinfo says `_vgui_menus 0` gets the text menus: the key
    // travels through the engine's userinfo into the game's `client_key` lookup.
    let map = cs_engine::ServerMap::stub_with_entities("test", spawns(2, 2));
    let mut b = Bench::with_userinfo(map, 2, &[("_vgui_menus", "0")]);
    b.idle_cmds = true;
    b.step(8);
    assert!(!player(&b, 0).vgui_menus);
    let msgs = b.messages();
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::ShowMenu { keys_bitmask: 0b11_0011, text, .. } if text == "#Team_Select_Spect"
    )));
    assert!(!msgs.iter().any(|m| matches!(m, CsUserMsg::VguiMenu { .. })));

    // And the default userinfo of the original client (`_vgui_menus 1`) gets the VGUI one.
    let b = bench(spawns(2, 2), 2);
    assert!(player(&b, 0).vgui_menus);
}

#[test]
fn the_first_joining_think_resets_the_menu_hides_the_defuser_and_sends_the_item_status() {
    // `JoiningThink`, `SHOWLTEXT` (vanilla, not `REGAMEDLL_FIXES`; player.cpp:3820, read
    // 2026-11-22): `ResetMenu`, the defuser status icon hidden, `SendItemStatus`; the team menu
    // follows from `PlayerThink`.
    let b = bench(spawns(2, 2), 2);
    let msgs = b.messages();
    let reset = msgs
        .iter()
        .position(
            |m| matches!(m, CsUserMsg::ShowMenu { keys_bitmask: 0, text, .. } if text.is_empty()),
        )
        .expect("the menu reset");
    let defuser = msgs
        .iter()
        .position(|m| {
            matches!(m, CsUserMsg::StatusIcon { status: 0, sprite_name, .. } if sprite_name == "defuser")
        })
        .expect("the defuser icon hidden");
    let items = msgs
        .iter()
        .position(|m| matches!(m, CsUserMsg::ItemStatus(0)))
        .expect("the item status");
    let menu = msgs
        .iter()
        .position(|m| is_team_menu(m, 0b11_0011))
        .expect("the team menu");
    assert!(reset < defuser && defuser < items && items < menu, "{reset} {defuser} {items} {menu}");
}

#[test]
fn the_default_team_is_the_one_with_fewer_round_wins() {
    // `SelectDefaultTeam` (the vanilla branch): even teams go to the team that has won fewer
    // rounds, a draw of the random number only when the wins are even too.
    let mut b = bench(spawns(3, 3), 2);
    b.run(|game, _| game.state_mut().num_ct_wins = 2);
    command(&mut b, 0, "jointeam 5");
    assert_eq!(player(&b, 0).team, Team::Terrorist);

    let mut b = bench(spawns(3, 3), 2);
    b.run(|game, _| game.state_mut().num_terrorist_wins = 1);
    command(&mut b, 0, "jointeam 5");
    assert_eq!(player(&b, 0).team, Team::Ct);

    // Fewer players come first: the terrorists lead on wins but the counter-terrorists have more
    // players, so the terrorists are the default again (they are the team with fewer players).
    let mut b = bench(spawns(3, 3), 3);
    enter(&mut b, 1);
    command(&mut b, 1, "jointeam 2");
    b.run(|game, _| game.state_mut().num_terrorist_wins = 3);
    command(&mut b, 0, "jointeam 5");
    assert_eq!(player(&b, 0).team, Team::Terrorist);
}

#[test]
fn joining_the_spectators_broadcasts_the_scoreboard_line_unreliably() {
    // The scoreboard line of a player that watches is `MSG_BROADCAST` (client.cpp:1962, read
    // 2026-11-22; `MSG_ALL` only with `REGAMEDLL_FIXES`): it goes out after the reliable
    // messages of the same command (`TeamInfo`, `ScoreAttrib`, `Spectator`).
    let mut b = bench(spawns(3, 3), 2);
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 6");
    let msgs = b.messages();
    let info = msgs
        .iter()
        .position(|m| matches!(m, CsUserMsg::ScoreInfo { player_id: 1, team_id: 0, .. }))
        .expect("the scoreboard line");
    let team = msgs
        .iter()
        .position(|m| matches!(m, CsUserMsg::TeamInfo { player_id: 1, team_name } if team_name == "SPECTATOR"))
        .expect("the team");
    let attrib = msgs
        .iter()
        .position(|m| matches!(m, CsUserMsg::ScoreAttrib { player_id: 1, .. }))
        .expect("the scoreboard marks");
    assert!(team < info && attrib < info, "{team} {attrib} {info}");
}
