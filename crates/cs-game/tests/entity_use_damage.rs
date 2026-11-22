//! Entity use and damage (Task 1 of phase 2E): the player's use key (`CBasePlayer::PlayerUse`),
//! a bullet and a blast that hurt entities that are not players (`CBaseEntity::TraceAttack`,
//! `TakeDamage`, `Killed`, `RadiusDamage`, `RadiusDamage2`), and `SUB_UseTargets` /
//! `FireTargets` with the `killtarget` and the `delay` of `CBaseDelay`. Against ReGameDLL_CS
//! `player.cpp`, `cbase.cpp`, `combat.cpp` and `subs.cpp` (read 2026-11-23).
//!
//! The entities are the `test_target` class of the `test-hooks` build: its use caps, health,
//! damageability and what it does on `Use` come from the map's keys, and every `Use` and death
//! goes to `GameState::dispatch_log`. The probe is slot 0 (a counter-terrorist at the origin
//! looking along +X, eyes 17 up), slot 1 a terrorist far away.

mod common;

use common::{Bench, entity, spawns};
use cs_assets::bsp::Entity;
use cs_engine::consts::FL_KILLME;
use cs_engine::{EdictId, EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::bullets::Bullet;
use cs_game::entity::{DispatchEvent, UseType};
use cs_game::player::{Team, dmg};
use cs_game::weapons::BulletType;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use glam::Vec3;

const IN_USE: u16 = 1 << 5;

const IMPULSE: &str = "8";
const CONTINUOUS: &str = "16";
const ONOFF: &str = "32";

/// A `test_target` at `origin` with the extra keys `keys`.
fn target(origin: &str, keys: &[(&str, &str)]) -> Entity {
    let mut pairs = vec![("classname", "test_target"), ("origin", origin)];
    pairs.extend_from_slice(keys);
    entity(&pairs)
}

/// The probe and a terrorist spawned on a stub map with `extra` entities, no gravity, the freeze
/// period over, the probe at the origin looking along +X, the terrorist 5000 units away.
fn arena(extra: Vec<Entity>) -> Bench {
    let mut entities = spawns(4, 4);
    entities.extend(extra);
    let mut b = Bench::stub_without_moves(entities);
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        game.end_freeze_period(eng);
        let (me, other) = (eng.client_edict(0), eng.client_edict(1));
        eng.set_origin(me, Vec3::ZERO);
        eng.set_origin(other, Vec3::new(5000.0, 0.0, 0.0));
        eng.ent_mut(me).unwrap().v_angle = Vec3::ZERO;
        game.set_next_attack(0, 0.0);
    });
    b
}

/// One command of the probe with `buttons`, as the engine runs it.
fn press(game: &mut CsGame, eng: &mut EngineApi<'_>, buttons: u16) {
    let id = eng.client_edict(0);
    let viewangles = eng.ent(id).unwrap().v_angle;
    eng.ent_mut(id).unwrap().button = i32::from(buttons);
    game.set_next_attack(0, 0.0);
    common::run_cmd(
        game,
        eng,
        0,
        &GoldSrcUserCmd { msec: 10, buttons, viewangles, ..Default::default() },
        0,
    );
}

/// The `test_target`s in index order.
fn targets(eng: &EngineApi<'_>) -> Vec<EdictId> {
    let mut found = Vec::new();
    let mut after = None;
    while let Some(id) = eng.find_by_classname(after, "test_target") {
        found.push(id);
        after = Some(id);
    }
    found
}

/// The entity of `class` that is the `n`th `test_target` of the map.
fn nth(b: &mut Bench, n: usize) -> EdictId {
    b.run(move |_, eng| targets(eng)[n])
}

/// One `Use` of a `test_target`: (target, activator, caller, type, value).
type UseCall = (EdictId, Option<EdictId>, Option<EdictId>, UseType, f32);

