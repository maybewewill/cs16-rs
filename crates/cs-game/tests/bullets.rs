//! Bullets (Task 7 of phase 2D): damage by range and penetration, the hit groups, armor, the
//! speed modifier after a hit, and the weapon in the death notice. Shots are fired by hand
//! through the `test-hooks` entry points, on a stub map where the players are the only things
//! in the way. The probe is slot 0 (edict 1); slots 1 and 2 are made players by the game.

mod common;

use common::{Bench, stub_spawns};
use cs_engine::{EngineApi, GameDll};
use cs_game::bullets::{
    Bullet, CHAR_TEX_COMPUTER, CHAR_TEX_CONCRETE, CHAR_TEX_GRATE, CHAR_TEX_METAL, CHAR_TEX_TILE,
    CHAR_TEX_VENT, CHAR_TEX_WOOD, PenetrationMaterial, bullet_penetration, penetration_material,
};
use cs_game::player::{ArmorType, Team, dmg, weapon_armor_ratio};
use cs_game::weapons::{BulletType, WeaponId, shared_random_float};
use cs_proto48::usermsg::{CsUserMsg, encode_cs_usermsg};
use glam::Vec3;

const HEAD: i32 = 1;
const CHEST: i32 = 2;
const STOMACH: i32 = 3;
const LEFT_ARM: i32 = 4;
const LEFT_LEG: i32 = 6;

/// Three players on a stub map: the probe (slot 0, counter-terrorist), and slots 1 and 2
/// (terrorists), spawned, without gravity.
fn arena() -> Bench {
    let mut b = Bench::stub_slots(stub_spawns(), 3);
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        game.client_put_in_server(eng, 2);
        eng.test_set_client_active(2, true);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        assert!(game.spawn_player(eng, 2, Team::Terrorist, 0));
    });
    b
}

/// Stands the player of `slot` at `origin`.
fn put(eng: &mut EngineApi<'_>, slot: u8, origin: Vec3) {
    let id = eng.client_edict(slot);
    eng.set_origin(id, origin);
}

/// The gun position of a player standing at `origin` (`GetGunPosition`: origin plus `view_ofs`,
/// 17 up for a standing player in this port).
fn eye(origin: Vec3) -> Vec3 {
    origin + Vec3::new(0.0, 0.0, 17.0)
}

fn health(b: &mut Bench, slot: usize) -> f32 {
    b.run(move |game, _| game.state().players[slot].as_ref().unwrap().health)
}

/// An AK-47-like bullet along +X from `src`.
fn rifle_bullet(src: Vec3, penetration: i32, seed: u32) -> Bullet {
    Bullet {
        src,
        dir: Vec3::X,
        spread: 0.0,
        distance: 8192.0,
        penetration,
        kind: BulletType::Player762mm,
        damage: 36,
        range_modifier: 0.98,
        pistol: false,
        shared_rand: seed,
    }
}

#[test]
fn bullet_damage_falls_off_with_range() {
    // `iCurrentDamage = int(iCurrentDamage * pow(flRangeModifier, flCurrentDistance / 500))`, the
    // distance being the one to the hit (the near face of the 32-wide box).
    let mut lost = Vec::new();
    for gap in [200.0_f32, 1000.0, 3000.0] {
        let mut b = arena();
        let before = health(&mut b, 1);
        b.run(move |game, eng| {
            put(eng, 0, Vec3::ZERO);
            put(eng, 1, Vec3::new(gap + 16.0, 0.0, 0.0));
            let bullet = rifle_bullet(eye(Vec3::ZERO), 1, 0);
            game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
        });
        let after = health(&mut b, 1);
        lost.push(before - after);
    }
    // 36 * 0.98^(d/500) with d the distance to the near face (the player's half width is 16).
    let expected: Vec<f32> = [200.0_f64, 1000.0, 3000.0]
        .iter()
        .map(|d| (36.0 * 0.98_f64.powf(d / 500.0)).trunc() as f32)
        .collect();
    assert_eq!(lost, expected);
    assert!(lost[0] > lost[1] && lost[1] > lost[2], "{lost:?}");
}

