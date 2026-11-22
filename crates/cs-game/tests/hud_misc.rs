//! The radio, the radar, the status bar and the night vision, on the wire. The probe in slot 0 is
//! the receiver the tests read from (the other slots have no client, and a message to them is not
//! delivered); the other players are the speakers and the targets.

mod common;

use common::{Bench, command, enter, join, spawns};
use cs_engine::{EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

/// A bench with the probe (slot 0) and slot 2 on the terrorists, slot 1 a counter-terrorist,
/// out of the freeze period of the first round. Slot 2 has no client, so it has no name.
fn bench() -> Bench {
    let mut b = Bench::stub_slots(spawns(3, 3), 3);
    b.idle_cmds = true;
    b.step(6);
    join(&mut b, 0, 1, 1);
    enter(&mut b, 1);
    join(&mut b, 1, 2, 1);
    enter(&mut b, 2);
    join(&mut b, 2, 1, 1);
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

/// Lets the radio timer (1.5 s) and the command timers run out.
fn wait_out_radio(b: &mut Bench) {
    b.step(160);
}

/// Runs `line` as the player of `slot` and returns what the probe was sent, in order.
fn send(b: &mut Bench, slot: u8, line: &str) -> Vec<SvcMessage> {
    b.probe.clear_messages();
    command(b, slot, line);
    b.step(2);
    b.probe.messages().iter().cloned().collect()
}

fn audio(msgs: &[SvcMessage]) -> Vec<(u8, String, i16)> {
    user(msgs)
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::SendAudio {
                sender_id,
                audio_code,
                pitch,
            } => Some((sender_id, audio_code, pitch)),
            _ => None,
        })
        .collect()
}

fn texts(msgs: &[SvcMessage]) -> Vec<(u8, String, Vec<String>)> {
    user(msgs)
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg {
                dest,
                message,
                params,
            } => Some((dest, message, params)),
            _ => None,
        })
        .collect()
}

fn user(msgs: &[SvcMessage]) -> Vec<CsUserMsg> {
    msgs.iter()
        .filter_map(|m| match m {
            SvcMessage::UserMsg { decoded, .. } => Some(decoded.clone()),
            _ => None,
        })
        .collect()
}

