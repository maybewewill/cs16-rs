//! The observer: the death camera of the dead, the cameras of a team that may only watch its
//! own, the spectators' flight, and a target that goes away. On the shared bench of `common`.

mod common;

use common::{Bench, command, enter, join, spawns};
use cs_engine::{EngineApi, EntVars, GameDll};
use cs_game::CsGame;
use cs_game::observer::{
    OBS_CHASE_FREE, OBS_IN_EYE, OBS_MAP_FREE, OBS_NONE, OBS_ROAMING,
};
use cs_game::player::{DEAD_NO, DEAD_RESPAWNABLE, dmg};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;
const IN_JUMP: u16 = 1 << 1;

fn cmd(buttons: u16, forwardmove: f32) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec: 10,
        buttons,
        forwardmove,
        ..Default::default()
    }
}

/// `n` frames in which the player of `slot` sends the command `c` (a frame is 10 ms).
fn pump(b: &mut Bench, slot: u8, n: usize, buttons: u16, forwardmove: f32) {
    for _ in 0..n {
        b.run(move |game, eng| game.run_usercmd(eng, slot, &cmd(buttons, forwardmove), 0));
    }
}

fn vars(b: &mut Bench, slot: u8) -> EntVars {
    b.run(move |_, eng| eng.ent(eng.client_edict(slot)).unwrap().clone())
}

fn unknown(msgs: &[CsUserMsg], name: &str) -> Vec<Vec<u8>> {
    msgs.iter()
        .filter_map(|m| match m {
            CsUserMsg::Unknown { name: n, payload } if n == name => Some(payload.clone()),
            _ => None,
        })
        .collect()
}

/// Both teams in the game, the game commenced and the first round restarted: slot 0 (the probe)
/// and slot 1 on `Team::Ct`, slots 2 and 3 on `Team::Terrorist`. Four slots, so the round goes
/// on while one of them dies.
fn four_players() -> Bench {
    players(4)
}

/// [`four_players`] on a server of `max_slots` slots.
fn players(max_slots: u8) -> Bench {
    let mut b = Bench::stub_slots(spawns(8, 8), max_slots);
    // The probe sends commands with time in them: the menus advance with the player's commands.
    b.idle_cmds = true;
    b.step(6);
    b.probe.clear_messages();
    join(&mut b, 0, 2, 1);
    for (slot, team_choice) in [(1, 2), (2, 1), (3, 1)] {
        enter(&mut b, slot);
        join(&mut b, slot, team_choice, 1);
    }
    // `GAME_COMMENCE_DELAY`, and the restart of the round.
    b.step(400);
    b.probe.clear_messages();
    b
}

fn kill_by(b: &mut Bench, victim: u8, killer: u8) {
    assert!(!b.run(move |game, eng| {
        game.damage_player(eng, victim, Some(killer), 500.0, dmg::BULLET)
    }));
}

/// The dead player has had its corpse and its camera: 3 s of dying, 3 s of lying.
fn through_the_death_cam(b: &mut Bench, slot: u8) {
    pump(b, slot, 800, 0, 0.0);
}