#[test]
fn penetration_by_material() {
    // The materials of `FireBullets3`: the power factor, the damage modifier, metal.
    let m = |c| penetration_material(c);
    let none = PenetrationMaterial { power_factor: None, damage_modifier: None, metal: false };
    assert_eq!(
        m(CHAR_TEX_METAL),
        PenetrationMaterial { power_factor: Some(0.15), damage_modifier: Some(0.2), metal: true }
    );
    assert_eq!(
        m(CHAR_TEX_CONCRETE),
        PenetrationMaterial { power_factor: Some(0.25), damage_modifier: None, metal: false }
    );
    assert_eq!(
        m(CHAR_TEX_GRATE),
        PenetrationMaterial { power_factor: Some(0.5), damage_modifier: Some(0.4), metal: true }
    );
    assert_eq!(
        m(CHAR_TEX_VENT),
        PenetrationMaterial { power_factor: Some(0.5), damage_modifier: Some(0.45), metal: true }
    );
    assert_eq!(
        m(CHAR_TEX_TILE),
        PenetrationMaterial { power_factor: Some(0.65), damage_modifier: Some(0.3), metal: false }
    );
    assert_eq!(
        m(CHAR_TEX_COMPUTER),
        PenetrationMaterial { power_factor: Some(0.4), damage_modifier: Some(0.45), metal: true }
    );
    assert_eq!(
        m(CHAR_TEX_WOOD),
        PenetrationMaterial { power_factor: None, damage_modifier: Some(0.6), metal: false }
    );
    // Dirt, glass, flesh, a texture the table does not know and none at all change nothing.
    for c in [b'D', b'Y', b'F', b'S', b'N', b'X', 0] {
        assert_eq!(m(c), none, "{}", c as char);
    }
    // The power of a bullet is an `int`: the 7.62 mm (39) through concrete is 9, through metal 5.
    let (power, distance) = bullet_penetration(BulletType::Player762mm);
    assert_eq!((power, distance), (39, 5000.0));
    assert_eq!((f64::from(power) * 0.25) as i32, 9);
    assert_eq!((f64::from(power) * 0.15) as i32, 5);
    assert_eq!(bullet_penetration(BulletType::Player9mm), (21, 800.0));
    assert_eq!(bullet_penetration(BulletType::Player338mag), (45, 8000.0));
    assert_eq!(bullet_penetration(BulletType::PlayerBuckshot), (0, 0.0));
}

#[test]
fn a_bullet_goes_through_a_player_and_hurts_the_next() {
    // A surface that is no brush model sets power 42 and the modifiers 0.75: the second player is
    // hit by `int(first * 0.75)` at the distance left, and each pass applies its own damage.
    let mut b = arena();
    b.run(|game, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(216.0, 0.0, 0.0));
        put(eng, 2, Vec3::new(416.0, 0.0, 0.0));
        let bullet = rifle_bullet(eye(Vec3::ZERO), 2, 0);
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
    });
    let first = (36.0 * 0.98_f64.powf(200.0 / 500.0)).trunc() as i32;
    assert_eq!(100.0 - health(&mut b, 1), first as f32);
    // The bullet leaves the first box at the hit plus 42 (x = 242) with (8192 - 200) * 0.75 left;
    // the second player's near face (x = 400) is 158 on. The damage is `int(first * 0.75)` going
    // in (a `float` product), then falls off over that distance.
    let carried = (first as f32 * 0.75) as i32;
    let second = (f64::from(carried) * 0.98_f64.powf(158.0 / 500.0)).trunc();
    assert_eq!(100.0 - health(&mut b, 2), second as f32);
}

