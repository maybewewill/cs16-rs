//! C4 weapon and planted bomb (Task 4 of phase 2E): arming and planting in a bomb zone
//! (`CC4`), `BarTime`, beeps, blinking `TE_GLOWSPRITE`, timer clamping and detonation (`C4Think`
//! / `Detonate2` / `Explode2`), `CBaseMonster::RadiusDamage` scaling with `info_map_parameters`
//! `bombradius`, CT defusal (`DefuseBombStart` / `DefuseBombEnd` 5s/10s), and the vanilla water
//! deletion quirk.

mod common;

use common::{Bench, entity, spawns};
use cs_assets::bsp::Entity;
use cs_engine::consts::{FL_ONGROUND, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_game::weapons::WeaponId;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::tempent::te;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;
const IN_USE: u16 = 1 << 5;
const CHAN_WEAPON: u8 = 1;
const CHAN_VOICE: u8 = 2;
const CHAN_ITEM: u8 = 3;

const C4_IDLE1: i32 = 0;
const C4_DROP: i32 = 2;
const C4_ARM: i32 = 3;

const ZONE_AT: Vec3 = Vec3::new(1000.0, 0.0, 0.0);

fn bomb_arena(extra: Vec<Entity>) -> Bench {
    let mut entities = spawns(4, 4);
    entities.push(entity(&[
        ("classname", "func_bomb_target"),
        ("origin", "1000 0 0"),
    ]));
    entities.extend(extra);
    let mut b = Bench::with_config(
        cs_engine::ServerMap::stub_with_entities("test", entities),
        cs_engine::ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[("cl_lw", "0")],
        false,
    );
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
                eng.set_origin(id, origin);
            }
        }
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        let cmd_args = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
        // Advance both slots through ShowLText -> ShowTeamSelect -> PickingTeam -> GetIntoGame
        // so `get_into_game` clears `not_killed`, equips default weapons, and starts the match.
        for _ in 0..2 {
            cmd_slot(game, eng, 0, 0, true);
            cmd_slot(game, eng, 1, 0, true);
        }
        game.client_command(eng, 0, &cmd_args("jointeam", "1"));
        game.client_command(eng, 0, &cmd_args("joinclass", "1"));
        game.client_command(eng, 1, &cmd_args("jointeam", "2"));
        game.client_command(eng, 1, &cmd_args("joinclass", "1"));
        cmd_slot(game, eng, 0, 0, true);
        cmd_slot(game, eng, 1, 0, true);
    });
    // Clear any pending `game_commence` restart cleanly so tests aren't interrupted 3 seconds in.
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        let t = eng.client_edict(0);
        let ct = eng.client_edict(1);
        eng.set_origin(t, Vec3::ZERO);
        eng.set_origin(ct, Vec3::new(3000.0, 0.0, 0.0));
        eng.ent_mut(t).unwrap().flags |= FL_ONGROUND;
        eng.ent_mut(ct).unwrap().flags |= FL_ONGROUND;
        ensure_c4(game, eng, 0);
        assert!(game.carried_weapons(0).contains(&WeaponId::Glock18));
    });
    // Drain initial join / spawn reliable messages at 20 Hz.
    b.step(12);
    b.probe.clear_messages();
    b
}

/// Gives C4 to `slot` (or refills it if already carried) and zeroes deploy delay.
fn ensure_c4(game: &mut CsGame, eng: &mut EngineApi<'_>, slot: u8) {
    assert!(game.test_give_c4(eng, slot));
    if !game.select_weapon(eng, slot, WeaponId::C4) {
        assert_eq!(
            game.player_weapon_state(slot).unwrap().2,
            Some(WeaponId::C4)
        );
    }
    game.set_next_attack(slot, 0.0);
    if let Some(w) = game.weapon_mut(slot, WeaponId::C4) {
        w.next_primary_attack = 0.0;
    }
}

