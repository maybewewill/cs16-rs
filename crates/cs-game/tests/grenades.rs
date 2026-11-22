//! The HE grenade, the flashbang and the smoke grenade (Task 13 of phase 2D): the throw (the
//! pin, the release, the bent pitch and the speed), the flight and the fuse, the blast of the
//! HE grenade, the flash and the blindness it leaves, the smoke grenade's events, and what
//! happens to the weapon after a throw. Against `wpn_hegrenade.cpp`, `wpn_flashbang.cpp`,
//! `wpn_smokegrenade.cpp`, `ggrenade.cpp` and `combat.cpp` of ReGameDLL_CS (read 2026-11-22).
//!
//! The probe is slot 0 (a counter-terrorist at the origin looking along +X), slot 1 a terrorist,
//! on a stub map with no gravity and no walls; the commands run through `common::run_cmd` with
//! the bench's 10 ms frame, the buttons set on the edict as the engine does before `CmdStart`.

mod common;

use common::{Bench, spawns};
use cs_engine::consts::{EF_NODRAW, FL_ONGROUND, MOVETYPE_BOUNCE, SOLID_BBOX};
use cs_engine::{EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::player::{Team, dmg};
use cs_game::weapons::{WEAPON_NOCLIP, Weapon, WeaponId, ammo_index, new_weapon};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;

fn approx(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

fn approx3(a: Vec3, b: Vec3, tolerance: f32) -> bool {
    (a - b).abs().max_element() <= tolerance
}

/// One command with `buttons`, as the engine runs it.
fn press(game: &mut CsGame, eng: &mut EngineApi<'_>, buttons: u16) {
    let id = eng.client_edict(0);
    let viewangles = eng.ent(id).unwrap().v_angle;
    eng.ent_mut(id).unwrap().button = i32::from(buttons);
    common::run_cmd(
        game,
        eng,
        0,
        &GoldSrcUserCmd { msec: 10, buttons, viewangles, ..Default::default() },
        0,
    );
}

fn weapon(game: &mut CsGame, id: WeaponId) -> &mut Weapon {
    game.weapon_mut(0, id).expect("the player carries the weapon")
}

/// The probe (slot 0, a counter-terrorist at the origin) and a terrorist (slot 1) at `victim`,
/// both spawned, the freeze period over, the victim's health at 1000 so that it survives.
fn arena(victim: Vec3) -> Bench {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        game.end_freeze_period(eng);
        let (me, other) = (eng.client_edict(0), eng.client_edict(1));
        eng.set_origin(me, Vec3::ZERO);
        eng.set_origin(other, victim);
        game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
    });
    b
}

/// Gives the probe grenade `id` with `ammo` of it, selects it and spends the deploy delay and
/// idle. A grenade has no clip (`WEAPON_NOCLIP`).
fn arm(game: &mut CsGame, eng: &mut EngineApi<'_>, id: WeaponId, ammo: i32) {
    assert!(game.give_weapon(eng, 0, new_weapon(id).unwrap(), WEAPON_NOCLIP, ammo));
    assert!(game.select_weapon(eng, 0, id));
    game.set_next_attack(0, 0.0);
    let w = weapon(game, id);
    w.next_primary_attack = 0.0;
    w.next_secondary_attack = 0.0;
    w.time_weapon_idle = 0.0;
}

/// Pulls the pin (one command with the fire button), spends the idle time and lets go (one
/// command with none): the grenade is thrown by the second.
fn throw(game: &mut CsGame, eng: &mut EngineApi<'_>, id: WeaponId) {
    press(game, eng, IN_ATTACK);
    weapon(game, id).time_weapon_idle = 0.0;
    press(game, eng, 0);
}

fn health(game: &CsGame, slot: usize) -> f32 {
    game.state().players[slot].as_ref().unwrap().health
}

fn ammo_of(game: &CsGame, name: &str) -> i32 {
    game.player_ammo(0, ammo_index(name) as usize).unwrap()
}

/// What the probe was sent: the user messages, and the `te_type` of every temp entity.
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