#[test]
fn dead_player_becomes_an_observer_after_the_death_cam() {
    let mut b = four_players();
    b.probe.clear_messages();
    kill_by(&mut b, 0, 2);

    // Dying: the camera chases the killer (`OBS_CHASE_FREE`, `iuser2` oneself, `iuser3` the
    // killer), the auto director is told to stop, and the dead cannot switch modes yet.
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2, v.iuser3), (OBS_CHASE_FREE, 1, 3));
    b.step(3);
    assert_eq!(unknown(&b.messages(), "ADStop").len(), 1);
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "3".into()]);
    });
    assert_eq!(vars(&mut b, 0).iuser1, OBS_CHASE_FREE);

    // Not an observer while the body lies there.
    pump(&mut b, 0, 100, 0, 0.0);
    let v = vars(&mut b, 0);
    assert_eq!(v.iuser1, OBS_CHASE_FREE);
    assert!(v.view_ofs.z > 0.0);

    // The death camera: invisible, not solid, not moving by itself, the health 1, in the mode
    // the player used last (roaming, the first time).
    through_the_death_cam(&mut b, 0);
    b.step(3);
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2, v.iuser3), (OBS_ROAMING, 0, 0));
    assert_ne!(v.effects & 128, 0, "EF_NODRAW");
    assert_eq!(v.solid, 0);
    assert_eq!(v.movetype, 0);
    assert_eq!(v.view_ofs, Vec3::ZERO);
    assert_eq!(v.health, 1.0);
    assert_ne!(v.deadflag, DEAD_NO);
    assert_eq!(v.max_speed, 900.0);

    let msgs = b.messages();
    assert!(msgs.contains(&CsUserMsg::HideWeapon(9)));
    // Everybody is told this player is a spectator now.
    assert_eq!(unknown(&msgs, "Spectator"), [vec![1, 1]]);
    // A roaming observer has the crosshair, and sees its mode on the screen.
    assert_eq!(unknown(&msgs, "Crosshair"), [vec![1]]);
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::TextMsg { message, .. } if message == "#Spec_Mode3"
    )));
    // The ammo HUD is told the player is dead, once.
    let weapons = msgs
        .iter()
        .filter(|m| {
            matches!(
                m,
                CsUserMsg::CurWeapon {
                    is_active: false,
                    weapon_id: 0xFF,
                    clip_ammo: -1,
                }
            )
        })
        .count();
    assert_eq!(weapons, 1);
    // The dead player's night vision is turned off (`Killed`).
    assert!(msgs.contains(&CsUserMsg::NvgToggle(false)));
    // `TE_KILLPLAYERATTACHMENTS` (125) for the player, to those who can hear it.
    assert!(b.probe.messages().iter().any(|m| matches!(
        m,
        SvcMessage::TempEntity { te_type: 125, raw } if raw.last() == Some(&1)
    )));

    // It may now switch modes: `specmode 4` (first person) goes on with the player who killed
    // it, whom it was following until then (`m_hObserverTarget` outlives the death).
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "4".into()]);
    });
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2), (OBS_IN_EYE, 3));

    // And a new round brings it back: no camera, visible, solid, with its health.
    b.run(|game, eng| assert!(game.respawn_player(eng, 0)));
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2, v.iuser3, v.effects), (0, 0, 0, 0));
    assert_eq!((v.deadflag, v.health, v.solid), (DEAD_NO, 100.0, 3));
}

#[test]
fn fadetoblack_does_not_chase_the_killer() {
    let mut b = four_players();
    b.run(|_, eng| eng.set_cvar("mp_fadetoblack", "1"));
    b.probe.clear_messages();
    kill_by(&mut b, 0, 2);
    b.step(3);
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2, v.iuser3), (OBS_NONE, 0, 0));
    // The screen fades out, stays out, for 3 seconds (the fixed point unit is 1/4096).
    assert!(b.messages().contains(&CsUserMsg::ScreenFade {
        duration: 12288,
        hold_time: 12288,
        flags: 5,
        color: [0, 0, 0, 255],
    }));
    // Whatever the other cvars say, the dead only see their own screen in first person.
    through_the_death_cam(&mut b, 0);
    let v = vars(&mut b, 0);
    assert_eq!(v.iuser1, OBS_IN_EYE);
}