#[test]
fn spread_uses_the_shared_seed() {
    // A player's spread is two `UTIL_SharedRandomFloat` draws per axis, seeds `seed`..`seed + 3`.
    let mut b = arena();
    for seed in [0_u32, 5, 77] {
        let got = b.run(move |game, eng| {
            put(eng, 0, Vec3::ZERO);
            let mut bullet = rifle_bullet(eye(Vec3::ZERO), 1, seed);
            bullet.spread = 0.04;
            game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet)
        });
        let x = shared_random_float(seed, -0.5, 0.5) + shared_random_float(seed + 1, -0.5, 0.5);
        let y = shared_random_float(seed + 2, -0.5, 0.5) + shared_random_float(seed + 3, -0.5, 0.5);
        assert_eq!(got, Vec3::new(x * 0.04, y * 0.04, 0.0), "seed {seed}");
    }
    // A different seed gives a different spread.
    let a = shared_random_float(0, -0.5, 0.5);
    let c = shared_random_float(1, -0.5, 0.5);
    assert_ne!(a, c);
}

#[test]
fn buckshot_pellets() {
    // The shotguns' `FireBullets` with no damage: each pellet does `int((1 - fraction) * 20)`
    // and all of them reach `TakeDamage` as one sum. A target 2000 units off takes 9 * 6.
    let mut b = arena();
    b.run(|game, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(2016.0, 0.0, 0.0));
        game.test_fire_buckshot(
            eng,
            0,
            Vec3::ZERO,
            9,
            eye(Vec3::ZERO),
            Vec3::ZERO,
            3000.0,
        );
    });
    let fraction = 2000.0_f32 / 3000.0;
    let per_pellet = ((1.0 - fraction) * 20.0).trunc();
    assert_eq!(per_pellet, 6.0);
    assert_eq!(health(&mut b, 1), 100.0 - 9.0 * per_pellet);
}

#[test]
fn headshot_multiplier_and_armor_ratio() {
    let mut b = arena();
    let hit = |b: &mut Bench, hitgroup: i32, weapon: Option<WeaponId>, damage: f32| {
        b.run(move |game, eng| game.test_trace_attack(eng, 1, 0, weapon, damage, hitgroup));
        let p = b.run(|game, _| game.state().players[1].clone().unwrap());
        b.run(|game, _| {
            let p = game.state_mut().players[1].as_mut().unwrap();
            p.health = 100.0;
            p.armor = 0.0;
            p.kevlar = ArmorType::None;
        });
        p
    };
    // The hit group multipliers, no armor.
    assert_eq!(hit(&mut b, HEAD, None, 20.0).health, 100.0 - 80.0);
    assert_eq!(hit(&mut b, CHEST, None, 20.0).health, 100.0 - 20.0);
    assert_eq!(hit(&mut b, STOMACH, None, 20.0).health, 100.0 - 25.0);
    assert_eq!(hit(&mut b, LEFT_ARM, None, 20.0).health, 100.0 - 20.0);
    assert_eq!(hit(&mut b, LEFT_LEG, None, 20.0).health, 100.0 - 15.0);
    assert_eq!(hit(&mut b, 0, None, 20.0).health, 100.0 - 20.0);

    // The armor ratio: 0.5, times the attacker's weapon.
    let ratio = |w| weapon_armor_ratio(w, 0.5);
    assert_eq!(ratio(WeaponId::Ak47), (0.5_f64 * 1.55) as f32);
    assert_eq!(ratio(WeaponId::Awp), (0.5_f64 * 1.95) as f32);
    assert_eq!(ratio(WeaponId::Mac10), (0.5_f64 * 0.95) as f32);
    assert_eq!(ratio(WeaponId::Knife), (0.5_f64 * 1.7) as f32);
    assert_eq!(ratio(WeaponId::M3), 0.5);
    assert_eq!(ratio(WeaponId::Usp), 0.5);
    for (w, f) in [
        (WeaponId::Aug, 1.4),
        (WeaponId::M4a1, 1.4),
        (WeaponId::Famas, 1.4),
        (WeaponId::Sg552, 1.4),
        (WeaponId::G3sg1, 1.65),
        (WeaponId::Sg550, 1.45),
        (WeaponId::M249, 1.5),
        (WeaponId::Elite, 1.05),
        (WeaponId::Glock18, 1.05),
        (WeaponId::Deagle, 1.5),
        (WeaponId::FiveSeven, 1.5),
        (WeaponId::P90, 1.5),
        (WeaponId::P228, 1.25),
        (WeaponId::Scout, 1.7),
        (WeaponId::Galil, 1.55),
    ] {
        assert_eq!(ratio(w), (0.5_f64 * f) as f32, "{w:?}");
    }

    // A helmet on a head hit: the AK-47's ratio 0.775 over 80 damage.
    b.run(|game, _| {
        let p = game.state_mut().players[1].as_mut().unwrap();
        p.armor = 100.0;
        p.kevlar = ArmorType::VestHelm;
    });
    b.run(|game, eng| game.test_trace_attack(eng, 1, 0, Some(WeaponId::Ak47), 20.0, HEAD));
    let p = b.run(|game, _| game.state().players[1].clone().unwrap());
    let r = (0.5_f64 * 1.55) as f32;
    let new = r * 80.0;
    let armor = (80.0 - new) * 0.5;
    assert_eq!(p.health, 100.0 - new.trunc());
    assert_eq!(p.armor, 100.0 - armor);
    // A vest without a helmet does not cover the head: the head hit goes through whole.
    b.run(|game, _| {
        let p = game.state_mut().players[1].as_mut().unwrap();
        p.health = 100.0;
        p.armor = 100.0;
        p.kevlar = ArmorType::Vest;
    });
    b.run(|game, eng| game.test_trace_attack(eng, 1, 0, None, 20.0, HEAD));
    let p = b.run(|game, _| game.state().players[1].clone().unwrap());
    assert_eq!((p.health, p.armor), (20.0, 100.0));
}