#[test]
fn throw_velocity_from_pitch() {
    // The pitch is bent: above the horizon (negative) `-10 + pitch * 80/90`, else
    // `-10 + pitch * 100/90`; the speed is `(90 - bent) * 6`, at most 750.
    for (pitch, yaw) in [(-30.0_f32, 0.0_f32), (0.0, 0.0), (45.0, 0.0), (0.0, 90.0), (89.0, 180.0)] {
        let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
        b.run(move |game, eng| {
            let bent = if pitch < 0.0 {
                (-10.0 + f64::from(pitch) * (80.0 / 90.0)) as f32
            } else {
                (-10.0 + f64::from(pitch) * (100.0 / 90.0)) as f32
            };
            let speed = ((90.0 - bent) * 6.0).min(750.0);
            // Forward of the bent pitch (positive pitch looks down).
            let (p, y) = (bent.to_radians(), yaw.to_radians());
            let forward = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
            arm(game, eng, WeaponId::HeGrenade, 1);
            let me = eng.client_edict(0);
            let v = eng.ent_mut(me).unwrap();
            v.v_angle = Vec3::new(pitch, yaw, 0.0);
            v.punchangle = Vec3::ZERO;
            v.velocity = Vec3::new(30.0, -20.0, 5.0);

            // The pin: the time of the pull and a release not yet seen, half a second of idle.
            let before = eng.time() as f32;
            press(game, eng, IN_ATTACK);
            let w = weapon(game, WeaponId::HeGrenade);
            assert!(approx(w.start_throw, before, 0.011), "m_flStartThrow {}", w.start_throw);
            assert_eq!(w.release_throw, 0.0, "m_flReleaseThrow is 0 until the release is seen");
            assert!(approx(w.time_weapon_idle, 0.5, 0.011), "idle {}", w.time_weapon_idle);
            assert!(game.grenades(eng).is_empty(), "nothing is thrown with the pin pulled");

            // The release: seen by the first idle after the pin; the idle time (0.5 s) holds the
            // throw back until it is spent.
            press(game, eng, 0);
            let w = weapon(game, WeaponId::HeGrenade);
            assert!(w.release_throw > 0.0, "the release is recorded");
            assert!(game.grenades(eng).is_empty(), "the pin was pulled half a second ago, not yet");
            weapon(game, WeaponId::HeGrenade).time_weapon_idle = 0.0;
            press(game, eng, 0);

            let grenades = game.grenades(eng);
            assert_eq!(grenades.len(), 1, "pitch {pitch} yaw {yaw}");
            let g = eng.ent(grenades[0]).unwrap().clone();
            // The player's own velocity and place are those the movement of the last command left
            // (it ran before the weapon did).
            let me_vars = eng.ent(eng.client_edict(0)).unwrap().clone();
            let expected = forward * speed + me_vars.velocity;
            assert!(approx3(g.velocity, expected, 0.05), "{:?} vs {expected:?}", g.velocity);
            // Hard numbers, not the formula: a level throw is bent 10 degrees up and flies at
            // (90 + 10) * 6 = 600 along it: 600 * cos 10 and 600 * sin 10.
            if pitch == 0.0 && yaw == 0.0 {
                let level = g.velocity - me_vars.velocity;
                assert!(approx3(level, Vec3::new(590.885, 0.0, 104.189), 0.05), "{level:?}");
            }
            // A pitch of 45 down bends to 40: (90 - 40) * 6 = 300 along it.
            if pitch == 45.0 {
                assert!(approx((g.velocity - me_vars.velocity).length(), 300.0, 0.05));
            }
            // A steep throw up is capped at 750.
            if pitch == -30.0 {
                assert!(approx((g.velocity - me_vars.velocity).length(), 750.0, 0.05));
            }
            let start = me_vars.origin + me_vars.view_ofs + forward * 16.0;
            assert!(approx3(g.origin, start, 0.05), "{:?} vs {start:?}", g.origin);
            assert_eq!(g.classname, "grenade");
            assert_eq!(g.owner, Some(eng.client_edict(0)));
            assert_eq!((g.movetype, g.solid), (MOVETYPE_BOUNCE, SOLID_BBOX));
            assert_eq!((g.dmg, g.gravity, g.friction), (100.0, 0.55, 0.7));
            assert_eq!(g.framerate, 1.0);
            assert!((3..=6).contains(&g.sequence), "sequence {}", g.sequence);
            // The fuse: 1.5 seconds from the throw.
            assert!(approx(g.dmgtime, eng.time() as f32 + 1.5, 0.011), "{}", g.dmgtime);
            // After the throw: nothing pulled, the last grenade gone, every timer 0.5 s out.
            let w = weapon(game, WeaponId::HeGrenade).clone();
            assert_eq!(w.start_throw, 0.0);
            for t in [w.next_primary_attack, w.next_secondary_attack, w.time_weapon_idle] {
                assert!(approx(t, 0.5, 0.011), "timer {t}");
            }
            assert_eq!(ammo_of(game, "HEGrenade"), 0);
        });
    }
}

#[test]
fn a_flashbang_and_a_smoke_grenade_fly_as_the_source_makes_them() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::Flashbang, 2);
        throw(game, eng, WeaponId::Flashbang);
        let g = eng.ent(game.grenades(eng)[0]).unwrap().clone();
        assert_eq!((g.dmg, g.gravity, g.friction), (35.0, 0.5, 0.8));
        assert_eq!(ammo_of(game, "Flashbang"), 1, "one of two thrown");
        // Not the last one: the timers are the ordinary ones (0.5 for the next throw, 0.75 for
        // the idle) and not pushed back by half a second more.
        let w = weapon(game, WeaponId::Flashbang).clone();
        assert!(approx(w.next_primary_attack, 0.5, 0.011));
        assert!(approx(w.time_weapon_idle, 0.75, 0.011));

        arm(game, eng, WeaponId::SmokeGrenade, 1);
        throw(game, eng, WeaponId::SmokeGrenade);
        let all = game.grenades(eng);
        assert_eq!(all.len(), 2);
        let g = eng.ent(all[1]).unwrap().clone();
        assert_eq!((g.dmg, g.gravity, g.friction), (35.0, 0.5, 0.8));
        assert_eq!(ammo_of(game, "SmokeGrenade"), 0);
    });
}

#[test]
fn a_fuse_under_a_tenth_of_a_second_stops_the_flashbang_and_goes_off_at_once() {
    // `ShootTimed` and `ShootSmokeGrenade` (not `ShootTimed2`) stop a grenade with `time < 0.1`
    // and think at once.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        let owner = eng.client_edict(0);
        assert!(game.test_shoot_flashbang(eng, owner, Vec3::new(100.0, 0.0, 0.0), Vec3::new(500.0, 0.0, 0.0), 0.05));
        let g = eng.ent(game.grenades(eng)[0]).unwrap().clone();
        assert_eq!(g.velocity, Vec3::ZERO);
        assert!(approx(g.nextthink as f32, eng.time() as f32, 0.001), "{}", g.nextthink);
    });
}

