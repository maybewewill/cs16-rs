//! Chat, names and text messages: `say` and `say_team` with their formats and who hears them, the
//! chat of the dead, a player changing its name, and what hostile text does. The probe in slot 0
//! is the receiver the tests read from (the other slots have no client, and a message to them
//! is not delivered); the senders are the other slots, whose commands the tests run.

mod common;

use common::{Bench, command, enter, join, spawns};
use cs_engine::{EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_proto48::usermsg::CsUserMsg;

/// A bench with the probe (slot 0) on the terrorists, slot 1 a counter-terrorist and slot 2 a
/// terrorist.
fn bench() -> Bench {
    let mut b = Bench::stub_slots(spawns(3, 3), 3);
    b.idle_cmds = true;
    b.step(6);
    join(&mut b, 0, 1, 1);
    enter(&mut b, 1);
    join(&mut b, 1, 2, 1);
    enter(&mut b, 2);
    join(&mut b, 2, 1, 1);
    // Let the freeze period of the map's first round run out, so nothing restarts the round (and
    // respawns the dead) while a test talks.
    // (Players joining start the game, which restarts the round once more.)
    for _ in 0..40 {
        b.step(100);
        let rules = &b.server.game().game.state().rules;
        if !rules.is_freeze_period() && b.now > 30.0 {
            break;
        }
    }
    assert!(!b.server.game().game.state().rules.is_freeze_period());
    for (slot, team) in [(0, Team::Terrorist), (1, Team::Ct), (2, Team::Terrorist)] {
        let p = b.server.game().game.state().players[slot]
            .clone()
            .expect("a player");
        assert_eq!((p.team, p.alive), (team, true), "slot {slot}");
    }
    b
}

/// Lets the flood timers of the chat run out (0.7 s).
fn wait_out_flood(b: &mut Bench) {
    b.step(70);
}

/// Runs `argv` as the player of `slot` once the flood timers have run out and returns what the
/// probe was sent as `SayText`: the sender index, the format key and the parameters.
fn say(b: &mut Bench, slot: u8, argv: &[&str]) -> Vec<(u8, String, Vec<String>)> {
    wait_out_flood(b);
    b.probe.clear_messages();
    let argv: Vec<String> = argv.iter().map(|s| (*s).to_string()).collect();
    b.run(move |game, eng| game.client_command(eng, slot, &argv));
    b.step(4);
    say_texts(b)
}

fn say_texts(b: &Bench) -> Vec<(u8, String, Vec<String>)> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::SayText {
                sender_id,
                text,
                params,
            } => Some((sender_id, text, params)),
            _ => None,
        })
        .collect()
}

/// The line the probe got for a `say` of `text` by `slot`, if it got one.
fn heard(b: &mut Bench, slot: u8, text: &str) -> Option<String> {
    let got = say(b, slot, &["say", text]);
    assert!(got.len() <= 1, "{got:?}");
    got.into_iter().next().map(|(_, _, params)| {
        assert_eq!(params.len(), 2);
        params[1].clone()
    })
}

fn kill(b: &mut Bench, slot: u8) {
    b.run(move |game, eng| game.damage_player(eng, slot, None, 500.0, 0));
    b.step(2);
}

fn set_name(b: &mut Bench, slot: u8, name: &'static str) {
    b.probe.clear_messages();
    b.run(move |game: &mut CsGame, eng: &mut EngineApi<'_>| {
        eng.set_client_key(slot, "name", name);
        game.client_userinfo_changed(eng, slot);
    });
    b.step(4);
}

fn name_of(b: &mut Bench, slot: u8) -> String {
    b.run(move |_, eng| eng.client_key(slot, "name").unwrap_or("").to_string())
}

#[test]
fn say_reaches_everyone_with_the_regamedll_format() {
    let mut b = bench();
    // Slot 1 (a counter-terrorist) speaks: the probe (a terrorist) hears it, from index 2.
    let got = say(&mut b, 1, &["say", "hello", "world"]);
    assert_eq!(
        got,
        vec![(
            2,
            "#Cstrike_Chat_All".to_string(),
            vec![String::new(), "hello world\n".to_string()]
        )]
    );
    // The probe speaks and hears itself, from index 1.
    let got = say(&mut b, 0, &["say", "gl hf"]);
    assert_eq!(
        got,
        vec![(
            1,
            "#Cstrike_Chat_All".to_string(),
            vec![String::new(), "gl hf\n".to_string()]
        )]
    );
}

#[test]
fn say_team_reaches_the_team_only() {
    let mut b = bench();
    // A counter-terrorist's team line is not for the probe, a terrorist.
    assert!(say(&mut b, 1, &["say_team", "rush", "b"]).is_empty());
    // A terrorist's is.
    let got = say(&mut b, 2, &["say_team", "rush", "b"]);
    assert_eq!(
        got,
        vec![(
            3,
            "#Cstrike_Chat_T".to_string(),
            vec![String::new(), "rush b\n".to_string()]
        )]
    );
    // The probe's own team line reaches itself, with the terrorists' key.
    let got = say(&mut b, 0, &["say_team", "hold"]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, "#Cstrike_Chat_T");
    // And the counter-terrorists' key is the other one.
    kill(&mut b, 0);
    let got = say(&mut b, 0, &["say_team", "oops"]);
    assert_eq!(got[0].1, "#Cstrike_Chat_T_Dead");
}