#[test]
fn armor_absorbs_as_regamedll() {
    let mut b = arena();
    let set = |b: &mut Bench, armor: f32, kevlar: ArmorType| {
        b.run(move |game, _| {
            let p = game.state_mut().players[1].as_mut().unwrap();
            p.health = 100.0;
            p.armor = armor;
            p.kevlar = kevlar;
        });
    };
    let state = |b: &mut Bench| b.run(|game, _| game.state().players[1].clone().unwrap());

    // A chest hit, 30 damage, ratio 0.5, bonus 0.5: 15 through, 7.5 armor.
    set(&mut b, 100.0, ArmorType::Vest);
    b.run(|game, eng| game.test_trace_attack(eng, 1, 0, None, 30.0, CHEST));
    let p = state(&mut b);
    assert_eq!((p.health, p.armor), (85.0, 92.5));

    // Armor that runs out: 5 armor covers 10 damage, so 40 - 10 = 30 goes through.
    set(&mut b, 5.0, ArmorType::Vest);
    b.run(|game, eng| game.test_trace_attack(eng, 1, 0, None, 40.0, CHEST));
    let p = state(&mut b);
    assert_eq!((p.health, p.armor, p.kevlar), (70.0, 0.0, ArmorType::None));

    // A fall is not stopped by armor.
    set(&mut b, 100.0, ArmorType::Vest);
    b.run(|game, eng| game.test_damage_with_weapon(eng, 1, None, None, 10.0, dmg::FALL));
    let p = state(&mut b);
    assert_eq!((p.health, p.armor), (90.0, 100.0));

    // An explosion (`DMG_BLAST`): its own branch, the hit group is generic, 50 damage with ratio
    // 0.5 is 25 through and 12.5 armor.
    set(&mut b, 100.0, ArmorType::Vest);
    b.run(|game, eng| game.test_damage_with_weapon(eng, 1, None, None, 50.0, dmg::BLAST));
    let p = state(&mut b);
    assert_eq!((p.health, p.armor), (75.0, 87.5));
    // With too little armor the explosion takes the whole armor value off the damage.
    set(&mut b, 10.0, ArmorType::Vest);
    b.run(|game, eng| game.test_damage_with_weapon(eng, 1, None, None, 50.0, dmg::BLAST));
    let p = state(&mut b);
    assert_eq!((p.health, p.armor, p.kevlar), (60.0, 0.0, ArmorType::None));
}