/// The `Use` calls of `test_target`s the game has seen, in order.
fn uses(b: &Bench) -> Vec<UseCall> {
    b.server
        .game()
        .game
        .state()
        .dispatch_log
        .iter()
        .filter_map(|e| match *e {
            DispatchEvent::Use { id, activator, caller, use_type, value_bits } => {
                Some((id, activator, caller, use_type, f32::from_bits(value_bits)))
            }
            _ => None,
        })
        .collect()
}

fn kills(b: &Bench) -> Vec<EdictId> {
    b.server
        .game()
        .game
        .state()
        .dispatch_log
        .iter()
        .filter_map(|e| match *e {
            DispatchEvent::Killed { id } => Some(id),
            _ => None,
        })
        .collect()
}

fn health_of(b: &mut Bench, id: EdictId) -> f32 {
    b.run(move |_, eng| eng.ent(id).unwrap().health)
}

/// The sounds the probe heard, as (sound index, channel).
fn sounds(b: &Bench) -> Vec<(u16, u8)> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Sound(s) => Some((s.sound_index, s.channel)),
            _ => None,
        })
        .collect()
}

/// `RadiusDamage` and the bullet's damage as a player takes them: `damage - distance * falloff`
/// with the `f32` operations of the source.
fn blast(damage: f32, radius: f32, src: Vec3, victim: Vec3) -> f32 {
    let falloff = damage / radius;
    let src = Vec3::new(src.x, src.y, src.z + 1.0);
    let adjusted = damage - (src - victim).length() * falloff;
    adjusted.max(0.0)
}

#[test]
fn use_key_uses_the_entity_in_front() {
    // Targets: [0] in front, [1] nearer but with no use caps, [2] behind, [3] to the side (90
    // degrees: the cosine 0 does not beat 0.7).
    let mut b = arena(vec![
        target("40 0 17", &[("caps", IMPULSE)]),
        target("30 0 17", &[("caps", "0")]),
        target("-40 0 17", &[("caps", IMPULSE)]),
        target("0 40 17", &[("caps", IMPULSE)]),
    ]);
    let (front, behind, side) = (nth(&mut b, 0), nth(&mut b, 2), nth(&mut b, 3));
    let me = b.run(|_, eng| eng.client_edict(0));
    let select = b.run(|_, eng| eng.sound_index("common/wpn_select.wav")).unwrap();
    let deny = b.run(|_, eng| eng.sound_index("common/wpn_denyselect.wav")).unwrap();

    // The key goes down: the entity in front is used once, as `Use(this, this, USE_SET, 1)`, and
    // the select sound plays on `CHAN_ITEM`.
    b.run(|game, eng| press(game, eng, IN_USE));
    b.step(2);
    assert_eq!(uses(&b), vec![(front, Some(me), Some(me), UseType::Set, 1.0)]);
    assert_eq!(sounds(&b), vec![(select, 3)]);

    // An impulse entity is not used again while the key is held, nor when it is released.
    b.run(|game, eng| press(game, eng, IN_USE));
    b.run(|game, eng| press(game, eng, 0));
    assert_eq!(uses(&b).len(), 1);

    // A new press uses it again.
    b.run(|game, eng| press(game, eng, IN_USE));
    assert_eq!(uses(&b).len(), 2);
    b.run(|game, eng| press(game, eng, 0));

    // With nothing in front within 64 units (the front entity moved away, the one behind and the
    // one to the side stay) nothing is used and the deny sound plays.
    b.run(move |_, eng| eng.set_origin(front, Vec3::new(200.0, 0.0, 17.0)));
    b.probe.clear_messages();
    b.run(|game, eng| press(game, eng, IN_USE));
    b.step(2);
    assert_eq!(uses(&b).len(), 2);
    assert_eq!(sounds(&b), vec![(deny, 3)]);
    let _ = (behind, side);
}

