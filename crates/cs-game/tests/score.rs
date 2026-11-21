//! The scoreboard and the death notices (Task 7): what a death sends to everyone and does to
//! the frags and deaths, and the team scores at the end of a round. The probe is in slot 0
//! (edict index 1); a second client in slot 1 (index 2) receives nothing, so what it would see
//! is read from the game's state. The messages are compared as their bytes on the wire.

mod common;

use common::{Bench, command, enter, join, spawns, stub_spawns};
use cs_engine::GameDll;
use cs_game::player::{Team, dmg};
use cs_proto48::usermsg::{CsUserMsg, encode_cs_usermsg};

/// The messages of the scoreboard the probe has received, as `(name, bytes)`.
fn scoreboard(b: &Bench) -> Vec<(&'static str, Vec<u8>)> {
    b.messages()
        .iter()
        .filter(|m| {
            matches!(
                m,
                CsUserMsg::DeathMsg { .. }
                    | CsUserMsg::ScoreInfo { .. }
                    | CsUserMsg::ScoreAttrib { .. }
                    | CsUserMsg::TeamScore { .. }
            )
        })
        .map(encode_cs_usermsg)
        .collect()
}

/// `(frags, deaths, alive)` of the players of slot 0 and slot 1.
fn scores(b: &mut Bench) -> [(i32, i32, bool); 2] {
    b.run(|game, _| {
        let s = game.state();
        let at = |i: usize| {
            let p = s.players[i].as_ref().unwrap();
            (p.frags, p.deaths, p.alive)
        };
        [at(0), at(1)]
    })
}

/// Two players in the game, the probe a counter-terrorist and the other a terrorist, spawned,
/// with the probe's log empty.
fn duel() -> Bench {
    let mut b = Bench::stub(stub_spawns());
    b.enter_second_client();
    b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.spawn_player(eng, 1, Team::Terrorist, 0);
    });
    b.step(3);
    b.probe.clear_messages();
    b
}

/// `DeathMsg` as it is on the wire: killer, victim, headshot, the weapon and its terminator.
fn death_bytes(killer: u8, victim: u8, headshot: u8, weapon: &str) -> (&'static str, Vec<u8>) {
    let mut v = vec![killer, victim, headshot];
    v.extend_from_slice(weapon.as_bytes());
    v.push(0);
    ("DeathMsg", v)
}

/// `ScoreInfo` as it is on the wire: the index and four little-endian shorts.
fn info_bytes(index: u8, frags: i16, deaths: i16, team: i16) -> (&'static str, Vec<u8>) {
    let mut v = vec![index];
    for short in [frags, deaths, 0, team] {
        v.extend_from_slice(&short.to_le_bytes());
    }
    ("ScoreInfo", v)
}

#[test]
fn death_updates_scores_for_everyone() {
    let mut b = duel();
    // The terrorist (index 2) kills the counter-terrorist (index 1): the death notice and the
    // victim marked dead on the scoreboard (both reliable, so first on the wire), then the
    // victim's line (one death, team 2) and the killer's line (one frag, team 1), which vanilla
    // sends unreliably (`MSG_BROADCAST`) and so come after the reliable stream in the packet.
    b.run(|game, eng| game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET));
    b.step(3);
    assert_eq!(
        scoreboard(&b),
        vec![
            death_bytes(2, 1, 0, "world"),
            ("ScoreAttrib", vec![1, 1]),
            info_bytes(1, 0, 1, 2),
            info_bytes(2, 1, 0, 1),
        ]
    );
    assert_eq!(scores(&mut b), [(0, 1, false), (1, 0, true)]);

    // A death of the terrorist by the world: the notice names `worldspawn` and nobody's frags
    // change; only the victim's line is sent.
    b.probe.clear_messages();
    b.run(|game, eng| game.damage_player(eng, 1, None, 500.0, dmg::CRUSH));
    b.step(3);
    assert_eq!(
        scoreboard(&b),
        vec![
            death_bytes(0, 2, 0, "worldspawn"),
            ("ScoreAttrib", vec![2, 1]),
            info_bytes(2, 1, 1, 1),
        ]
    );
    assert_eq!(scores(&mut b), [(0, 1, false), (1, 1, false)]);

    // A player that kills itself (the `kill` command) loses a frag, and is its own killer in the
    // notice. With no weapons yet the weapon is `world`.
    b.run(|game, eng| {
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.spawn_player(eng, 1, Team::Terrorist, 0);
    });
    b.step(3);
    b.probe.clear_messages();
    command(&mut b, 0, "kill");
    b.step(3);
    assert_eq!(
        scoreboard(&b),
        vec![
            death_bytes(1, 1, 0, "world"),
            ("ScoreAttrib", vec![1, 1]),
            info_bytes(1, -1, 2, 2),
            info_bytes(1, -1, 2, 2),
        ]
    );
    assert_eq!(scores(&mut b), [(-1, 2, false), (1, 1, true)]);
}

