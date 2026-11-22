//! The flashlight and its battery (`FlashBat`), the spray paint (`impulse 201`), the hints
//! (`HudTextArgs`), the Geiger counter and the pacing of `PlayerDeathThink`, ported from
//! ReGameDLL_CS `player.cpp` (read 2026-11-22). The spray test on a stock map returns without
//! assertions when the game is not installed.

mod common;

use common::{Bench, command, discover, dust2, enter, join, spawns};
use cs_engine::consts::{FL_ONGROUND, FL_SPECTATOR};
use cs_engine::{EngineApi, GameDll, ServerMap};
use cs_game::CsGame;
use cs_game::player::{DEAD_DEAD, Team, dmg};
use cs_game::weapons::{WeaponId, new_weapon};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const EF_DIMLIGHT: u32 = 8;

/// The payloads of the messages named `name` that the game sent the probe (the ones the protocol
/// crate has a variant for are put back into their bytes).
fn payloads(b: &Bench, name: &str) -> Vec<Vec<u8>> {
    b.messages()
        .iter()
        .filter_map(|m| match m {
            CsUserMsg::Unknown { name: n, payload } if n == name => Some(payload.clone()),
            CsUserMsg::Unknown { .. } => None,
            m => {
                let (n, bytes) = cs_proto48::usermsg::encode_cs_usermsg(m);
                (n == name).then_some(bytes)
            }
        })
        .collect()
}

/// The text of each `HudTextArgs` the probe got: the hint, its `isHint` byte and its argument
/// count, in `WRITE_STRING`, `WRITE_BYTE`, `WRITE_BYTE` order.
fn hints(b: &Bench) -> Vec<String> {
    payloads(b, "HudTextArgs")
        .iter()
        .map(|p| {
            let end = p.iter().position(|&c| c == 0).expect("the text ends with a NUL");
            assert_eq!(p[end + 1..], [1, 0], "isHint 1, no arguments");
            String::from_utf8_lossy(&p[..end]).into_owned()
        })
        .collect()
}

/// A stub-map server with the probe on the counter-terrorists, running a command every frame.
fn bench() -> Bench {
    let mut b = Bench::stub(spawns(4, 4));
    b.idle_cmds = true;
    b.step(6);
    join(&mut b, 0, 2, 1);
    b.run(|_, eng| eng.set_cvar("sv_gravity", "0"));
    b
}

/// One command from the probe with `impulse` (the engine copies it to `pev->impulse`).
fn impulse(b: &mut Bench, impulse: u8) {
    b.probe.queue_cmd(GoldSrcUserCmd {
        msec: 10,
        impulse,
        ..Default::default()
    });
    b.step(1);
}

fn effects(b: &mut Bench) -> u32 {
    b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().effects)
}

// ---- the flashlight ----

#[test]
fn the_battery_is_charged_to_100_in_two_flashbat_messages_after_a_spawn() {
    let mut b = bench();
    b.step(100);
    b.probe.clear_messages();
    // `Spawn` sets `m_iFlashBattery = 99` and the clock to 1 (due): the first `UpdateClientData`
    // charges it to 100 and the clock then runs out 0.2 s later (`m_flFlashLightTime = 0`), as
    // the golden capture of HLDS has it, `FlashBat 100` twice.
    assert!(b.run(|game, eng| game.respawn_player(eng, 0)));
    b.step(100);
    assert_eq!(payloads(&b, "FlashBat"), [vec![100], vec![100]]);
    assert_eq!(b.run(|game, _| game.flash_battery(0)), Some(100));
    // Nothing more once it is full.
    b.probe.clear_messages();
    b.step(300);
    assert!(payloads(&b, "FlashBat").is_empty());
}