#[test]
fn use_key_picks_the_entity_closest_to_the_view() {
    // Two usable entities in front, one a little off the line of sight: the one the view points
    // at wins, whichever is nearer.
    let mut b = arena(vec![
        target("20 20 17", &[("caps", IMPULSE)]),
        target("50 0 17", &[("caps", IMPULSE)]),
    ]);
    let (off, on) = (nth(&mut b, 0), nth(&mut b, 1));
    b.run(|game, eng| press(game, eng, IN_USE));
    let used: Vec<EdictId> = uses(&b).iter().map(|u| u.0).collect();
    assert_eq!(used, vec![on]);
    let _ = off;
}

#[test]
fn continuous_use_repeats_while_held() {
    let mut b = arena(vec![target("40 0 17", &[("caps", CONTINUOUS)])]);
    let t = nth(&mut b, 0);
    let me = b.run(|_, eng| eng.client_edict(0));
    // Every command with the key down uses it again (the first press and the held ones alike).
    for n in 1..=4 {
        b.run(|game, eng| press(game, eng, IN_USE));
        assert_eq!(uses(&b).len(), n);
    }
    assert!(uses(&b).iter().all(|u| *u == (t, Some(me), Some(me), UseType::Set, 1.0)));
    // Released, it stops.
    b.run(|game, eng| press(game, eng, 0));
    b.run(|game, eng| press(game, eng, 0));
    assert_eq!(uses(&b).len(), 4);
}

#[test]
fn on_off_use_fires_on_the_press_and_on_the_release() {
    let mut b = arena(vec![target("40 0 17", &[("caps", ONOFF)])]);
    let t = nth(&mut b, 0);
    let me = b.run(|_, eng| eng.client_edict(0));
    b.run(|game, eng| press(game, eng, IN_USE));
    b.run(|game, eng| press(game, eng, IN_USE));
    assert_eq!(uses(&b), vec![(t, Some(me), Some(me), UseType::Set, 1.0)]);
    // The "off" use: value 0.
    b.run(|game, eng| press(game, eng, 0));
    assert_eq!(
        uses(&b),
        vec![(t, Some(me), Some(me), UseType::Set, 1.0), (t, Some(me), Some(me), UseType::Set, 0.0)]
    );
    // Nothing more once it is up.
    b.run(|game, eng| press(game, eng, 0));
    assert_eq!(uses(&b).len(), 2);
}

#[test]
fn a_bullet_damages_a_damageable_entity() {
    // [0] takes damage, [1] (beyond it, in line) does not. The near face of [0] is 200 units
    // from the muzzle.
    let mut b = arena(vec![
        target("216 0 17", &[("damageable", "1"), ("health", "100")]),
        target("400 0 17", &[("damageable", "0"), ("health", "100")]),
    ]);
    let (hurt, deaf) = (nth(&mut b, 0), nth(&mut b, 1));
    let bullet = Bullet {
        src: Vec3::new(0.0, 0.0, 17.0),
        dir: Vec3::X,
        spread: 0.0,
        distance: 8192.0,
        penetration: 1,
        kind: BulletType::Player762mm,
        damage: 36,
        range_modifier: 0.98,
        pistol: false,
        shared_rand: 0,
    };
    b.run(move |game, eng| {
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
    });
    // `int(36 * 0.98^(200 / 500))` of damage, taken as a float health.
    let expected = (36.0 * 0.98_f64.powf(200.0 / 500.0)).trunc() as f32;
    assert_eq!(health_of(&mut b, hurt), 100.0 - expected);
    assert_eq!(health_of(&mut b, deaf), 100.0);

    // An entity that does not take damage lets the bullet stop on it, hurt nothing: aim at it
    // alone (the damageable one is out of the way).
    b.run(move |_, eng| eng.set_origin(hurt, Vec3::new(216.0, 500.0, 17.0)));
    b.run(move |game, eng| {
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
    });
    assert_eq!(health_of(&mut b, deaf), 100.0);
}