/// The raw payloads of the temp entities `TE_PLAYERATTACHMENT` the probe got.
fn icons(msgs: &[SvcMessage]) -> Vec<Vec<u8>> {
    msgs.iter()
        .filter_map(|m| match m {
            SvcMessage::TempEntity { te_type: 124, raw } => Some(raw.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn radio_reaches_the_team_with_sound() {
    let mut b = bench();
    wait_out_radio(&mut b);
    // A terrorist's radio reaches the terrorist probe: the sentence, the text and the icon.
    let got = send(&mut b, 2, "coverme");
    assert_eq!(
        audio(&got),
        vec![(3, "%!MRAD_COVERME".to_string(), 100)],
        "{got:?}"
    );
    assert_eq!(
        texts(&got),
        vec![(
            5,
            "3".to_string(),
            vec![
                "#Game_radio".to_string(),
                String::new(),
                "#Cover_me".to_string()
            ]
        )]
    );
    let sprite = b
        .run(|_, eng| eng.model_index("sprites/radio.spr"))
        .expect("the radio sprite is precached");
    // entity 3, offset 35 (280 in eighths), the sprite, 15 tenths of a second.
    let mut want = vec![3u8, 0x18, 0x01];
    want.extend_from_slice(&sprite.to_le_bytes());
    want.extend_from_slice(&15u16.to_le_bytes());
    assert_eq!(icons(&got), vec![want]);

    // An enemy's radio does not reach the probe at all.
    wait_out_radio(&mut b);
    let got = send(&mut b, 1, "coverme");
    assert!(audio(&got).is_empty() && texts(&got).is_empty() && icons(&got).is_empty());

    // The speaker hears itself, from its own index and name.
    wait_out_radio(&mut b);
    let got = send(&mut b, 0, "go");
    assert_eq!(audio(&got), vec![(1, "%!MRAD_GO".to_string(), 100)]);
    assert_eq!(
        texts(&got),
        vec![(
            5,
            "1".to_string(),
            vec!["#Game_radio".into(), "p".into(), "#Go_go_go".into()]
        )]
    );
}

#[test]
fn radio_menus_and_aliases_pick_the_sentences() {
    let mut b = bench();
    wait_out_radio(&mut b);
    // `radio3` opens the third menu on the speaker (slot 2 has no client to see it) and key 5
    // of it speaks "in position".
    command(&mut b, 2, "radio3");
    let menu = b.server.game().game.state().players[2].as_ref().unwrap().menu;
    assert_eq!(menu, cs_game::menus::Menu::Radio3);
    let got = send(&mut b, 2, "menuselect 5");
    assert_eq!(audio(&got), vec![(3, "%!MRAD_INPOS".to_string(), 100)]);
    assert_eq!(texts(&got)[0].2[2], "#In_position");

    // The radio menu of the probe itself is shown as a text menu with its keys.
    wait_out_radio(&mut b);
    b.probe.clear_messages();
    command(&mut b, 0, "radio2");
    let shown: Vec<_> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::ShowMenu {
                keys_bitmask,
                display_time,
                text,
                ..
            } => Some((keys_bitmask, display_time, text)),
            _ => None,
        })
        .collect();
    assert_eq!(shown, vec![(0b10_0011_1111, -1, "#RadioB".to_string())]);

    // Key 1 of the third menu is one of two answers; every other command is fixed.
    for (alias, code) in [("roger", None), ("negative", Some("%!MRAD_NEGATIVE")), ("report", Some("%!MRAD_REPORTIN"))] {
        wait_out_radio(&mut b);
        let got = send(&mut b, 2, alias);
        let a = audio(&got);
        assert_eq!(a.len(), 1, "{alias}");
        match code {
            Some(code) => assert_eq!(a[0].1, code),
            None => assert!(["%!MRAD_AFFIRM", "%!MRAD_ROGER"].contains(&a[0].1.as_str())),
        }
    }
}

#[test]
fn radio_is_limited_and_silent_for_the_dead_and_the_deaf() {
    let mut b = bench();
    wait_out_radio(&mut b);
    assert_eq!(audio(&send(&mut b, 2, "coverme")).len(), 1);
    // Within 1.5 seconds nothing is said; after it, it is.
    assert!(audio(&send(&mut b, 2, "coverme")).is_empty());
    wait_out_radio(&mut b);
    assert_eq!(audio(&send(&mut b, 2, "coverme")).len(), 1);

    // 60 messages a spawn: with one left the next is the last.
    b.server.game_mut().game.state_mut().players[2]
        .as_mut()
        .unwrap()
        .hud
        .radio_messages = 1;
    wait_out_radio(&mut b);
    assert_eq!(audio(&send(&mut b, 2, "coverme")).len(), 1);
    wait_out_radio(&mut b);
    assert!(audio(&send(&mut b, 2, "coverme")).is_empty());
    assert_eq!(
        b.server.game().game.state().players[2].as_ref().unwrap().hud.radio_messages,
        0
    );

    // A player that ignores the radio hears nothing, and is told so.
    wait_out_radio(&mut b);
    b.probe.clear_messages();
    command(&mut b, 0, "ignorerad");
    let told = texts(&b.probe.messages().iter().cloned().collect::<Vec<_>>());
    assert_eq!(told, vec![(4, "#Ignore_Radio".to_string(), vec![])]);
    b.server.game_mut().game.state_mut().players[1]
        .as_mut()
        .unwrap()
        .hud
        .radio_messages = 5;
    b.server.game_mut().game.state_mut().players[2]
        .as_mut()
        .unwrap()
        .hud
        .radio_messages = 5;
    let got = send(&mut b, 2, "followme");
    assert!(audio(&got).is_empty() && texts(&got).is_empty());
    command(&mut b, 0, "ignorerad");

    // A dead player says nothing (and the dead may not even ask).
    wait_out_radio(&mut b);
    b.run(|game, eng| game.damage_player(eng, 2, None, 500.0, 0));
    b.step(3);
    let got = send(&mut b, 2, "followme");
    assert!(audio(&got).is_empty() && texts(&got).is_empty());
}

#[test]
fn radar_shows_teammates_only() {
    let mut b = bench();
    // Both players run a command (`PreThink`, which runs `UpdateClientData`), after the radar is due.
    b.step(150);
    b.probe.clear_messages();
    let origins = b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        // The radar speaks of players that moved: put both far from where the radar last saw them.
        let mut origins = Vec::new();
        for (slot, origin) in [(1u8, Vec3::new(-640.0, 320.0, 48.0)), (2, Vec3::new(960.0, -480.0, 48.0))] {
            game.place_player(eng, slot, origin);
            common::commands_of(game, eng, slot, 1);
            origins.push(origin);
        }
        origins
    });
    b.step(3);
    let radars: Vec<_> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::Radar { player_id, origin } => Some((player_id, origin)),
            _ => None,
        })
        .collect();
    // The terrorist in slot 2 (edict 3) is on the probe's radar, where it is; the
    // counter-terrorist in slot 1 (edict 2) is not.
    assert!(radars.iter().all(|r| r.0 != 2), "{radars:?}");
    assert!(radars.contains(&(3, origins[1])), "{radars:?}");

    // A second update that has not waited a second, or has not moved 64 units, says nothing.
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.place_player(eng, 2, Vec3::new(960.0, -480.0, 48.0 + 8.0));
        common::commands_of(game, eng, 2, 1);
    });
    b.step(120);
    b.run(|game, eng| {
        game.place_player(eng, 2, Vec3::new(960.0, -480.0, 48.0 + 8.0));
        common::commands_of(game, eng, 2, 1);
    });
    b.step(3);
    assert!(
        !b.messages().iter().any(|m| matches!(m, CsUserMsg::Radar { player_id: 3, .. })),
        "{:?}",
        b.messages()
    );

    // A dead teammate is not on the radar either, and a dead one sends none.
    b.run(|game, eng| game.damage_player(eng, 2, None, 500.0, 0));
    b.step(150);
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.place_player(eng, 2, Vec3::new(-960.0, 480.0, 48.0));
        common::commands_of(game, eng, 2, 1);
    });
    b.step(3);
    assert!(!b.messages().iter().any(|m| matches!(m, CsUserMsg::Radar { .. })));
}

