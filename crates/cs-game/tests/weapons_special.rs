//! The snipers, the shotguns and the knife (Task 11 of phase 2D): the rows of the tables
//! against `wpn_shared/*.cpp` (ReGameDLL_CS, read 2026-11-22), the zoom of the scopes and its
//! resume after a shot, the special reload of the shotguns, the buckshot cone, the knife's
//! slash, stab and hull reach.
//!
//! The probe is slot 0 (a counter-terrorist), slot 1 a terrorist, on a stub map with no
//! gravity; the commands run through `common::run_cmd` with the bench's 10 ms frame, the
//! buttons set on the edict as the engine does before `CmdStart`.

mod common;

use common::{Bench, spawns};
use cs_engine::{EngineApi, EntTrace, GameDll, MoveFilter};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_game::weapons::knife::find_hull_intersection;
use cs_game::weapons::shotguns::{M3_CONE_VECTOR, XM1014_CONE_VECTOR};
use cs_game::weapons::{
    ITEM_FLAG_NOFIREUNDERWATER, WEAPON_NOCLIP, Weapon, WeaponId, default_give, item_info,
    new_weapon,
};
use cs_proto48::delta::GoldSrcUserCmd;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;
const IN_ATTACK2: u16 = 1 << 11;
const IN_RELOAD: u16 = 1 << 13;

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// One command with `buttons`, as the engine runs it.
fn press(game: &mut CsGame, eng: &mut EngineApi<'_>, buttons: u16) {
    let id = eng.client_edict(0);
    eng.ent_mut(id).unwrap().button = i32::from(buttons);
    common::run_cmd(
        game,
        eng,
        0,
        &GoldSrcUserCmd { msec: 10, buttons, ..Default::default() },
        0,
    );
}

fn weapon(game: &mut CsGame, id: WeaponId) -> &mut Weapon {
    game.weapon_mut(0, id).expect("the player carries the weapon")
}

fn fov(game: &CsGame) -> u8 {
    game.player_weapon_state(0).unwrap().3
}

fn pev_fov(eng: &EngineApi<'_>) -> f32 {
    eng.ent(eng.client_edict(0)).unwrap().fov
}

fn health(game: &CsGame, slot: usize) -> f32 {
    game.state().players[slot].as_ref().unwrap().health
}

/// The probe (slot 0, a counter-terrorist at the origin looking along +X) and a terrorist
/// (slot 1) at `(distance, offset, 17)` (the eye height of the probe), both spawned, the
/// freeze period over, the victim's health at 1000 so that it survives a volley.
fn arena(distance: f32, offset: f32) -> Bench {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        game.client_put_in_server(eng, 1);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        game.end_freeze_period(eng);
        let (me, other) = (eng.client_edict(0), eng.client_edict(1));
        eng.set_origin(me, Vec3::ZERO);
        eng.set_origin(other, Vec3::new(distance, offset, 17.0));
        game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
    });
    b
}

/// Gives the probe weapon `id` with `clip` rounds and `reserve` in reserve, selects it and
/// spends the deploy delay and idle.
fn arm(game: &mut CsGame, eng: &mut EngineApi<'_>, id: WeaponId, clip: i32, reserve: i32) {
    assert!(game.give_weapon(eng, 0, new_weapon(id).unwrap(), clip, reserve));
    assert!(game.select_weapon(eng, 0, id));
    game.set_next_attack(0, 0.0);
    let w = weapon(game, id);
    w.next_primary_attack = 0.0;
    w.next_secondary_attack = 0.0;
    w.time_weapon_idle = 0.0;
}