#[test]
fn an_entity_shot_to_death_is_killed_and_removed() {
    let mut b = arena(vec![target("216 0 17", &[("damageable", "1"), ("health", "10")])]);
    let t = nth(&mut b, 0);
    let bullet = Bullet {
        src: Vec3::new(0.0, 0.0, 17.0),
        dir: Vec3::X,
        spread: 0.0,
        distance: 8192.0,
        penetration: 1,
        kind: BulletType::Player762mm,
        damage: 36,
        range_modifier: 0.98,
        pistol: false,
        shared_rand: 0,
    };
    // `Killed`: `DEAD_DEAD`, and `UTIL_Remove` (not solid, `FL_KILLME`, no `targetname`); the
    // engine frees the entity after the frame, so it is looked at within the call.
    let v = b.run(move |game, eng| {
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
        eng.ent(t).cloned().expect("not freed within the call")
    });
    assert_eq!(kills(&b), vec![t]);
    assert_eq!(v.deadflag, 2);
    assert_ne!(v.flags & FL_KILLME, 0);
    assert_eq!(v.solid, cs_engine::consts::SOLID_NOT);
    // Dead, it takes no more damage (a second blast finds nothing to hurt).
    let before = b.server.game().game.state().dispatch_log.len();
    b.run(|game, eng| {
        game.test_radius_damage(eng, Vec3::new(216.0, 0.0, 17.0), None, None, None, 500.0, 350.0, dmg::EXPLOSION);
    });
    assert_eq!(b.server.game().game.state().dispatch_log.len(), before);
}

#[test]
fn radius_damage_reaches_entities() {
    // At 0, 100 and 300 units from the blast (a 100-damage, 350-radius one 100 up), a damageable
    // entity loses `damage - distance * falloff` as a float; one that takes no damage and one
    // beyond the radius lose nothing.
    let src = Vec3::new(0.0, 0.0, 100.0);
    let spots = [(0.0_f32, "0 0 100"), (100.0, "100 0 100"), (300.0, "300 0 100")];
    let mut extra: Vec<Entity> = spots
        .iter()
        .map(|(_, o)| target(o, &[("damageable", "1"), ("health", "1000")]))
        .collect();
    extra.push(target("100 100 100", &[("damageable", "0"), ("health", "1000")]));
    extra.push(target("360 0 100", &[("damageable", "1"), ("health", "1000")]));
    let mut b = arena(extra);
    let ids: [EdictId; 5] = std::array::from_fn(|n| nth(&mut b, n));
    b.run(move |game, eng| {
        let me = eng.client_edict(0);
        game.test_radius_damage(eng, src, None, None, Some(me), 100.0, 350.0, dmg::EXPLOSION);
    });
    for (i, (_, _)) in spots.iter().enumerate() {
        let origin = b.run(move |_, eng| eng.ent(ids[i]).unwrap().origin);
        let expected = 1000.0 - blast(100.0, 350.0, src, origin);
        assert_eq!(health_of(&mut b, ids[i]), expected, "target {i}");
    }
    assert_eq!(health_of(&mut b, ids[3]), 1000.0);
    assert_eq!(health_of(&mut b, ids[4]), 1000.0);
}

#[test]
fn radius_damage2_caps_the_damage() {
    // The blast of the planted C4: 75 at most, here through `TraceAttack`/`ApplyMultiDamage`
    // (the line from the blast ends on the near face of each entity).
    let mut b = arena(vec![
        target("100 0 100", &[("damageable", "1"), ("health", "1000")]),
        target("-100 0 100", &[("damageable", "1"), ("health", "1000")]),
    ]);
    let (a, c) = (nth(&mut b, 0), nth(&mut b, 1));
    b.run(move |game, eng| {
        let me = eng.client_edict(0);
        cs_game::grenade::radius_damage2(
            game.state_mut(),
            eng,
            Vec3::new(0.0, 0.0, 100.0),
            None,
            Some(me),
            200.0,
            350.0,
            dmg::BLAST,
        );
    });
    // Both hurt, the damage capped at 75 (the uncapped values are 200 - 100 * 200 / 350 = 142.8).
    assert_eq!(health_of(&mut b, a), 925.0);
    assert_eq!(health_of(&mut b, c), 925.0);
}