#[test]
fn grenade_bounces_and_detonates_on_time() {
    // An HE grenade thrown level flies on (no gravity, no walls here) and goes off one think
    // after its fuse is spent: the fuse is 1.5 s, the tumble think runs every 0.1 s and changes
    // the think when `dmgtime` has passed, so the explosion follows 0.1 to 0.2 s after.
    let mut b = arena(Vec3::new(1000.0, 120.0, 17.0));
    let (thrown, grenade) = b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        let me = eng.client_edict(0);
        let v = eng.ent_mut(me).unwrap();
        v.v_angle = Vec3::ZERO;
        v.punchangle = Vec3::ZERO;
        throw(game, eng, WeaponId::HeGrenade);
        (eng.time(), game.grenades(eng)[0])
    });
    b.probe.clear_messages();
    let mut exploded = None;
    for _ in 0..300 {
        let seen = b.run(move |_, eng| {
            eng.ent(grenade).map(|v| (eng.time(), v.effects & EF_NODRAW != 0, v.origin, v.velocity))
        });
        if let Some((time, true, origin, velocity)) = seen {
            assert_eq!(velocity, Vec3::ZERO, "the explosion stops it");
            exploded = Some((time, origin));
            break;
        }
    }
    let (time, origin) = exploded.expect("the grenade went off");
    let fuse = time - thrown;
    assert!((1.55..=1.85).contains(&fuse), "went off {fuse} s after the throw");

    // The victim took what `RadiusDamage` gives: `100 - distance * (100 / 350)`, cut to an int,
    // the distance from one unit above the grenade.
    b.step(2);
    let victim = Vec3::new(1000.0, 120.0, 17.0);
    let distance = (origin + Vec3::Z - victim).length();
    let expected = (100.0_f32 - distance * (100.0_f32 / 350.0)).trunc();
    assert!(expected > 0.0, "the victim is within the blast");
    b.run(move |game, _| {
        assert_eq!(health(game, 1), 1000.0 - expected, "distance {distance}");
    });
    // Two explosions and the sparks went out to the probe.
    let tes = temp_entities(&b);
    assert_eq!(tes.iter().filter(|&&t| t == 3).count(), 2, "TE_EXPLOSION twice: {tes:?}");
    // 0.55 s after the blast the grenade is gone (`Smoke3_C`).
    b.step(70);
    assert!(b.run(move |_, eng| eng.ent(grenade).is_none()), "the grenade is removed");
    assert!(temp_entities(&b).contains(&5), "TE_SMOKE");
}

#[test]
fn bounce_touch_follows_the_source() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        throw(game, eng, WeaponId::HeGrenade);
        let grenade = game.grenades(eng)[0];
        let me = eng.client_edict(0);
        let other = eng.client_edict(1);
        let set = |eng: &mut EngineApi<'_>, flags: u32, velocity: Vec3| {
            let v = eng.ent_mut(grenade).unwrap();
            v.flags = flags;
            v.velocity = velocity;
        };

        // The thrower does not stop it.
        set(eng, 0, Vec3::new(100.0, 0.0, 0.0));
        game.test_bounce_touch(eng, grenade, me);
        assert_eq!(eng.ent(grenade).unwrap().velocity, Vec3::new(100.0, 0.0, 0.0));
        // On the ground: a fifth of the speed goes, the sequence is 1, and the model's frame
        // rate follows the speed (`|v| / 200`, 0 below a half, at most 1).
        set(eng, FL_ONGROUND, Vec3::new(100.0, 0.0, 0.0));
        game.test_bounce_touch(eng, grenade, other);
        let v = eng.ent(grenade).unwrap();
        assert!(approx3(v.velocity, Vec3::new(80.0, 0.0, 0.0), 1e-4), "{:?}", v.velocity);
        assert_eq!(v.sequence, 1);
        assert_eq!(v.framerate, 0.0, "80 / 200 is under a half");
        set(eng, FL_ONGROUND, Vec3::new(500.0, 0.0, 0.0));
        game.test_bounce_touch(eng, grenade, other);
        assert_eq!(eng.ent(grenade).unwrap().framerate, 1.0);
        set(eng, FL_ONGROUND, Vec3::new(150.0, 0.0, 0.0));
        game.test_bounce_touch(eng, grenade, other);
        assert!(approx(eng.ent(grenade).unwrap().framerate, 0.6, 1e-4), "120 / 200");

        // In the air the speed stays; the tenth bounce stops it on the ground.
        set(eng, 0, Vec3::new(100.0, 0.0, 0.0));
        for i in 0..10 {
            game.test_bounce_touch(eng, grenade, other);
            let v = eng.ent(grenade).unwrap();
            assert_eq!(v.velocity, Vec3::new(100.0, 0.0, 0.0), "bounce {i}");
            assert_eq!(v.flags & FL_ONGROUND, 0, "bounce {i}");
        }
        game.test_bounce_touch(eng, grenade, other);
        let v = eng.ent(grenade).unwrap();
        assert_ne!(v.flags & FL_ONGROUND, 0, "after ten bounces it lies still");
        assert_eq!(v.velocity, Vec3::ZERO);
        assert_eq!(v.groundentity, Some(eng.world()));

        // After ten bounces any touch in the air stops it again (the source's `>= 10` test).
        set(eng, 0, Vec3::new(100.0, 0.0, 0.0));
        game.test_bounce_touch(eng, grenade, other);
        assert_eq!(eng.ent(grenade).unwrap().velocity, Vec3::ZERO);
    });
}

#[test]
fn a_see_through_breakable_throws_the_grenade_back_at_twice_the_speed() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        throw(game, eng, WeaponId::HeGrenade);
        let grenade = game.grenades(eng)[0];
        let glass = eng.create_entity("func_breakable").unwrap();
        eng.ent_mut(glass).unwrap().rendermode = 2;
        eng.ent_mut(grenade).unwrap().velocity = Vec3::new(100.0, 0.0, 0.0);
        game.test_bounce_touch(eng, grenade, glass);
        assert_eq!(eng.ent(grenade).unwrap().velocity, Vec3::new(-200.0, 0.0, 0.0));
        // An opaque one (`kRenderNormal`) is an ordinary touch: in the air the speed stays.
        eng.ent_mut(glass).unwrap().rendermode = 0;
        eng.ent_mut(grenade).unwrap().velocity = Vec3::new(100.0, 0.0, 0.0);
        game.test_bounce_touch(eng, grenade, glass);
        assert_eq!(eng.ent(grenade).unwrap().velocity, Vec3::new(100.0, 0.0, 0.0));
    });
}

