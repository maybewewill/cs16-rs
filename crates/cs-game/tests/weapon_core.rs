//! The weapon state machine (`CBasePlayerWeapon::ItemPostFrame` and the helpers every weapon
//! calls, ReGameDLL_CS `weapons.cpp`, read 2026-11-22) on a test weapon: the timers, the attack
//! and reload rules, the deploy, the reloads, the recoil, the bursts, the client's HUD.
//!
//! The commands run through `common::run_cmd` with the frame of the bench (10 ms), and the
//! tests set the buttons on the edict as the engine does before `CmdStart`.

mod common;

use common::{Bench, spawns};
use cs_engine::{EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_game::weapons::core::{WeaponCtx, can_attack, has_secondary_attack_by_id};
use cs_game::weapons::{
    WEAPONS_PRECACHE_ORDER, WeaponBehavior, WeaponId, ammo_index, new_weapon,
};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;
const IN_ATTACK2: u16 = 1 << 11;
const IN_RELOAD: u16 = 1 << 13;

/// A weapon that counts what the state machine asks of it: the fields the machine does not touch
/// serve as counters (`glock18_shots_fired` the primary attacks, `famas_shots_fired` the
/// secondary ones, `swing` the idles, `default_ammo` the reloads).
#[derive(Clone)]
struct TestGun(WeaponId);

impl WeaponBehavior for TestGun {
    fn id(&self) -> WeaponId {
        self.0
    }
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.default_deploy(&*self, "models/v_usp.mdl", "models/p_usp.mdl", 6, "onehanded", true)
    }
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.weapon.glock18_shots_fired += 1;
        if w.weapon.clip <= 0 {
            w.play_empty_sound();
            w.weapon.next_primary_attack = w.get_next_attack_delay(0.2);
            return;
        }
        w.weapon.clip -= 1;
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.15);
        // Recoil, with a direction that never changes by chance.
        w.weapon.shots_fired += 1;
        w.kick_back(1.0, 0.5, 0.25, 0.125, 3.0, 1.0, 1_000_000);
    }
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.weapon.famas_shots_fired += 1;
    }
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.weapon.default_ammo += 1;
        let max = w.max_clip();
        w.default_reload(max, 5, 2.0);
    }
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        w.weapon.swing += 1;
    }
}

/// A shotgun (an M3) that reloads with `DefaultShotgunReload`.
#[derive(Clone)]
struct TestShotgun;

impl WeaponBehavior for TestShotgun {
    fn id(&self) -> WeaponId {
        WeaponId::M3
    }
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.default_deploy(&*self, "models/v_m3.mdl", "models/p_m3.mdl", 6, "shotgun", true)
    }
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.weapon.default_ammo += 1;
        w.default_shotgun_reload(2, 1, 0.45, 0.55, Some("weapons/reload1.wav"), None);
    }
}

fn cmd() -> GoldSrcUserCmd {
    GoldSrcUserCmd { msec: 10, ..Default::default() }
}

/// One command with `buttons`, as the engine runs it: `v.button` is set before `CmdStart`.
fn press(game: &mut CsGame, eng: &mut EngineApi<'_>, buttons: u16) {
    let id = eng.client_edict(0);
    eng.ent_mut(id).unwrap().button = i32::from(buttons);
    common::run_cmd(game, eng, 0, &GoldSrcUserCmd { buttons, ..cmd() }, 0);
}

fn weapon(game: &mut CsGame, id: WeaponId) -> &mut cs_game::weapons::Weapon {
    game.weapon_mut(0, id).expect("the player carries the weapon")
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

/// A stub-map server whose probe is a counter-terrorist out of the freeze period, carrying
/// `TestGun(id)` with 12 rounds loaded and 36 in reserve, selected, with the deploy delay spent.
fn armed(id: WeaponId) -> Bench {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        assert!(game.give_weapon(eng, 0, Box::new(TestGun(id)), 12, 36));
        assert!(game.select_weapon(eng, 0, id));
        game.set_next_attack(0, 0.0);
        let w = weapon(game, id);
        w.time_weapon_idle = 0.0;
    });
    b
}