#[test]
fn use_targets_fires_by_targetname() {
    // [0] uses its own targets ("door"); [1] and [2] are "door"; [3] is "other"; an `info_target`
    // that is also "door" gets the use and does nothing.
    let mut b = arena(vec![
        target("1000 0 0", &[("fires", "1"), ("target", "door")]),
        target("1100 0 0", &[("targetname", "door")]),
        target("1200 0 0", &[("targetname", "door")]),
        target("1300 0 0", &[("targetname", "other")]),
        entity(&[("classname", "info_target"), ("origin", "1400 0 0"), ("targetname", "door")]),
    ]);
    let ids: [EdictId; 4] = std::array::from_fn(|n| nth(&mut b, n));
    let me = b.run(|_, eng| eng.client_edict(0));
    b.run(move |game, eng| {
        cs_game::targets::use_targets(game.state_mut(), eng, ids[0], Some(me), UseType::Toggle, 0.5);
    });
    // In index order, with the activator and the entity that fired as the caller.
    assert_eq!(
        uses(&b),
        vec![
            (ids[1], Some(me), Some(ids[0]), UseType::Toggle, 0.5),
            (ids[2], Some(me), Some(ids[0]), UseType::Toggle, 0.5),
        ]
    );

    // An entity on its way out (`FL_KILLME`) is not used.
    b.run(move |_, eng| eng.ent_mut(ids[1]).unwrap().flags |= FL_KILLME);
    b.run(move |game, eng| {
        cs_game::targets::use_targets(game.state_mut(), eng, ids[0], None, UseType::On, 1.0);
    });
    assert_eq!(uses(&b).len(), 3);
    assert_eq!(uses(&b)[2], (ids[2], None, Some(ids[0]), UseType::On, 1.0));

    // Nothing named: nothing is used, whatever the name.
    b.run(move |game, eng| {
        eng.ent_mut(ids[0]).unwrap().target = "nobody".to_string();
        cs_game::targets::use_targets(game.state_mut(), eng, ids[0], None, UseType::On, 1.0);
        eng.ent_mut(ids[0]).unwrap().target = String::new();
        cs_game::targets::use_targets(game.state_mut(), eng, ids[0], None, UseType::On, 1.0);
        cs_game::targets::fire_targets(game.state_mut(), eng, "", None, None, UseType::On, 1.0);
    });
    assert_eq!(uses(&b).len(), 3);
}

#[test]
fn an_entity_with_no_targetname_is_never_found() {
    let mut b = arena(vec![target("1000 0 0", &[]), target("1100 0 0", &[("targetname", "x")])]);
    let (plain, named) = (nth(&mut b, 0), nth(&mut b, 1));
    b.run(move |_, eng| {
        assert_eq!(cs_game::targets::find_by_targetname(eng, None, "x"), Some(named));
        assert_eq!(cs_game::targets::find_by_targetname(eng, Some(named), "x"), None);
        assert_eq!(cs_game::targets::find_by_targetname(eng, None, ""), None);
        assert_ne!(cs_game::targets::find_by_targetname(eng, None, "x"), Some(plain));
    });
}

#[test]
fn killtarget_removes_the_entities_it_names_before_the_targets_are_used() {
    let mut b = arena(vec![
        target("1000 0 0", &[("fires", "1"), ("target", "door"), ("killtarget", "victim")]),
        target("1100 0 0", &[("targetname", "door")]),
        target("1200 0 0", &[("targetname", "victim")]),
    ]);
    let (source, door, victim) = (nth(&mut b, 0), nth(&mut b, 1), nth(&mut b, 2));
    let v = b.run(move |game, eng| {
        cs_game::targets::use_targets(game.state_mut(), eng, source, None, UseType::Off, 0.0);
        eng.ent(victim).cloned().expect("not freed within the call")
    });
    // The victim is removed (`UTIL_Remove`: `FL_KILLME`, no name) and not used; the door is used.
    assert_eq!(uses(&b), vec![(door, None, Some(source), UseType::Off, 0.0)]);
    assert_ne!(v.flags & FL_KILLME, 0);
    assert!(v.targetname.is_empty());
}