#[test]
fn sniper_rows_match_source() {
    // GetItemInfo of each (slot, position, ammo, max ammo, clip, flags, weight, name), and the
    // `_MAX_SPEED`/`_DAMAGE` constants of `weapons.h`, and the cones of the shotguns.
    type Row = (WeaponId, i32, i32, Option<&'static str>, i32, i32, i32, i32, &'static str);
    let rows: [Row; 7] = [
        (WeaponId::Scout, 0, 9, Some("762Nato"), 90, 10, 0, 30, "weapon_scout"),
        (WeaponId::Awp, 0, 2, Some("338Magnum"), 30, 10, 0, 30, "weapon_awp"),
        (WeaponId::G3sg1, 0, 3, Some("762Nato"), 90, 20, 0, 20, "weapon_g3sg1"),
        (WeaponId::Sg550, 0, 16, Some("556Nato"), 90, 30, 0, 20, "weapon_sg550"),
        (WeaponId::M3, 0, 5, Some("buckshot"), 32, 8, ITEM_FLAG_NOFIREUNDERWATER, 20, "weapon_m3"),
        (
            WeaponId::Xm1014,
            0,
            12,
            Some("buckshot"),
            32,
            7,
            ITEM_FLAG_NOFIREUNDERWATER,
            20,
            "weapon_xm1014",
        ),
        (WeaponId::Knife, 2, 1, None, -1, WEAPON_NOCLIP, 0, 0, "weapon_knife"),
    ];
    for (id, slot, position, ammo, max_ammo, clip, flags, weight, name) in rows {
        let i = item_info(id).unwrap_or_else(|| panic!("no item info for {id:?}"));
        assert_eq!(
            (i.slot, i.position, i.ammo1, i.max_ammo1, i.max_clip, i.flags, i.weight, i.name),
            (slot, position, ammo, max_ammo, clip, flags, weight, name),
            "{id:?}"
        );
        assert_eq!(i.ammo2, None, "{id:?}");
        assert_eq!(i.id, id);
    }
    // The default clip each spawns with.
    for (id, give) in [
        (WeaponId::Scout, 10),
        (WeaponId::Awp, 10),
        (WeaponId::G3sg1, 20),
        (WeaponId::Sg550, 30),
        (WeaponId::M3, 8),
        (WeaponId::Xm1014, 7),
    ] {
        assert_eq!(default_give(id), Some(give), "{id:?}");
    }
    assert_eq!(default_give(WeaponId::Knife), None);

    // `GetMaxSpeed` of an unzoomed gun, from the behaviour.
    for (id, speed) in [
        (WeaponId::Scout, 260.0),
        (WeaponId::Awp, 210.0),
        (WeaponId::G3sg1, 210.0),
        (WeaponId::Sg550, 210.0),
        (WeaponId::M3, 230.0),
        (WeaponId::Xm1014, 240.0),
    ] {
        let b = new_weapon(id).unwrap();
        assert_eq!(b.max_speed(&Weapon::new(id)), speed, "{id:?}");
    }

    // The cones are the ones in the source files (`Vector(0.0675, 0.0675, 0.0)` and
    // `Vector(0.0725, 0.0725, 0.0)`), not the survey's.
    assert_eq!(M3_CONE_VECTOR, Vec3::new(0.0675, 0.0675, 0.0));
    assert_eq!(XM1014_CONE_VECTOR, Vec3::new(0.0725, 0.0725, 0.0));
}