/// Respawns `viewer` (which makes it forget the format it was last sent, so the next sighting
/// sends the whole bar), puts it at (0, 0, 100) looking down +X and `target` 200 units in front
/// of it, and returns the status messages the viewer gets in the next 0.3 seconds. The
/// target has no client and stays where it is put; the viewer falls for the 0.2 seconds
/// the status bar waits between two updates, which keeps the target in the crosshair.
fn look_at(b: &mut Bench, viewer: u8, target: u8) -> Vec<CsUserMsg> {
    b.step(40);
    b.probe.clear_messages();
    b.run(move |game: &mut CsGame, eng: &mut EngineApi<'_>| {
        game.respawn_player(eng, viewer);
        place_pair(game, eng, viewer, target);
    });
    b.step(30);
    b.messages()
}

fn place_pair(game: &mut CsGame, eng: &mut EngineApi<'_>, viewer: u8, target: u8) {
    // Everybody else is out of the way (a player with no client stays where it was put).
    for other in (0..4u8).filter(|&s| s != viewer && s != target) {
        game.place_player(eng, other, Vec3::new(0.0, 6000.0 + 100.0 * f32::from(other), 100.0));
    }
    game.place_player(eng, viewer, Vec3::new(0.0, 0.0, 100.0));
    game.place_player(eng, target, Vec3::new(200.0, 0.0, 100.0));
    let id = eng.client_edict(viewer);
    if let Some(v) = eng.ent_mut(id) {
        v.v_angle = Vec3::ZERO;
        v.punchangle = Vec3::ZERO;
    }
    game.update_client_data(eng, viewer);
}