/// Runs one command for `slot` with `buttons`, keeping `FL_ONGROUND` as requested (on a stub
/// map `pmove` has no floor to stand on and clears `FL_ONGROUND` unless restored before `PostThink`).
fn cmd_slot(game: &mut CsGame, eng: &mut EngineApi<'_>, slot: u8, buttons: u16, on_ground: bool) {
    let id = eng.client_edict(slot);
    let viewangles = eng.ent(id).unwrap().v_angle;
    eng.ent_mut(id).unwrap().button = i32::from(buttons);
    let cmd = GoldSrcUserCmd {
        msec: 10,
        buttons,
        viewangles,
        ..Default::default()
    };
    game.run_usercmd(eng, slot, &cmd, 0);
    if let Some(v) = eng.ent_mut(id) {
        if on_ground {
            v.flags |= FL_ONGROUND;
        } else {
            v.flags &= !FL_ONGROUND;
        }
    }
    // Simulate `SV_LinkEdict(id, true)` trigger touches between `run_usercmd` and `post_usercmd`
    // (on a stub map without `clc_move`, `run_usercmd` alone does not dispatch touches).
    if let Some((pmin, pmax)) = eng.ent(id).map(|v| (v.absmin, v.absmax)) {
        for other in eng.entity_ids() {
            if other == id {
                continue;
            }
            if let Some(o) = eng.ent(other)
                && o.solid == SOLID_TRIGGER
                && o.absmin.x <= pmax.x
                && o.absmax.x >= pmin.x
                && o.absmin.y <= pmax.y
                && o.absmax.y >= pmin.y
                && o.absmin.z <= pmax.z
                && o.absmax.z >= pmin.z
            {
                game.touch(eng, other, id);
            }
        }
    }
    game.post_usercmd(eng, slot, &cmd);
}

fn step_cmd(b: &mut Bench, slot: u8, buttons: u16, on_ground: bool, frames: usize) {
    for _ in 0..frames {
        b.run(move |game, eng| cmd_slot(game, eng, slot, buttons, on_ground));
    }
}

fn temp_entities(b: &Bench) -> Vec<u8> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::TempEntity { te_type, .. } => Some(*te_type),
            _ => None,
        })
        .collect()
}

fn sound_indices(b: &Bench) -> Vec<(u16, u8)> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Sound(s) => Some((s.sound_index, s.channel)),
            _ => None,
        })
        .collect()
}

fn text_msgs(b: &Bench) -> Vec<(u8, String)> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg { dest, message, .. } => Some((dest, message)),
            _ => None,
        })
        .collect()
}

fn sound_id(b: &mut Bench, sample: &str) -> u16 {
    let sample = sample.to_string();
    b.run(move |_, eng| eng.precache_sound(&sample).expect("sound precached"))
}

fn plant_bomb_at_zone(b: &mut Bench, slot: u8) -> EdictId {
    b.run(move |game, eng| {
        let id = eng.client_edict(slot);
        eng.set_origin(id, ZONE_AT);
        eng.ent_mut(id).unwrap().flags |= FL_ONGROUND;
        game.set_next_attack(slot, 0.0);
    });
    // Wait for `HandleSignals` (every 0.5 s) via `run_usercmd`.
    step_cmd(b, slot, 0, true, 60);
    for _ in 0..320 {
        b.run(move |game, eng| cmd_slot(game, eng, slot, IN_ATTACK, true));
        let planted = b.run(|game, eng| game.grenades(eng).into_iter().next());
        if let Some(bomb) = planted {
            return bomb;
        }
    }
    panic!("C4 did not plant within 3.2 seconds");
}