#[test]
fn observer_follows_a_teammate_with_forcecamera() {
    let mut b = four_players();
    b.run(|_, eng| eng.set_cvar("mp_forcecamera", "1"));
    kill_by(&mut b, 0, 2);
    through_the_death_cam(&mut b, 0);
    b.step(3);

    // Roaming is not allowed: the observer is on the map, following the one teammate.
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2, v.iuser3), (OBS_MAP_FREE, 2, 0));
    let teammate = vars(&mut b, 1);
    assert_eq!(v.origin, teammate.origin);
    // The followed player's health is sent to the follower.
    assert!(
        unknown(&b.messages(), "SpecHealth2")
            .iter()
            .any(|m| m == &[100, 2])
    );

    // The player it follows is only a teammate, however often it asks for the next one.
    for _ in 0..3 {
        pump(&mut b, 0, 30, IN_ATTACK, 0.0);
        pump(&mut b, 0, 30, 0, 0.0);
        let v = vars(&mut b, 0);
        assert_eq!((v.iuser1, v.iuser2), (OBS_MAP_FREE, 2));
    }
    b.run(|game, eng| {
        game.client_command(eng, 0, &["follownext".into()]);
    });
    assert_eq!(vars(&mut b, 0).iuser2, 2);

    // First person is not roaming: a team that may only watch its own can use it.
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "4".into()]);
    });
    assert_eq!(vars(&mut b, 0).iuser1, OBS_IN_EYE);
    b.step(3);
    // Entering first person clears the follower's screen as `UTIL_ScreenFade(.., 0.001)` does:
    // a duration of 4/4096 s, no hold, no flags, an alpha of 0.
    assert!(b.messages().contains(&CsUserMsg::ScreenFade {
        duration: 4,
        hold_time: 0,
        flags: 0,
        color: [0, 0, 0, 0],
    }));
    assert!(b.messages().contains(&CsUserMsg::BarTime(0)));
    // What the followed player sees is what the follower is sent (here: that its origin is the
    // target's, `SetupVisibility` looks through the target's eye).
    let eye = b.run(|game, eng| game.setup_visibility(eng, 0));
    let mate = vars(&mut b, 1);
    assert_eq!(eye, Some(mate.origin + mate.view_ofs));
    let own = vars(&mut b, 0);
    assert_eq!(own.origin, mate.origin);

    // With `mp_forcecamera 0` anyone may be followed: the next one is an enemy.
    b.run(|_, eng| eng.set_cvar("mp_forcecamera", "0"));
    pump(&mut b, 0, 30, 0, 0.0);
    b.run(|game, eng| {
        game.client_command(eng, 0, &["follownext".into()]);
    });
    assert_eq!(vars(&mut b, 0).iuser2, 3);
    // And the previous one back.
    pump(&mut b, 0, 30, 0, 0.0);
    b.run(|game, eng| {
        game.client_command(eng, 0, &["follownext".into(), "1".into()]);
    });
    assert_eq!(vars(&mut b, 0).iuser2, 2);
}

#[test]
fn spectator_team_roams() {
    let mut b = Bench::stub_slots(spawns(3, 3), 2);
    command(&mut b, 0, "jointeam 6");
    let v = vars(&mut b, 0);
    // A spectator is an observer who roams: invisible, not solid, with no one to follow.
    assert_eq!((v.iuser1, v.iuser2, v.iuser3), (OBS_ROAMING, 0, 0));
    assert_ne!(v.effects & 128, 0);
    assert_eq!((v.solid, v.movetype, v.view_ofs), (0, 0, Vec3::ZERO));
    assert_eq!(v.max_speed, 900.0);
    // `GetPlayerSpawnSpot` for a player with no team: the first `info_player_deathmatch`
    // (`spawns(3, 3)` has them at x = -100, -300, -500, looking along y 90); `StartObserver`
    // takes the spot's origin and angles.
    assert_eq!(v.origin, Vec3::new(-100.0, 0.0, 0.0));
    assert_eq!(v.angles, Vec3::new(0.0, 90.0, 0.0));
    assert_eq!(v.velocity, Vec3::ZERO);
    let before = v.origin;

    // A command that asks to go forward flies it forward, through anything, at no more than
    // the spectators' speed; with no input it slows down and stops.
    pump(&mut b, 0, 50, 0, 400.0);
    let flying = vars(&mut b, 0);
    assert!(flying.origin.x > before.x + 50.0, "{:?}", flying.origin);
    assert!((flying.origin.z - before.z).abs() < 1.0);
    assert!(flying.velocity.length() <= 500.0 + 0.01);
    pump(&mut b, 0, 300, 0, 0.0);
    let stopped = vars(&mut b, 0);
    assert_eq!(stopped.velocity, Vec3::ZERO);

    // With nobody to watch, a follow mode falls back to roaming and says so.
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "4".into()]);
    });
    assert_eq!(vars(&mut b, 0).iuser1, OBS_ROAMING);
    b.step(3);
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::TextMsg { message, .. } if message == "#Spec_NoTarget"
    )));

    // A player in the game can be followed; jumping changes the mode, and the watcher flies
    // to the player.
    enter(&mut b, 1);
    join(&mut b, 1, 1, 1);
    let player = vars(&mut b, 1);
    pump(&mut b, 0, 30, 0, 0.0);
    pump(&mut b, 0, 1, IN_JUMP, 0.0);
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2), (OBS_MAP_FREE, 2));
    assert_eq!(v.origin, player.origin);
    // `specmode` asks for a mode.
    pump(&mut b, 0, 30, 0, 0.0);
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "3".into()]);
    });
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2), (OBS_ROAMING, 0));
    // A mode outside of the range is first person.
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "99".into()]);
    });
    assert_eq!(vars(&mut b, 0).iuser1, OBS_IN_EYE);
    // Observer commands of a player that is not an observer are remembered, not run.
    let before = vars(&mut b, 1);
    b.run(|game, eng| {
        game.client_command(eng, 1, &["specmode".into(), "5".into()]);
        game.client_command(eng, 1, &["follownext".into()]);
    });
    let after = vars(&mut b, 1);
    assert_eq!((after.iuser1, after.origin), (before.iuser1, before.origin));
    assert_eq!(after.iuser1, OBS_NONE);
}