#[test]
fn zoom_cycle_and_resume() {
    let mut b = arena(300.0, 0.0);
    b.run(|game, eng| {
        // The zoom cycles of each rifle: 90 -> 40 -> 15 (10 for the AWP) -> 90, 0.3 s apart.
        for (id, last, speed, zoom_speed) in [
            (WeaponId::Scout, 15u8, 260.0, 220.0),
            (WeaponId::Awp, 10, 210.0, 150.0),
            (WeaponId::G3sg1, 15, 210.0, 150.0),
            (WeaponId::Sg550, 15, 210.0, 150.0),
        ] {
            arm(game, eng, id, 10, 40);
            assert_eq!(fov(game), 90, "{id:?}");
            assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, speed, "{id:?}");
            let frame = eng.frametime();

            press(game, eng, IN_ATTACK2);
            assert_eq!((fov(game), pev_fov(eng)), (40, 40.0), "{id:?}");
            assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, zoom_speed, "{id:?}");
            assert!(approx(weapon(game, id).next_secondary_attack, 0.3 - frame), "{id:?}");
            // The timer is still running: another press does nothing.
            press(game, eng, IN_ATTACK2);
            assert_eq!(fov(game), 40, "{id:?}");

            weapon(game, id).next_secondary_attack = 0.0;
            press(game, eng, IN_ATTACK2);
            assert_eq!((fov(game), pev_fov(eng)), (last, f32::from(last)), "{id:?}");
            weapon(game, id).next_secondary_attack = 0.0;
            press(game, eng, IN_ATTACK2);
            assert_eq!((fov(game), pev_fov(eng)), (90, 90.0), "{id:?}");
            assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, speed, "{id:?}");
        }

        // The Scout: a shot from the scope closes it (`m_iFOV` only; `pev->fov` stays) and the
        // scope opens again, at the zoom it had, once the primary timer (1.25 s) is spent.
        arm(game, eng, WeaponId::Scout, 10, 40);
        press(game, eng, IN_ATTACK2);
        assert_eq!(fov(game), 40);
        weapon(game, WeaponId::Scout).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Scout).clip, 9);
        assert_eq!(fov(game), 90, "the shot closed the scope");
        assert_eq!(pev_fov(eng), 40.0, "the Scout leaves pev->fov alone");
        let mut frames = 0;
        while fov(game) == 90 {
            press(game, eng, 0);
            frames += 1;
            assert!(frames < 200, "the scope never came back");
        }
        assert_eq!(fov(game), 40);
        assert_eq!(pev_fov(eng), 40.0);
        // 1.25 s of 10 ms frames, the press of the shot included.
        assert!((124..=127).contains(&frames), "{frames}");

        // The AWP: both FOVs go to 90 at the shot, and the zoom comes back after 1.45 s.
        arm(game, eng, WeaponId::Awp, 10, 40);
        press(game, eng, IN_ATTACK2);
        press(game, eng, 0);
        weapon(game, WeaponId::Awp).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        assert_eq!((fov(game), pev_fov(eng)), (10, 10.0));
        weapon(game, WeaponId::Awp).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Awp).clip, 9);
        assert_eq!((fov(game), pev_fov(eng)), (90, 90.0));
        let mut frames = 0;
        while fov(game) == 90 {
            press(game, eng, 0);
            frames += 1;
            assert!(frames < 200);
        }
        assert_eq!((fov(game), pev_fov(eng)), (10, 10.0));
        assert!((143..=147).contains(&frames), "{frames}");

        // A click on an empty Scout closes the scope all the same (the test is before the
        // clip test).
        arm(game, eng, WeaponId::Scout, 0, 0);
        weapon(game, WeaponId::Scout).fire_on_empty = false;
        press(game, eng, IN_ATTACK2);
        assert_eq!(fov(game), 40);
        weapon(game, WeaponId::Scout).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(fov(game), 90);
        assert_eq!(weapon(game, WeaponId::Scout).clip, 0);

        // The G3SG1 and the SG550 shoot from the scope without closing it.
        for id in [WeaponId::G3sg1, WeaponId::Sg550] {
            arm(game, eng, id, 10, 40);
            press(game, eng, IN_ATTACK2);
            assert_eq!(fov(game), 40, "{id:?}");
            weapon(game, id).next_secondary_attack = 0.0;
            press(game, eng, IN_ATTACK);
            assert_eq!(weapon(game, id).clip, 9, "{id:?}");
            assert_eq!(fov(game), 40, "{id:?}");
        }

        // A reload in the scope closes it (the FOV is set to the last step and the zoom
        // cycle run once more): all four guns.
        for id in [WeaponId::Scout, WeaponId::Awp, WeaponId::G3sg1, WeaponId::Sg550] {
            arm(game, eng, id, 4, 40);
            press(game, eng, IN_ATTACK2);
            assert_eq!(fov(game), 40, "{id:?}");
            let w = weapon(game, id);
            w.next_secondary_attack = 0.0;
            w.next_primary_attack = -0.5;
            press(game, eng, IN_RELOAD);
            assert!(weapon(game, id).in_reload, "{id:?}");
            assert_eq!((fov(game), pev_fov(eng)), (90, 90.0), "{id:?}");
        }
    });
}