#[test]
fn impulse_100_toggles_the_flashlight_when_the_rules_allow_it() {
    let mut b = bench();
    b.step(100);
    // `mp_flashlight 0` (the default): `FlashlightTurnOn` returns at once.
    b.probe.clear_messages();
    impulse(&mut b, 100);
    b.step(3);
    assert!(payloads(&b, "Flashlight").is_empty());
    assert_eq!(effects(&mut b) & EF_DIMLIGHT, 0);

    b.run(|_, eng| eng.set_cvar("mp_flashlight", "1"));
    b.probe.clear_messages();
    impulse(&mut b, 100);
    b.step(3);
    // `Flashlight 1 <battery>` and `EF_DIMLIGHT`.
    assert_eq!(payloads(&b, "Flashlight"), [vec![1, 100]]);
    assert_ne!(effects(&mut b) & EF_DIMLIGHT, 0);

    // The battery drains one unit per 1.2 s while it is on, `FlashBat` each time.
    b.probe.clear_messages();
    b.step(260);
    assert_eq!(payloads(&b, "FlashBat"), [vec![99], vec![98]]);

    // Off: `Flashlight 0 <battery>`, `EF_DIMLIGHT` gone, then it charges back, a unit per 0.2 s,
    // up to 100 and one `FlashBat` more for the clock running out.
    b.probe.clear_messages();
    impulse(&mut b, 100);
    b.step(3);
    assert_eq!(payloads(&b, "Flashlight"), [vec![0, 98]]);
    assert_eq!(effects(&mut b) & EF_DIMLIGHT, 0);
    b.probe.clear_messages();
    b.step(100);
    assert_eq!(payloads(&b, "FlashBat"), [vec![99], vec![100], vec![100]]);
}

#[test]
fn an_empty_battery_turns_the_flashlight_off() {
    let mut b = bench();
    b.run(|_, eng| eng.set_cvar("mp_flashlight", "1"));
    b.step(100);
    b.run(|game, _| game.set_flash_battery(0, 1));
    b.probe.clear_messages();
    impulse(&mut b, 100);
    b.step(150);
    // `--m_iFlashBattery <= 0`: the light goes off (`Flashlight 0 0`) before the `FlashBat 0`.
    assert_eq!(payloads(&b, "Flashlight"), [vec![1, 1], vec![0, 0]]);
    assert_eq!(payloads(&b, "FlashBat")[0], vec![0]);
    assert_eq!(effects(&mut b) & EF_DIMLIGHT, 0);
}

// ---- the Geiger counter ----

#[test]
fn the_geiger_counter_tells_its_range_once_two_seconds_after_the_spawn() {
    let mut b = bench();
    b.step(100);
    b.probe.clear_messages();
    assert!(b.run(|game, eng| game.respawn_player(eng, 0)));
    // `m_flgeigerDelay = time + 2`.
    b.step(150);
    assert!(payloads(&b, "Geiger").is_empty());
    b.step(100);
    // `byte(1000 / 4)`: the range differs from the 1000 `m_igeigerRangePrev` starts at, once.
    assert_eq!(payloads(&b, "Geiger"), [vec![250]]);
    b.step(500);
    assert_eq!(payloads(&b, "Geiger").len(), 1);
}

// ---- the hints ----

#[test]
fn the_first_buy_hint_of_a_round_is_a_hudtextargs() {
    let mut b = bench();
    b.step(20);
    // The stub map has no buy zone, so a spawn point counts as one.
    assert_eq!(hints(&b)[0], "#Hint_press_buy_to_purchase");
    let first = payloads(&b, "HudTextArgs")[0].clone();
    assert_eq!(first, b"#Hint_press_buy_to_purchase\0\x01\0");
    // Once per round: the history keeps it.
    b.probe.clear_messages();
    b.step(1000);
    assert!(hints(&b).is_empty());
    // A spawn forgets the round's hints (`DHM_ROUND_CLEAR`) and the queue starts over; the hint
    // is told again at the next command.
    assert!(b.run(|game, eng| game.respawn_player(eng, 0)));
    b.step(20);
    assert_eq!(hints(&b), ["#Hint_press_buy_to_purchase"]);
}

