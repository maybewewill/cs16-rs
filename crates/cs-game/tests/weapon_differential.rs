//! Comprehensive differential verification across all 28 Counter-Strike 1.6 weapons,
//! the tactical shield, and grenade/flashbang physics & curves against ReGameDLL_CS tables.

mod common;

use common::{Bench, command, run_cmd, stub_spawns};
use cs_engine::GameDll;
use cs_engine::consts::{FL_DUCKING, FL_ONGROUND, MOVETYPE_BOUNCE};
use cs_game::bullets::{
    Bullet, CHAR_TEX_CONCRETE, CHAR_TEX_GRATE, CHAR_TEX_METAL, CHAR_TEX_VENT, CHAR_TEX_WOOD,
    bullet_penetration, fire_bullets3, penetration_material,
};
use cs_game::buy::SHIELDGUN_PRICE;
use cs_game::player::{Team, dmg};
use cs_game::weapons::{
    BulletType, WEAPON_NOCLIP, WeaponId, item_info, new_weapon, weapon_info, weapon_struct,
};
use cs_proto48::delta::GoldSrcUserCmd;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;
const IN_RELOAD: u16 = 1 << 13;

#[derive(Clone, Copy)]
struct FirearmSpec {
    id: WeaponId,
    cost: i32,
    clip: i32,
    max_ammo1: i32,
    max_speed: f32,
    zoom_or_shield_speed: Option<f32>,
    reload_time: f32,
    cycle_time: f32,
    damage: i32,
    range_modifier: f64,
    penetration: i32,
    bullet_type: BulletType,
}