#[test]
fn he_radius_damage_as_regamedll() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        let me = eng.client_edict(0);
        let inflictor = eng.create_entity("grenade").unwrap();
        let src = Vec3::new(0.0, 0.0, 100.0);
        let falloff = 100.0_f32 / 350.0;
        // The damage falls linearly with the distance from one unit above the source to the
        // victim's origin, to 0 at 350; an int of it reaches the victim, through the air.
        for d in [0.0_f32, 50.0, 100.0, 200.0, 300.0, 349.0] {
            let victim = Vec3::new(d, 0.0, 101.0);
            game.place_player(eng, 1, victim);
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            game.test_radius_damage(eng, src, Some(inflictor), None, Some(me), 100.0, 350.0, dmg::EXPLOSION);
            let expected = (100.0_f32 - d * falloff).trunc();
            assert_eq!(health(game, 1), 1000.0 - expected, "distance {d}");
        }
        // Past the radius nothing happens.
        game.place_player(eng, 1, Vec3::new(351.0, 0.0, 101.0));
        game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
        game.test_radius_damage(eng, src, Some(inflictor), None, Some(me), 100.0, 350.0, dmg::EXPLOSION);
        assert_eq!(health(game, 1), 1000.0);
        // A dead player takes nothing (`takedamage` is `DAMAGE_NO`).
        game.place_player(eng, 1, Vec3::new(10.0, 0.0, 101.0));
        game.state_mut().players[1].as_mut().unwrap().takes_damage = false;
        game.test_radius_damage(eng, src, Some(inflictor), None, Some(me), 100.0, 350.0, dmg::EXPLOSION);
        assert_eq!(health(game, 1), 1000.0);
    });
}

#[test]
fn he_blast_is_the_throwers_kill_and_reaches_350_units() {
    // The grenade's own explosion (here the grenade is put where it goes off and its fuse is
    // spent): a victim 100 units away that dies gives the thrower the kill and the death notice
    // says `grenade`.
    let mut b = arena(Vec3::new(100.0, 0.0, 1.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        throw(game, eng, WeaponId::HeGrenade);
        let grenade = game.grenades(eng)[0];
        game.state_mut().players[1].as_mut().unwrap().health = 10.0;
        eng.ent_mut(eng.client_edict(1)).unwrap().health = 10.0;
        // Stop it where it is and let it go off: the fuse is over.
        let at = eng.time() + 0.01;
        let v = eng.ent_mut(grenade).unwrap();
        v.velocity = Vec3::ZERO;
        v.origin = Vec3::ZERO;
        v.dmgtime = 0.0;
        v.nextthink = at;
    });
    b.probe.clear_messages();
    b.step(60);
    let death = b
        .messages()
        .iter()
        .find_map(|m| match m {
            CsUserMsg::DeathMsg { killer_id, victim_id, weapon, .. } => {
                Some((*killer_id, *victim_id, weapon.clone()))
            }
            _ => None,
        })
        .expect("the victim died");
    assert_eq!(death, (1, 2, "grenade".to_string()));
    b.run(|game, _| {
        assert_eq!(game.state().players[0].as_ref().unwrap().frags, 1, "the thrower's kill");
        assert!(!game.state().players[1].as_ref().unwrap().alive);
    });

    // 340 units away (350 is the radius, `100 * 3.5`) the blast still hurts: 2 points of 100;
    // 360 units away it does not.
    for (distance, lost) in [(340.0_f32, 2.0_f32), (360.0, 0.0)] {
        let mut b = arena(Vec3::new(distance, 0.0, 1.0));
        b.run(|game, eng| {
            arm(game, eng, WeaponId::HeGrenade, 1);
            throw(game, eng, WeaponId::HeGrenade);
            let grenade = game.grenades(eng)[0];
            let at = eng.time() + 0.01;
            let v = eng.ent_mut(grenade).unwrap();
            v.velocity = Vec3::ZERO;
            v.origin = Vec3::ZERO;
            v.dmgtime = 0.0;
            v.nextthink = at;
        });
        b.step(60);
        b.run(move |game, _| assert_eq!(health(game, 1), 1000.0 - lost, "{distance} units"));
    }
}

#[test]
fn friendly_fire_off_spares_a_teammate_but_not_the_thrower() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        let me = eng.client_edict(0);
        // A grenade the probe threw (it keeps the thrower's team).
        arm(game, eng, WeaponId::HeGrenade, 1);
        throw(game, eng, WeaponId::HeGrenade);
        let inflictor = game.grenades(eng)[0];
        // The terrorist changes sides.
        game.state_mut().players[1].as_mut().unwrap().team = Team::Ct;
        game.place_player(eng, 1, Vec3::new(50.0, 0.0, 1.0));
        game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
        game.state_mut().players[0].as_mut().unwrap().health = 1000.0;
        eng.ent_mut(me).unwrap().health = 1000.0;
        game.test_radius_damage(eng, Vec3::new(0.0, 0.0, 0.0), Some(inflictor), Some(i32::from(Team::Ct.id())), Some(me), 100.0, 350.0, dmg::EXPLOSION);
        assert_eq!(health(game, 1), 1000.0, "a teammate (mp_friendlyfire is 0)");
        assert!(health(game, 0) < 1000.0, "the thrower hurts itself");
    });
}

/// Puts the probe at the origin looking along +X and the terrorist (slot 1) at `at`, then
/// flashes from `src` and returns the `ScreenFade` the probe was sent and its blind fields.
/// A `ScreenFade` as the probe got it: duration, hold time, flags and colour.
type Fade = (u16, u16, u16, [u8; 4]);
/// The blind fields: until, start, hold, fade, alpha.
type Blind = (f32, f32, f32, f32, i32);

fn flash(b: &mut Bench, at: Vec3, src: Vec3) -> (Option<Fade>, Blind) {
    b.probe.clear_messages();
    let now = b.run(move |game, eng| {
        let (me, thrower) = (eng.client_edict(0), eng.client_edict(1));
        game.place_player(eng, 0, at);
        let v = eng.ent_mut(me).unwrap();
        v.v_angle = Vec3::ZERO;
        v.view_ofs = Vec3::new(0.0, 0.0, 17.0);
        let inflictor = eng.create_entity("grenade").unwrap();
        game.test_radius_flash(eng, src, inflictor, Some(thrower), 4.0);
        eng.time()
    });
    b.step(10);
    // (The spawn's own fade, a black one with no alpha, may still be on its way.)
    let fade = b.messages().iter().find_map(|m| match m {
        CsUserMsg::ScreenFade { duration, hold_time, flags, color } if color[..3] == [255, 255, 255] => {
            Some((*duration, *hold_time, *flags, *color))
        }
        _ => None,
    });
    let _ = now;
    let state = b.run(|game, _| game.blind_state(0).unwrap());
    (fade, state)
}