#[test]
fn velocity_modifier_on_hit() {
    let mut b = arena();
    let modifier = |b: &mut Bench| {
        b.run(|game, _| game.velocity_modifier(1).unwrap())
    };
    assert_eq!(modifier(&mut b), 1.0);
    // An ordinary hit: no knockback, the speed modifier 0.5.
    b.run(|game, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(200.0, 0.0, 0.0));
        game.test_damage_with_weapon(eng, 1, Some(0), Some(WeaponId::Usp), 10.0, dmg::BULLET);
    });
    assert_eq!(modifier(&mut b), 0.5);
    let v = b.run(|game, eng| {
        let _ = game;
        eng.ent(eng.client_edict(1)).unwrap().velocity
    });
    // (a player is not pushed by a bullet, only by `TakeDamageImpulse`)
    assert_eq!(v, Vec3::ZERO);

    // A heavy rifle hit on a standing player moving slowly: knocked back 170 and 0.65.
    b.run(|game, eng| {
        game.set_velocity_modifier(1, 1.0);
        eng.ent_mut(eng.client_edict(1)).unwrap().velocity = Vec3::ZERO;
        game.test_damage_with_weapon(eng, 1, Some(0), Some(WeaponId::Ak47), 10.0, dmg::BULLET);
    });
    assert_eq!(modifier(&mut b), 0.65);
    let v = b.run(|game, eng| {
        let _ = game;
        eng.ent(eng.client_edict(1)).unwrap().velocity
    });
    assert!(v.x >= 170.0, "{v:?}");

    // Ducking: the ordinary flinch even with the AK-47.
    b.run(|game, eng| {
        game.set_velocity_modifier(1, 1.0);
        eng.ent_mut(eng.client_edict(1)).unwrap().flags |= cs_engine::consts::FL_DUCKING;
        game.test_damage_with_weapon(eng, 1, Some(0), Some(WeaponId::Ak47), 10.0, dmg::BULLET);
    });
    assert_eq!(modifier(&mut b), 0.5);
    // A hit on the legs by the AK-47 is an ordinary flinch too (checked on a standing player).
    b.run(|game, eng| {
        game.set_velocity_modifier(1, 1.0);
        eng.ent_mut(eng.client_edict(1)).unwrap().flags &= !cs_engine::consts::FL_DUCKING;
        game.test_trace_attack(eng, 1, 0, Some(WeaponId::Ak47), 10.0, LEFT_LEG);
    });
    assert_eq!(modifier(&mut b), 0.5);
}

/// `DeathMsg` as it is on the wire: killer, victim, headshot, the weapon and its terminator.
fn death_bytes(killer: u8, victim: u8, headshot: u8, weapon: &str) -> (&'static str, Vec<u8>) {
    let mut v = vec![killer, victim, headshot];
    v.extend_from_slice(weapon.as_bytes());
    v.push(0);
    ("DeathMsg", v)
}

fn death_msgs(b: &Bench) -> Vec<(&'static str, Vec<u8>)> {
    b.messages()
        .iter()
        .filter(|m| matches!(m, CsUserMsg::DeathMsg { .. }))
        .map(encode_cs_usermsg)
        .collect()
}