#[test]
fn a_client_with_autohelp_off_gets_only_the_forced_hints() {
    let map = ServerMap::stub_with_entities("test", spawns(4, 4));
    let mut b = Bench::with_userinfo(map, 2, &[("_ah", "0")]);
    b.idle_cmds = true;
    b.step(6);
    join(&mut b, 0, 2, 1);
    b.step(200);
    // `HintMessageEx`: `bOverride || m_bShowHints`; the buy hint is not forced.
    assert!(hints(&b).is_empty());
    // Dying tells the dead how to change the view, forced (`HintMessage("#Spec_Duck", TRUE,
    // TRUE)`).
    b.probe.clear_messages();
    assert!(!b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::BULLET)));
    b.step(10);
    assert_eq!(hints(&b), ["#Spec_Duck"]);
}

/// The probe and a teammate on the counter-terrorists, two terrorists, all spawned and with the
/// hints of the join (the buy hint, the sight of a friend or an enemy) long over, so the next
/// hint is shown at the next command.
fn four_players() -> Bench {
    let mut b = Bench::stub_slots(spawns(8, 8), 4);
    b.idle_cmds = true;
    b.step(6);
    join(&mut b, 0, 2, 1);
    for (slot, team_choice) in [(1, 2), (2, 1), (3, 1)] {
        enter(&mut b, slot);
        join(&mut b, slot, team_choice, 1);
    }
    b.run(|_, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_friendlyfire", "1");
    });
    b.step(2500);
    b
}

#[test]
fn hurting_a_teammate_hints_the_attacker_and_marks_the_victim() {
    let mut b = four_players();
    b.probe.clear_messages();
    // The probe (the attacker) hurts slot 1, a teammate.
    assert!(b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        let attacker = Some(eng.client_edict(0));
        game.damage_player_by(eng, 1, attacker, attacker, 10.0, dmg::BULLET)
    }));
    b.step(5);
    assert_eq!(hints(&b), ["#Hint_try_not_to_injure_teammates"]);
    // QUIRK (vanilla, not `REGAMEDLL_FIXES`): the victim's history is the one that is looked at
    // and set, so the same victim does not make the hint come again, and the attacker's own
    // history does not have the bit.
    let (victim, attacker) = b.run(|game, _| (game.display_history(1).unwrap(), game.display_history(0).unwrap()));
    assert_ne!(victim & (1 << 9), 0, "DHF_FRIEND_INJURED of the victim");
    assert_eq!(attacker & (1 << 9), 0, "not of the attacker");

    // A hint waits for the one before it: dying queues `#Spec_Duck`, which is held back until
    // the 6 seconds of the hint on the screen are over (`m_tmMessageEnd`).
    b.probe.clear_messages();
    assert!(!b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::BULLET)));
    b.step(300);
    assert!(hints(&b).is_empty());
    b.step(400);
    assert_eq!(hints(&b), ["#Spec_Duck"]);
    // Once per entry: `DHF_SPEC_DUCK` is not cleared by a spawn.
    let history = b.run(|game, _| game.display_history(0)).unwrap();
    assert_ne!(history & (1 << 22), 0);
}

#[test]
fn a_dry_weapon_hints_out_of_ammo_once() {
    let mut b = bench();
    b.step(800);
    b.probe.clear_messages();
    b.run(|game, eng| {
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Deagle).unwrap(), 0, 0));
        assert!(game.select_weapon(eng, 0, WeaponId::Deagle));
    });
    b.step(100);
    assert_eq!(hints(&b), ["#Hint_out_of_ammo"]);
    b.step(800);
    assert_eq!(hints(&b).len(), 1);
}

// ---- PlayerDeathThink ----

#[test]
fn a_body_on_the_ground_slows_down_at_every_command() {
    let mut b = bench();
    assert!(!b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::BULLET)));
    b.step(5);
    // Three commands in the same frame, the body "on the ground" each time: the velocity falls
    // by 20 a command (`PlayerDeathThink` runs from `PreThink`, not once per tenth of a second).
    let speeds = b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        let id = eng.client_edict(0);
        let mut out = Vec::new();
        for _ in 0..3 {
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::new(100.0, 0.0, 0.0);
                v.flags |= FL_ONGROUND;
            }
            let cmd = GoldSrcUserCmd { msec: 0, ..Default::default() };
            game.run_usercmd(eng, 0, &cmd, 0);
            out.push(eng.ent(id).unwrap().velocity.length());
        }
        out
    });
    // `flForward = velocity.Length() - 20`: exactly 80, each time.
    for s in speeds {
        assert!((s - 80.0).abs() < 1e-3, "the velocity of the body decays by 20: {s}");
    }
}