#[test]
fn delay_uses_the_targets_later_through_a_delayed_use_entity() {
    let mut b = arena(vec![
        target("1000 0 0", &[("fires", "1"), ("target", "door"), ("delay", "1.5")]),
        target("1100 0 0", &[("targetname", "door")]),
    ]);
    let (source, door) = (nth(&mut b, 0), nth(&mut b, 1));
    let me = b.run(|_, eng| eng.client_edict(0));
    let start = b.run(move |game, eng| {
        cs_game::targets::use_targets(game.state_mut(), eng, source, Some(me), UseType::Toggle, 0.0);
        eng.time()
    });
    // Nothing yet; a `DelayedUse` entity waits with the use type in `button`, the target, the
    // player activator as its owner, and its think due `delay` seconds from now.
    assert!(uses(&b).is_empty());
    let (temp, v) = b.run(|_, eng| {
        let t = eng.find_by_classname(None, "DelayedUse").expect("a DelayedUse entity");
        (t, eng.ent(t).unwrap().clone())
    });
    assert_eq!(v.target, "door");
    assert_eq!(v.button, UseType::Toggle as i32);
    assert_eq!(v.owner, Some(me));
    assert!((v.nextthink - (start + 1.5)).abs() < 0.01, "{} vs {}", v.nextthink, start);

    // Not before the time...
    b.step(100);
    assert!(uses(&b).is_empty());
    // ...and then once, with the DelayedUse entity as the caller and a value of 0, after which
    // the entity is gone.
    b.step(70);
    assert_eq!(uses(&b), vec![(door, Some(me), Some(temp), UseType::Toggle, 0.0)]);
    b.step(5);
    assert!(b.run(|_, eng| eng.find_by_classname(None, "DelayedUse")).is_none());
    // The delay does not repeat: the temp's own delay is 0.
    b.step(200);
    assert_eq!(uses(&b).len(), 1);
}

#[test]
fn a_delayed_use_does_not_keep_a_non_player_activator() {
    let mut b = arena(vec![
        target("1000 0 0", &[("fires", "1"), ("target", "door"), ("delay", "0.5")]),
        target("1100 0 0", &[("targetname", "door")]),
    ]);
    let (source, door) = (nth(&mut b, 0), nth(&mut b, 1));
    b.run(move |game, eng| {
        // The activator is the door itself: not a player, so the delayed use has none.
        cs_game::targets::use_targets(game.state_mut(), eng, source, Some(door), UseType::On, 0.0);
    });
    b.step(60);
    let u = uses(&b);
    assert_eq!(u.len(), 1);
    assert_eq!((u[0].0, u[0].1, u[0].3), (door, None, UseType::On));
}

#[test]
fn players_are_unchanged() {
    // A bullet and a blast still hurt a player exactly as before: through `TakeDamage` of the
    // player, with the damage the range and the falloff give.
    let mut b = arena(vec![]);
    let bullet = Bullet {
        src: Vec3::new(0.0, 0.0, 17.0),
        dir: Vec3::X,
        spread: 0.0,
        distance: 8192.0,
        penetration: 1,
        kind: BulletType::Player762mm,
        damage: 36,
        range_modifier: 0.98,
        pistol: false,
        shared_rand: 0,
    };
    let (shot, blasted) = b.run(move |game, eng| {
        game.place_player(eng, 1, Vec3::new(216.0, 0.0, 0.0));
        let before = game.state().players[1].as_ref().unwrap().health;
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
        let shot = before - game.state().players[1].as_ref().unwrap().health;

        game.place_player(eng, 1, Vec3::new(100.0, 0.0, 101.0));
        game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
        let me = eng.client_edict(0);
        game.test_radius_damage(eng, Vec3::new(0.0, 0.0, 100.0), None, None, Some(me), 100.0, 350.0, dmg::EXPLOSION);
        let blasted = 1000.0 - game.state().players[1].as_ref().unwrap().health;
        (shot, blasted)
    });
    assert_eq!(shot, (36.0 * 0.98_f64.powf(200.0 / 500.0)).trunc() as f32);
    assert_eq!(blasted, (100.0_f32 - 100.0 * (100.0_f32 / 350.0)).trunc());
}