/// What `UTIL_ScreenFade` writes for `seconds`: `FixedUnsigned16(seconds, 1 << 12)`.
fn fixed(seconds: f32) -> u16 {
    (seconds * 4096.0) as u16
}

#[test]
fn flash_blinds_by_angle_and_distance() {
    // `flAdjustedDamage = 4 - distance * (4 / 1500)`.
    let damage_at = |distance: f32| 4.0 - distance * (4.0 / 1500.0);
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    // The probe at the origin, the flash 500 units in front of it (+X), at its eye height.
    // `flAdjustedDamage = 4 - distance * (4 / 1500)`; looking at the flash: alpha 255, fade
    // `3 * damage`, hold `damage / 1.5`. (The traced point is on the body, a random 0.5 to 1.1
    // of the eye height up: a few units off the distance.)
    let (fade, state) = flash(&mut b, Vec3::ZERO, Vec3::new(500.0, 0.0, 17.0));
    // The first trace ends on the face of the probe's box, 16 units short of its centre.
    let damage = damage_at(500.0 - 16.0);
    let (duration, hold, flags, color) = fade.expect("the probe was blinded");
    assert_eq!((flags, color[..3].to_vec(), color[3]), (0, vec![255, 255, 255], 255));
    assert!(duration.abs_diff(fixed(damage * 3.0)) <= 100, "fade {duration} vs {}", fixed(damage * 3.0));
    assert!(hold.abs_diff(fixed(damage / 1.5)) <= 60, "hold {hold} vs {}", fixed(damage / 1.5));
    // `Blind(fadeTime * 0.33, fadeHold, fadeTime, alpha)`.
    let (until, start, state_hold, state_fade, alpha) = state;
    assert_eq!(alpha, 255);
    assert!(approx(state_fade, damage * 3.0, 0.03), "{state_fade}");
    assert!(approx(state_hold, damage / 1.5, 0.02), "{state_hold}");
    assert!(approx(until - start, state_fade * 0.33, 1e-3), "{} {}", until - start, state_fade * 0.33);
    assert!(start > 0.0);

    // From behind (the flash at -X): alpha 200, fade `1.75 * damage`, hold `damage / 3.5`.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    let (fade, state) = flash(&mut b, Vec3::ZERO, Vec3::new(-500.0, 0.0, 17.0));
    let (duration, hold, _, color) = fade.expect("the probe was blinded");
    assert_eq!(color[3], 200);
    assert!(duration.abs_diff(fixed(damage * 1.75)) <= 100, "fade {duration}");
    assert!(hold.abs_diff(fixed(damage / 3.5)) <= 60, "hold {hold}");
    assert_eq!(state.4, 200);

    // Closer is stronger: at 100 units 3.73 of 4 against 2.67 at 500.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    let (near, _) = flash(&mut b, Vec3::ZERO, Vec3::new(100.0, 0.0, 17.0));
    assert!(near.unwrap().0.abs_diff(fixed(damage_at(100.0 - 16.0) * 3.0)) <= 100);

    // 1500 units is the limit: none beyond it, and at 1400 almost nothing but still a flash.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    let (fade, state) = flash(&mut b, Vec3::ZERO, Vec3::new(1600.0, 0.0, 17.0));
    assert!(fade.is_none(), "{fade:?}");
    assert_eq!(state.4, 0);
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    let (fade, _) = flash(&mut b, Vec3::ZERO, Vec3::new(1400.0, 0.0, 17.0));
    assert!(fade.is_some());
}

#[test]
fn a_flash_does_not_lighten_the_blindness_the_player_has() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    // Already blind: alpha 255, a fade of 10 s, hold 5 s, started a second ago.
    let now = b.run(|game, eng| {
        let t = eng.time() as f32;
        game.set_blind(0, t - 1.0, 3.0, 5.0, 10.0, 255);
        t
    });
    // A flash from behind has alpha 200 and a fade under 10: the old ones stay, and the hold
    // is the new one (the old hold is added only to a facing flash of alpha 255).
    let (fade, state) = flash(&mut b, Vec3::ZERO, Vec3::new(-300.0, 0.0, 17.0));
    let (_, _, _, color) = fade.expect("blinded again");
    assert_eq!(color[3], 255, "the stronger alpha wins");
    assert!(approx(state.3, 10.0, 1e-3), "the longer fade wins: {}", state.3);
    let damage = 4.0 - (300.0 - 16.0) * (4.0 / 1500.0);
    assert!(approx(state.2, damage / 3.5, 0.02), "hold {}", state.2);
    let _ = now;

    // A facing flash of alpha 255 adds what is left of the old hold (5 - 1 = 4 s).
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        let t = eng.time() as f32;
        game.set_blind(0, t - 1.0, 3.0, 5.0, 10.0, 255);
    });
    let (_, state) = flash(&mut b, Vec3::ZERO, Vec3::new(300.0, 0.0, 17.0));
    assert!(approx(state.2, damage / 1.5 + 4.0, 0.1), "hold {}", state.2);
}

#[test]
fn a_dead_player_is_not_flashed_and_blindness_ends_with_the_spawn() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        game.damage_player(eng, 0, None, 500.0, 0);
    });
    let (fade, state) = flash(&mut b, Vec3::ZERO, Vec3::new(300.0, 0.0, 17.0));
    assert!(fade.is_none() && state.4 == 0, "{fade:?} {state:?}");

    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        let t = eng.time() as f32;
        game.set_blind(0, t, 3.0, 5.0, 10.0, 255);
        assert_eq!(game.blind_state(0).unwrap().4, 255);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert_eq!(game.blind_state(0).unwrap(), (0.0, 0.0, 0.0, 0.0, 0));
    });
}