/// The `StatusText` lines and the `StatusValue` pairs of `msgs`.
type Status = (Vec<(u8, String)>, Vec<(u8, i16)>);

fn status(msgs: &[CsUserMsg]) -> Status {
    let mut text = Vec::new();
    let mut values = Vec::new();
    for m in msgs {
        match m {
            CsUserMsg::StatusText { line, text: t } => text.push((*line, t.clone())),
            CsUserMsg::StatusValue { index, value } => values.push((*index, *value)),
            _ => {}
        }
    }
    (text, values)
}

#[test]
fn status_bar_names_the_teammate_under_the_crosshair() {
    let mut b = bench();
    // The probe (slot 0, a terrorist) looks at slot 2, a terrorist: "name: health" for a
    // teammate, its type 1, its edict index 3 and its health in percent.
    let got = look_at(&mut b, 0, 2);
    let (text, values) = status(&got);
    assert_eq!(text, vec![(0, "1 %c1: %p2\n2  %h: %i3%%".to_string())], "{got:?}");
    assert_eq!(values, vec![(1, 1), (2, 3), (3, 100)]);

    // Hurt, the percentage follows; the format does not change, so only the value is sent.
    b.run(|game, eng| game.damage_player(eng, 2, None, 40.0, 0));
    b.probe.clear_messages();
    b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| place_pair(game, eng, 0, 2));
    b.step(30);
    let (text, values) = status(&b.messages());
    assert!(text.is_empty());
    assert_eq!(values, vec![(3, 60)]);
}

#[test]
fn status_bar_names_an_enemy_without_health_and_forgets_after_two_seconds() {
    let mut b = bench();
    // Looking at slot 1, a counter-terrorist: type 2, its index, no health.
    let got = look_at(&mut b, 0, 1);
    let (text, values) = status(&got);
    assert_eq!(text, vec![(0, "1 %c1: %p2".to_string())]);
    assert_eq!(values, vec![(1, 2), (2, 2), (3, 0)]);

    // Looking away, the name is held for two seconds and then cleared.
    b.probe.clear_messages();
    b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        game.place_player(eng, 1, Vec3::new(200.0, 2000.0, 100.0));
        game.update_client_data(eng, 0);
    });
    b.step(10);
    assert!(status(&b.messages()).1.is_empty(), "held");
    b.step(250);
    let (_, values) = status(&b.messages());
    assert_eq!(values, vec![(1, 0), (2, 0)]);
}

#[test]
fn mp_playerid_limits_what_the_status_bar_names() {
    let mut b = bench();
    // Default 0 (everyone) is registered: the cvar exists with its vanilla default.
    assert_eq!(
        b.run(|_, eng| eng.cvar_str("mp_playerid")),
        Some("0".to_string())
    );
    // 1 (teammates only): a teammate is named in full, an enemy is not named at all (the
    // format is a blank, the type and the index are still sent).
    b.run(|_, eng| eng.set_cvar("mp_playerid", "1"));
    let (text, values) = status(&look_at(&mut b, 0, 2));
    assert_eq!(text, vec![(0, "1 %c1: %p2\n2  %h: %i3%%".to_string())]);
    assert_eq!(values, vec![(1, 1), (2, 3), (3, 100)]);
    let (text, values) = status(&look_at(&mut b, 0, 1));
    assert_eq!(text, vec![(0, " ".to_string())]);
    assert_eq!(values, vec![(1, 2), (2, 2), (3, 0)]);
    // 2 (off): nobody is named, the health of a teammate is still sent.
    b.run(|_, eng| eng.set_cvar("mp_playerid", "2"));
    let (text, values) = status(&look_at(&mut b, 0, 2));
    assert_eq!(text, vec![(0, " ".to_string())]);
    assert_eq!(values, vec![(1, 1), (2, 3), (3, 100)]);
    let (text, values) = status(&look_at(&mut b, 0, 1));
    assert_eq!(text, vec![(0, " ".to_string())]);
    assert_eq!(values, vec![(1, 2), (2, 2), (3, 0)]);
}