#[test]
fn observer_target_leaves() {
    let mut b = Bench::stub_slots(spawns(3, 3), 3);
    command(&mut b, 0, "jointeam 6");
    for slot in 1..3 {
        enter(&mut b, slot);
        join(&mut b, slot, 1, 1);
    }
    pump(&mut b, 0, 30, 0, 0.0);
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "4".into()]);
    });
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2), (OBS_IN_EYE, 2));

    // The followed player leaves: the follower is moved on to the other one.
    b.run(|game, eng| game.client_disconnect(eng, 1));
    pump(&mut b, 0, 60, 0, 0.0);
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2), (OBS_IN_EYE, 3));
    let other = vars(&mut b, 2);
    assert_eq!(v.origin, other.origin);

    // The last one leaves: nobody to follow, the observer roams, and remembers the mode it had.
    b.run(|game, eng| game.client_disconnect(eng, 2));
    pump(&mut b, 0, 60, 0, 0.0);
    let v = vars(&mut b, 0);
    assert_eq!((v.iuser1, v.iuser2), (OBS_ROAMING, 0));
    // No stale target is looked through: it sees from its own eye, as every player does.
    let eye = b.run(|game, eng| game.setup_visibility(eng, 0));
    assert_eq!(eye, Some(v.origin + v.view_ofs));
}

/// What `UpdateClientData` hands the engine for a player in first person: the state of the
/// player it follows, with the observer's own `iuser*`.
#[test]
fn first_person_observer_is_sent_the_followed_players_state() {
    let mut b = four_players();
    b.run(|_, eng| eng.set_cvar("mp_forcecamera", "0"));
    kill_by(&mut b, 0, 2);
    through_the_death_cam(&mut b, 0);
    b.run(|game, eng| {
        game.client_command(eng, 0, &["specmode".into(), "4".into()]);
    });
    let (data, followed): (_, EntVars) = b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        let data = game.update_client_data(eng, 0);
        (data, eng.ent(eng.client_edict(2)).unwrap().clone())
    });
    // Following the killer, who is an enemy: `mp_forcecamera 0` allows it.
    assert_eq!((data.iuser1, data.iuser2, data.iuser3), (OBS_IN_EYE, 3, 0));
    assert_eq!(data.origin, followed.origin);
    assert_eq!(data.health, followed.health);
    assert_eq!(data.view_ofs, followed.view_ofs);
    // Every player is sent the world from its own eye (`SetupVisibility`), a follower from the
    // one it follows.
    let eyes = b.run(|game, eng| {
        [
            game.setup_visibility(eng, 1),
            game.setup_visibility(eng, 0),
            game.setup_visibility(eng, 9),
        ]
    });
    let (mate, killer) = (vars(&mut b, 1), vars(&mut b, 2));
    assert_eq!(eyes[0], Some(mate.origin + mate.view_ofs));
    assert_eq!(eyes[1], Some(killer.origin + killer.view_ofs));
    assert_eq!(eyes[2], None);
}