#[test]
fn deploy_sets_next_attack_0_75() {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        assert!(game.give_weapon(eng, 0, Box::new(TestGun(WeaponId::Usp)), 12, 36));
        assert_eq!(game.player_weapon_state(0).unwrap().2, None, "nothing is selected by a give");
        assert!(game.select_weapon(eng, 0, WeaponId::Usp));
        assert!(!game.select_weapon(eng, 0, WeaponId::Usp), "it is the active item already");
        let now = eng.time() as f32;
        let (next_attack, _, active, fov, _) = game.player_weapon_state(0).unwrap();
        // `DefaultDeploy`: `m_flNextAttack = 0.75` (a countdown), the idle 1.5, the zoom reset.
        assert_eq!(next_attack, 0.75);
        assert_eq!(active, Some(WeaponId::Usp));
        assert_eq!(fov, 90);
        let w = weapon(game, WeaponId::Usp);
        assert_eq!(w.time_weapon_idle, 1.5);
        assert_eq!(w.last_fire_time, 0.0);
        assert_eq!(w.decrease_shots_fired, now);
        assert_eq!(w.model_name, "models/v_usp.mdl");
        let v = eng.ent(eng.client_edict(0)).unwrap();
        assert_eq!(v.weaponanim, 6);
        assert_eq!(v.fov, 90.0);
        // `SelectItem` ends with `ResetMaxSpeed`: the weapon's speed.
        assert_eq!(v.max_speed, 250.0);
    });
}

#[test]
fn item_post_frame_timeline() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        let ft = eng.frametime();
        assert!((ft - 0.01).abs() < 1e-4, "the bench runs 10 ms frames: {ft}");
        let usp = WeaponId::Usp;

        // The attack takes a timer that is spent (`<= 0`) but not one that is still running.
        weapon(game, usp).next_primary_attack = 0.001;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, usp).glock18_shots_fired, 0);
        assert!(approx(weapon(game, usp).next_primary_attack, 0.001 - ft));
        press(game, eng, IN_ATTACK);
        let w = weapon(game, usp);
        assert_eq!(w.glock18_shots_fired, 1, "the timer ran out");
        assert_eq!(w.clip, 11);
        // `GetNextAttackDelay(0.15)` is the delay itself, then `PostThink` takes a frame off.
        assert_eq!(w.next_primary_attack, 0.15_f32 - ft);
        // Held: nothing more until the timer is spent.
        for _ in 0..5 {
            press(game, eng, IN_ATTACK);
        }
        assert_eq!(weapon(game, usp).glock18_shots_fired, 1);
        // Exactly 0 attacks (the `<=` of `CanAttack`).
        weapon(game, usp).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, usp).glock18_shots_fired, 2);
        assert_eq!(weapon(game, usp).clip, 10);

        // The reload wants the primary timer STRICTLY below 0.
        weapon(game, usp).next_primary_attack = 0.0;
        let reloads = weapon(game, usp).default_ammo;
        press(game, eng, IN_RELOAD);
        assert_eq!(weapon(game, usp).default_ammo, reloads, "0 is not below 0");
        assert_eq!(weapon(game, usp).next_primary_attack, -ft);
        press(game, eng, IN_RELOAD);
        assert_eq!(weapon(game, usp).default_ammo, reloads + 1);
        // `DefaultReload`: `m_flNextAttack` is the delay itself, the idle `delay + 0.5`.
        let (next_attack, ..) = game.player_weapon_state(0).unwrap();
        assert_eq!(next_attack, 2.0_f32 - ft);
        let w = weapon(game, usp);
        assert!(w.in_reload);
        assert_eq!(w.time_weapon_idle, 2.5_f32 - ft);
        // While the player's `m_flNextAttack` runs, `ItemPostFrame` does not run at all.
        let idles = w.swing;
        press(game, eng, 0);
        assert_eq!(weapon(game, usp).swing, idles);
        assert!(weapon(game, usp).in_reload);
        // The reload completes when `m_flNextAttack <= 0`: the clip takes what fits.
        let ammo = ammo_index("45ACP") as usize;
        let reserve = game.player_ammo(0, ammo).unwrap();
        assert_eq!(reserve, 36);
        game.set_next_attack(0, 0.0);
        press(game, eng, 0);
        let w = weapon(game, usp);
        assert!(!w.in_reload);
        assert_eq!(w.clip, 12);
        assert_eq!(game.player_ammo(0, ammo), Some(34));

        // No fire button: the idle runs (and the empty sound is allowed again).
        let idles = weapon(game, usp).swing;
        press(game, eng, 0);
        assert_eq!(weapon(game, usp).swing, idles + 1);
        assert!(weapon(game, usp).play_empty_sound);

        // A pistol's shots fired go back to 0 when the buttons are up.
        weapon(game, usp).shots_fired = 5;
        press(game, eng, 0);
        assert_eq!(weapon(game, usp).shots_fired, 0);

        // An empty clip reloads by itself once the primary timer is below 0, and the idle is
        // then not run (`Reload(); return;`).
        let w = weapon(game, usp);
        w.clip = 0;
        w.next_primary_attack = -0.5;
        w.in_reload = false;
        let (reloads, idles) = (w.default_ammo, w.swing);
        game.set_next_attack(0, 0.0);
        press(game, eng, 0);
        assert_eq!(weapon(game, usp).default_ammo, reloads + 1);
        assert_eq!(weapon(game, usp).swing, idles);

        // The secondary attack has its own timer and clears its button on the edict.
        game.set_next_attack(0, 0.0);
        let w = weapon(game, usp);
        w.in_reload = false;
        w.next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        assert_eq!(weapon(game, usp).famas_shots_fired, 1);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().button & i32::from(IN_ATTACK2), 0);
    });
}