#[test]
fn the_dead_are_dead_dead_at_once_without_a_death_animation() {
    // The server has no model loader here, so there is no sequence to wait for: the first
    // command after the death is `DEAD_DEAD`, and the death time is that command's.
    let mut b = bench();
    assert!(!b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::BULLET)));
    b.step(3);
    assert_eq!(b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().deadflag), DEAD_DEAD);
}

// ---- impulse 201 ----

#[test]
fn spray_paints_a_decal_where_the_player_looks_every_decalfrequency() {
    let Some(vfs) = discover() else {
        return;
    };
    let mut b = Bench::on(dust2(&vfs));
    // The decal table the engine reads from `decals.wad`: only the name matters here.
    b.server.set_decals(vec!["{shot1".to_string(), "{lambda06".to_string()]);
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
    });
    b.step(5);
    let spray = |b: &mut Bench| {
        b.probe.clear_messages();
        b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
            let id = eng.client_edict(0);
            if let Some(v) = eng.ent_mut(id) {
                v.impulse = 201;
            }
            // Looking down at the floor, within 128 units of the eyes.
            let cmd = GoldSrcUserCmd {
                msec: 10,
                viewangles: Vec3::new(89.0, 0.0, 0.0),
                ..Default::default()
            };
            common::run_cmd(game, eng, 0, &cmd, 0);
            (eng.find_by_classname(None, "spraycan").is_some(), eng.ent(id).unwrap().impulse)
        })
    };
    let decals = |b: &Bench| {
        b.probe
            .messages()
            .iter()
            .filter(|m| matches!(m, SvcMessage::TempEntity { te_type, .. } if *te_type == 116 || *te_type == 104))
            .count()
    };
    // The can is made at once with the sound; it paints and is gone a tenth of a second later.
    let (can, left_over) = spray(&mut b);
    assert!(can, "the spray can exists");
    assert_eq!(left_over, 0, "the impulse is consumed");
    b.step(20);
    assert!(!b.run(|_, eng| eng.find_by_classname(None, "spraycan").is_some()));
    assert_eq!(decals(&b), 1, "one `TE_WORLDDECAL`/`TE_DECAL` of `DECAL_LAMBDA6`");
    // `decalfrequency` (30 s): a second spray at once does nothing, one after it does.
    let (can, _) = spray(&mut b);
    assert!(!can);
    b.step(3100);
    let (can, _) = spray(&mut b);
    assert!(can);
}

// ---- the other hints ----

/// Four players, the probe (CT) looking along +x at a teammate (`friend`) or an enemy 200 units
/// ahead, the rest far away; hanging in the air, the join hints over.
fn sighted(friend: bool, spectator: bool) -> Bench {
    let mut b = four_players();
    b.run(move |game: &mut CsGame, eng: &mut EngineApi<'_>| {
        game.place_player(eng, 1, Vec3::new(0.0, 6000.0, 100.0));
        game.place_player(eng, 2, Vec3::new(0.0, -6000.0, 100.0));
        game.place_player(eng, 3, Vec3::new(0.0, -7000.0, 100.0));
        game.place_player(eng, 0, Vec3::new(0.0, 0.0, 100.0));
        game.place_player(eng, if friend { 1 } else { 2 }, Vec3::new(200.0, 0.0, 100.0));
        // What the joins showed is forgotten, but the round has started (`DHF_ROUND_STARTED`).
        for slot in 0..4 {
            game.set_display_history(slot, 1 << 1);
        }
        let id = eng.client_edict(0);
        if let Some(v) = eng.ent_mut(id) {
            v.v_angle = Vec3::ZERO;
            if spectator {
                v.flags |= FL_SPECTATOR;
            }
        }
    });
    b.probe.clear_messages();
    b
}