#[test]
fn sniper_shots_by_source() {
    // The victim's box starts 24 units ahead: the widest spread of an airborne shooter (the
    // bench has no floor) is 8 units off the line there, so every shot lands.
    let mut b = arena(40.0, 0.0);
    b.run(|game, eng| {
        let frame = eng.frametime();
        // Each sniper takes one round, waits its cycle time and its idle, and the damage of
        // its row, less the fall-off of the range modifier over 24 units, lands on the victim.
        for (id, cycle, idle, damage, range) in [
            (WeaponId::Scout, 1.25_f32, 1.8_f32, 75.0_f64, 0.98_f64),
            (WeaponId::Awp, 1.45, 2.0, 115.0, 0.99),
            (WeaponId::G3sg1, 0.25, 1.8, 80.0, 0.98),
            (WeaponId::Sg550, 0.25, 1.8, 70.0, 0.98),
        ] {
            arm(game, eng, id, 10, 40);
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            let before = health(game, 1);
            // Zoom first for the G3SG1 and the SG550 so that the shot is in the scope; the
            // others are shot from the scope too (the spread of the scope is the smaller).
            press(game, eng, IN_ATTACK2);
            weapon(game, id).next_secondary_attack = 0.0;
            press(game, eng, IN_ATTACK);
            let w = weapon(game, id);
            assert_eq!(w.clip, 9, "{id:?}");
            assert!(approx(w.next_primary_attack, cycle - frame), "{id:?} {}", w.next_primary_attack);
            assert!(approx(w.next_secondary_attack, cycle - frame), "{id:?}");
            assert!(approx(w.time_weapon_idle, idle - frame), "{id:?} {}", w.time_weapon_idle);
            // `int(damage * pow(range modifier, distance / 500))`, a generic hit group.
            let lost = before - health(game, 1);
            let expected = (damage * range.powf(24.0 / 500.0)) as i32 as f32;
            assert_eq!(lost, expected, "{id:?}");
        }
    });
}