#[test]
fn dead_chat_as_regamedll() {
    let mut b = bench();
    kill(&mut b, 0);
    // The dead probe does not hear the living...
    assert!(say(&mut b, 1, &["say", "alive"]).is_empty());
    assert!(say(&mut b, 2, &["say_team", "alive"]).is_empty());
    // ...but hears the dead, with the dead keys, team or not.
    kill(&mut b, 1);
    let got = say(&mut b, 1, &["say", "dead"]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, "#Cstrike_Chat_AllDead");
    // A dead counter-terrorist's team line is for the counter-terrorists only.
    assert!(say(&mut b, 1, &["say_team", "dead"]).is_empty());
    kill(&mut b, 2);
    let got = say(&mut b, 2, &["say_team", "dead"]);
    assert_eq!(got[0].1, "#Cstrike_Chat_T_Dead");
    // The dead speaker hears itself.
    let got = say(&mut b, 0, &["say", "still here"]);
    assert_eq!((got[0].0, got[0].1.as_str()), (1, "#Cstrike_Chat_AllDead"));
    // A living player does not hear the dead.
    b.run(|game, eng| game.respawn_player(eng, 0));
    b.step(2);
    assert!(say(&mut b, 1, &["say", "dead"]).is_empty());
}

#[test]
fn ignoremsg_mutes_the_global_chat() {
    let mut b = bench();
    b.probe.clear_messages();
    command(&mut b, 0, "ignoremsg");
    assert!(b.messages().iter().any(|m| matches!(m,
        CsUserMsg::TextMsg { dest: 4, message, .. } if message == "#Ignore_Broadcast_Messages")));
    // Its own team still gets through, the other team does not.
    assert!(say(&mut b, 1, &["say", "enemy"]).is_empty());
    assert_eq!(say(&mut b, 2, &["say", "friend"]).len(), 1);
    command(&mut b, 0, "ignoremsg");
    assert!(say(&mut b, 2, &["say", "friend"]).is_empty());
    command(&mut b, 0, "ignoremsg");
    assert_eq!(say(&mut b, 1, &["say", "enemy"]).len(), 1);
}

#[test]
fn name_change_is_announced_and_limited() {
    let mut b = bench();
    // A living player's new name is announced to everyone, from its index.
    set_name(&mut b, 0, "newname");
    let announced: Vec<_> = say_texts(&b);
    assert_eq!(
        announced,
        vec![(
            1,
            "#Cstrike_Name_Change".to_string(),
            vec!["p".to_string(), "newname".to_string()]
        )]
    );
    assert_eq!(name_of(&mut b, 0), "newname");
    // The same name again says nothing.
    set_name(&mut b, 0, "newname");
    assert!(say_texts(&b).is_empty());

    // `%`, `&` and a leading `#` are not allowed in a name.
    set_name(&mut b, 0, "#a%b&c");
    assert_eq!(name_of(&mut b, 0), "*a b c");
    assert_eq!(say_texts(&b)[0].2, ["newname", "*a b c"]);

    // A dead player must wait for its next spawn: told so, name kept, nothing announced.
    kill(&mut b, 0);
    set_name(&mut b, 0, "later");
    assert!(say_texts(&b).is_empty());
    assert_eq!(name_of(&mut b, 0), "*a b c");
    assert!(b.messages().iter().any(|m| matches!(m,
        CsUserMsg::TextMsg { dest: 3, message, .. } if message == "#Name_change_at_respawn")));
    b.probe.clear_messages();
    b.run(|game, eng| game.respawn_player(eng, 0));
    b.step(4);
    assert_eq!(name_of(&mut b, 0), "later");
    assert!(say_texts(&b).is_empty());
}

#[test]
fn name_changes_through_the_engine_are_rate_limited() {
    let mut b = bench();
    b.probe.clear_messages();
    b.probe.send_stringcmd("setinfo name first");
    b.probe.send_stringcmd("setinfo name second");
    b.step(6);
    // The engine accepts the first and drops the second; the game announces one change.
    assert_eq!(say_texts(&b).len(), 1);
    assert_eq!(name_of(&mut b, 0), "first");
}