#[test]
fn can_attack_and_secondary_attack_tables() {
    // `CanAttack` with `CLIENT_WEAPONS`: a predicted weapon waits for 0, others for the time.
    assert!(can_attack(0.0, 0.0, true));
    assert!(!can_attack(0.001, 0.0, true));
    assert!(can_attack(-1.0, 0.0, false));
    assert!(can_attack(5.0, 5.0, false));
    assert!(!can_attack(5.1, 5.0, false));
    // `HasSecondaryAttack`: false for these, true for the rest (vanilla: the UMP45 has one).
    use WeaponId::*;
    for id in [Ak47, Xm1014, Mac10, Elite, FiveSeven, Mp5n, M249, M3, Tmp, Deagle, P228, P90, C4, Galil] {
        assert!(!has_secondary_attack_by_id(id), "{id:?}");
    }
    for id in [Ump45, Usp, Glock18, Famas, Aug, Sg552, M4a1, Scout, Awp, G3sg1, Sg550, Knife, HeGrenade] {
        assert!(has_secondary_attack_by_id(id), "{id:?}");
    }
}

#[test]
fn secondary_attack_is_masked_for_weapons_without_one() {
    let mut b = armed(WeaponId::Ak47);
    b.run(|game, eng| {
        weapon(game, WeaponId::Ak47).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        let w = weapon(game, WeaponId::Ak47);
        assert_eq!(w.famas_shots_fired, 0, "an AK-47 has no secondary attack");
        // With IN_ATTACK2 masked out, no fire button is down: the idle runs.
        assert_eq!(w.swing, 1);
    });
}

#[test]
fn default_reload_moves_ammo_into_the_clip() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        let usp = WeaponId::Usp;
        let ammo = ammo_index("45ACP") as usize;
        // A full clip does not reload.
        weapon(game, usp).next_primary_attack = -1.0;
        press(game, eng, IN_RELOAD);
        assert!(!weapon(game, usp).in_reload, "nothing fits");
        // No ammo in reserve: no reload either.
        weapon(game, usp).clip = 3;
        game.set_player_ammo(0, ammo, 0);
        press(game, eng, IN_RELOAD);
        assert!(!weapon(game, usp).in_reload);
        // Less in reserve than the clip lacks: it takes what there is.
        game.set_player_ammo(0, ammo, 4);
        press(game, eng, IN_RELOAD);
        assert!(weapon(game, usp).in_reload);
        game.set_next_attack(0, 0.0);
        press(game, eng, 0);
        assert_eq!(weapon(game, usp).clip, 7);
        assert_eq!(game.player_ammo(0, ammo), Some(0));
        // The legacy counters follow `TabulateAmmo` (index 5 is `ammo_45acp`).
        assert_eq!(game.tabulated_ammo(0).unwrap()[5], 0);
    });
}