#[test]
fn a_teammate_kill_costs_the_killer_a_frag_and_the_money() {
    let mut b = duel();
    b.run(|game, eng| {
        eng.set_cvar("mp_friendlyfire", "1");
        game.spawn_player(eng, 0, Team::Ct, 0);
        game.spawn_player(eng, 1, Team::Ct, 0);
    });
    b.step(3);
    b.probe.clear_messages();
    let before = b.run(|game, _| game.state().players[1].as_ref().unwrap().money);
    b.run(|game, eng| game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET));
    b.step(3);
    // The killer (index 2) lost the frag, and the line sent is the one with it.
    assert_eq!(
        scoreboard(&b),
        vec![
            death_bytes(2, 1, 0, "world"),
            ("ScoreAttrib", vec![1, 1]),
            info_bytes(1, 0, 1, 2),
            info_bytes(2, -1, 0, 2),
        ]
    );
    assert_eq!(scores(&mut b), [(0, 1, false), (-1, 0, true)]);
    // The same death is -3300 for the killer and one team kill: `money::killed_by`, once.
    let (after, team_kills) = b.run(|game, _| {
        let p = game.state().players[1].as_ref().unwrap();
        (p.money, p.team_kills)
    });
    assert_eq!(after, (before - 3300).max(0));
    assert_eq!(team_kills, 1);
}

#[test]
fn fall_death_message_as_regamedll() {
    let mut b = duel();
    // The fall damage has the world (`VARS(eoNullEntity)`) as attacker and inflictor: the
    // notice is from no one, with the classname of the world as its weapon, and no frag is
    // lost or won (the world's frags change in the original).
    b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::FALL));
    b.step(3);
    assert_eq!(
        scoreboard(&b),
        vec![
            death_bytes(0, 1, 0, "worldspawn"),
            ("ScoreAttrib", vec![1, 1]),
            info_bytes(1, 0, 1, 2),
        ]
    );
    assert_eq!(scores(&mut b), [(0, 1, false), (0, 0, true)]);
}

#[test]
fn a_player_leaving_is_marked_dead_then_emptied() {
    let mut b = duel();
    b.run(|game, eng| game.client_disconnect(eng, 1));
    b.step(3);
    // `SetScoreboardAttributes()` while it still has a team, then the empty line and no team.
    let names: Vec<_> = scoreboard(&b);
    assert_eq!(
        names,
        vec![("ScoreAttrib", vec![2, 1]), info_bytes(2, 0, 0, 0)]
    );
}

#[test]
fn team_score_after_round() {
    // One terrorist (the probe) and one counter-terrorist: the game starts, and when the
    // counter-terrorist dies the round is the terrorists'.
    let mut b = Bench::stub_slots(spawns(8, 8), 3);
    b.idle_cmds = true;
    b.step(6);
    b.probe.clear_messages();
    b.run(|_, eng| {
        eng.set_cvar("mp_freezetime", "2");
        eng.set_cvar("mp_roundtime", "2");
    });
    join(&mut b, 0, 1, 1);
    enter(&mut b, 1);
    join(&mut b, 1, 2, 1);
    b.step(330);
    b.step(230);
    b.probe.clear_messages();
    b.run(|game, eng| game.damage_player(eng, 1, None, 500.0, 0));
    b.step(3);
    b.step(3);
    let team_scores: Vec<_> = scoreboard(&b)
        .into_iter()
        .filter(|(name, _)| *name == "TeamScore")
        .collect();
    // `UpdateTeamScores`: CT first, then TERRORIST, each the rounds won.
    let team = |name: &str, score: i16| {
        let mut v = name.as_bytes().to_vec();
        v.push(0);
        v.extend_from_slice(&score.to_le_bytes());
        ("TeamScore", v)
    };
    assert_eq!(team_scores, vec![team("CT", 0), team("TERRORIST", 1)]);
    assert_eq!(
        b.run(|game, _| (
            game.state().num_terrorist_wins,
            game.state().num_ct_wins
        )),
        (1, 0)
    );
}