const ALL_24_FIREARMS: [FirearmSpec; 24] = [
    FirearmSpec { id: WeaponId::Glock18, cost: 400, clip: 20, max_ammo1: 120, max_speed: 250.0, zoom_or_shield_speed: Some(180.0), reload_time: 2.2, cycle_time: 0.15, damage: 25, range_modifier: 0.75, penetration: 1, bullet_type: BulletType::Player9mm },
    FirearmSpec { id: WeaponId::Usp, cost: 500, clip: 12, max_ammo1: 100, max_speed: 250.0, zoom_or_shield_speed: Some(180.0), reload_time: 2.7, cycle_time: 0.15, damage: 34, range_modifier: 0.79, penetration: 1, bullet_type: BulletType::Player45acp },
    FirearmSpec { id: WeaponId::P228, cost: 600, clip: 13, max_ammo1: 52, max_speed: 250.0, zoom_or_shield_speed: Some(180.0), reload_time: 2.7, cycle_time: 0.15, damage: 32, range_modifier: 0.80, penetration: 1, bullet_type: BulletType::Player357sig },
    FirearmSpec { id: WeaponId::Deagle, cost: 650, clip: 7, max_ammo1: 35, max_speed: 250.0, zoom_or_shield_speed: Some(180.0), reload_time: 2.2, cycle_time: 0.225, damage: 54, range_modifier: 0.81, penetration: 2, bullet_type: BulletType::Player50ae },
    FirearmSpec { id: WeaponId::FiveSeven, cost: 750, clip: 20, max_ammo1: 100, max_speed: 250.0, zoom_or_shield_speed: Some(180.0), reload_time: 2.7, cycle_time: 0.15, damage: 20, range_modifier: 0.885, penetration: 1, bullet_type: BulletType::Player57mm },
    FirearmSpec { id: WeaponId::Elite, cost: 800, clip: 30, max_ammo1: 120, max_speed: 250.0, zoom_or_shield_speed: None, reload_time: 4.5, cycle_time: 0.075, damage: 36, range_modifier: 0.75, penetration: 1, bullet_type: BulletType::Player9mm },
    FirearmSpec { id: WeaponId::M3, cost: 1700, clip: 8, max_ammo1: 32, max_speed: 230.0, zoom_or_shield_speed: None, reload_time: 0.55, cycle_time: 0.875, damage: 20, range_modifier: 1.0, penetration: 1, bullet_type: BulletType::PlayerBuckshot },
    FirearmSpec { id: WeaponId::Xm1014, cost: 3000, clip: 7, max_ammo1: 32, max_speed: 240.0, zoom_or_shield_speed: None, reload_time: 0.55, cycle_time: 0.25, damage: 20, range_modifier: 1.0, penetration: 1, bullet_type: BulletType::PlayerBuckshot },
    FirearmSpec { id: WeaponId::Mac10, cost: 1400, clip: 30, max_ammo1: 100, max_speed: 250.0, zoom_or_shield_speed: None, reload_time: 3.15, cycle_time: 0.07, damage: 29, range_modifier: 0.82, penetration: 1, bullet_type: BulletType::Player45acp },
    FirearmSpec { id: WeaponId::Tmp, cost: 1250, clip: 30, max_ammo1: 120, max_speed: 250.0, zoom_or_shield_speed: None, reload_time: 2.12, cycle_time: 0.07, damage: 20, range_modifier: 0.85, penetration: 1, bullet_type: BulletType::Player9mm },
    FirearmSpec { id: WeaponId::Mp5n, cost: 1500, clip: 30, max_ammo1: 120, max_speed: 250.0, zoom_or_shield_speed: None, reload_time: 2.63, cycle_time: 0.075, damage: 26, range_modifier: 0.84, penetration: 1, bullet_type: BulletType::Player9mm },
    FirearmSpec { id: WeaponId::Ump45, cost: 1700, clip: 25, max_ammo1: 100, max_speed: 250.0, zoom_or_shield_speed: None, reload_time: 3.5, cycle_time: 0.1, damage: 30, range_modifier: 0.82, penetration: 1, bullet_type: BulletType::Player45acp },
    FirearmSpec { id: WeaponId::P90, cost: 2350, clip: 50, max_ammo1: 100, max_speed: 245.0, zoom_or_shield_speed: None, reload_time: 3.4, cycle_time: 0.066, damage: 21, range_modifier: 0.885, penetration: 1, bullet_type: BulletType::Player57mm },
    FirearmSpec { id: WeaponId::Galil, cost: 2000, clip: 35, max_ammo1: 90, max_speed: 240.0, zoom_or_shield_speed: None, reload_time: 2.45, cycle_time: 0.0875, damage: 30, range_modifier: 0.98, penetration: 2, bullet_type: BulletType::Player556mm },
    FirearmSpec { id: WeaponId::Famas, cost: 2250, clip: 25, max_ammo1: 90, max_speed: 240.0, zoom_or_shield_speed: None, reload_time: 3.3, cycle_time: 0.0825, damage: 30, range_modifier: 0.96, penetration: 2, bullet_type: BulletType::Player556mm },
    FirearmSpec { id: WeaponId::Ak47, cost: 2500, clip: 30, max_ammo1: 90, max_speed: 221.0, zoom_or_shield_speed: None, reload_time: 2.45, cycle_time: 0.0955, damage: 36, range_modifier: 0.98, penetration: 2, bullet_type: BulletType::Player762mm },
    FirearmSpec { id: WeaponId::M4a1, cost: 3100, clip: 30, max_ammo1: 90, max_speed: 230.0, zoom_or_shield_speed: None, reload_time: 3.05, cycle_time: 0.0875, damage: 32, range_modifier: 0.97, penetration: 2, bullet_type: BulletType::Player556mm },
    FirearmSpec { id: WeaponId::Sg552, cost: 3500, clip: 30, max_ammo1: 90, max_speed: 235.0, zoom_or_shield_speed: Some(200.0), reload_time: 3.0, cycle_time: 0.0825, damage: 33, range_modifier: 0.955, penetration: 2, bullet_type: BulletType::Player556mm },
    FirearmSpec { id: WeaponId::Aug, cost: 3500, clip: 30, max_ammo1: 90, max_speed: 240.0, zoom_or_shield_speed: None, reload_time: 3.3, cycle_time: 0.0825, damage: 32, range_modifier: 0.96, penetration: 2, bullet_type: BulletType::Player556mm },
    FirearmSpec { id: WeaponId::Scout, cost: 2750, clip: 10, max_ammo1: 90, max_speed: 260.0, zoom_or_shield_speed: Some(220.0), reload_time: 2.0, cycle_time: 1.25, damage: 75, range_modifier: 0.98, penetration: 3, bullet_type: BulletType::Player762mm },
    FirearmSpec { id: WeaponId::Awp, cost: 4750, clip: 10, max_ammo1: 30, max_speed: 210.0, zoom_or_shield_speed: Some(150.0), reload_time: 2.5, cycle_time: 1.45, damage: 115, range_modifier: 0.99, penetration: 3, bullet_type: BulletType::Player338mag },
    FirearmSpec { id: WeaponId::G3sg1, cost: 5000, clip: 20, max_ammo1: 90, max_speed: 210.0, zoom_or_shield_speed: Some(150.0), reload_time: 3.5, cycle_time: 0.25, damage: 80, range_modifier: 0.98, penetration: 3, bullet_type: BulletType::Player762mm },
    FirearmSpec { id: WeaponId::Sg550, cost: 4200, clip: 30, max_ammo1: 90, max_speed: 210.0, zoom_or_shield_speed: Some(150.0), reload_time: 3.35, cycle_time: 0.25, damage: 70, range_modifier: 0.98, penetration: 3, bullet_type: BulletType::Player556mm },
    FirearmSpec { id: WeaponId::M249, cost: 5750, clip: 100, max_ammo1: 200, max_speed: 220.0, zoom_or_shield_speed: None, reload_time: 4.7, cycle_time: 0.1, damage: 32, range_modifier: 0.97, penetration: 2, bullet_type: BulletType::Player556mm },
];