#[test]
fn shotgun_special_reload_and_pump() {
    let mut b = arena(300.0, 0.0);
    b.run(|game, eng| {
        let frame = eng.frametime();
        // (weapon, the delay of one shell, the pump animation)
        for (id, shell, pump) in [(WeaponId::M3, 0.45_f32, 4), (WeaponId::Xm1014, 0.3, 4)] {
            let max = if id == WeaponId::M3 { 8 } else { 7 };
            arm(game, eng, id, max - 3, 20);
            let ammo = game.player_ammo(0, cs_game::weapons::ammo_index("buckshot") as usize).unwrap();
            assert_eq!(ammo, 20);
            weapon(game, id).next_primary_attack = -0.5;

            // Stage 0: the start of the reload, 0.55 s on every timer.
            press(game, eng, IN_RELOAD);
            let w = weapon(game, id);
            assert_eq!(w.in_special_reload, 1, "{id:?}");
            assert!(approx(w.time_weapon_idle, 0.55 - frame), "{id:?}");
            assert!(approx(w.next_secondary_attack, 0.55 - frame), "{id:?}");
            assert!(approx(w.next_primary_attack, 0.55 - frame), "{id:?}");
            assert!(approx(game.player_weapon_state(0).unwrap().0, 0.55 - frame), "{id:?}");
            assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 5, "start reload");

            // Run on with the buttons up: a shell goes in per `shell` seconds until the clip
            // is full, then the gun pumps.
            let mut clips = vec![weapon(game, id).clip];
            let mut anims = Vec::new();
            let mut stage2_idle = None;
            for _ in 0..400 {
                press(game, eng, 0);
                let w = weapon(game, id);
                if clips.last() != Some(&w.clip) {
                    clips.push(w.clip);
                }
                if w.in_special_reload == 2 && stage2_idle.is_none() {
                    stage2_idle = Some(w.time_weapon_idle);
                }
                let anim = eng.ent(eng.client_edict(0)).unwrap().weaponanim;
                if anims.last() != Some(&anim) {
                    anims.push(anim);
                }
                if weapon(game, id).in_special_reload == 0 {
                    break;
                }
            }
            assert_eq!(clips, vec![max - 3, max - 2, max - 1, max], "{id:?}");
            assert_eq!(weapon(game, id).in_special_reload, 0, "{id:?}");
            // The shells came out of the reserve: 3 of 20.
            let ammo = game.player_ammo(0, cs_game::weapons::ammo_index("buckshot") as usize).unwrap();
            assert_eq!(ammo, 17, "{id:?}");
            // Stage 1 -> 2 set the idle to the shell's delay.
            assert!(approx(stage2_idle.unwrap(), shell - frame), "{id:?}: {stage2_idle:?}");
            // The animations: start (5), insert (3), ..., pump (4).
            assert!(anims.contains(&3) && anims.contains(&pump), "{id:?}: {anims:?}");
            assert_eq!(*anims.last().unwrap(), pump, "{id:?}: {anims:?}");
            let w = weapon(game, id);
            assert!(approx(w.time_weapon_idle, 1.5 - frame), "{id:?}: {}", w.time_weapon_idle);
        }

        // A shot in the middle of the reload ends it (`m_fInSpecialReload = 0`).
        arm(game, eng, WeaponId::M3, 5, 20);
        weapon(game, WeaponId::M3).next_primary_attack = -0.5;
        press(game, eng, IN_RELOAD);
        assert_eq!(weapon(game, WeaponId::M3).in_special_reload, 1);
        let w = weapon(game, WeaponId::M3);
        w.next_primary_attack = 0.0;
        game.set_next_attack(0, 0.0);
        press(game, eng, IN_ATTACK);
        let w = weapon(game, WeaponId::M3);
        assert_eq!(w.clip, 4);
        assert_eq!(w.in_special_reload, 0);

        // A dry click: the empty gun starts a reload instead and waits a second.
        arm(game, eng, WeaponId::Xm1014, 0, 20);
        let w = weapon(game, WeaponId::Xm1014);
        w.next_primary_attack = 0.0;
        w.fire_on_empty = true;
        press(game, eng, IN_ATTACK);
        let w = weapon(game, WeaponId::Xm1014);
        assert_eq!(w.in_special_reload, 1, "the reload started");
        assert!(approx(w.next_primary_attack, 1.0 - frame), "{}", w.next_primary_attack);
    });
}

#[test]
fn shotgun_fire_timing() {
    let mut b = arena(40.0, 0.0);
    b.run(|game, eng| {
        let frame = eng.frametime();
        // (weapon, cycle, idle, pump time, pellets)
        for (id, cycle, idle, pump, pellets) in [
            (WeaponId::M3, 0.875_f32, 2.5_f32, 0.5_f32, 9.0_f32),
            (WeaponId::Xm1014, 0.25, 2.25, 0.125, 6.0),
        ] {
            arm(game, eng, id, 5, 20);
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            let before = health(game, 1);
            press(game, eng, IN_ATTACK);
            let w = weapon(game, id);
            assert_eq!(w.clip, 4, "{id:?}");
            assert!(approx(w.next_primary_attack, cycle - frame), "{id:?}");
            assert!(approx(w.next_secondary_attack, cycle - frame), "{id:?}");
            assert!(approx(w.time_weapon_idle, idle - frame), "{id:?}");
            assert_eq!(w.pump_time, pump, "{id:?}");
            assert_eq!(w.in_special_reload, 0);
            // Point blank, every pellet hits: `int((1 - fraction) * 20)` each, 19 at 24 units.
            let lost = before - health(game, 1);
            assert_eq!(lost, pellets * 19.0, "{id:?}");
            // The recoil lifted the view; the brass is due.
            assert!(eng.ent(eng.client_edict(0)).unwrap().punchangle.x < -2.0, "{id:?}");
        }
        // The last round: the idle is short and no pump is due.
        arm(game, eng, WeaponId::M3, 1, 20);
        weapon(game, WeaponId::M3).pump_time = 0.0;
        press(game, eng, IN_ATTACK);
        let w = weapon(game, WeaponId::M3);
        assert_eq!(w.clip, 0);
        assert_eq!(w.pump_time, 0.0);
        assert!(approx(w.time_weapon_idle, 0.875 - frame));
    });
}