#[test]
fn the_first_friend_and_the_first_enemy_in_the_crosshair_are_hints() {
    // `#Hint_spotted_a_friend`, once (`DHF_FRIEND_SEEN`).
    let mut b = sighted(true, false);
    b.step(60);
    assert_eq!(hints(&b), ["#Hint_spotted_a_friend"]);
    assert_ne!(b.run(|g, _| g.display_history(0)).unwrap() & (1 << 7), 0);
    b.probe.clear_messages();
    b.step(1000);
    assert!(hints(&b).is_empty());

    // `#Hint_spotted_an_enemy`, once (`DHF_ENEMY_SEEN`).
    let mut b = sighted(false, false);
    b.step(60);
    assert_eq!(hints(&b), ["#Hint_spotted_an_enemy"]);
    assert_ne!(b.run(|g, _| g.display_history(0)).unwrap() & (1 << 8), 0);
}

#[test]
fn a_spectator_is_not_told_about_friends() {
    // `!(pev->flags & FL_SPECTATOR)` guards the friend hint and its history bit.
    let mut b = sighted(true, true);
    b.step(60);
    assert!(hints(&b).is_empty(), "{:?}", hints(&b));
    assert_eq!(b.run(|g, _| g.display_history(0)).unwrap() & (1 << 7), 0);
}

#[test]
fn the_first_night_vision_purchase_hints_how_to_use_it() {
    let mut b = four_players();
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().money = 5000);
    b.probe.clear_messages();
    command(&mut b, 0, "nvgs");
    b.step(5);
    assert_eq!(hints(&b), ["#Hint_use_nightvision"]);
    assert_ne!(b.run(|g, _| g.display_history(0)).unwrap() & (1 << 20), 0);
}

#[test]
fn the_first_kills_of_a_teammate_and_of_an_enemy_are_hints() {
    let mut b = four_players();
    b.run(|game, _| {
        for slot in 0..4 {
            game.set_display_history(slot, 1 << 1);
        }
    });
    b.probe.clear_messages();
    // A teammate: the hit is `#Hint_try_not_to_injure_teammates` (the victim's first injury),
    // the kill `#Hint_careful_around_teammates`, a hint at a time.
    assert!(!b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        let me = Some(eng.client_edict(0));
        game.damage_player_by(eng, 1, me, me, 500.0, dmg::BULLET)
    }));
    b.step(800);
    assert_eq!(
        hints(&b),
        ["#Hint_try_not_to_injure_teammates", "#Hint_careful_around_teammates"]
    );
    // An enemy: `#Hint_win_round_by_killing_enemy`.
    b.step(500);
    b.probe.clear_messages();
    assert!(!b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        let me = Some(eng.client_edict(0));
        game.damage_player_by(eng, 2, me, me, 500.0, dmg::BULLET)
    }));
    // (The hints of what the probe sees meanwhile wait in the queue too.)
    b.step(1500);
    assert!(hints(&b).contains(&"#Hint_win_round_by_killing_enemy".to_string()), "{:?}", hints(&b));
}

#[test]
fn a_player_punished_for_a_team_kill_is_told_when_it_dies() {
    let mut b = four_players();
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().punished_for_tk = true);
    b.probe.clear_messages();
    assert!(!b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::BULLET)));
    b.step(800);
    // The punishment first, then how to look around (both forced, shown to the dead).
    assert_eq!(hints(&b), ["#Hint_cannot_play_because_tk", "#Spec_Duck"]);
    assert!(!b.run(|game, _| game.state().players[0].as_ref().unwrap().punished_for_tk));
}

#[test]
fn a_dead_player_has_the_entity_think_of_player_death_think_too() {
    // `Killed`: `SetThink(PlayerDeathThink)`, `nextthink = time + 0.1`; the think reschedules
    // itself while the dead wait with no button down, on a team.
    let mut b = bench();
    let now = b.run(|_, eng| eng.time());
    assert!(!b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::BULLET)));
    let at = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().nextthink);
    assert!(at > now, "the think is scheduled: {at}");
    b.step(100);
    let (later, t) = b.run(|_, eng| (eng.ent(eng.client_edict(0)).unwrap().nextthink, eng.time()));
    assert!(later > 0.0 && later <= t + 0.2, "it keeps rescheduling: {later} at {t}");
}