#[test]
fn cannot_plant_outside_bomb_zone_or_in_air() {
    let mut b = bomb_arena(vec![]);
    // Outside any bomb zone: `#C4_Plant_At_Bomb_Spot`, maxspeed stays 250.
    b.run(|game, eng| {
        cmd_slot(game, eng, 0, IN_ATTACK, true);
        let v = eng.ent(eng.client_edict(0)).unwrap();
        assert_eq!(v.max_speed, 250.0);
        assert!(game.grenades(eng).is_empty());
    });
    b.step(10);
    assert!(
        text_msgs(&b)
            .iter()
            .any(|(d, t)| *d == 4 && t == "#C4_Plant_At_Bomb_Spot"),
        "{:?}",
        text_msgs(&b)
    );
    b.probe.clear_messages();

    // Move into zone, spend 0.5s `HandleSignals` + 1.0s attack cooldown, then attempt in air.
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), ZONE_AT);
    });
    step_cmd(&mut b, 0, 0, true, 110);
    b.probe.clear_messages();
    b.run(|game, eng| {
        cmd_slot(game, eng, 0, IN_ATTACK, false);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, 250.0);
        assert!(game.grenades(eng).is_empty());
    });
    b.step(10);
    assert!(
        text_msgs(&b)
            .iter()
            .any(|(d, t)| *d == 4 && t == "#C4_Plant_Must_Be_On_Ground"),
        "{:?}",
        text_msgs(&b)
    );
}

#[test]
fn planting_freezes_progresses_and_plants_after_three_seconds() {
    let mut b = bomb_arena(vec![]);
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), ZONE_AT);
    });
    step_cmd(&mut b, 0, 0, true, 60);
    b.probe.clear_messages();

    // First attack frame freezes to 1.0, starts 3 s progress bar and C4_ARM anim.
    b.run(|game, eng| {
        cmd_slot(game, eng, 0, IN_ATTACK, true);
        let v = eng.ent(eng.client_edict(0)).unwrap();
        assert_eq!(v.max_speed, 1.0);
        assert_eq!(v.weaponanim, C4_ARM);
    });
    b.step(10);
    assert!(b.messages().contains(&CsUserMsg::BarTime(3)));

    // Step past 2.25 s (`armed_time - 0.75`): placement animation switches to `C4_DROP`.
    let mut saw_drop = false;
    let mut planted = None;
    for _ in 0..320 {
        saw_drop |= b.run(|game, eng| {
            cmd_slot(game, eng, 0, IN_ATTACK, true);
            eng.ent(eng.client_edict(0)).unwrap().weaponanim == C4_DROP
        });
        if let Some(bomb) = b.run(|game, eng| game.grenades(eng).into_iter().next()) {
            planted = Some(bomb);
            break;
        }
    }
    assert!(saw_drop, "expected C4_DROP placement animation before finish");
    let bomb = planted.expect("bomb spawned after 3 seconds");
    b.step(10);

    let c4_plant = sound_id(&mut b, "weapons/c4_plant.wav");
    assert!(sound_indices(&b).contains(&(c4_plant, CHAN_WEAPON)));

    let msgs = b.messages();
    assert!(msgs.contains(&CsUserMsg::SendAudio {
        sender_id: 0,
        audio_code: "%!MRAD_BOMBPL".to_string(),
        pitch: 100,
    }));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::BombDrop { planted: true, .. }
    )));
    assert!(
        text_msgs(&b).iter().any(|(d, t)| *d == 4 && t == "#Bomb_Planted"),
        "{:?}",
        text_msgs(&b)
    );

    b.run(move |game, eng| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(!p.has_c4);
        assert!(!game.state().rules.bomb_dropped);
        let player = eng.ent(eng.client_edict(0)).unwrap();
        assert_eq!(player.body, 0);
        assert!(player.max_speed >= 240.0, "max_speed restored: {}", player.max_speed);
        // C4 was retired and destroyed.
        assert!(
            game.weapon_mut(0, WeaponId::C4).is_none(),
            "carried={:?} state={:?}",
            game.carried_weapons(0),
            game.player_weapon_state(0)
        );
        let w_c4 = eng.model_index("models/w_c4.mdl").unwrap();
        assert_eq!(eng.ent(bomb).unwrap().modelindex, w_c4);
    });
}