#[test]
fn a_player_is_not_usable() {
    // The other player's origin 30 units in front of the probe: not a use target (a player's
    // `ObjectCaps` have no use bits), so the use key is "empty".
    let mut b = arena(vec![]);
    let deny = b.run(|_, eng| eng.sound_index("common/wpn_denyselect.wav")).unwrap();
    b.run(|game, eng| {
        game.place_player(eng, 1, Vec3::new(30.0, 0.0, 0.0));
        press(game, eng, IN_USE);
    });
    b.step(2);
    assert_eq!(sounds(&b), vec![(deny, 3)]);
    assert!(uses(&b).is_empty());
}

/// A map for the push tests: the victim `[0]` at 200 units on X (a 32-unit box, health 1000,
/// damageable) and a non-damageable `[1]` 100 units behind it, the inflictor and attacker.
fn push_arena() -> (Bench, EdictId, EdictId) {
    let mut b = arena(vec![
        target("200 0 17", &[("damageable", "1"), ("health", "1000")]),
        target("100 0 17", &[("damageable", "0")]),
    ]);
    let (victim, source) = (nth(&mut b, 0), nth(&mut b, 1));
    (b, victim, source)
}

fn velocity_of(b: &mut Bench, id: EdictId) -> Vec3 {
    b.run(move |_, eng| eng.ent(id).unwrap().velocity)
}

fn assert_near(a: Vec3, b: Vec3) {
    assert!((a - b).length() < 0.01, "{a:?} is not {b:?}");
}

#[test]
fn take_damage_pushes_walking_and_stepping_entities_away_from_the_inflictor() {
    use cs_engine::consts::{MOVETYPE_STEP, MOVETYPE_WALK};
    let (mut b, victim, source) = push_arena();
    // `flDamage * ((32 * 32 * 72) / (32 * 32 * 32)) * 5` = 11.25 per point, along the direction
    // from the inflictor's centre to the victim's origin (+X).
    for (movetype, pushed) in [(MOVETYPE_WALK, true), (MOVETYPE_STEP, true), (0, false)] {
        b.run(move |_, eng| {
            let v = eng.ent_mut(victim).unwrap();
            v.movetype = movetype;
            v.velocity = Vec3::ZERO;
        });
        let hurt = b.run(move |game, eng| {
            game.test_take_damage(eng, victim, Some(source), Some(source), 10.0, dmg::BULLET)
        });
        assert!(hurt);
        let expected = if pushed { Vec3::new(112.5, 0.0, 0.0) } else { Vec3::ZERO };
        assert_near(velocity_of(&mut b, victim), expected);
    }
    // 110 points of damage and more are 1237.5 and are cut at 1000.
    b.run(move |game, eng| {
        let v = eng.ent_mut(victim).unwrap();
        v.movetype = MOVETYPE_WALK;
        v.velocity = Vec3::new(0.0, 5.0, 0.0);
        game.test_take_damage(eng, victim, Some(source), Some(source), 110.0, dmg::BULLET);
    });
    assert_near(velocity_of(&mut b, victim), Vec3::new(1000.0, 5.0, 0.0));
}

#[test]
fn a_trigger_attacker_does_not_push() {
    use cs_engine::consts::{MOVETYPE_WALK, SOLID_TRIGGER};
    let (mut b, victim, source) = push_arena();
    b.run(move |game, eng| {
        eng.ent_mut(victim).unwrap().movetype = MOVETYPE_WALK;
        eng.ent_mut(source).unwrap().solid = SOLID_TRIGGER;
        // The damage is done, the push is not ("don't let hurt brushes or other triggers move
        // player").
        assert!(game.test_take_damage(eng, victim, Some(source), Some(source), 10.0, dmg::BULLET));
    });
    assert_eq!(velocity_of(&mut b, victim), Vec3::ZERO);
    assert_eq!(health_of(&mut b, victim), 990.0);
    // No attacker, no push either (the source would read a null pointer).
    b.run(move |game, eng| {
        assert!(game.test_take_damage(eng, victim, Some(source), None, 10.0, dmg::BULLET));
    });
    assert_eq!(velocity_of(&mut b, victim), Vec3::ZERO);
}