#[test]
fn a_target_that_left_restarts_the_search_at_the_observer() {
    let mut b = Bench::stub_slots(spawns(3, 3), 4);
    enter(&mut b, 1);
    command(&mut b, 1, "jointeam 6");
    for (slot, team_choice) in [(0, 1), (2, 2), (3, 1)] {
        if slot != 0 {
            enter(&mut b, slot);
        }
        join(&mut b, slot, team_choice, 1);
    }
    pump(&mut b, 1, 30, 0, 0.0);
    // Edict indexes: the observer is 2; it follows 3, then 4.
    b.run(|game, eng| {
        game.client_command(eng, 1, &["specmode".into(), "4".into()]);
    });
    assert_eq!(vars(&mut b, 1).iuser2, 3);
    pump(&mut b, 1, 30, 0, 0.0);
    b.run(|game, eng| {
        game.client_command(eng, 1, &["follownext".into()]);
    });
    assert_eq!(vars(&mut b, 1).iuser2, 4);
    // Player 4 leaves: the old handle is null, so the search goes on from the observer (index 2)
    // to 3, not from the leaver around to 1.
    b.run(|game, eng| game.client_disconnect(eng, 3));
    pump(&mut b, 1, 60, 0, 0.0);
    assert_eq!(vars(&mut b, 1).iuser2, 3);
}

#[test]
fn a_late_joiner_watches_from_a_spawn_point() {
    let mut b = players(5);
    // More than 20 seconds into a round with both teams playing, nobody spawns any more.
    b.step(3000);
    enter(&mut b, 4);
    join(&mut b, 4, 2, 1);
    b.step(5);
    let v = vars(&mut b, 4);
    assert_eq!(v.deadflag, DEAD_RESPAWNABLE);
    assert_ne!(v.iuser1, OBS_NONE);
    assert_ne!(v.effects & 128, 0, "EF_NODRAW");
    // `GetPlayerSpawnSpot` put it one unit above a counter-terrorist spawn point (`spawns(8, 8)`
    // has them at x = 100, 300, ..., y 0, z 0, looking along y 90), and `StartObserver` kept
    // that origin and took the point's angles.
    assert_eq!((v.origin.y, v.origin.z), (0.0, 1.0));
    assert!(v.origin.x > 0.0 && v.origin.x % 200.0 == 100.0, "{:?}", v.origin);
    assert_eq!(v.angles, Vec3::new(0.0, 90.0, 0.0));
}

#[test]
fn joining_the_spectators_with_fadetoblack_fades_the_screen_back_in() {
    let mut b = Bench::stub_slots(spawns(3, 3), 2);
    b.run(|_, eng| eng.set_cvar("mp_fadetoblack", "1"));
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 6");
    let msgs = b.messages();
    // `StartObserver` sends `Spectator` and so does `HandleMenu_ChooseTeam` after it.
    assert_eq!(unknown(&msgs, "Spectator"), [vec![1, 1], vec![1, 1]]);
    // `UTIL_ScreenFade(pPlayer, Vector(0, 0, 0), 0.001, 0, 0, FFADE_IN)`; with the screen held
    // black the observer's own effects clear sends no blindness fade.
    let fades: Vec<&CsUserMsg> = msgs
        .iter()
        .filter(|m| matches!(m, CsUserMsg::ScreenFade { .. }))
        .collect();
    assert_eq!(
        fades,
        [&CsUserMsg::ScreenFade {
            duration: 4,
            hold_time: 0,
            flags: 0,
            color: [0, 0, 0, 0],
        }]
    );
}

#[test]
fn the_camera_cvars_default_to_vanilla() {
    let mut b = Bench::stub_slots(spawns(1, 1), 2);
    let values = b.run(|_, eng| {
        [
            eng.cvar_f32("mp_forcecamera"),
            eng.cvar_f32("mp_forcechasecam"),
            eng.cvar_f32("mp_fadetoblack"),
        ]
    });
    assert_eq!(values, [0.0; 3]);
}