/// Fires `volleys` volleys of weapon `id` at the victim of [`arena`] and counts the volleys
/// that hurt it.
fn volleys_that_hit(id: WeaponId, volleys: usize, offset: f32) -> usize {
    let mut b = arena(500.0, offset);
    b.run(move |game, eng| {
        arm(game, eng, id, 8, 20);
        let mut hits = 0;
        for _ in 0..volleys {
            let w = weapon(game, id);
            w.clip = 8;
            w.next_primary_attack = 0.0;
            w.next_secondary_attack = 0.0;
            w.in_special_reload = 0;
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            // The shot goes along `v_angle + punchangle`: no leftover recoil.
            let me = eng.client_edict(0);
            eng.ent_mut(me).unwrap().punchangle = Vec3::ZERO;
            press(game, eng, IN_ATTACK);
            if health(game, 1) < 1000.0 {
                hits += 1;
            }
        }
        hits
    })
}

#[test]
fn buckshot_spread() {
    // At 500 units the pellets of the M3 stay within `0.0675 * 500 = 33.75` of the aim line
    // (the cone is a circle of radius 1 times the constant); the victim's box reaches up to
    // 516 along the line, where the widest pellet is at `0.0675 * 516 = 34.8`. A victim whose
    // near side edge is at 35.5 is never hit; the XM1014's wider cone (`0.0725`) would be
    // (a pellet at 37.4), but only the rare ones.
    let m3 = volleys_that_hit(WeaponId::M3, 600, 16.0 + 35.5);
    assert_eq!(m3, 0, "no M3 pellet strays past its cone");
    let xm = volleys_that_hit(WeaponId::Xm1014, 600, 16.0 + 38.0);
    assert_eq!(xm, 0, "no XM1014 pellet strays past its cone");

    // Inside the cone the pellets do land: at 80 % of it the victim is hit by a good part of
    // the volleys (an M3 volley is nine pellets).
    let m3_inside = volleys_that_hit(WeaponId::M3, 300, 16.0 + 0.6 * 33.75);
    assert!(m3_inside > 30, "{m3_inside}");
    let xm_inside = volleys_that_hit(WeaponId::Xm1014, 300, 16.0 + 0.6 * 36.25);
    assert!(xm_inside > 20, "{xm_inside}");

    // A victim far off to the side is never hit by a straight aim: control.
    assert_eq!(volleys_that_hit(WeaponId::M3, 50, 200.0), 0);
}

/// The victim looks along `yaw`.
fn face(eng: &mut EngineApi<'_>, yaw: f32) {
    let other = eng.client_edict(1);
    eng.ent_mut(other).unwrap().angles = Vec3::new(0.0, yaw, 0.0);
}