#[test]
fn say_is_sanitized_and_bounded() {
    let mut b = bench();
    // `%` that begins anything but `%l` is a space; a `#` key is only text.
    assert_eq!(
        heard(&mut b, 1, "a%sb %d %l x %").as_deref(),
        Some("a sb  d %l x %\n")
    );
    assert_eq!(
        heard(&mut b, 1, "#Cstrike_Chat_All").as_deref(),
        Some("#Cstrike_Chat_All\n")
    );
    // Control characters go, and the white space at the ends.
    assert_eq!(
        heard(&mut b, 1, "\u{1}a\u{2}\n b\t\u{7f} ").as_deref(),
        Some("a b\u{7f}\n")
    );
    assert_eq!(heard(&mut b, 1, "  \u{202e}hi \u{200b}").as_deref(), Some("hi\n"));
    // The quotes of a quoted line are removed (and the last character with them).
    assert_eq!(heard(&mut b, 1, "\"hi there\"").as_deref(), Some("hi there\n"));
    // Nothing to say: nothing is sent.
    for text in ["", " ", "\u{1}\u{2}", "\t\n", "\"", "\u{200b}"] {
        assert_eq!(heard(&mut b, 1, text), None, "{text:?}");
    }
    assert!(say(&mut b, 1, &["say"]).is_empty());
    // A long line is cut to what the message holds: 128 less the key, the newline and the
    // terminator.
    let long = "x".repeat(4096);
    let got = heard(&mut b, 1, &long).expect("a line");
    assert_eq!(got.len(), 128 - 3 - "#Cstrike_Chat_All".len() + 1);
    assert!(got.ends_with("x\n"));
    // A cut never splits a character.
    let long = "\u{e9}".repeat(300);
    let got = heard(&mut b, 1, &long).expect("a line");
    assert!(got.len() <= 109 && got.trim_end().chars().all(|c| c == '\u{e9}'));
    // An argument with embedded NUL stays inside its message.
    let got = heard(&mut b, 1, "a\0b").expect("a line");
    assert_eq!(got, "ab\n");
}

#[test]
fn chat_floods_are_dropped() {
    let mut b = bench();
    wait_out_flood(&mut b);
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.client_command(eng, 1, &["say".to_string(), "one".to_string()]);
    });
    b.step(10);
    b.run(|game, eng| {
        game.client_command(eng, 1, &["say".to_string(), "two".to_string()]);
    });
    b.step(4);
    let got = say_texts(&b);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].2[1], "one\n");
}

#[test]
fn a_line_right_after_a_spawn_is_heard() {
    let mut b = bench();
    assert_eq!(say(&mut b, 0, &["say", "first"]).len(), 1);
    // Spawning clears the flood timers: no waiting needed.
    b.run(|game, eng| game.respawn_player(eng, 0));
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.client_command(eng, 0, &["say".to_string(), "again".to_string()]);
    });
    b.step(4);
    let got = say_texts(&b);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].2[1], "again\n");
}

#[test]
fn spectators_and_unassigned_players_chat_as_the_dead() {
    let mut b = bench();
    // Slot 1 goes to the spectators, the probe too (a spectator is a dead player).
    // (A living player may only become a spectator in the freeze period, so they die first.)
    kill(&mut b, 0);
    kill(&mut b, 1);
    command(&mut b, 1, "jointeam 6");
    command(&mut b, 0, "jointeam 6");
    let team = |b: &Bench, s: usize| b.server.game().game.state().players[s].as_ref().unwrap().team;
    assert_eq!((team(&b, 0), team(&b, 1)), (Team::Spectator, Team::Spectator));
    // Slot 2 stays a terrorist and alive: the spectator probe does not hear it.
    assert!(say(&mut b, 2, &["say", "alive"]).is_empty());
    let got = say(&mut b, 1, &["say", "spec"]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, "#Cstrike_Chat_AllSpec");
    let got = say(&mut b, 1, &["say_team", "spec"]);
    assert_eq!(got[0].1, "#Cstrike_Chat_Spec");
    // The probe itself, a spectator, is heard by itself with the same keys.
    let got = say(&mut b, 0, &["say_team", "me"]);
    assert_eq!((got[0].0, got[0].1.as_str()), (1, "#Cstrike_Chat_Spec"));

    // A player that has not joined (slot 2 after it is made to leave its team) is dead and on
    // no team: "dead" for all, and its team line is for the unassigned only.
    b.run(|game, _| {
        let p = game.state_mut().players[2].as_mut().unwrap();
        p.team = Team::Unassigned;
    });
    b.run(|game, eng| game.damage_player(eng, 2, None, 500.0, 0));
    b.step(2);
    let got = say(&mut b, 2, &["say", "hi"]);
    assert_eq!(got[0].1, "#Cstrike_Chat_AllDead");
    assert!(say(&mut b, 2, &["say_team", "hi"]).is_empty());
}

#[test]
fn a_name_is_cut_to_31_bytes() {
    let mut b = bench();
    set_name(&mut b, 0, "abcdefghijklmnopqrstuvwxyz0123456789ABCD");
    assert_eq!(name_of(&mut b, 0), "abcdefghijklmnopqrstuvwxyz01234");
    assert_eq!(say_texts(&b)[0].2[1], "abcdefghijklmnopqrstuvwxyz01234");
}

#[test]
fn a_second_name_change_while_dead_replaces_the_pending_one() {
    let mut b = bench();
    kill(&mut b, 0);
    set_name(&mut b, 0, "one");
    set_name(&mut b, 0, "two");
    assert_eq!(name_of(&mut b, 0), "p");
    b.run(|game, eng| game.respawn_player(eng, 0));
    b.step(2);
    assert_eq!(name_of(&mut b, 0), "two");
}