#[test]
fn death_msg_names_the_weapon_and_headshot() {
    let mut b = arena();
    b.probe.clear_messages();
    // A headshot with the AK-47 by the probe (killer 1) on slot 1 (index 2).
    b.run(|game, eng| game.test_trace_attack(eng, 1, 0, Some(WeaponId::Ak47), 100.0, HEAD));
    b.step(3);
    assert_eq!(death_msgs(&b), vec![death_bytes(1, 2, 1, "ak47")]);

    // A body hit with the knife: no headshot flag, `weapon_knife` without its prefix.
    b.probe.clear_messages();
    b.run(|game, eng| game.test_trace_attack(eng, 2, 0, Some(WeaponId::Knife), 500.0, CHEST));
    b.step(3);
    assert_eq!(death_msgs(&b), vec![death_bytes(1, 3, 0, "knife")]);

    // An attacker that holds nothing, and the world, are `world`.
    b.run(|game, eng| {
        game.respawn_player(eng, 1);
        game.respawn_player(eng, 2);
    });
    b.step(3);
    b.probe.clear_messages();
    b.run(|game, eng| game.test_damage_with_weapon(eng, 1, Some(0), None, 500.0, dmg::BULLET));
    b.step(3);
    assert_eq!(death_msgs(&b), vec![death_bytes(1, 2, 0, "world")]);
    // A suicide has no inflictor: `world`.
    b.probe.clear_messages();
    common::command(&mut b, 2, "kill");
    assert_eq!(death_msgs(&b), vec![death_bytes(3, 3, 0, "world")]);
}

#[test]
fn spread_displaces_along_right_and_up_of_the_view() {
    // `dir = forward + x * spread * right + y * spread * up`, the vectors from the view angles
    // through pm's `angle_vectors`. A target placed on that exact ray is hit; none of a swapped,
    // mirrored or negated ray would be (checked against the box, 16 wide, 36 high).
    let angles = Vec3::new(25.0, 40.0, 15.0);
    let (forward, right, up) = cs_sim::pm::math::angle_vectors(angles);
    let spread = 0.1_f32;
    let draw = |seed: u32| {
        let x = shared_random_float(seed, -0.5, 0.5) + shared_random_float(seed + 1, -0.5, 0.5);
        let y = shared_random_float(seed + 2, -0.5, 0.5) + shared_random_float(seed + 3, -0.5, 0.5);
        (x, y)
    };
    let src = eye(Vec3::new(-100.0, 50.0, 0.0));
    let at = |x: f32, y: f32, a: Vec3, b: Vec3| src + (forward + (x * spread) * a + (y * spread) * b) * 1000.0;
    let outside = |p: Vec3, q: Vec3| {
        let d = (p - q).abs();
        d.x > 40.0 || d.y > 40.0 || d.z > 40.0
    };
    let seed = (0..2000_u32)
        .step_by(4)
        .find(|&s| {
            let (x, y) = draw(s);
            let real = at(x, y, right, up);
            [at(y, x, right, up), at(x, y, up, right), at(-x, y, right, up), at(x, -y, right, up), at(x, y, -right, -up), src + forward * 1000.0]
                .iter()
                .all(|&other| outside(real, other))
        })
        .expect("a seed that separates every wrong ray");
    let (x, y) = draw(seed);
    let target = at(x, y, right, up);
    let mut b = arena();
    b.run(move |game, eng| {
        put(eng, 0, Vec3::new(-100.0, 50.0, 0.0));
        put(eng, 1, target);
        let mut bullet = rifle_bullet(src, 1, seed);
        bullet.dir = forward;
        bullet.spread = spread;
        game.test_fire_bullets3(eng, 0, None, angles, &bullet);
    });
    assert!(health(&mut b, 1) < 100.0, "the displaced ray hit its target");
}

#[test]
fn hostile_bullet_input_is_refused_quietly() {
    let mut b = arena();
    b.run(|game, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(216.0, 0.0, 0.0));
        let mut bullet = rifle_bullet(eye(Vec3::ZERO), 2, 0);
        bullet.dir = Vec3::new(f32::NAN, 0.0, 0.0);
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
        let mut bullet = rifle_bullet(eye(Vec3::ZERO), 2, 0);
        bullet.distance = 0.0;
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
        let mut bullet = rifle_bullet(eye(Vec3::ZERO), i32::MAX, 0);
        bullet.range_modifier = f32::NAN;
        game.test_fire_bullets3(eng, 0, None, Vec3::new(f32::NAN, 0.0, 0.0), &bullet);
        let bullet = rifle_bullet(eye(Vec3::ZERO), i32::MAX, 0);
        game.test_fire_bullets3(eng, 0, None, Vec3::ZERO, &bullet);
        // A stale attacker slot.
        game.test_damage_with_weapon(eng, 1, Some(9), Some(WeaponId::Ak47), 10.0, dmg::BULLET);
    });
    // The shot with a huge penetration count went through the clamp and hit once or twice.
    assert!(health(&mut b, 1) < 100.0);
}