#[test]
fn a_dead_teammate_still_hears_the_team_radio() {
    let mut b = bench();
    wait_out_radio(&mut b);
    // Dead, and then an observer: neither has `FL_SPECTATOR`, so the player branch of
    // `CBasePlayer::Radio` applies and its team is heard.
    b.run(|game, eng| game.damage_player(eng, 0, Some(1), 500.0, 1 << 1));
    b.step(3);
    assert_eq!(audio(&send(&mut b, 2, "coverme")).len(), 1);
    pump(&mut b, 0, 800);
    wait_out_radio(&mut b);
    assert_eq!(audio(&send(&mut b, 2, "coverme")).len(), 1);
    // Not the other team's.
    wait_out_radio(&mut b);
    assert!(audio(&send(&mut b, 1, "coverme")).is_empty());
}

#[test]
fn night_vision_toggles_for_a_player_that_has_it() {
    let mut b = bench();
    let toggles = |b: &Bench| -> Vec<bool> {
        b.messages()
            .into_iter()
            .filter_map(|m| match m {
                CsUserMsg::NvgToggle(on) => Some(on),
                _ => None,
            })
            .collect()
    };
    // Without the goggles nothing happens.
    b.step(40);
    b.probe.clear_messages();
    command(&mut b, 0, "nightvision");
    assert!(toggles(&b).is_empty());

    assert!(b.server.game_mut().game.grant_night_vision(0));
    b.step(40);
    b.probe.clear_messages();
    command(&mut b, 0, "nightvision");
    assert_eq!(toggles(&b), vec![true]);
    b.step(40);
    b.probe.clear_messages();
    command(&mut b, 0, "nightvision");
    assert_eq!(toggles(&b), vec![false]);

    // Dying switches it off; `drop` is accepted and does nothing.
    b.step(40);
    command(&mut b, 0, "nightvision");
    b.step(40);
    b.probe.clear_messages();
    command(&mut b, 0, "drop");
    b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, 0));
    b.step(3);
    assert_eq!(toggles(&b), vec![false]);
}

// ---- observers ----

/// Both teams in the game: the probe (slot 0) and slot 1 on the counter-terrorists, slots 2 and 3
/// on the terrorists; four slots, so the round goes on while the probe dies and watches.
fn four_players() -> Bench {
    let mut b = Bench::stub_slots(spawns(8, 8), 4);
    b.idle_cmds = true;
    b.step(6);
    join(&mut b, 0, 2, 1);
    for (slot, team_choice) in [(1, 2), (2, 1), (3, 1)] {
        enter(&mut b, slot);
        join(&mut b, slot, team_choice, 1);
    }
    b.step(400);
    b.run(|_, eng| eng.set_cvar("mp_forcecamera", "0"));
    b
}

fn pump(b: &mut Bench, slot: u8, n: usize) {
    for _ in 0..n {
        let cmd = cs_proto48::delta::GoldSrcUserCmd {
            msec: 10,
            ..Default::default()
        };
        b.run(move |game, eng| common::run_cmd(game, eng, slot, &cmd, 0));
    }
}

/// The probe is killed by slot 2, goes through the death camera and then watches in `mode`
/// (`specmode`), following the killer.
fn probe_observes(b: &mut Bench, mode: &str) {
    assert!(!b.run(|game, eng| game.damage_player(eng, 0, Some(2), 500.0, 1 << 1)));
    pump(b, 0, 800);
    let mode = mode.to_string();
    b.run(move |game, eng| game.client_command(eng, 0, &["specmode".into(), mode]));
    b.step(3);
}