#[test]
fn releasing_attack_or_leaving_zone_cancels_plant() {
    // Part A: releasing IN_ATTACK cancels via `WeaponIdle`.
    let mut b = bomb_arena(vec![]);
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), ZONE_AT);
    });
    step_cmd(&mut b, 0, 0, true, 60);
    b.run(|game, eng| {
        cmd_slot(game, eng, 0, IN_ATTACK, true);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, 1.0);
    });
    b.probe.clear_messages();
    b.run(|game, eng| {
        cmd_slot(game, eng, 0, 0, true);
        let v = eng.ent(eng.client_edict(0)).unwrap();
        assert_eq!(v.max_speed, 250.0);
        assert_eq!(v.weaponanim, C4_IDLE1);
        assert!(game.grenades(eng).is_empty());
    });
    b.step(10);
    assert!(b.messages().contains(&CsUserMsg::BarTime(0)));

    // Part B: holding IN_ATTACK while walking out of bomb zone cancels once `HandleSignals` clears.
    b.run(|game, eng| {
        game.set_next_attack(0, 0.0);
        if let Some(w) = game.weapon_mut(0, WeaponId::C4) {
            w.next_primary_attack = 0.0;
        }
        cmd_slot(game, eng, 0, IN_ATTACK, true);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, 1.0);
    });
    b.probe.clear_messages();
    b.run(|_, eng| {
        eng.set_origin(eng.client_edict(0), Vec3::ZERO);
    });
    step_cmd(&mut b, 0, IN_ATTACK, true, 100);
    b.step(10);
    assert!(
        text_msgs(&b)
            .iter()
            .any(|(d, t)| *d == 4 && t == "#C4_Arming_Cancelled"),
        "{:?}",
        text_msgs(&b)
    );
    assert!(b.messages().contains(&CsUserMsg::BarTime(0)));
    b.run(|game, eng| {
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, 250.0);
        assert!(game.grenades(eng).is_empty());
    });
}

#[test]
fn c4_beeps_blinks_and_detonates_at_c4timer() {
    let mut b = bomb_arena(vec![entity(&[
        ("classname", "test_target"),
        ("targetname", "bomb_fired"),
        ("origin", "1000 0 0"),
    ])]);
    b.run(|game, eng| {
        eng.set_cvar("mp_c4timer", "5");
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.c4_timer, 10);
        assert_eq!(eng.cvar_f32("mp_c4timer"), 10.0);

        eng.set_cvar("mp_c4timer", "120");
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.c4_timer, 90);
        assert_eq!(eng.cvar_f32("mp_c4timer"), 90.0);

        eng.set_cvar("mp_c4timer", "10");
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        eng.set_origin(eng.client_edict(0), ZONE_AT);
        eng.set_origin(eng.client_edict(1), Vec3::new(3000.0, 0.0, 0.0));
        ensure_c4(game, eng, 0);
        if let Some(bt) = eng.find_by_classname(None, "func_bomb_target") {
            eng.ent_mut(bt).unwrap().target = "bomb_fired".to_string();
        }
        game.state_mut().dispatch_log.clear();
    });

    let bomb = plant_bomb_at_zone(&mut b, 0);
    b.probe.clear_messages();

    // Advance 10.3 seconds so waves, beeps, LED blinks and `Explode2` all fire.
    b.step(1030);

    let beep1 = sound_id(&mut b, "weapons/c4_beep1.wav");
    let explode1 = sound_id(&mut b, "weapons/c4_explode1.wav");
    let sounds = sound_indices(&b);
    assert!(sounds.contains(&(beep1, CHAN_VOICE)), "missing c4_beep1 in {sounds:?}");
    assert!(sounds.contains(&(explode1, CHAN_WEAPON)), "missing c4_explode1 in {sounds:?}");

    let tes = temp_entities(&b);
    assert!(tes.contains(&te::GLOWSPRITE), "expected TE_GLOWSPRITE blink: {tes:?}");
    assert_eq!(
        tes.iter().filter(|&&t| t == te::SPRITE).count(),
        4,
        "expected 4 TE_SPRITE fireballs: {tes:?}"
    );

    let msgs = b.messages();
    assert!(msgs.contains(&CsUserMsg::BombPickup));
    assert!(msgs.iter().any(|m| matches!(m, CsUserMsg::Scenario { active: false, .. })));
    assert!(msgs.iter().any(|m| matches!(m, CsUserMsg::ScreenShake { .. })));

    b.run(move |game, eng| {
        assert!(game.state().rules.target_bombed);
        assert!(!game.state().rules.bomb_dropped);
        assert_eq!(game.state().players[0].as_ref().unwrap().frags, 3);
        assert!(
            game.state()
                .dispatch_log
                .iter()
                .any(|e| matches!(e, cs_game::entity::DispatchEvent::Use { .. })),
            "expected cur_bomb_target Use in {:?}",
            game.state().dispatch_log
        );
        assert!(eng.ent(bomb).is_some());
    });

    b.step(100);
    assert!(temp_entities(&b).contains(&te::SMOKE));
    b.run(move |_, eng| assert!(eng.ent(bomb).is_none()));
}