#[test]
fn bullet_impact_effects_emit_blood_sparks_helmet_and_knife_streak() {
    use cs_engine::MoveFilter;
    use cs_proto48::messages::SvcMessage;
    use cs_proto48::tempent::te;

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

    // 1. Flesh chest shot on unarmored player -> TE_BLOODSPRITE.
    let mut b = arena();
    b.probe.clear_messages();
    b.run(|game, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(150.0, 0.0, 0.0));
        game.test_trace_attack(eng, 1, 0, Some(WeaponId::Ak47), 36.0, CHEST);
    });
    b.step(6);
    assert!(
        temp_entities(&b).contains(&te::BLOODSPRITE),
        "flesh hit must emit TE_BLOODSPRITE, got {:?}",
        temp_entities(&b)
    );

    // 2. Helmet headshot on VestHelm target -> TE_STREAK_SPLASH (no blood).
    let mut b = arena();
    b.run(|game, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(150.0, 0.0, 0.0));
        if let Some(p) = game.state_mut().players[1].as_mut() {
            p.kevlar = ArmorType::VestHelm;
            p.armor = 100.0;
        }
    });
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.test_trace_attack(eng, 1, 0, Some(WeaponId::Ak47), 36.0, HEAD);
    });
    b.step(6);
    let helmet_tes = temp_entities(&b);
    assert!(
        helmet_tes.contains(&te::STREAK_SPLASH),
        "helmet hit must emit TE_STREAK_SPLASH, got {helmet_tes:?}"
    );
    assert!(
        !helmet_tes.contains(&te::BLOODSPRITE),
        "helmet hit should not bleed, got {helmet_tes:?}"
    );

    // 3. Shield frontal deflection -> TE_SPARKS + zero damage.
    let mut b = arena();
    b.run(|game, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(150.0, 0.0, 0.0));
        let v1 = eng.client_edict(1);
        let ent1 = eng.ent_mut(v1).unwrap();
        ent1.angles = Vec3::new(0.0, 180.0, 0.0);
        ent1.v_angle = Vec3::new(0.0, 180.0, 0.0);
        let p = game.state_mut().players[1].as_mut().unwrap();
        p.owns_shield = true;
        p.shield_drawn = true;
        p.armor = 0.0;
    });
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.test_trace_attack(eng, 1, 0, Some(WeaponId::Ak47), 36.0, CHEST);
    });
    b.step(6);
    let shield_tes = temp_entities(&b);
    assert!(
        shield_tes.contains(&te::SPARKS),
        "shield hit must emit TE_SPARKS, got {shield_tes:?}"
    );
    assert_eq!(health(&mut b, 1), 100.0, "shield must block all frontal damage");

    // 4. Gunshot decal trace on a solid obstacle -> TE_GUNSHOTDECAL.
    let mut b = arena();
    b.server.set_decals(vec!["{shot1".into(), "{blood1".into()]);
    b.probe.clear_messages();
    b.run(|_, eng| {
        put(eng, 0, Vec3::ZERO);
        put(eng, 1, Vec3::new(150.0, 0.0, 0.0));
        let shooter = eng.client_edict(0);
        let tr = eng.trace_line(
            eye(Vec3::ZERO),
            eye(Vec3::new(200.0, 0.0, 0.0)),
            MoveFilter::Normal,
            Some(shooter),
        );
        cs_game::bullets::gunshot_decal_trace(eng, &tr, 0, None);
    });
    b.step(6);
    assert!(
        temp_entities(&b).contains(&te::GUNSHOTDECAL),
        "gunshot_decal_trace must emit TE_GUNSHOTDECAL, got {:?}",
        temp_entities(&b)
    );
}