#[test]
fn knife_slash_and_stab_damage_and_timing() {
    let mut b = arena(40.0, 0.0);
    b.run(|game, eng| {
        let frame = eng.frametime();
        arm(game, eng, WeaponId::Knife, WEAPON_NOCLIP, 0);
        weapon(game, WeaponId::Knife).swing = 0;
        let hurt = |game: &CsGame| 1000.0 - health(game, 1);
        let reset = |game: &mut CsGame| {
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            let w = weapon(game, WeaponId::Knife);
            w.next_primary_attack = 0.0;
            w.next_secondary_attack = 0.0;
            w.time_weapon_idle = 0.0;
        };
        // The deploy: speed 250 and the player's own 0.75 s are spent by `arm`.
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().max_speed, 250.0);

        // A slash that hits: 15 damage, 0.4 s on the primary, 0.5 s on the secondary, the idle
        // 2 s, the first of the two animations.
        face(eng, 180.0);
        press(game, eng, IN_ATTACK);
        assert_eq!(hurt(game), 15.0);
        let w = weapon(game, WeaponId::Knife);
        assert!(approx(w.next_primary_attack, 0.4 - frame), "{}", w.next_primary_attack);
        assert!(approx(w.next_secondary_attack, 0.5 - frame));
        assert!(approx(w.time_weapon_idle, 2.0 - frame));
        assert_eq!(w.swing, 1);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 6);

        // The second slash uses the other animation.
        reset(game);
        press(game, eng, IN_ATTACK);
        assert_eq!(hurt(game), 15.0);
        assert_eq!(weapon(game, WeaponId::Knife).swing, 2);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 7);

        // A slash into the air: 0.35 s, the secondary 0.5 s, the idle 2 s; the swing counter
        // moves on all the same.
        reset(game);
        let other = eng.client_edict(1);
        eng.set_origin(other, Vec3::new(300.0, 0.0, 17.0));
        press(game, eng, IN_ATTACK);
        assert_eq!(hurt(game), 0.0);
        let w = weapon(game, WeaponId::Knife);
        assert!(approx(w.next_primary_attack, 0.35 - frame), "{}", w.next_primary_attack);
        assert!(approx(w.next_secondary_attack, 0.5 - frame));
        assert!(approx(w.time_weapon_idle, 2.0 - frame));
        assert_eq!(w.swing, 3);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 6);

        // The stab: 65 damage to a victim that faces the knife, 1.1 s on both timers.
        eng.set_origin(other, Vec3::new(40.0, 0.0, 17.0));
        reset(game);
        face(eng, 180.0);
        press(game, eng, IN_ATTACK2);
        assert_eq!(hurt(game), 65.0);
        let w = weapon(game, WeaponId::Knife);
        assert!(approx(w.next_primary_attack, 1.1 - frame), "{}", w.next_primary_attack);
        assert!(approx(w.next_secondary_attack, 1.1 - frame));
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 4);

        // In the back (the victim looks the way the knife does): three times the damage.
        reset(game);
        face(eng, 0.0);
        press(game, eng, IN_ATTACK2);
        assert_eq!(hurt(game), 195.0);

        // Across (a right angle): no bonus.
        reset(game);
        face(eng, 90.0);
        press(game, eng, IN_ATTACK2);
        assert_eq!(hurt(game), 65.0);

        // A stab out of reach: the reach is 32 units and the hull of the trace is 16 wide
        // ahead, so the near face of the box at 56 units is out; the miss waits 1 s on both
        // timers.
        reset(game);
        eng.set_origin(other, Vec3::new(72.0, 0.0, 17.0));
        press(game, eng, IN_ATTACK2);
        assert_eq!(hurt(game), 0.0);
        let w = weapon(game, WeaponId::Knife);
        assert!(approx(w.next_primary_attack, 1.0 - frame), "{}", w.next_primary_attack);
        assert!(approx(w.next_secondary_attack, 1.0 - frame));
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 5);
        // ... but the slash reaches 48 (plus the hull's 16): the near face at 50 units is in
        // its reach and out of the stab's (32 + 16).
        reset(game);
        eng.set_origin(other, Vec3::new(66.0, 0.0, 17.0));
        press(game, eng, IN_ATTACK);
        assert_eq!(hurt(game), 15.0);
        reset(game);
        press(game, eng, IN_ATTACK2);
        assert_eq!(hurt(game), 0.0, "the stab's reach is 32");

        // The knife idles with its own animation and cannot be dropped.
        let w = weapon(game, WeaponId::Knife);
        w.time_weapon_idle = 0.0;
        w.next_primary_attack = 0.0;
        eng.set_origin(other, Vec3::new(300.0, 0.0, 17.0));
        press(game, eng, 0);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, 0);
        assert!(approx(weapon(game, WeaponId::Knife).time_weapon_idle, 20.0 - frame));
    });
}