/// The status messages the probe gets while it looks at the terrorist 200 units ahead for 0.3 s
/// of commands (the status bar runs with the client data update of each command, every 0.2 s).
fn status_values(b: &mut Bench) -> Vec<(u8, i16)> {
    b.run(|game, eng| {
        game.place_player(eng, 0, Vec3::ZERO);
        game.place_player(eng, 1, Vec3::new(200.0, 0.0, 0.0));
        let v = eng.ent_mut(eng.client_edict(0)).unwrap();
        v.v_angle = Vec3::ZERO;
        v.punchangle = Vec3::ZERO;
    });
    b.probe.clear_messages();
    for _ in 0..30 {
        b.run(|game, eng| common::commands_of(game, eng, 0, 1));
    }
    b.step(3);
    b.messages()
        .iter()
        .filter_map(|m| match m {
            CsUserMsg::StatusValue { index, value } => Some((*index, *value)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_blind_player_sees_no_name_in_the_status_bar() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    let named = status_values(&mut b);
    assert!(named.contains(&(2, 2)), "the enemy at edict 2 is named: {named:?}");

    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        let t = eng.time() as f32;
        // Blind for the next 10 seconds.
        game.set_blind(0, t, 10.0, 5.0, 10.0, 255);
    });
    assert!(status_values(&mut b).is_empty(), "no name while blind");
}

/// The id of the event `name` (precached by `WeaponsPrecache`).
fn event_index(b: &mut Bench, name: &'static str) -> u16 {
    b.run(move |_, eng| eng.precache_event(name))
}

#[test]
fn smoke_puffs_events() {
    // The smoke grenade lies on the ground (the stub map has no floor: it is put there), goes
    // off when its fuse is spent and plays `events/createsmoke.sc` at its origin; then, once a
    // second 21 times, a puff event with the place of the detonation as its angles.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    let event = event_index(&mut b, "events/createsmoke.sc");
    assert_ne!(event, 0);
    let grenade = b.run(|game, eng| {
        arm(game, eng, WeaponId::SmokeGrenade, 1);
        throw(game, eng, WeaponId::SmokeGrenade);
        let grenade = game.grenades(eng)[0];
        assert!(eng.ent(grenade).unwrap().velocity.length() > 100.0);
        grenade
    });
    // Its flight ends: it lies on the ground (the world).
    b.run(move |_, eng| {
        let world = eng.world();
        let v = eng.ent_mut(grenade).unwrap();
        v.velocity = Vec3::ZERO;
        v.flags |= FL_ONGROUND;
        v.groundentity = Some(world);
    });
    b.probe.clear_messages();
    let origin = b.run(move |_, eng| eng.ent(grenade).unwrap().origin);
    b.step(2400);
    let events: Vec<_> = b
        .probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Event(events) => Some(events.clone()),
            _ => None,
        })
        .flatten()
        .filter(|e| e.index == event)
        .collect();
    let args: Vec<_> = events.iter().map(|e| e.args.unwrap_or_default()).collect();
    // The first event: iparam2 1, the origin of the grenade; then the puffs: iparam2 4.
    assert_eq!(args[0].iparam2, 1, "{:?}", args[0]);
    assert!(approx3(args[0].origin, origin, 0.2), "{:?} vs {origin:?}", args[0].origin);
    let puffs = &args[1..];
    assert_eq!(puffs.len(), 22, "SG_Smoke runs 22 times: {}", puffs.len());
    for p in puffs {
        assert_eq!(p.iparam2, 4);
        // `flSmokeInterval = RANDOM_FLOAT(1.5, 3.5) * 100`.
        assert!((150..=350).contains(&p.iparam1), "{}", p.iparam1);
        // The place of the detonation is the angles; a puff's direction is 3 to 8 units.
        assert!(approx3(p.angles, args[0].origin, 0.2), "{:?}", p.angles);
        let direction = (p.fparam1 * p.fparam1 + p.fparam2 * p.fparam2).sqrt();
        assert!((2.9..=8.1).contains(&direction), "{direction}");
    }
    // Turned 30 degrees more each time (): consecutive puffs differ by 30 degrees (the
    // direction is a vector along the grenade's forward vector, which is fixed).
    let turn = |a: &cs_proto48::event::EventArgs| a.fparam2.atan2(a.fparam1).to_degrees();
    for pair in puffs.windows(2) {
        let step = (turn(&pair[1]) - turn(&pair[0])).rem_euclid(360.0);
        assert!(approx(step, 30.0, 1.5) || approx(step, 30.0 + 360.0, 1.5), "turn {step}");
    }
    b.run(move |_, eng| assert!(eng.ent(grenade).is_none(), "gone after the last puff"));
}

#[test]
fn light_smoke_when_another_grenade_is_near_and_spent() {
    // `SG_Detonate`: a grenade within 250 units whose fuse has run out makes the cloud light
    // (the first event's `bparam1`).
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    let event = event_index(&mut b, "events/createsmoke.sc");
    let (smoke, other) = b.run(|game, eng| {
        arm(game, eng, WeaponId::SmokeGrenade, 1);
        throw(game, eng, WeaponId::SmokeGrenade);
        let smoke = game.grenades(eng)[0];
        let origin = eng.ent(smoke).unwrap().origin;
        let world = eng.world();
        let v = eng.ent_mut(smoke).unwrap();
        v.velocity = Vec3::ZERO;
        v.flags |= FL_ONGROUND;
        v.groundentity = Some(world);
        v.dmgtime = 0.0;
        // Another grenade 100 units away, long spent.
        let other = eng.create_entity("grenade").unwrap();
        let o = eng.ent_mut(other).unwrap();
        o.origin = origin + Vec3::new(100.0, 0.0, 0.0);
        o.dmgtime = 0.0;
        (smoke, other)
    });
    let _ = (smoke, other);
    b.probe.clear_messages();
    b.step(30);
    let first = b
        .probe
        .messages()
        .iter()
        .find_map(|m| match m {
            SvcMessage::Event(events) => events.iter().find(|e| e.index == event).copied(),
            _ => None,
        })
        .expect("the smoke went off");
    assert_eq!(first.args.unwrap().bparam1, 1, "a light cloud");
}