#[test]
fn shotgun_reload_stages() {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        assert!(game.give_weapon(eng, 0, Box::new(TestShotgun), 4, 20));
        assert!(game.select_weapon(eng, 0, WeaponId::M3));
        let m3 = WeaponId::M3;
        let buck = ammo_index("buckshot") as usize;
        game.set_next_attack(0, 0.0);
        let w = weapon(game, m3);
        w.time_weapon_idle = 0.0;
        w.next_primary_attack = -1.0;
        let ft = eng.frametime();

        // Stage 0: the start animation, everything waits `fStartDelay` (0.55).
        press(game, eng, IN_RELOAD);
        let w = weapon(game, m3);
        assert_eq!(w.in_special_reload, 1);
        assert_eq!(w.next_secondary_attack, 0.55_f32 - ft);
        assert_eq!(w.time_weapon_idle, 0.55_f32 - ft);
        assert_eq!(w.next_primary_attack, 0.55_f32 - ft);
        assert_eq!(game.player_weapon_state(0).unwrap().0, 0.55_f32 - ft);

        // Stage 1 waits for the idle timer (the `m_flTimeWeaponIdle > 0` refusal).
        game.set_next_attack(0, 0.0);
        weapon(game, m3).next_primary_attack = -1.0;
        press(game, eng, IN_RELOAD);
        assert_eq!(weapon(game, m3).in_special_reload, 1, "the idle timer still runs");
        weapon(game, m3).time_weapon_idle = 0.0;
        press(game, eng, IN_RELOAD);
        let w = weapon(game, m3);
        assert_eq!(w.in_special_reload, 2);
        assert_eq!(w.time_weapon_idle, 0.45_f32 - ft);
        assert_eq!(w.next_reload, 0.45);

        // Stage 2 loads a shell: one more in the clip, one less in reserve, back to stage 1.
        press(game, eng, IN_RELOAD);
        let w = weapon(game, m3);
        assert_eq!(w.clip, 5);
        assert_eq!(w.in_special_reload, 1);
        assert_eq!(game.player_ammo(0, buck), Some(19));

        // A full clip refuses.
        let w = weapon(game, m3);
        w.clip = 8;
        w.in_special_reload = 0;
        press(game, eng, IN_RELOAD);
        assert_eq!(weapon(game, m3).in_special_reload, 0);
    });
}

#[test]
fn kick_back_as_vanilla() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        let usp = WeaponId::Usp;
        let id = eng.client_edict(0);
        let fire = |game: &mut CsGame, eng: &mut EngineApi<'_>, shots_before: i32, direction: i32| {
            eng.ent_mut(id).unwrap().punchangle = Vec3::ZERO;
            let w = weapon(game, usp);
            w.next_primary_attack = 0.0;
            w.shots_fired = shots_before;
            w.direction = direction;
            press(game, eng, IN_ATTACK);
            eng.ent(id).unwrap().punchangle
        };
        // The first shot: the bases only; direction 0 kicks to the left (negative yaw).
        let punch = fire(game, eng, 0, 0);
        assert_eq!((punch.x, punch.y), (-1.0, -0.5));
        // The second adds `shots * modifier`: up 2 * 0.25 + 1, lateral 2 * 0.125 + 0.5.
        let punch = fire(game, eng, 1, 0);
        assert_eq!((punch.x, punch.y), (-1.5, -0.75));
        // Direction 1 kicks to the right.
        let punch = fire(game, eng, 1, 1);
        assert_eq!((punch.x, punch.y), (-1.5, 0.75));
        // Both are bounded: up_max 3, lateral_max 1.
        let punch = fire(game, eng, 9, 1);
        assert_eq!((punch.x, punch.y), (-3.0, 1.0));
        let punch = fire(game, eng, 9, 0);
        assert_eq!((punch.x, punch.y), (-3.0, -1.0));
        // The direction is kept (the odds of a flip were 1 in a million).
        assert_eq!(weapon(game, usp).direction, 0);
    });
}

#[test]
fn kick_back_flips_the_direction_with_certainty_at_zero() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        // `RANDOM_LONG(0, 0)` is always 0, so `direction_change` 0 flips every time.
        #[derive(Clone)]
        struct Flipper;
        impl WeaponBehavior for Flipper {
            fn id(&self) -> WeaponId {
                WeaponId::Usp
            }
            fn deploy(&mut self, _w: &mut WeaponCtx<'_, '_>) -> bool {
                true
            }
            fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
                w.weapon.shots_fired = 1;
                w.kick_back(1.0, 0.5, 0.25, 0.125, 3.0, 1.0, 0);
                w.weapon.next_primary_attack = 1.0;
            }
        }
        // Same id: the Flipper takes the place of the test gun, which stays the active item.
        assert!(game.give_weapon(eng, 0, Box::new(Flipper), 12, 36));
        game.set_next_attack(0, 0.0);
        assert_eq!(weapon(game, WeaponId::Usp).direction, 0);
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Usp).direction, 1);
        weapon(game, WeaponId::Usp).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Usp).direction, 0);
    });
}