#[test]
fn radius_damage2_with_an_inflictor_pushes_through_the_trace() {
    // The blast's inflictor stands at the blast; the line ends on the victim's near face, so the
    // damage goes through `TraceAttack`/`ApplyMultiDamage` and its `TakeDamage` knows the
    // inflictor: 75 points, pushed 843.75 away from it.
    use cs_engine::consts::MOVETYPE_WALK;
    let mut b = arena(vec![
        target("100 0 100", &[("damageable", "1"), ("health", "1000")]),
        target("0 0 100", &[("damageable", "0")]),
    ]);
    let (victim, source) = (nth(&mut b, 0), nth(&mut b, 1));
    b.run(move |game, eng| {
        eng.ent_mut(victim).unwrap().movetype = MOVETYPE_WALK;
        let me = eng.client_edict(0);
        cs_game::grenade::radius_damage2(
            game.state_mut(),
            eng,
            Vec3::new(0.0, 0.0, 100.0),
            Some(source),
            Some(me),
            200.0,
            350.0,
            dmg::BLAST,
        );
    });
    assert_eq!(health_of(&mut b, victim), 925.0);
    assert_near(velocity_of(&mut b, victim), Vec3::new(843.75, 0.0, 0.0));
}

#[test]
fn radius_damage2_calls_take_damage_directly_when_nothing_is_in_the_way() {
    // A non-solid entity does not stop the line (fraction 1): `TakeDamage` is called at once,
    // with the damage the distance gives (here under the cap of 75), and the push follows.
    use cs_engine::consts::{MOVETYPE_WALK, SOLID_NOT};
    let mut b = arena(vec![
        target("100 0 100", &[("damageable", "1"), ("health", "1000")]),
        target("0 0 100", &[("damageable", "0")]),
    ]);
    let (victim, source) = (nth(&mut b, 0), nth(&mut b, 1));
    b.run(move |game, eng| {
        let v = eng.ent_mut(victim).unwrap();
        v.movetype = MOVETYPE_WALK;
        v.solid = SOLID_NOT;
        let me = eng.client_edict(0);
        cs_game::grenade::radius_damage2(
            game.state_mut(),
            eng,
            Vec3::new(0.0, 0.0, 100.0),
            Some(source),
            Some(me),
            100.0,
            350.0,
            dmg::BLAST,
        );
    });
    let origin = Vec3::new(100.0, 0.0, 100.0);
    let expected = blast(100.0, 350.0, Vec3::new(0.0, 0.0, 100.0), origin).min(75.0);
    assert_eq!(health_of(&mut b, victim), 1000.0 - expected);
    assert!(velocity_of(&mut b, victim).x > 0.0);
}

#[test]
fn a_removed_entity_still_takes_damage_until_it_is_freed() {
    // `UTIL_Remove` marks `FL_KILLME` and clears the name but keeps `takedamage` (only `Killed`
    // sets `DAMAGE_NO`): a blast in the same frame still hurts it.
    let mut b = arena(vec![target("100 0 100", &[("damageable", "1"), ("health", "1000")])]);
    let t = nth(&mut b, 0);
    let health = b.run(move |game, eng| {
        let v = eng.ent_mut(t).unwrap();
        v.flags |= FL_KILLME;
        v.targetname.clear();
        game.test_radius_damage(eng, Vec3::new(0.0, 0.0, 100.0), None, None, None, 100.0, 350.0, dmg::EXPLOSION);
        eng.ent(t).unwrap().health
    });
    assert!(health < 1000.0);
}