/// The probe joins the counter-terrorists after the round has started: it waits for the next
/// round as a spectator (`FL_SPECTATOR`, `GetIntoGame`), and watches in `mode`.
fn late_probe_watches(mode: &str) -> Bench {
    let mut b = Bench::stub_slots(spawns(8, 8), 4);
    b.idle_cmds = true;
    b.step(6);
    for (slot, team_choice) in [(1, 2), (2, 1), (3, 1)] {
        enter(&mut b, slot);
        join(&mut b, slot, team_choice, 1);
    }
    b.step(3000);
    b.run(|_, eng| eng.set_cvar("mp_forcecamera", "0"));
    join(&mut b, 0, 2, 1);
    pump(&mut b, 0, 50);
    let flags = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().flags);
    assert_ne!(flags & cs_engine::consts::FL_SPECTATOR, 0, "waits as a spectator");
    // Jumping makes a spectator follow the first player it finds.
    let jump = cs_proto48::delta::GoldSrcUserCmd {
        msec: 10,
        buttons: 1 << 1,
        ..Default::default()
    };
    b.run(move |game, eng| common::run_cmd(game, eng, 0, &jump, 0));
    pump(&mut b, 0, 50);
    let mode = mode.to_string();
    b.run(move |game, eng| game.client_command(eng, 0, &["specmode".into(), mode]));
    b.step(3);
    b
}

fn probe_vars(b: &mut Bench) -> (i32, i32) {
    b.run(|_, eng| {
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (v.iuser1, v.iuser2)
    })
}

#[test]
fn an_observer_hears_the_radio_of_the_player_it_follows_and_its_team() {
    let mut b = late_probe_watches("4");
    // In first person; move on to slot 2 (edict 3), a terrorist.
    for _ in 0..6 {
        if probe_vars(&mut b) == (4, 3) {
            break;
        }
        pump(&mut b, 0, 30);
        b.run(|game, eng| game.client_command(eng, 0, &["follownext".into()]));
    }
    assert_eq!(probe_vars(&mut b), (4, 3));
    // The target speaks: the observer hears it.
    wait_out_radio(&mut b);
    let got = send(&mut b, 2, "coverme");
    assert_eq!(audio(&got), vec![(3, "%!MRAD_COVERME".to_string(), 100)]);
    assert_eq!(texts(&got).len(), 1);
    assert_eq!(icons(&got).len(), 1);
    // A teammate of the target (slot 3) too; an enemy of the target (slot 1) does not reach it.
    let got = send(&mut b, 3, "coverme");
    assert_eq!(audio(&got), vec![(4, "%!MRAD_COVERME".to_string(), 100)]);
    let got = send(&mut b, 1, "coverme");
    assert!(audio(&got).is_empty() && texts(&got).is_empty());

    // Chasing from a distance counts too; roaming (nobody it follows) does not.
    b.run(|game, eng| game.client_command(eng, 0, &["specmode".into(), "2".into()]));
    b.step(3);
    assert_eq!(probe_vars(&mut b).0, 2);
    wait_out_radio(&mut b);
    assert_eq!(audio(&send(&mut b, 3, "go")).len(), 1);
    b.run(|game, eng| game.client_command(eng, 0, &["specmode".into(), "3".into()]));
    b.step(3);
    assert_eq!(probe_vars(&mut b).0, 3);
    wait_out_radio(&mut b);
    assert!(audio(&send(&mut b, 3, "go")).is_empty());

    // `ignorerad` silences the observer as it does a player.
    b.run(|game, eng| game.client_command(eng, 0, &["specmode".into(), "4".into()]));
    b.step(3);
    command(&mut b, 0, "ignorerad");
    wait_out_radio(&mut b);
    assert!(audio(&send(&mut b, 3, "go")).is_empty());
}