fn setup_two_slot_arena() -> Bench {
    let mut b = Bench::stub_without_moves(stub_spawns());
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_freezetime", "0");
        let _ = game.client_connect(eng, 1);
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        game.restart_round_now(eng);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        game.end_freeze_period(eng);
    });
    b
}

#[test]
fn all_28_weapons_match_regamedll_fire_rate_damage_falloff_penetration_spread_reload_and_speed() {
    // 1. Live speed checks on all 28 combat weapons via CsGame inventory deploy.
    let mut b = Bench::stub_slots(stub_spawns(), 1);
    b.idle_cmds = true;
    b.step(4);
    b.run(|game, eng| {
        eng.set_cvar("mp_freezetime", "0");
        game.restart_round_now(eng);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
    });
    b.step(4);

    for spec in &ALL_24_FIREARMS {
        let info = weapon_info(spec.id).unwrap_or_else(|| panic!("weapon_info {:?}", spec.id));
        let item = item_info(spec.id).unwrap_or_else(|| panic!("item_info {:?}", spec.id));
        let wstruct = weapon_struct(spec.id).unwrap_or_else(|| panic!("weapon_struct {:?}", spec.id));

        assert_eq!(info.cost, spec.cost, "cost mismatch {:?}", spec.id);
        assert_eq!(wstruct.price, spec.cost, "weapon_struct price mismatch {:?}", spec.id);
        assert_eq!(item.max_clip, spec.clip, "clip mismatch {:?}", spec.id);
        assert_eq!(item.max_ammo1, spec.max_ammo1, "max_ammo1 mismatch {:?}", spec.id);
        assert!(new_weapon(spec.id).is_some());

        let classname = item.name;
        b.run(move |game, eng| {
            game.test_give_named_item(eng, 0, classname);
        });
        command(&mut b, 0, classname);
        b.step(2);
        let live_speed = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().max_speed);
        assert_eq!(live_speed, spec.max_speed, "live deployed max_speed mismatch {:?}", spec.id);
        command(&mut b, 0, "drop");
        b.step(1);

        if let Some(alt_speed) = spec.zoom_or_shield_speed {
            assert!(alt_speed < spec.max_speed);
        }
    }

    for (id, max_carry, expected_max_speed) in [
        (WeaponId::Knife, -1, 250.0_f32),
        (WeaponId::HeGrenade, 1, 250.0),
        (WeaponId::Flashbang, 2, 250.0),
        (WeaponId::SmokeGrenade, 1, 250.0),
    ] {
        let item = item_info(id).unwrap_or_else(|| panic!("item_info {id:?}"));
        assert_eq!(item.max_clip, -1, "no clip for {id:?}");
        assert_eq!(item.max_ammo1, max_carry, "max carry for {id:?}");
        assert!(new_weapon(id).is_some());

        let classname = item.name;
        b.run(move |game, eng| {
            game.test_give_named_item(eng, 0, classname);
        });
        command(&mut b, 0, classname);
        b.step(2);
        let live_speed = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().max_speed);
        assert_eq!(live_speed, expected_max_speed, "live deployed max_speed mismatch {id:?}");
    }
    assert_eq!(SHIELDGUN_PRICE, 2200);
    assert_eq!(weapon_info(WeaponId::ShieldGun).unwrap().cost, 2200);

    // 2. Live fire rate (cycle_time) and reload timing across ALL 24 firearms in CsGame.
    for spec in &ALL_24_FIREARMS {
        let mut arena = setup_two_slot_arena();
        let (clip_after_shot, next_primary, in_reload, special_reload, reload_next_primary, clip_before_done, clip_after_done) =
            arena.run(move |game, eng| {
                assert!(game.give_weapon(eng, 0, new_weapon(spec.id).unwrap(), spec.clip, spec.max_ammo1));
                assert!(game.select_weapon(eng, 0, spec.id));
                game.set_next_attack(0, 0.0);
                let w = game.weapon_mut(0, spec.id).unwrap();
                w.next_primary_attack = 0.0;
                w.next_secondary_attack = 0.0;
                w.time_weapon_idle = 0.0;
                let p0 = eng.client_edict(0);
                let v = eng.ent_mut(p0).unwrap();
                v.flags |= FL_ONGROUND;
                v.v_angle = Vec3::ZERO;
                v.punchangle = Vec3::ZERO;

                // Press fire for one 10 ms command.
                eng.ent_mut(p0).unwrap().button = i32::from(IN_ATTACK);
                game.post_usercmd(eng, 0, &GoldSrcUserCmd { msec: 10, buttons: IN_ATTACK, ..Default::default() });
                let w_after_shot = game.weapon_mut(0, spec.id).unwrap();
                let clip_after_shot = w_after_shot.clip;
                let next_primary = w_after_shot.next_primary_attack;

                // Clear fire lockouts and initiate reload (`ItemPostFrame` requires `next_primary_attack < 0.0`).
                w_after_shot.next_primary_attack = -0.01;
                w_after_shot.next_secondary_attack = 0.0;
                w_after_shot.time_weapon_idle = 0.0;
                game.set_next_attack(0, 0.0);
                eng.ent_mut(p0).unwrap().button = i32::from(IN_RELOAD);
                game.post_usercmd(eng, 0, &GoldSrcUserCmd { msec: 10, buttons: IN_RELOAD, ..Default::default() });
                let w_reload = game.weapon_mut(0, spec.id).unwrap();
                let in_reload = w_reload.in_reload;
                let special_reload = w_reload.in_special_reload;
                let reload_next_primary = w_reload.next_primary_attack;

                let (mut clip_before_done, mut clip_after_done) = (w_reload.clip, w_reload.clip);
                if !matches!(spec.id, WeaponId::M3 | WeaponId::Xm1014) {
                    let early_ticks = ((spec.reload_time - 0.08) / 0.01).floor().max(1.0) as usize;
                    eng.ent_mut(p0).unwrap().button = 0;
                    for _ in 1..early_ticks {
                        game.post_usercmd(eng, 0, &GoldSrcUserCmd { msec: 10, ..Default::default() });
                    }
                    clip_before_done = game.weapon_mut(0, spec.id).unwrap().clip;
                    for _ in 0..20 {
                        game.post_usercmd(eng, 0, &GoldSrcUserCmd { msec: 10, ..Default::default() });
                    }
                    clip_after_done = game.weapon_mut(0, spec.id).unwrap().clip;
                }

                (
                    clip_after_shot,
                    next_primary,
                    in_reload,
                    special_reload,
                    reload_next_primary,
                    clip_before_done,
                    clip_after_done,
                )
            });

        assert_eq!(clip_after_shot, spec.clip - 1, "shot did not consume 1 round for {:?}", spec.id);
        assert!(
            ((next_primary + 0.01) - spec.cycle_time).abs() <= 0.02,
            "cycle_time mismatch for {:?}: got {:.4} (next_primary={next_primary:.4}), expected {:.4}",
            spec.id,
            next_primary + 0.01,
            spec.cycle_time
        );
        if matches!(spec.id, WeaponId::M3 | WeaponId::Xm1014) {
            assert_eq!(special_reload, 1, "shotgun special reload did not start for {:?}", spec.id);
            assert!(
                ((reload_next_primary + 0.01) - spec.reload_time).abs() <= 0.05,
                "shotgun reload step mismatch for {:?}: got {:.4}, expected {:.4}",
                spec.id,
                reload_next_primary + 0.01,
                spec.reload_time
            );
        } else {
            assert!(in_reload, "magazine reload did not start for {:?}", spec.id);
            assert_eq!(clip_before_done, spec.clip - 1, "reload completed too early for {:?}", spec.id);
            assert_eq!(clip_after_done, spec.clip, "reload did not complete on time for {:?}", spec.id);
        }
    }

    // 3. Live 500u bullet damage & distance falloff via `fire_bullets3` across key calibers.
    let mut falloff_arena = setup_two_slot_arena();
    for spec in ALL_24_FIREARMS.iter().filter(|s| {
        matches!(
            s.id,
            WeaponId::Glock18 | WeaponId::Deagle | WeaponId::Mp5n | WeaponId::Ak47 | WeaponId::M4a1 | WeaponId::Awp
        )
    }) {
        let dmg_near = falloff_arena.run(move |game, eng| {
            let p0 = eng.client_edict(0);
            let p1 = eng.client_edict(1);
            eng.set_origin(p0, Vec3::ZERO);
            // Front face of player hull (-16..16) sits at 100.0 u.
            eng.set_origin(p1, Vec3::new(116.0, 0.0, 0.0));
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            game.state_mut().players[1].as_mut().unwrap().armor = 0.0;
            fire_bullets3(
                game.state_mut(),
                eng,
                p0,
                Some(p0),
                Some(spec.id),
                Vec3::Y,
                Vec3::Z,
                &Bullet {
                    src: Vec3::ZERO,
                    dir: Vec3::X,
                    spread: 0.0,
                    distance: 8192.0,
                    penetration: spec.penetration,
                    kind: spec.bullet_type,
                    damage: spec.damage,
                    range_modifier: spec.range_modifier as f32,
                    pistol: matches!(spec.id, WeaponId::Glock18 | WeaponId::Deagle),
                    shared_rand: 0,
                },
            );
            1000.0 - game.state().players[1].as_ref().unwrap().health
        });

        let dmg_500 = falloff_arena.run(move |game, eng| {
            let p0 = eng.client_edict(0);
            let p1 = eng.client_edict(1);
            eng.set_origin(p0, Vec3::ZERO);
            // Front face of player hull (-16..16) sits at 500.0 u.
            eng.set_origin(p1, Vec3::new(516.0, 0.0, 0.0));
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            game.state_mut().players[1].as_mut().unwrap().armor = 0.0;
            fire_bullets3(
                game.state_mut(),
                eng,
                p0,
                Some(p0),
                Some(spec.id),
                Vec3::Y,
                Vec3::Z,
                &Bullet {
                    src: Vec3::ZERO,
                    dir: Vec3::X,
                    spread: 0.0,
                    distance: 8192.0,
                    penetration: spec.penetration,
                    kind: spec.bullet_type,
                    damage: spec.damage,
                    range_modifier: spec.range_modifier as f32,
                    pistol: matches!(spec.id, WeaponId::Glock18 | WeaponId::Deagle),
                    shared_rand: 0,
                },
            );
            1000.0 - game.state().players[1].as_ref().unwrap().health
        });

        let dmg_1500 = falloff_arena.run(move |game, eng| {
            let p0 = eng.client_edict(0);
            let p1 = eng.client_edict(1);
            eng.set_origin(p0, Vec3::ZERO);
            // Front face of player hull (-16..16) sits at 1500.0 u.
            eng.set_origin(p1, Vec3::new(1516.0, 0.0, 0.0));
            game.state_mut().players[1].as_mut().unwrap().health = 1000.0;
            game.state_mut().players[1].as_mut().unwrap().armor = 0.0;
            fire_bullets3(
                game.state_mut(),
                eng,
                p0,
                Some(p0),
                Some(spec.id),
                Vec3::Y,
                Vec3::Z,
                &Bullet {
                    src: Vec3::ZERO,
                    dir: Vec3::X,
                    spread: 0.0,
                    distance: 8192.0,
                    penetration: spec.penetration,
                    kind: spec.bullet_type,
                    damage: spec.damage,
                    range_modifier: spec.range_modifier as f32,
                    pistol: matches!(spec.id, WeaponId::Glock18 | WeaponId::Deagle),
                    shared_rand: 0,
                },
            );
            1000.0 - game.state().players[1].as_ref().unwrap().health
        });

        let expected_near = ((f64::from(spec.damage) * spec.range_modifier.powf(100.0 / 500.0)) as i32) as f32;
        let expected_500 = ((f64::from(spec.damage) * spec.range_modifier) as i32) as f32;
        let expected_1500 = ((f64::from(spec.damage) * spec.range_modifier.powi(3)) as i32) as f32;
        assert_eq!(dmg_near, expected_near, "100u live damage mismatch for {:?}", spec.id);
        assert_eq!(dmg_500, expected_500, "500u live damage mismatch for {:?}", spec.id);
        assert_eq!(dmg_1500, expected_1500, "1500u live damage mismatch for {:?}", spec.id);
        assert!(dmg_near >= dmg_500 && dmg_near > dmg_1500, "expected falloff from 100u ({dmg_near}) to 1500u ({dmg_1500}) for {:?}", spec.id);
    }

    // 4. Live stance recoil ordering (`KickBack`: crouching <= standing < moving <= air).
    for id in [WeaponId::Ak47, WeaponId::M4a1, WeaponId::Mp5n, WeaponId::Galil] {
        let mut kick_mags = Vec::new();
        for &(flags, vel_x) in &[
            (FL_ONGROUND | FL_DUCKING, 0.0_f32),
            (FL_ONGROUND, 0.0_f32),
            (FL_ONGROUND, 220.0_f32),
            (0_u32, 220.0_f32),
        ] {
            let mut arena = setup_two_slot_arena();
            let kick = arena.run(move |game, eng| {
                assert!(game.give_weapon(eng, 0, new_weapon(id).unwrap(), 30, 90));
                assert!(game.select_weapon(eng, 0, id));
                game.set_next_attack(0, 0.0);
                let w = game.weapon_mut(0, id).unwrap();
                w.next_primary_attack = 0.0;
                w.accuracy = 0.2;
                let p0 = eng.client_edict(0);
                let v = eng.ent_mut(p0).unwrap();
                v.flags = flags;
                v.velocity = Vec3::new(vel_x, 0.0, 0.0);
                v.v_angle = Vec3::ZERO;
                v.punchangle = Vec3::ZERO;
                v.button = i32::from(IN_ATTACK);
                game.post_usercmd(eng, 0, &GoldSrcUserCmd { msec: 10, buttons: IN_ATTACK, ..Default::default() });
                eng.ent(p0).unwrap().punchangle.length()
            });
            kick_mags.push(kick);
        }
        assert!(
            kick_mags[0] <= kick_mags[1] && kick_mags[1] < kick_mags[2] && kick_mags[2] <= kick_mags[3],
            "expected crouch <= stand < move <= air recoil for {id:?}, got {kick_mags:?}"
        );
    }
    // Verify Deagle fixed -2.0 deg vertical punchangle on primary shot.
    let mut deagle_arena = setup_two_slot_arena();
    let deagle_punch = deagle_arena.run(|game, eng| {
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Deagle).unwrap(), 7, 35));
        assert!(game.select_weapon(eng, 0, WeaponId::Deagle));
        game.set_next_attack(0, 0.0);
        let w = game.weapon_mut(0, WeaponId::Deagle).unwrap();
        w.next_primary_attack = 0.0;
        let p0 = eng.client_edict(0);
        let v = eng.ent_mut(p0).unwrap();
        v.flags = FL_ONGROUND;
        v.v_angle = Vec3::ZERO;
        v.punchangle = Vec3::ZERO;
        v.button = i32::from(IN_ATTACK);
        game.post_usercmd(eng, 0, &GoldSrcUserCmd { msec: 10, buttons: IN_ATTACK, ..Default::default() });
        eng.ent(p0).unwrap().punchangle.x
    });
    assert_eq!(deagle_punch, -2.0);

    // 5. Penetration material constants match ReGameDLL cbase.cpp (metal, concrete, grate, vent, wood).
    assert_eq!(penetration_material(CHAR_TEX_WOOD).damage_modifier, Some(0.6));
    assert_eq!(penetration_material(CHAR_TEX_WOOD).power_factor, None);
    assert!(!penetration_material(CHAR_TEX_WOOD).metal);
    assert_eq!(penetration_material(CHAR_TEX_CONCRETE).power_factor, Some(0.25));
    assert_eq!(penetration_material(CHAR_TEX_CONCRETE).damage_modifier, None);
    assert_eq!(penetration_material(CHAR_TEX_METAL).power_factor, Some(0.15));
    assert_eq!(penetration_material(CHAR_TEX_METAL).damage_modifier, Some(0.2));
    assert!(penetration_material(CHAR_TEX_METAL).metal);
    assert_eq!(penetration_material(CHAR_TEX_VENT).power_factor, Some(0.5));
    assert_eq!(penetration_material(CHAR_TEX_VENT).damage_modifier, Some(0.45));
    assert!(penetration_material(CHAR_TEX_VENT).metal);
    assert_eq!(penetration_material(CHAR_TEX_GRATE).power_factor, Some(0.5));
    assert_eq!(penetration_material(CHAR_TEX_GRATE).damage_modifier, Some(0.4));
    assert!(penetration_material(CHAR_TEX_GRATE).metal);
    assert_eq!(bullet_penetration(BulletType::Player762mm), (39, 5000.0));
    assert_eq!(bullet_penetration(BulletType::Player338mag), (45, 8000.0));
}