#[test]
fn bomb_explosion_uses_map_radius_times_three_point_five() {
    let mut b = bomb_arena(vec![entity(&[
        ("classname", "info_map_parameters"),
        ("bombradius", "200"),
        ("buying", "0"),
    ])]);
    b.run(|game, eng| {
        eng.set_cvar("mp_c4timer", "10");
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        assert_eq!(game.state().rules.bomb_radius(), 200.0);

        game.client_put_in_server(eng, 2);
        eng.test_set_client_active(2, true);
        assert!(game.spawn_player(eng, 2, Team::Ct, 0));

        eng.set_origin(eng.client_edict(0), ZONE_AT);
        eng.set_origin(eng.client_edict(1), ZONE_AT + Vec3::new(600.0, 0.0, 0.0));
        eng.set_origin(eng.client_edict(2), ZONE_AT + Vec3::new(750.0, 0.0, 0.0));
        ensure_c4(game, eng, 0);
    });

    let _bomb = plant_bomb_at_zone(&mut b, 0);
    b.probe.clear_messages();
    b.step(1030);

    b.run(|game, _| {
        let p0 = game.state().players[0].as_ref().unwrap();
        let p1 = game.state().players[1].as_ref().unwrap();
        let p2 = game.state().players[2].as_ref().unwrap();
        assert!(!p0.alive, "planter next to bomb should die");
        assert_eq!(p0.deaths, 0, "vanilla killed_by_bomb skips PlayerKilled / deaths++");
        assert!(
            p1.health < 100.0 && p1.health > 0.0,
            "600 < 700 should take falloff dmg: {}",
            p1.health
        );
        assert_eq!(p2.health, 100.0, "750 > 700 must take zero damage");
    });
    assert!(
        !b.messages().iter().any(|m| matches!(m, CsUserMsg::DeathMsg { .. })),
        "killed_by_bomb sends no DeathMsg"
    );
}