#[test]
fn a_dead_observer_gets_the_teammate_status_bar_from_2048_units() {
    let mut b = four_players();
    // Alive, the probe sees an enemy (slot 2): the enemy format is on its screen.
    b.step(40);
    b.probe.clear_messages();
    b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 100.0));
        game.place_player(eng, 2, Vec3::new(200.0, 0.0, 100.0));
        let id = eng.client_edict(0);
        if let Some(v) = eng.ent_mut(id) {
            v.v_angle = Vec3::ZERO;
        }
    });
    b.step(30);
    assert_eq!(
        status(&b.messages()).0.last(),
        Some(&(0, "1 %c1: %p2".to_string()))
    );

    // Dead and watching, it still looks 2048 units like a player: nothing at 3000.
    b.probe.clear_messages();
    probe_observes(&mut b, "3");
    // The format on its screen is the teammate one (it was sent while it chased the killer).
    assert_eq!(
        status(&b.messages()).0.last(),
        Some(&(0, "1 %c1: %p2
2  %h: %i3%%".to_string()))
    );
    b.probe.clear_messages();
    let aim = |far: f32, who: u8, away: u8| {
        move |game: &mut CsGame, eng: &mut EngineApi<'_>| {
            game.place_player(eng, 0, Vec3::new(0.0, 0.0, 100.0));
            let id = eng.client_edict(0);
            if let Some(v) = eng.ent_mut(id) {
                v.v_angle = Vec3::ZERO;
                v.view_ofs = Vec3::ZERO;
            }
            game.place_player(eng, away, Vec3::new(0.0, 6000.0, 100.0));
            game.place_player(eng, who, Vec3::new(far, 0.0, 100.0));
        }
    };
    b.run(aim(3000.0, 2, 3));
    b.step(30);
    assert!(status(&b.messages()).1.is_empty(), "{:?}", b.messages());
    // At 1500 it is shown the name and the health of anyone, enemy or not: the teammate
    // format, type 2 (the observer's team is not the terrorists'), the edict index, the health.
    b.run(aim(1500.0, 2, 3));
    b.step(30);
    let (_, values) = status(&b.messages());
    assert_eq!(values, vec![(1, 2), (2, 3), (3, 100)]);
}