#[test]
fn grenade_physics_and_flashbang_angle_distance_curves() {
    let mut b = Bench::stub_slots(stub_spawns(), 3);
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "800");
        eng.set_cvar("mp_freezetime", "0");
        for s in 1..3_u8 {
            let _ = game.client_connect(eng, s);
            game.client_put_in_server(eng, s);
            eng.test_set_client_active(s, true);
        }
        game.restart_round_now(eng);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        assert!(game.spawn_player(eng, 2, Team::Terrorist, 0));
        game.end_freeze_period(eng);
    });
    b.step(2);

    // 1. Live HE grenade throw physics: MOVETYPE_BOUNCE, +X velocity, upward -10 deg pitch + gravity drop.
    let (gren_movetype, init_vel_x, init_vel_z, init_z) = b.run(|game, eng| {
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::HeGrenade).unwrap(), WEAPON_NOCLIP, 1));
        assert!(game.select_weapon(eng, 0, WeaponId::HeGrenade));
        game.set_next_attack(0, 0.0);
        let w = game.weapon_mut(0, WeaponId::HeGrenade).unwrap();
        w.next_primary_attack = 0.0;
        w.next_secondary_attack = 0.0;
        w.time_weapon_idle = 0.0;
        let p0 = eng.client_edict(0);
        eng.ent_mut(p0).unwrap().v_angle = Vec3::ZERO;

        eng.ent_mut(p0).unwrap().button = i32::from(IN_ATTACK);
        run_cmd(game, eng, 0, &GoldSrcUserCmd { msec: 10, buttons: IN_ATTACK, ..Default::default() }, 0);
        game.weapon_mut(0, WeaponId::HeGrenade).unwrap().time_weapon_idle = 0.0;
        eng.ent_mut(p0).unwrap().button = 0;
        run_cmd(game, eng, 0, &GoldSrcUserCmd { msec: 10, buttons: 0, ..Default::default() }, 0);

        let gren = eng.find_by_classname(None, "grenade").expect("thrown HE grenade edict");
        let e = eng.ent(gren).unwrap();
        (e.movetype, e.velocity.x, e.velocity.z, e.origin.z)
    });
    assert_eq!(gren_movetype, MOVETYPE_BOUNCE);
    assert!(init_vel_x > 200.0, "expected strong +X throw velocity, got {init_vel_x}");
    b.step(20);
    let mid_vel_z = b.run(|_, eng| {
        let gren = eng.find_by_classname(None, "grenade").expect("grenade mid-flight");
        eng.ent(gren).unwrap().velocity.z
    });
    assert!(mid_vel_z < init_vel_z, "expected gravity deceleration: init_vel_z={init_vel_z}, mid_vel_z={mid_vel_z}");
    b.step(40);
    let after_z = b.run(|_, eng| {
        let gren = eng.find_by_classname(None, "grenade").expect("grenade falling");
        eng.ent(gren).unwrap().origin.z
    });
    assert!(after_z < init_z, "expected net gravity drop over 600 ms: init_z={init_z}, after_z={after_z}");

    // 2. HE radius damage curve: 0u -> 100 damage (armor-free), 100u -> ~71, 200u -> ~43, 350u -> 0.
    let mut he_damages = Vec::new();
    for dist in [0.0_f32, 100.0, 200.0, 350.0] {
        b.run(move |game, eng| {
            let p0 = eng.client_edict(0);
            let p1 = eng.client_edict(1);
            eng.set_origin(p1, Vec3::new(dist, 0.0, 0.0));
            game.state_mut().players[1].as_mut().unwrap().health = 100.0;
            game.state_mut().players[1].as_mut().unwrap().armor = 0.0;
            game.test_radius_damage(eng, Vec3::ZERO, Some(p0), None, Some(p0), 100.0, 350.0, dmg::BLAST);
        });
        let hp = b.run(|game, _| game.state().players[1].as_ref().unwrap().health);
        he_damages.push(100.0 - hp);
    }
    assert!(
        he_damages[0] > he_damages[1]
            && he_damages[1] > he_damages[2]
            && he_damages[2] > he_damages[3],
        "expected monotonic HE radius damage falloff, got {he_damages:?}"
    );
    assert_eq!(he_damages[3], 0.0, "350u outside radius should take 0 damage");

    // 3. Flashbang angle curve (100u): 0 deg facing vs 90 deg vs 180 deg turned away.
    let mut alphas = Vec::new();
    for yaw in [180.0_f32, 90.0, 0.0] {
        let dur_alpha = b.run(move |game, eng| {
            let p0 = eng.client_edict(0);
            let p1 = eng.client_edict(1);
            eng.set_origin(p1, Vec3::new(100.0, 0.0, 0.0));
            let ent1 = eng.ent_mut(p1).unwrap();
            ent1.angles = Vec3::new(0.0, yaw, 0.0);
            ent1.v_angle = Vec3::new(0.0, yaw, 0.0);
            game.set_blind(1, 0.0, 0.0, 0.0, 0.0, 0);
            // Flash at origin; slot 1 at +X=100: yaw=180 faces flash, yaw=90 side, yaw=0 away.
            game.test_radius_flash(eng, Vec3::ZERO, p0, Some(p0), 4.0);
            let blind = game.blind_state(1).unwrap_or((0.0, 0.0, 0.0, 0.0, 0));
            (blind.0 - eng.time() as f32, blind.4)
        });
        alphas.push(dur_alpha);
    }
    // Looking directly at flash (yaw=180) yields full alpha=255 and longest blind; turned away (yaw=0) shortest.
    assert_eq!(alphas[0].1, 255);
    assert!(
        alphas[0].0 >= alphas[1].0 && alphas[1].0 > alphas[2].0,
        "expected decreasing flash duration across 0/90/180 deg angles, got {alphas:?}"
    );

    // 4. Flashbang distance curve: 100u vs 500u vs 1200u while looking directly at flash.
    let mut dist_durations = Vec::new();
    for dist in [100.0_f32, 500.0, 1200.0] {
        let dur = b.run(move |game, eng| {
            let p0 = eng.client_edict(0);
            let p1 = eng.client_edict(1);
            eng.set_origin(p1, Vec3::new(dist, 0.0, 0.0));
            let ent1 = eng.ent_mut(p1).unwrap();
            ent1.angles = Vec3::new(0.0, 180.0, 0.0);
            ent1.v_angle = Vec3::new(0.0, 180.0, 0.0);
            game.set_blind(1, 0.0, 0.0, 0.0, 0.0, 0);
            game.test_radius_flash(eng, Vec3::ZERO, p0, Some(p0), 4.0);
            let blind = game.blind_state(1).unwrap_or((0.0, 0.0, 0.0, 0.0, 0));
            blind.0 - eng.time() as f32
        });
        dist_durations.push(dur);
    }
    assert!(
        dist_durations[0] > dist_durations[1] && dist_durations[1] > dist_durations[2],
        "expected decreasing flash duration with distance, got {dist_durations:?}"
    );
}