#[test]
fn a_grenade_in_the_world_is_taken_out_of_it() {
    // `TumbleThink`: out of the world (here 5000 units along an axis) the grenade is removed.
    let mut b = arena(Vec3::new(1000.0, 1000.0, 0.0));
    let grenade = b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        throw(game, eng, WeaponId::HeGrenade);
        let grenade = game.grenades(eng)[0];
        let v = eng.ent_mut(grenade).unwrap();
        v.velocity = Vec3::ZERO;
        v.origin = Vec3::new(4200.0, 0.0, 0.0);
        grenade
    });
    b.step(20);
    assert!(b.run(move |_, eng| eng.ent(grenade).is_none()));
}

/// Gives the probe a knife and a USP besides the grenades `ammo` says: `(id, ammo)`.
fn kit(game: &mut CsGame, eng: &mut EngineApi<'_>, grenades: &[(WeaponId, i32)]) {
    for id in [WeaponId::Knife, WeaponId::Usp] {
        assert!(game.give_weapon(eng, 0, new_weapon(id).unwrap(), if id == WeaponId::Knife { WEAPON_NOCLIP } else { 12 }, 24));
    }
    let (first, ammo) = grenades[0];
    for &(id, ammo) in grenades {
        assert!(game.give_weapon(eng, 0, new_weapon(id).unwrap(), WEAPON_NOCLIP, ammo));
    }
    assert!(game.select_weapon(eng, 0, first));
    let _ = ammo;
    game.set_next_attack(0, 0.0);
    for &(id, _) in grenades {
        let w = weapon(game, id);
        w.next_primary_attack = 0.0;
        w.next_secondary_attack = 0.0;
        w.time_weapon_idle = 0.0;
    }
}

/// Runs idle commands until the active weapon is not `id` (at most 200 of them, 10 ms each).
fn idle_until_switched(game: &mut CsGame, eng: &mut EngineApi<'_>, from: WeaponId) -> usize {
    for n in 0..200 {
        press(game, eng, 0);
        if game.player_weapon_state(0).unwrap().2 != Some(from) {
            return n;
        }
    }
    panic!("still holding {from:?}");
}

#[test]
fn the_last_grenade_thrown_retires_the_weapon() {
    // The throw pushes the idle back half a second; the first idle after it (the release was
    // seen) finds no ammo and `RetireWeapon`s: the view model goes, the heaviest other weapon
    // (the USP, weight 5, over the knife's 0) is deployed, the grenade is destroyed.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        kit(game, eng, &[(WeaponId::HeGrenade, 1)]);
        throw(game, eng, WeaponId::HeGrenade);
        assert_eq!(game.player_weapon_state(0).unwrap().2, Some(WeaponId::HeGrenade), "not yet");
        let n = idle_until_switched(game, eng, WeaponId::HeGrenade);
        // About half a second of commands.
        assert!((45..=55).contains(&n), "{n} commands");
        assert_eq!(game.player_weapon_state(0).unwrap().2, Some(WeaponId::Usp));
        assert!(!game.carried_weapons(0).contains(&WeaponId::HeGrenade), "destroyed: {:?}", game.carried_weapons(0));
        let (mask, _) = game.weapons_mask(0).unwrap();
        assert_eq!(mask & WeaponId::HeGrenade.bit(), 0, "pev->weapons has no HE bit");
        // The Deploy of the USP set its model and the player's `m_flNextAttack`.
        let me = eng.client_edict(0);
        assert_ne!(eng.ent(me).unwrap().viewmodel, 0);
        assert!(game.player_weapon_state(0).unwrap().0 > 0.0, "the deploy delay");
    });
}

#[test]
fn a_flashbang_retires_even_with_another_left_and_the_he_grenade_is_drawn_again() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        // Two flashbangs: one thrown, one left. The flashbang's idle after the throw is
        // `RetireWeapon` whatever is left: the second is not drawn.
        kit(game, eng, &[(WeaponId::Flashbang, 2)]);
        throw(game, eng, WeaponId::Flashbang);
        idle_until_switched(game, eng, WeaponId::Flashbang);
        assert_eq!(game.player_weapon_state(0).unwrap().2, Some(WeaponId::Usp));
        assert!(game.carried_weapons(0).contains(&WeaponId::Flashbang), "the second is still carried");
        assert_eq!(ammo_of(game, "Flashbang"), 1);
    });
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        // The smoke grenade and the HE grenade have one each at most, so this is the
        // `else`: with ammo left (a second HE, given directly) the HE grenade draws again.
        kit(game, eng, &[(WeaponId::HeGrenade, 1)]);
        game.set_player_ammo(0, ammo_index("HEGrenade") as usize, 2);
        throw(game, eng, WeaponId::HeGrenade);
        assert_eq!(ammo_of(game, "HEGrenade"), 1);
        // Idle until the release is dealt with: `m_flReleaseThrow` goes back to -1.
        for _ in 0..100 {
            press(game, eng, 0);
        }
        assert_eq!(game.player_weapon_state(0).unwrap().2, Some(WeaponId::HeGrenade), "still in hand");
        let w = weapon(game, WeaponId::HeGrenade).clone();
        assert_eq!(w.release_throw, -1.0);
        assert_eq!(w.start_throw, 0.0);
        assert!(w.time_weapon_idle > 9.0 && w.time_weapon_idle <= 15.0, "10 to 15 s: {}", w.time_weapon_idle);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 3, "HEGRENADE_DRAW");
    });
}

#[test]
fn the_flashbang_idle_has_the_sources_bug() {
    // Over three quarters, `iAnim = *(int *)&flRand`: the bits of the float are the animation.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::Flashbang, 2);
        // Find a state of the generator whose next `RANDOM_FLOAT(0, 1)` is above 0.75.
        let mut state = 1_u32;
        let rand = loop {
            game.test_set_rng(state);
            let r = game.test_random_long(0, 65535) as f32 / 65535.0;
            if r > 0.75 {
                break r;
            }
            state += 1;
        };
        game.test_set_rng(state);
        weapon(game, WeaponId::Flashbang).time_weapon_idle = 0.0;
        press(game, eng, 0);
        let me = eng.client_edict(0);
        assert_eq!(eng.ent(me).unwrap().weaponanim, rand.to_bits() as i32);
        assert!(approx(weapon(game, WeaponId::Flashbang).time_weapon_idle, 2.5, 0.011));
    });
}