#[test]
fn empty_sound_plays_while_the_flag_is_set() {
    let mut b = armed(WeaponId::Usp);
    let pistol = b.run(|_, eng| eng.sound_index("weapons/dryfire_pistol.wav").unwrap());
    let rifle = b.run(|_, eng| eng.sound_index("weapons/dryfire_rifle.wav").unwrap());
    b.probe.clear_messages();
    b.run(|game, eng| {
        let usp = WeaponId::Usp;
        weapon(game, usp).clip = 0;
        game.set_player_ammo(0, ammo_index("45ACP") as usize, 0);
        // The flag starts off (the entity's memory is zeroed): the empty attack is silent.
        assert!(!weapon(game, usp).play_empty_sound);
        weapon(game, usp).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, usp).glock18_shots_fired, 1);
        assert_eq!(weapon(game, usp).next_primary_attack, 0.2_f32 - eng.frametime());
    });
    b.step(3);
    let heard = |b: &Bench, index: u16| {
        b.probe
            .messages()
            .iter()
            .filter(|m| matches!(m, SvcMessage::Sound(s) if s.sound_index == index && s.channel == 1))
            .count()
    };
    assert_eq!(heard(&b, pistol), 0);
    b.run(|game, eng| {
        let usp = WeaponId::Usp;
        // The idle (the button released) arms the click; vanilla never disarms it, so every
        // empty attack clicks.
        press(game, eng, 0);
        assert!(weapon(game, usp).play_empty_sound);
        for _ in 0..2 {
            weapon(game, usp).next_primary_attack = 0.0;
            press(game, eng, IN_ATTACK);
        }
        assert!(weapon(game, usp).play_empty_sound, "`PlayEmptySound` does not clear the flag");
    });
    b.step(3);
    assert_eq!(heard(&b, pistol), 2, "a pistol clicks on its own sound");
    assert_eq!(heard(&b, rifle), 0);
}

#[test]
fn timers_count_down_in_post_think() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        let usp = WeaponId::Usp;
        let ft = eng.frametime();
        let w = weapon(game, usp);
        w.next_primary_attack = 0.5;
        w.next_secondary_attack = 0.5;
        w.time_weapon_idle = 0.5;
        game.set_next_attack(0, 0.5);
        press(game, eng, 0);
        let w = weapon(game, usp);
        assert_eq!(w.next_primary_attack, 0.5_f32 - ft);
        assert_eq!(w.next_secondary_attack, 0.5_f32 - ft);
        assert_eq!(w.time_weapon_idle, 0.5_f32 - ft);
        assert_eq!(game.player_weapon_state(0).unwrap().0, 0.5_f32 - ft);

        // The clamps: -1 for the primary attack, -0.001 for the rest and `m_flNextAttack`.
        let w = weapon(game, usp);
        w.next_primary_attack = -5.0;
        w.next_secondary_attack = -5.0;
        w.time_weapon_idle = -5.0;
        game.set_next_attack(0, -5.0);
        press(game, eng, 0);
        let w = weapon(game, usp);
        assert_eq!(w.next_primary_attack, -1.0);
        assert_eq!(w.next_secondary_attack, -0.001);
        assert_eq!(w.time_weapon_idle, -0.001);
        assert_eq!(game.player_weapon_state(0).unwrap().0, -0.001);

        // An idle of 1000 (a weapon that must not idle) is left alone.
        weapon(game, usp).time_weapon_idle = 1000.0;
        press(game, eng, 0);
        assert_eq!(weapon(game, usp).time_weapon_idle, 1000.0);

        // `pt_end` runs for the dead as well (`PostThink` jumps to it).
        weapon(game, usp).next_secondary_attack = 0.5;
        game.damage_player(eng, 0, None, 500.0, 0);
        common::run_cmd(game, eng, 0, &cmd(), 0);
        assert_eq!(weapon(game, usp).next_secondary_attack, 0.5_f32 - ft);
    });
}