#[test]
fn a_spectator_looks_8192_units_for_the_status_bar() {
    let mut b = late_probe_watches("3");
    b.probe.clear_messages();
    b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 100.0));
        let id = eng.client_edict(0);
        if let Some(v) = eng.ent_mut(id) {
            v.v_angle = Vec3::ZERO;
            v.view_ofs = Vec3::ZERO;
        }
        game.place_player(eng, 3, Vec3::new(0.0, 6000.0, 100.0));
        // Slot 2 (edict 3), a terrorist, 5000 units ahead: beyond what a player sees.
        game.place_player(eng, 2, Vec3::new(5000.0, 0.0, 100.0));
    });
    b.step(30);
    let (text, values) = status(&b.messages());
    assert_eq!(text, vec![(0, "1 %c1: %p2
2  %h: %i3%%".to_string())]);
    assert_eq!(values, vec![(1, 2), (2, 3), (3, 100)]);
}

#[test]
fn night_vision_follows_the_player_the_observer_watches() {
    let mut b = four_players();
    probe_observes(&mut b, "4");
    assert_eq!(probe_vars(&mut b), (4, 3));
    assert!(b.server.game_mut().game.grant_night_vision(2));
    let toggles = |b: &Bench| -> Vec<bool> {
        b.messages()
            .into_iter()
            .filter_map(|m| match m {
                CsUserMsg::NvgToggle(on) => Some(on),
                _ => None,
            })
            .collect()
    };
    b.step(40);
    b.probe.clear_messages();
    command(&mut b, 2, "nightvision");
    assert_eq!(toggles(&b), vec![true]);
    b.step(40);
    b.probe.clear_messages();
    command(&mut b, 2, "nightvision");
    assert_eq!(toggles(&b), vec![false]);

    // On again, then the watched player spawns: its goggles and the observer's go off.
    b.step(40);
    command(&mut b, 2, "nightvision");
    b.probe.clear_messages();
    b.run(|game, eng| game.respawn_player(eng, 2));
    b.step(3);
    assert_eq!(toggles(&b), vec![false]);

    // An observer that watches someone else is left alone.
    b.step(40);
    b.probe.clear_messages();
    command(&mut b, 3, "nightvision");
    assert!(toggles(&b).is_empty());
}

/// The probe (slot 0) joins `team_choice` (1 terrorists, 2 counter-terrorists), slot 1 the
/// other team and slot 2 the probe's team; returns what the probe was sent from the moment
/// everyone is in until the freeze period of the round has ended.
fn through_the_freeze_end(team_choice: u8) -> (Vec<SvcMessage>, Vec<(u8, String, i16)>) {
    let mut b = Bench::stub_slots(spawns(3, 3), 3);
    b.idle_cmds = true;
    b.step(6);
    let other = 3 - team_choice;
    join(&mut b, 0, team_choice, 1);
    enter(&mut b, 1);
    join(&mut b, 1, other, 1);
    enter(&mut b, 2);
    join(&mut b, 2, team_choice, 1);
    b.probe.clear_messages();
    let mut all: Vec<SvcMessage> = Vec::new();
    let mut calls = Vec::new();
    for _ in 0..60 {
        b.step(100);
        let got: Vec<SvcMessage> = b.probe.messages().iter().cloned().collect();
        b.probe.clear_messages();
        calls.extend(
            audio(&got)
                .into_iter()
                .filter(|a| a.1.starts_with("%!MRAD_")),
        );
        all.extend(got);
        if !calls.is_empty() {
            break;
        }
    }
    (all, calls)
}

/// F4: `OnRoundFreezeEnd` has the first joined player of each team call one of four sentences
/// over the radio, without a text. The probe is the first of its team, so it hears itself;
/// it is run once as a terrorist and once as a counter-terrorist.
#[test]
fn the_round_start_radio_call_reaches_the_first_player_of_each_team() {
    for team_choice in [1u8, 2] {
        let (all, calls) = through_the_freeze_end(team_choice);
        assert_eq!(calls.len(), 1, "team {team_choice}: {calls:?}");
        let (sender, code, pitch) = &calls[0];
        assert_eq!(
            (*sender, *pitch),
            (1, 100),
            "the first joined player is slot 0"
        );
        assert!(
            [
                "%!MRAD_MOVEOUT",
                "%!MRAD_LETSGO",
                "%!MRAD_LOCKNLOAD",
                "%!MRAD_GO"
            ]
            .contains(&code.as_str()),
            "{code}"
        );
        // No verbose text (`Radio(sentence)`): no `#Game_radio`, but the icon over the speaker.
        assert!(
            texts(&all)
                .iter()
                .all(|t| t.2.first().map(String::as_str) != Some("#Game_radio")),
            "{:?}",
            texts(&all)
        );
        assert_eq!(icons(&all).len(), 1, "{:?}", icons(&all));
    }
}

/// F27: the radar tells the team at most once a second (`m_tmNextRadarUpdate`): a player that
/// moved far twice within the second is sent once; after the second it is sent again. Counted
/// in server frames, no wall clock.
#[test]
fn the_radar_is_sent_at_most_once_a_second() {
    let mut b = bench();
    b.step(150);
    let radars = |b: &Bench| {
        b.messages()
            .iter()
            .filter(|m| matches!(m, CsUserMsg::Radar { player_id: 3, .. }))
            .count()
    };
    let hop = |b: &mut Bench, x: f32| {
        b.run(move |game: &mut CsGame, eng: &mut EngineApi<'_>| {
            game.place_player(eng, 2, Vec3::new(x, -480.0, 48.0));
            common::commands_of(game, eng, 2, 1);
        });
        b.step(2);
    };
    b.probe.clear_messages();
    hop(&mut b, 1000.0);
    assert_eq!(radars(&b), 1);
    // 100 units further, 0.04 s later: due only after a second.
    hop(&mut b, 1100.0);
    b.step(40);
    hop(&mut b, 1200.0);
    assert_eq!(radars(&b), 1, "{:?}", b.messages());
    // More than a second after the first one.
    b.step(80);
    hop(&mut b, 1300.0);
    assert_eq!(radars(&b), 2);
}