#[test]
fn ct_defuses_in_ten_or_five_seconds_and_t_cannot() {
    let mut b = bomb_arena(vec![]);
    b.run(|game, eng| {
        eng.set_cvar("mp_c4timer", "45");
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        eng.set_origin(eng.client_edict(0), ZONE_AT);
        ensure_c4(game, eng, 0);
    });
    let bomb = plant_bomb_at_zone(&mut b, 0);

    // Terrorist presses +USE facing bomb -> ignored.
    b.run(|game, eng| {
        let t = eng.client_edict(0);
        eng.set_origin(t, ZONE_AT + Vec3::new(-32.0, 0.0, 0.0));
        eng.ent_mut(t).unwrap().v_angle = Vec3::ZERO;
        cmd_slot(game, eng, 0, IN_USE, true);
        assert!(!game.state().players[0].as_ref().unwrap().is_defusing);
    });

    // CT presses +USE -> starts 10s defusal.
    b.probe.clear_messages();
    b.run(|game, eng| {
        let ct = eng.client_edict(1);
        eng.set_origin(ct, ZONE_AT + Vec3::new(-32.0, 0.0, 0.0));
        eng.ent_mut(ct).unwrap().v_angle = Vec3::ZERO;
        game.set_next_attack(1, 0.0);
        cmd_slot(game, eng, 1, IN_USE, true);
        assert!(game.state().players[1].as_ref().unwrap().is_defusing);
        assert_eq!(eng.ent(ct).unwrap().max_speed, 1.0);
    });
    b.step(10);
    let disarm = sound_id(&mut b, "weapons/c4_disarm.wav");
    assert!(sound_indices(&b).contains(&(disarm, CHAN_ITEM)));

    // Release +USE and wait 0.65 s -> defusal aborts (`NEXT_DEFUSE_TIME = 0.5`).
    step_cmd(&mut b, 1, 0, true, 65);
    b.run(|game, eng| {
        let ct = eng.client_edict(1);
        assert!(!game.state().players[1].as_ref().unwrap().is_defusing);
        assert!(eng.ent(ct).unwrap().max_speed >= 240.0);
    });

    // Hold +USE continuously for ~10 s -> defused!
    for _ in 0..1030 {
        b.run(|game, eng| cmd_slot(game, eng, 1, IN_USE, true));
        if b.run(|game, _| game.state().rules.bomb_defused) {
            break;
        }
    }
    b.step(10);
    let disarmed = sound_id(&mut b, "weapons/c4_disarmed.wav");
    assert!(sound_indices(&b).contains(&(disarmed, CHAN_WEAPON)));
    assert!(b.messages().contains(&CsUserMsg::SendAudio {
        sender_id: 0,
        audio_code: "%!MRAD_BOMBDEF".to_string(),
        pitch: 100,
    }));
    b.run(move |game, eng| {
        assert!(game.state().rules.bomb_defused);
        assert_eq!(game.state().players[1].as_ref().unwrap().frags, 3);
        assert!(!game.state().players[1].as_ref().unwrap().is_defusing);
        assert!(eng.ent(bomb).is_none());
    });

    // Subcase B: CT with defuse kit (`has_defuser = true`) defuses in ~5 s.
    b.run(|game, eng| {
        eng.set_cvar("mp_c4timer", "45");
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        eng.set_origin(eng.client_edict(0), ZONE_AT);
        eng.set_origin(eng.client_edict(1), Vec3::new(3000.0, 0.0, 0.0));
        ensure_c4(game, eng, 0);
        game.test_set_defuser(1, true);
    });
    let _bomb2 = plant_bomb_at_zone(&mut b, 0);
    b.run(|game, eng| {
        let ct = eng.client_edict(1);
        eng.set_origin(ct, ZONE_AT + Vec3::new(-32.0, 0.0, 0.0));
        eng.ent_mut(ct).unwrap().v_angle = Vec3::ZERO;
        game.set_next_attack(1, 0.0);
    });
    let mut defused_frames = 0;
    for i in 1..=600 {
        if b.run(|game, eng| {
            cmd_slot(game, eng, 1, IN_USE, true);
            game.state().rules.bomb_defused
        }) {
            defused_frames = i;
            break;
        }
    }
    assert!(
        (490..=550).contains(&defused_frames),
        "defuser kit should finish at ~5 s (500 frames), took {defused_frames}"
    );
}

#[test]
fn bomb_in_water_vanishes_without_exploding() {
    let mut b = bomb_arena(vec![]);
    b.run(|game, eng| {
        eng.set_cvar("mp_c4timer", "10");
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        eng.set_origin(eng.client_edict(0), ZONE_AT);
        eng.set_origin(eng.client_edict(1), Vec3::new(3000.0, 0.0, 0.0));
        ensure_c4(game, eng, 0);
    });
    let bomb = plant_bomb_at_zone(&mut b, 0);
    b.run(move |_, eng| {
        let v = eng.ent_mut(bomb).unwrap();
        v.movetype = cs_engine::consts::MOVETYPE_NONE;
        v.waterlevel = 1;
    });
    b.probe.clear_messages();
    b.step(1050);

    let explode1 = sound_id(&mut b, "weapons/c4_explode1.wav");
    assert!(!sound_indices(&b).contains(&(explode1, CHAN_WEAPON)));
    assert!(!temp_entities(&b).contains(&te::SPRITE));
    b.run(move |game, eng| {
        assert!(!game.state().rules.target_bombed, "vanilla water bomb does not explode");
        assert!(eng.ent(bomb).is_none(), "vanilla water bomb is deleted");
    });
}