#[test]
fn fov_change_sends_set_fov() {
    let mut b = armed(WeaponId::Usp);
    b.step(2);
    b.probe.clear_messages();
    b.run(|game, _| game.set_fov(0, 55));
    b.step(2);
    let msgs = b.messages();
    assert!(msgs.contains(&CsUserMsg::SetFov(55)), "{msgs:?}");
    // The edict's field of view follows the player's (`pev->fov = m_iFOV`), and the client's
    // cache is up to date.
    let (fov, client_fov, edict_fov) = b.run(|game, eng| {
        let (_, _, _, fov, client_fov) = game.player_weapon_state(0).unwrap();
        (fov, client_fov, eng.ent(eng.client_edict(0)).unwrap().fov)
    });
    assert_eq!((fov, client_fov, edict_fov), (55, 55, 55.0));
    // The weapon is told the FOV changed: its `CurWeapon` goes out again.
    assert!(
        msgs.iter().any(|m| matches!(m, CsUserMsg::CurWeapon { is_active: true, weapon_id: 16, .. })),
        "{msgs:?}"
    );
    // Nothing more while it stays.
    b.probe.clear_messages();
    b.step(2);
    let msgs = b.messages();
    assert!(!msgs.iter().any(|m| matches!(m, CsUserMsg::SetFov(_) | CsUserMsg::CurWeapon { .. })));
}

#[test]
fn cur_weapon_follows_the_clip_and_the_active_item() {
    let mut b = armed(WeaponId::Usp);
    b.step(2);
    b.probe.clear_messages();
    b.run(|game, eng| {
        // A shot: the clip is 11.
        weapon(game, WeaponId::Usp).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
    });
    b.step(2);
    let cur: Vec<_> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::CurWeapon { is_active, weapon_id, clip_ammo } => Some((is_active, weapon_id, clip_ammo)),
            _ => None,
        })
        .collect();
    assert_eq!(cur, [(true, 16, 11)]);
    // The reserve is told with `AmmoX` once, when it changes.
    b.probe.clear_messages();
    b.run(|game, _| game.set_player_ammo(0, ammo_index("45ACP") as usize, 30));
    b.step(2);
    let ammo: Vec<_> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::AmmoX { ammo_id, amount } => Some((i32::from(ammo_id), amount)),
            _ => None,
        })
        .collect();
    assert_eq!(ammo, [(ammo_index("45ACP"), 30)]);
}

#[test]
fn can_shoot_after_freeze() {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        // A spawn in the freeze period: no shooting, and the client is told (`iuser3`).
        assert!(!game.player_weapon_state(0).unwrap().1);
        assert!(game.give_weapon(eng, 0, Box::new(TestGun(WeaponId::Usp)), 12, 36));
        assert!(game.select_weapon(eng, 0, WeaponId::Usp));
        game.set_next_attack(0, 0.0);
        weapon(game, WeaponId::Usp).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Usp).glock18_shots_fired, 0, "frozen");
        let data = game.update_client_data(eng, 0);
        assert_eq!(data.iuser3 & 1, 0, "PLAYER_CAN_SHOOT is off");
        assert_eq!(data.iuser3 & 2, 2, "PLAYER_FREEZE_TIME_OVER is set during the freeze");

        // `OnRoundFreezeEnd` lets everybody shoot.
        game.end_freeze_period(eng);
        assert!(game.player_weapon_state(0).unwrap().1);
        let data = game.update_client_data(eng, 0);
        assert_eq!(data.iuser3 & 1, 1);
        assert_eq!(data.iuser3 & 2, 0);
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Usp).glock18_shots_fired, 1);
    });
    // `PlayerThink` gives the shot back too, a player that spawned during a freeze that ended
    // without it (a joiner) is covered by the same rule.
    b.run(|game, eng| {
        game.set_next_attack(0, 0.0);
        let cmd = GoldSrcUserCmd { msec: 10, ..Default::default() };
        common::run_cmd(game, eng, 0, &cmd, 0);
        assert!(game.player_weapon_state(0).unwrap().1);
    });
}

#[test]
fn defusing_blocks_the_shot_and_the_can_shoot_bit() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        game.set_defusing(0, true);
        weapon(game, WeaponId::Usp).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Usp).glock18_shots_fired, 0, "defusing");
        let data = game.update_client_data(eng, 0);
        assert_eq!(data.iuser3 & 1, 0, "PLAYER_CAN_SHOOT is off while defusing");
        game.set_defusing(0, false);
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Usp).glock18_shots_fired, 1);
        let data = game.update_client_data(eng, 0);
        assert_eq!(data.iuser3 & 1, 1);
    });
}