#[test]
fn knife_hull_intersection() {
    let mut b = arena(40.0, 0.0);
    b.run(|game, eng| {
        let me = eng.client_edict(0);
        let other = eng.client_edict(1);
        let src = Vec3::new(0.0, 0.0, 17.0);

        // The hull of the slash reaches a victim that stands beside the line: the line (48
        // units along +X) passes it, the box of the head hull (16 to either side) does not.
        arm(game, eng, WeaponId::Knife, WEAPON_NOCLIP, 0);
        eng.set_origin(other, Vec3::new(40.0, 24.0, 17.0));
        let line = eng.trace_line(src, src + Vec3::new(48.0, 0.0, 0.0), MoveFilter::Normal, Some(me));
        assert_eq!(line.trace.fraction, 1.0, "the line misses");
        game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
        face(eng, 180.0);
        press(game, eng, IN_ATTACK);
        assert_eq!(1000.0 - health(game, 1), 15.0, "the hull found the victim");
        assert_eq!(weapon(game, WeaponId::Knife).swing, 1);

        // `FindHullIntersection`, a hull hit that ends beyond a victim straight ahead: the line
        // through twice the way finds it, and replaces the trace.
        eng.set_origin(other, Vec3::new(50.0, 0.0, 17.0));
        let mut tr = eng.trace_line(src, Vec3::new(24.0, 0.0, 17.0), MoveFilter::NoMonsters, Some(me));
        tr.trace.fraction = 0.5;
        tr.trace.end_pos = Vec3::new(24.0, 0.0, 17.0);
        tr.ent = None;
        find_hull_intersection(
            eng,
            src,
            &mut tr,
            Vec3::new(-16.0, -16.0, -18.0),
            Vec3::new(16.0, 16.0, 32.0),
            me,
        );
        assert!(tr.trace.fraction < 1.0);
        assert_eq!(tr.ent, Some(other), "the line through twice the way hit the victim");
        assert!((tr.trace.end_pos.x - 34.0).abs() < 1.0, "{:?}", tr.trace.end_pos);

        // A victim the straight line passes but a corner of the box does not: the nearest
        // corner's hit wins (the lower one, 15.75 units from the line's height).
        eng.set_origin(other, Vec3::new(50.0, 30.0, 17.0));
        let mut tr = EntTrace { trace: tr.trace, ent: None };
        tr.trace.end_pos = Vec3::new(24.0, 0.0, 17.0);
        tr.trace.fraction = 0.5;
        let straight = eng.trace_line(src, Vec3::new(48.0, 0.0, 17.0), MoveFilter::Normal, Some(me));
        assert_eq!(straight.trace.fraction, 1.0);
        find_hull_intersection(
            eng,
            src,
            &mut tr,
            Vec3::new(-16.0, -16.0, -18.0),
            Vec3::new(16.0, 16.0, 32.0),
            me,
        );
        assert_eq!(tr.ent, Some(other));
        assert!(tr.trace.end_pos.z < 17.0, "the corner below the line is the nearest: {:?}", tr.trace.end_pos);

        // Nothing there: the trace is left as it was.
        eng.set_origin(other, Vec3::new(900.0, 900.0, 17.0));
        let mut tr = EntTrace { trace: tr.trace, ent: Some(me) };
        tr.trace.end_pos = Vec3::new(24.0, 0.0, 17.0);
        tr.trace.fraction = 0.5;
        find_hull_intersection(
            eng,
            src,
            &mut tr,
            Vec3::new(-16.0, -16.0, -18.0),
            Vec3::new(16.0, 16.0, 32.0),
            me,
        );
        assert_eq!(tr.ent, Some(me));
        assert_eq!(tr.trace.fraction, 0.5);
        assert_eq!(tr.trace.end_pos, Vec3::new(24.0, 0.0, 17.0));
    });
}