#[test]
fn holstering_a_grenade_forgets_the_pin_and_pushes_the_next_attack_back() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        kit(game, eng, &[(WeaponId::HeGrenade, 1), (WeaponId::SmokeGrenade, 1)]);
        press(game, eng, IN_ATTACK);
        assert_ne!(weapon(game, WeaponId::HeGrenade).start_throw, 0.0);
        assert!(game.select_weapon(eng, 0, WeaponId::SmokeGrenade));
        let w = weapon(game, WeaponId::HeGrenade).clone();
        assert_eq!((w.start_throw, w.release_throw), (0.0, -1.0));
        // The deploy of the smoke grenade then sets the delay to 0.75 s.
        assert!(approx(game.player_weapon_state(0).unwrap().0, 0.75, 0.011));
        // A grenade the player has none of is destroyed by the holster.
        game.set_player_ammo(0, ammo_index("SmokeGrenade") as usize, 0);
        assert!(game.select_weapon(eng, 0, WeaponId::HeGrenade));
        assert!(!game.carried_weapons(0).contains(&WeaponId::SmokeGrenade));
    });
}

#[test]
fn a_player_killed_holding_fire_and_a_grenade_throws_it() {
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        let me = eng.client_edict(0);
        let v = eng.ent_mut(me).unwrap();
        v.button = i32::from(IN_ATTACK);
        v.angles = Vec3::new(3.0, 90.0, 0.0);
        game.damage_player(eng, 0, Some(1), 500.0, 0);
        let grenades = game.grenades(eng);
        assert_eq!(grenades.len(), 1, "thrown at death");
        let g = eng.ent(grenades[0]).unwrap().clone();
        // QUIRK: `pev->angles` is the velocity.
        assert_eq!(g.velocity, Vec3::new(3.0, 90.0, 0.0));
        assert_eq!(g.owner, Some(me));
        assert_eq!(ammo_of(game, "HEGrenade"), 1, "vanilla does not take the ammo");
    });
    // No fire button, no throw.
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        game.damage_player(eng, 0, Some(1), 500.0, 0);
        assert!(game.grenades(eng).is_empty());
    });
}

#[test]
fn the_bounce_and_the_blast_leave_edict_state_a_round_restart_clears() {
    // A grenade thrown is taken away by the next round (`CleanUpMap`), as the spark showers
    // are not (they end themselves).
    let mut b = arena(Vec3::new(5000.0, 0.0, 0.0));
    b.run(|game, eng| {
        arm(game, eng, WeaponId::HeGrenade, 1);
        throw(game, eng, WeaponId::HeGrenade);
        assert_eq!(game.grenades(eng).len(), 1);
        game.restart_round_now(eng);
        assert!(game.grenades(eng).is_empty());
    });
}

#[test]
fn a_thrown_grenade_bounces_off_the_floor_of_a_real_map_and_goes_off() {
    // On de_dust2 (returns without assertions when the game is not installed): an HE grenade
    // thrown down at the floor of the spawn falls, bounces (its vertical speed turns upward),
    // and goes off 1.55 to 1.85 s after the throw.
    let Some(vfs) = common::discover() else { return };
    let mut b = Bench::on(common::dust2(&vfs));
    let (t0, grenade) = b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        for _ in 0..50 {
            common::run_cmd(game, eng, 0, &GoldSrcUserCmd { msec: 10, ..Default::default() }, 0);
        }
        assert_ne!(eng.ent(eng.client_edict(0)).unwrap().flags & FL_ONGROUND, 0, "settled on the floor");
        arm(game, eng, WeaponId::HeGrenade, 1);
        // Look 60 degrees down: the bent pitch is still down, so the grenade goes into the floor.
        eng.ent_mut(eng.client_edict(0)).unwrap().v_angle = Vec3::new(60.0, 0.0, 0.0);
        throw(game, eng, WeaponId::HeGrenade);
        (eng.time(), game.grenades(eng)[0])
    });
    let mut bounced = false;
    let mut went_down = false;
    let mut exploded_at = None;
    for _ in 0..300 {
        let seen = b.run(move |_, eng| {
            eng.ent(grenade).map(|v| (eng.time(), v.velocity.z, v.effects & EF_NODRAW != 0))
        });
        let Some((time, vz, nodraw)) = seen else { break };
        if vz < -50.0 {
            went_down = true;
        }
        if went_down && vz > 20.0 {
            bounced = true;
        }
        if nodraw && exploded_at.is_none() {
            exploded_at = Some(time);
        }
    }
    assert!(bounced, "it hit the floor and came up again");
    let fuse = exploded_at.expect("it went off") - t0;
    assert!((1.55..=1.85).contains(&fuse), "went off {fuse} s after the throw");
    assert!(b.run(move |_, eng| eng.ent(grenade).is_none()), "gone after the last puff of smoke");
}

#[test]
fn a_grenade_keeps_the_team_of_the_thrower_for_friendly_fire() {
    // `pGrenade->m_iTeam == m_iTeam`: the thrower (a counter-terrorist) changes sides after the
    // throw; its grenade still spares the counter-terrorist next to it (mp_friendlyfire is 0)
    // and hurts the terrorist.
    for (victim_team, hurt) in [(Team::Ct, false), (Team::Terrorist, true)] {
        let mut b = arena(Vec3::new(100.0, 0.0, 1.0));
        b.run(move |game, eng| {
            arm(game, eng, WeaponId::HeGrenade, 1);
            throw(game, eng, WeaponId::HeGrenade);
            let grenade = game.grenades(eng)[0];
            game.state_mut().players[0].as_mut().unwrap().team = Team::Terrorist;
            game.state_mut().players[1].as_mut().unwrap().team = victim_team;
            let at = eng.time() + 0.01;
            let v = eng.ent_mut(grenade).unwrap();
            v.velocity = Vec3::ZERO;
            v.origin = Vec3::ZERO;
            v.dmgtime = 0.0;
            v.nextthink = at;
        });
        b.step(60);
        b.run(move |game, _| {
            assert_eq!(health(game, 1) < 1000.0, hurt, "{victim_team:?}");
        });
    }
}