#[test]
fn spawn_clears_next_attack() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        game.set_next_attack(0, 5.0);
        assert!(game.respawn_player(eng, 0));
        assert_eq!(game.player_weapon_state(0).unwrap().0, 0.0);
    });
}

#[test]
fn game_over_clears_the_buttons() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        game.force_game_over(true);
        weapon(game, WeaponId::Usp).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().button, 0);
        assert_eq!(weapon(game, WeaponId::Usp).glock18_shots_fired, 0);
        game.force_game_over(false);
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Usp).glock18_shots_fired, 1);
    });
}

#[test]
fn spawned_player_without_a_shield_has_gamestate_1() {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        let id = eng.client_edict(0);
        // `HITGROUP_SHIELD_DISABLED`: the studio traces skip the shield box.
        assert_eq!(eng.ent(id).unwrap().gamestate, 1);
        // And so does a corpse.
        eng.ent_mut(id).unwrap().gamestate = 0;
        game.damage_player(eng, 0, None, 500.0, 0);
        assert_eq!(eng.ent(id).unwrap().gamestate, 1);
        eng.ent_mut(id).unwrap().gamestate = 0;
        assert!(game.respawn_player(eng, 0));
        assert_eq!(eng.ent(id).unwrap().gamestate, 1);
    });
}

#[test]
fn fire_remaining_bursts() {
    let mut b = armed(WeaponId::Glock18);
    b.run(|game, eng| {
        let glock = WeaponId::Glock18;
        let now = eng.time() as f32;
        // A burst in progress: no time test for the Glock, a shot per command, three in all.
        let w = weapon(game, glock);
        w.clip = 20;
        w.glock18_shoot = now + 0.1;
        w.glock18_shots_fired = 0;
        press(game, eng, 0);
        let w = weapon(game, glock);
        assert_eq!((w.clip, w.glock18_shots_fired), (19, 1));
        assert_eq!(w.glock18_shoot, now + 0.1);
        press(game, eng, 0);
        press(game, eng, 0);
        let w = weapon(game, glock);
        assert_eq!((w.clip, w.glock18_shots_fired), (17, 3));
        assert_eq!(w.glock18_shoot, 0.0, "the third shot ends the burst");
        press(game, eng, 0);
        assert_eq!(weapon(game, glock).clip, 17);
        // The effect flag of the shot.
        assert_ne!(eng.ent(eng.client_edict(0)).unwrap().effects & 2, 0);

        // A spent clip ends it with the count at 3 (the empty weapon then reloads by itself).
        let w = weapon(game, glock);
        w.clip = 0;
        w.glock18_shoot = now + 0.1;
        w.glock18_shots_fired = 1;
        press(game, eng, 0);
        let w = weapon(game, glock);
        assert_eq!((w.clip, w.glock18_shots_fired, w.glock18_shoot), (0, 3, 0.0));

        // The Famas's burst waits for the absolute time.
        game.set_next_attack(0, 0.0);
        let w = weapon(game, glock);
        w.in_reload = false;
        w.clip = 10;
        w.famas_shoot = now + 5.0;
        w.famas_shots_fired = 0;
        press(game, eng, 0);
        assert_eq!(weapon(game, glock).clip, 10, "not yet");
        weapon(game, glock).famas_shoot = now - 0.5;
        press(game, eng, 0);
        let w = weapon(game, glock);
        assert_eq!((w.clip, w.famas_shots_fired), (9, 1));
        assert_eq!(w.famas_shoot, now + 0.1);
    });
}

#[test]
fn the_zoom_resumes_when_the_weapon_may_shoot() {
    let mut b = armed(WeaponId::Usp);
    b.run(|game, eng| {
        game.set_fov(0, 40);
        game.set_resume_zoom(0, 90);
        weapon(game, WeaponId::Usp).next_primary_attack = 0.3;
        press(game, eng, 0);
        assert_eq!(game.player_weapon_state(0).unwrap().3, 40, "the timer still runs");
        weapon(game, WeaponId::Usp).next_primary_attack = 0.0;
        press(game, eng, 0);
        assert_eq!(game.player_weapon_state(0).unwrap().3, 90);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().fov, 90.0);
    });
}

#[test]
fn rifles_shots_fired_decay_and_delayed_fire() {
    let mut b = armed(WeaponId::Ak47);
    b.run(|game, eng| {
        let ak = WeaponId::Ak47;
        let now = eng.time() as f32;
        // A rifle's shots fired fall by one per `0.0225` s once the delay is over.
        let w = weapon(game, ak);
        w.shots_fired = 3;
        w.decrease_shots_fired = now - 1.0;
        press(game, eng, 0);
        let w = weapon(game, ak);
        assert_eq!(w.shots_fired, 2);
        assert_eq!(w.decrease_shots_fired, now + 0.0225);
        press(game, eng, 0);
        assert_eq!(weapon(game, ak).shots_fired, 2, "not before the delay is over");
        // `m_bDelayFire`: the count is held to 15 and the decay waits 0.4 s.
        let w = weapon(game, ak);
        w.delay_fire = true;
        w.shots_fired = 20;
        press(game, eng, 0);
        let w = weapon(game, ak);
        assert!(!w.delay_fire);
        assert_eq!(w.shots_fired, 15);
        assert_eq!(w.decrease_shots_fired, now + 0.4);
    });
}

#[test]
fn reload_sound_reaches_nearby_players() {
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.enter_second_client();
    b.probe.clear_messages();
    let distance = b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        game.end_freeze_period(eng);
        // The shooter is player 1; the probe (slot 0) stands 100 units away.
        let (a, o) = (eng.client_edict(0), eng.client_edict(1));
        eng.set_origin(a, Vec3::new(0.0, 0.0, 100.0));
        eng.set_origin(o, Vec3::new(100.0, 0.0, 100.0));
        let distance = (eng.ent(a).unwrap().origin - eng.ent(o).unwrap().origin).length();
        assert!(game.give_weapon(eng, 1, Box::new(TestGun(WeaponId::Usp)), 3, 36));
        assert!(game.select_weapon(eng, 1, WeaponId::Usp));
        game.set_next_attack(1, 0.0);
        let w = game.weapon_mut(1, WeaponId::Usp).unwrap();
        w.next_primary_attack = -1.0;
        w.time_weapon_idle = 0.0;
        // Player 1 presses reload.
        let id = eng.client_edict(1);
        eng.ent_mut(id).unwrap().button = i32::from(IN_RELOAD);
        common::run_cmd(game, eng, 1, &GoldSrcUserCmd { buttons: IN_RELOAD, ..cmd() }, 0);
        distance
    });
    b.step(3);
    let heard: Vec<Vec<u8>> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::Unknown { name, payload } if name == "ReloadSound" => Some(payload),
            _ => None,
        })
        .collect();
    let volume = ((1.0 - (distance / 512.0)) * 255.0) as i32 as u8;
    assert_eq!(heard, [vec![volume, 1]]);
}

#[test]
fn the_registry_has_every_weapon() {
    for id in WEAPONS_PRECACHE_ORDER {
        let weapon = new_weapon(id).unwrap_or_else(|| panic!("{id:?}"));
        assert_eq!(weapon.id(), id);
    }
    assert!(new_weapon(WeaponId::None).is_none());
    assert!(new_weapon(WeaponId::Glock).is_none());
    assert!(new_weapon(WeaponId::ShieldGun).is_none());
    // The count: 25 guns, the knife, three grenades and the C4.
    assert_eq!(WEAPONS_PRECACHE_ORDER.len(), 29);
}

#[test]
fn weapons_run_through_the_engine() {
    // The same machine under the real command path: the probe's commands carry the buttons and
    // the engine's frame time.
    let mut b = Bench::stub_without_moves(spawns(4, 4));
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        assert!(game.give_weapon(eng, 0, Box::new(TestGun(WeaponId::Usp)), 12, 36));
        assert!(game.select_weapon(eng, 0, WeaponId::Usp));
        game.set_next_attack(0, 0.0);
        weapon(game, WeaponId::Usp).next_primary_attack = 0.0;
    });
    b.probe.suppress_moves(false);
    b.probe.queue_cmd(GoldSrcUserCmd { msec: 10, buttons: IN_ATTACK, ..Default::default() });
    b.step(1);
    let (shots, timer) = b.run(|game, _| {
        let w = weapon(game, WeaponId::Usp);
        (w.glock18_shots_fired, w.next_primary_attack)
    });
    assert!(shots >= 1, "the probe's command fired the weapon");
    assert!(timer < 0.15, "and the engine's frame time ran the timer down: {timer}");
}
