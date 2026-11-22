//! The data a client needs to predict its weapons and the animation of its third-person model
//! (Task 9 of phase 2D): `GetWeaponData`, the weapon fields of `UpdateClientData`, the
//! `skiplocal` of `SendWeaponAnim`, and `CBasePlayer::SetAnimation`. ReGameDLL_CS `client.cpp`,
//! `weapons.cpp`, `player.cpp` and `animation.cpp`, read 2026-11-22, vanilla build.
//!
//! The tests that need the player model's sequences read it from the installed game and return
//! when it is missing (`common::discover`).

mod common;

use std::sync::Arc;

use common::{Bench, discover, join, spawns};
use cs_assets::mdl::StudioPoseData;
use cs_engine::{EngineApi, GameDll};
use cs_game::CsGame;
use cs_game::weapons::core::{PlayerAnim, WeaponCtx};
use cs_game::weapons::{WeaponBehavior, WeaponId, ammo_index, new_weapon};
use cs_proto48::delta::{GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::messages::SvcMessage;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;
const IN_RELOAD: u16 = 1 << 13;
const FL_DUCKING: u32 = 1 << 14;
/// `PLAYER_CAN_SHOOT`, `PLAYER_FREEZE_TIME_OVER`, `PLAYER_HOLDING_SHIELD` (`cdll_dll.h`).
const PLAYER_CAN_SHOOT: i32 = 1 << 0;
const PLAYER_FREEZE_TIME_OVER: i32 = 1 << 1;

/// Every weapon id that has a behaviour (the guns, the knife, the grenades and the C4).
fn all_weapons() -> Vec<WeaponId> {
    (1..=30).filter_map(WeaponId::from_i32).filter(|id| new_weapon(*id).is_some()).collect()
}

/// One command with `buttons`, as the engine runs it (`v.button` is set before `CmdStart`).
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

/// A counter-terrorist (slot 0) on a stub map, out of the freeze period, with the probe's
/// `cl_lw` as `userinfo` says.
fn arena(userinfo: &[(&str, &str)]) -> Bench {
    let map = cs_engine::ServerMap::stub_with_entities("test", spawns(4, 4));
    let mut b = Bench::with_userinfo(map, 2, userinfo);
    // As the golden capture does: jointeam 2, joinclass 5 (the default items follow).
    join(&mut b, 0, 2, 5);
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        game.end_freeze_period(eng);
    });
    b
}

/// `GetWeaponData` of slot 0.
fn weapon_data(b: &mut Bench) -> Box<[GoldSrcWeaponData; 64]> {
    b.run(|game, eng| {
        let mut out = Box::new([GoldSrcWeaponData::default(); 64]);
        assert!(game.get_weapon_data(eng, 0, &mut out));
        out
    })
}

/// What `GetWeaponData` must hold for a weapon whose fields are `w` (`client.cpp` 4947-5018,
/// the quirks included: the reused fields, the floors of -0.001).
fn expected(w: &cs_game::weapons::Weapon) -> GoldSrcWeaponData {
    let floor = |t: f32| t.max(-0.001);
    GoldSrcWeaponData {
        m_i_id: w.id as i32,
        m_i_clip: w.clip,
        m_fl_next_primary_attack: floor(w.next_primary_attack),
        m_fl_next_secondary_attack: floor(w.next_secondary_attack),
        m_fl_time_weapon_idle: floor(w.time_weapon_idle),
        m_f_in_reload: i32::from(w.in_reload),
        m_f_in_special_reload: w.in_special_reload,
        m_fl_next_reload: floor(w.next_reload),
        m_f_in_zoom: w.shots_fired,
        m_f_aimed_damage: w.last_fire,
        m_i_weapon_state: w.weapon_state,
        fuser2: w.start_throw,
        fuser3: w.release_throw,
        iuser1: w.swing,
        ..GoldSrcWeaponData::default()
    }
}

#[test]
fn weapon_data_round_trip_per_weapon() {
    let mut b = arena(&[]);
    let ids = all_weapons();
    b.run({
        let ids = ids.clone();
        move |game, eng| {
            for id in ids {
                game.give_weapon(eng, 0, new_weapon(id).unwrap(), 5, 40);
            }
        }
    });
    for &id in &ids {
        // A series of commands with this weapon in hand: draw, shoot, reload, idle.
        b.run(move |game, eng| {
            if !game.select_weapon(eng, 0, id) {
                return;
            }
            for _ in 0..80 {
                press(game, eng, 0);
            }
            for _ in 0..4 {
                press(game, eng, IN_ATTACK);
            }
            for _ in 0..3 {
                press(game, eng, 0);
            }
            press(game, eng, IN_RELOAD);
            for _ in 0..3 {
                press(game, eng, 0);
            }
            // Fields the weapons keep to themselves: the reused ones carry distinct values.
            if let Some(w) = game.weapon_mut(0, id) {
                w.shots_fired = 7;
                w.last_fire = 1.25;
                w.start_throw = 3.5;
                w.release_throw = 4.5;
                w.swing = 1;
                w.next_reload = -3.0;
            }
        });
        // The data and the state it is the data of, in the same frame.
        let (data, states) = b.run(|game, eng| {
            let mut out = Box::new([GoldSrcWeaponData::default(); 64]);
            assert!(game.get_weapon_data(eng, 0, &mut out));
            (out, ids_state(game))
        });
        let mut checked = 0;
        for (wid, w) in &states {
            let got = &data[*wid as usize];
            assert_eq!(*got, expected(w), "weapon {wid:?}: the data is the server's state");
            checked += 1;
        }
        assert_eq!(checked, states.len());
        assert!(checked >= ids.len(), "every weapon the player carries is in the list");
    }
    // The unused entries are zero.
    let data = weapon_data(&mut b);
    assert_eq!(data[0], GoldSrcWeaponData::default());
    assert_eq!(data[63], GoldSrcWeaponData::default());
}

/// A copy of the fields of every weapon the player of slot 0 carries.
fn ids_state(game: &mut CsGame) -> Vec<(WeaponId, cs_game::weapons::Weapon)> {
    all_weapons()
        .into_iter()
        .filter_map(|id| game.weapon_mut(0, id).map(|w| (id, w.clone())))
        .collect()
}

#[test]
fn weapon_data_floors_the_timers_and_leaves_the_rest() {
    let mut b = arena(&[]);
    b.run(|game, eng| {
        game.give_weapon(eng, 0, new_weapon(WeaponId::Ak47).unwrap(), 30, 90);
        let w = game.weapon_mut(0, WeaponId::Ak47).unwrap();
        w.next_primary_attack = -5.0;
        w.next_secondary_attack = -0.5;
        w.time_weapon_idle = 1000.0;
        w.next_reload = 0.25;
        w.pump_time = 9.0;
    });
    let data = weapon_data(&mut b);
    let ak = &data[WeaponId::Ak47 as usize];
    assert_eq!(ak.m_fl_next_primary_attack, -0.001);
    assert_eq!(ak.m_fl_next_secondary_attack, -0.001);
    assert_eq!(ak.m_fl_time_weapon_idle, 1000.0, "the idle of 1000 is sent as it is");
    assert_eq!(ak.m_fl_next_reload, 0.25);
    assert_eq!(ak.m_fl_pump_time, 0.0, "m_flPumpTime is not part of GetWeaponData");
}

#[test]
fn weapon_data_of_a_slot_with_no_player_is_empty() {
    let mut b = arena(&[]);
    let data = b.run(|game, eng| {
        let mut out = Box::new([GoldSrcWeaponData { m_i_id: 9, ..Default::default() }; 64]);
        assert!(game.get_weapon_data(eng, 1, &mut out), "C returns 1 for no player too");
        out
    });
    assert!(data.iter().all(|e| *e == GoldSrcWeaponData::default()));
}

/// A weapon that plays two animations, one with `skiplocal` and one without.
#[derive(Clone)]
struct AnimGun;

impl WeaponBehavior for AnimGun {
    fn id(&self) -> WeaponId {
        WeaponId::Glock18
    }
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.default_deploy(&*self, "models/v_usp.mdl", "models/p_usp.mdl", 6, "onehanded", true)
    }
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.send_weapon_anim(3, true);
        w.weapon.next_primary_attack = 1.0;
    }
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.send_weapon_anim(4, false);
        w.weapon.next_secondary_attack = 1.0;
    }
}

fn weapon_anims(b: &Bench) -> Vec<(u8, u8)> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::WeaponAnim { anim, body } => Some((*anim, *body)),
            _ => None,
        })
        .collect()
}

#[test]
fn skiplocal_weapon_anims_skip_lw_clients() {
    // `cl_lw 1`: the client plays what it predicts, and `skiplocal` anims are not sent; `cl_lw 0`:
    // everything is sent. `pev->weaponanim` is set either way.
    for (lw, expect_skiplocal_sent) in [("1", false), ("0", true)] {
        let mut b = arena(&[("cl_lw", lw)]);
        b.run(|game, eng| {
            assert!(game.give_weapon(eng, 0, Box::new(AnimGun), 12, 24));
            game.select_weapon(eng, 0, WeaponId::Glock18);
            game.set_next_attack(0, 0.0);
            game.weapon_mut(0, WeaponId::Glock18).unwrap().body = 5;
        });
        b.probe.clear_messages();
        b.run(|game, eng| {
            press(game, eng, IN_ATTACK);
        });
        b.step(2);
        let skiplocal_sent = weapon_anims(&b).contains(&(3, 5));
        assert_eq!(skiplocal_sent, expect_skiplocal_sent, "cl_lw {lw}: the skiplocal animation");
        let weaponanim = b.run(|_game, eng| eng.ent(eng.client_edict(0)).unwrap().weaponanim);
        assert_eq!(weaponanim, 3, "the weapon's animation is recorded for the client data");

        b.probe.clear_messages();
        b.run(|game, eng| {
            game.weapon_mut(0, WeaponId::Glock18).unwrap().next_secondary_attack = 0.0;
            // IN_ATTACK2.
            press(game, eng, 1 << 11);
        });
        b.step(2);
        assert!(
            weapon_anims(&b).contains(&(4, 5)),
            "cl_lw {lw}: an animation without skiplocal is always sent, with the weapon's body"
        );
    }
}

/// The ammo slot of `name` and the reserve the player of slot 0 has of it.
fn reserve(game: &mut CsGame, name: &str) -> (i32, i32) {
    let index = ammo_index(name);
    (index, game.player_ammo(0, usize::try_from(index).unwrap()).unwrap())
}

#[test]
fn client_data_weapon_fields() {
    let mut b = arena(&[]);
    let (data, usp_index, reserve_45, tabulated) = b.run(|game, eng| {
        game.select_weapon(eng, 0, WeaponId::Usp);
        game.set_next_attack(0, 0.5);
        let data = game.update_client_data(eng, 0);
        let (index, reserve_45) = reserve(game, "45acp");
        (data, index, reserve_45, game.tabulated_ammo(0).unwrap())
    });
    assert_eq!(data.m_i_id, 16, "the id of the weapon in hand");
    assert_eq!(data.maxspeed, 250.0);
    assert_eq!(data.m_fl_next_attack, 0.5);
    assert_eq!(
        data.vuser4,
        Vec3::new(usp_index as f32, reserve_45 as f32, 0.0),
        "the ammo type of the weapon in hand and what is left of it"
    );
    assert!(reserve_45 > 0 && usp_index >= 0);
    // The legacy counters: `ammo_45acp` in `vuser2.y`.
    assert_eq!(data.vuser2, Vec3::new(tabulated[4] as f32, tabulated[5] as f32, tabulated[6] as f32));
    assert_eq!(data.vuser2.y, reserve_45 as f32);
    assert_eq!(data.vuser3, Vec3::new(tabulated[7] as f32, tabulated[8] as f32, tabulated[9] as f32));
    assert_eq!(
        (data.ammo_shells, data.ammo_nails, data.ammo_cells, data.ammo_rockets),
        (tabulated[0], tabulated[1], tabulated[2], tabulated[3])
    );
    // `weapons`: the suit, the knife and the USP.
    assert_eq!(data.weapons, (1 << 31) | (1 << 29) | (1 << 16));
    let view = b.run(|_game, eng| {
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (i32::from(v.viewmodel), v.weaponanim, eng.model_index("models/v_usp.mdl"))
    });
    assert_eq!(data.viewmodel, view.0);
    assert_eq!(Some(data.viewmodel), view.2.map(i32::from), "the USP's view model");
    assert_eq!(data.weaponanim, view.1);
    // CAN_SHOOT (the freeze period is over) and no FREEZE bit.
    assert_eq!(data.iuser3 & PLAYER_CAN_SHOOT, PLAYER_CAN_SHOOT);
    assert_eq!(data.iuser3 & PLAYER_FREEZE_TIME_OVER, 0);
}

#[test]
fn client_data_weapon_fields_need_cl_lw() {
    // `sendweapons` is `cl_lw`: a client that does not predict gets the view model, the animation
    // and the weapon bits, but no id, no ammo, no `m_flNextAttack` and no `iuser3` bits.
    let mut b = arena(&[("cl_lw", "0")]);
    let data = b.run(|game, eng| {
        game.select_weapon(eng, 0, WeaponId::Usp);
        game.set_next_attack(0, 0.5);
        game.update_client_data(eng, 0)
    });
    assert_eq!(data.m_i_id, 0);
    assert_eq!(data.m_fl_next_attack, 0.0);
    assert_eq!(data.vuser4, Vec3::ZERO);
    assert_eq!(data.vuser2, Vec3::ZERO);
    assert_eq!(data.iuser3, 0);
    assert_ne!(data.viewmodel, 0);
    assert_eq!(data.weapons, (1 << 31) | (1 << 29) | (1 << 16));
    assert_eq!(data.maxspeed, 250.0);
}

#[test]
fn a_knife_has_no_ammo_in_the_client_data() {
    let mut b = arena(&[]);
    let data = b.run(|game, eng| {
        assert!(game.select_weapon(eng, 0, WeaponId::Knife));
        game.update_client_data(eng, 0)
    });
    assert_eq!(data.m_i_id, 29);
    assert_eq!(data.vuser4, Vec3::new(-1.0, 0.0, 0.0), "no ammo type: -1 and 0");
}

/// The values of the HLDS capture after `jointeam 2`, `joinclass 5` (the golden join, the first
/// client data with weapons), through the engine's `svc_clientdata` to the probe: the USP and
/// the knife with their `weapon_data_t`, the HUD's ammo, the weapon bits.
#[test]
fn golden_values_after_joinclass() {
    let map = cs_engine::ServerMap::stub_with_entities("test", spawns(4, 4));
    let mut b = Bench::with_slots(map, 2);
    // With the game's models (the animation lookups need the player model), when it is installed.
    let vfs = discover().map(Arc::new);
    if vfs.is_none() {
        eprintln!("NOTE: the installed game was not found; the golden sequence numbers are not checked");
    }
    if let Some(vfs) = &vfs {
        let vfs = Arc::clone(vfs);
        b.server.set_model_loader(Arc::new(move |name| vfs.read_bytes(name).ok()));
    }
    join(&mut b, 0, 2, 5);
    // The freeze period is over (the capture's  is the USP's, not the frozen 1).
    b.run(|game, eng| game.end_freeze_period(eng));
    b.step(5);
    let cd = b.probe.client_data().expect("client data").clone();
    let wd = b.probe.weapon_data();
    // HLDS: `weapons=0xa0010000 m_iId=16 maxspeed=250 vuser2[1]=24 vuser4[0]=6 vuser4[1]=24`.
    assert_eq!(cd.weapons, 0xa001_0000);
    assert_eq!(cd.m_i_id, 16);
    assert_eq!(cd.maxspeed, 250.0);
    assert_eq!(cd.vuser2.y, 24.0);
    assert_eq!(cd.vuser4.x, ammo_index("45acp") as f32);
    assert_eq!(cd.vuser4.x, 6.0);
    assert_eq!(cd.vuser4.y, 24.0);
    assert_eq!(cd.weaponanim, 14, "HLDS: the unsilenced USP's draw");
    let view = b.run(|_game, eng| eng.model_index("models/v_usp.mdl"));
    assert_eq!(Some(cd.viewmodel as u16), view);
    assert!(cd.m_fl_next_attack >= 0.0 && cd.m_fl_next_attack <= 0.75, "{}", cd.m_fl_next_attack);
    // HLDS: `weapon16.m_iClip=12 weapon16.m_iId=16 weapon29.m_iClip=-1 weapon29.m_iId=29`, the
    // timers at -0.001 and the idle counting down from 1.5.
    let usp = &wd[&16];
    let knife = &wd[&29];
    assert_eq!((usp.m_i_id, usp.m_i_clip), (16, 12));
    assert_eq!((knife.m_i_id, knife.m_i_clip), (29, -1));
    for w in [usp, knife] {
        assert_eq!(w.m_fl_next_primary_attack, -0.001);
        assert_eq!(w.m_fl_next_secondary_attack, -0.001);
        assert!(w.m_fl_time_weapon_idle > 0.0 && w.m_fl_time_weapon_idle <= 1.5);
    }
    assert_eq!(wd.len(), 2, "only the weapons the player carries");
    // The player's own entity carries what the animation and the deploy wrote (HLDS:
    // `sequence=19 gaitsequence=1 framerate=1 weaponmodel=168`, the model of `p_usp.mdl`).
    let player = b.probe.entities().get(&1).expect("the player's entity").clone();
    let weapon_model = b.run(|_game, eng| eng.model_index("models/p_usp.mdl"));
    assert_eq!(Some(player.weaponmodel), weapon_model, "the USP's third-person model");
    assert_ne!(player.weaponmodel, 0);
    assert_eq!(player.framerate, 1.0);
    if vfs.is_some() {
        assert_eq!(player.gaitsequence, 1, "the idle gait (`idle1`)");
        assert_eq!(player.sequence, 19, "`ref_aim_onehanded`");
    }
}

// ---- the third-person animation ------------------------------------------------------------

/// The player model of the installed game and a server whose engine has it, a counter-terrorist
/// with the USP out of the freeze period. `None` without the game.
fn with_model() -> Option<(Bench, StudioPoseData)> {
    let Some(vfs) = discover() else {
        eprintln!(
            "SKIPPED: the installed game was not found (set CS16_GAMEDIR); the test reads the real models/player.mdl"
        );
        return None;
    };
    let bytes = vfs.read_bytes("models/player.mdl").ok()?;
    let model = StudioPoseData::parse(&bytes).expect("the player model parses");
    let mut b = arena(&[]);
    b.server.set_model_loader(Arc::new(move |name| vfs.read_bytes(name).ok()));
    Some((b, model))
}

fn index_of(model: &StudioPoseData, label: &str) -> i32 {
    model
        .sequences
        .iter()
        .position(|s| s.label.eq_ignore_ascii_case(label))
        .unwrap_or_else(|| {
            let labels: Vec<_> = model.sequences.iter().map(|s| s.label.as_str()).collect();
            panic!("the player model has no sequence {label}: {labels:?}")
        }) as i32
}

/// What the animation writes into the edict.
#[derive(Debug, Clone, Copy)]
struct Pev {
    sequence: i32,
    gait: i32,
    frame: f32,
    animtime: f32,
    framerate: f32,
    effects: u32,
    activity: i32,
}

/// Runs `f` as the game, and reads the animation of the player right after it, in the same
/// frame (the next one's `PostThink` would animate the player again).
fn animate(
    b: &mut Bench,
    f: impl FnOnce(&mut CsGame, &mut EngineApi<'_>) + 'static,
) -> Pev {
    b.run(move |game, eng| {
        f(game, eng);
        let v = eng.ent(eng.client_edict(0)).unwrap();
        Pev {
            sequence: i32::from(v.sequence),
            gait: i32::from(v.gaitsequence),
            frame: v.frame,
            animtime: v.animtime,
            framerate: v.framerate,
            effects: v.effects,
            activity: game.anim_state(0).unwrap().0,
        }
    })
}

/// Respawns the player (a fresh `m_Activity` history aside, the spawn's idle sequence) with the
/// animation set `ext`.
fn respawn(game: &mut CsGame, eng: &mut EngineApi<'_>, ext: &str) {
    game.respawn_player(eng, 0);
    game.set_anim_extension(0, ext);
    // The first command of the new life walks it out of its last `m_Activity` (a death one).
    game.set_animation(eng, 0, PlayerAnim::Idle, 0);
}

fn activity_of(model: &StudioPoseData, sequence: i32) -> i32 {
    model.sequences[usize::try_from(sequence).unwrap()].activity
}

#[test]
fn set_animation_picks_the_source_sequence() {
    let Some((mut b, model)) = with_model() else {
        return;
    };
    // The spawn's `pev->sequence = LookupActivity(ACT_IDLE)`: an idle sequence of the model.
    let spawned = animate(&mut b, |game, eng| {
        game.respawn_player(eng, 0);
    });
    assert_eq!(activity_of(&model, spawned.sequence), 1, "an ACT_IDLE sequence");

    // Standing, shooting with a one-handed gun: `ref_shoot_onehanded`, from frame 0, at
    // `framerate` 1, `animtime` now; `m_Activity` is `ACT_RANGE_ATTACK1`.
    let shot = animate(&mut b, |game, eng| game.set_animation(eng, 0, PlayerAnim::Attack1, 0));
    assert_eq!(shot.sequence, index_of(&model, "ref_shoot_onehanded"));
    assert_eq!((shot.frame, shot.framerate), (0.0, 1.0));
    assert!(shot.animtime > 0.0);
    assert_eq!(shot.activity, 28);

    // Ducking: the crouch variant.
    let crouch_shot = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().flags |= FL_DUCKING;
        game.set_animation(eng, 0, PlayerAnim::Attack1, 0);
    });
    assert_eq!(crouch_shot.sequence, index_of(&model, "crouch_shoot_onehanded"));

    // A reload: the reload sequence, from frame 0, and `EF_NOINTERP` (the sequence does not loop).
    let reload = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().flags |= FL_DUCKING;
        game.set_animation(eng, 0, PlayerAnim::Reload, 0);
    });
    assert_eq!(reload.activity, 32, "m_Activity: ACT_RELOAD");
    assert_eq!(reload.sequence, index_of(&model, "crouch_reload_onehanded"));
    assert_eq!(reload.frame, 0.0);
    assert_eq!(reload.effects & 32, 32, "EF_NOINTERP: the reload does not loop");

    // The animation set of another weapon: the rifle's.
    let rifle = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().flags &= !FL_DUCKING;
        game.set_anim_extension(0, "rifle");
        game.set_animation(eng, 0, PlayerAnim::Attack1, 0);
    });
    assert_eq!(rifle.sequence, index_of(&model, "ref_shoot_rifle"));
    // And the knife's, which has the longest shot.
    let knife = animate(&mut b, |game, eng| {
        game.set_anim_extension(0, "knife");
        game.set_animation(eng, 0, PlayerAnim::Attack1, 0);
    });
    assert_eq!(knife.sequence, index_of(&model, "ref_shoot_knife"));
}

#[test]
fn walking_and_running_pick_the_gait() {
    let Some((mut b, model)) = with_model() else {
        return;
    };
    // On the ground at 250 units/s, long after the last shot (`m_flLastFired` is -15 from the
    // spawn): the run gait, and the upper body's `run` (the model has none for this set, so the
    // weapon's aim).
    let running = animate(&mut b, |game, eng| {
        respawn(game, eng, "onehanded");
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.flags |= 1 << 9; // FL_ONGROUND
        v.velocity = Vec3::new(250.0, 0.0, 0.0);
        game.set_animation(eng, 0, PlayerAnim::Walk, 0);
    });
    assert_eq!(activity_of(&model, running.gait), 4, "an ACT_RUN sequence");
    assert!(
        running.sequence == index_of(&model, "run")
            || running.sequence == index_of(&model, "ref_aim_onehanded"),
        "the upper body runs or aims: {}",
        model.sequences[running.sequence as usize].label
    );
    assert_eq!(running.activity, 4, "m_Activity: ACT_RUN");

    // Slow: the walk gait and the aim sequence of the weapon.
    let walking = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().velocity = Vec3::new(100.0, 0.0, 0.0);
        game.set_animation(eng, 0, PlayerAnim::Walk, 0);
    });
    assert_eq!(activity_of(&model, walking.gait), 3, "an ACT_WALK sequence");
    assert_eq!(walking.sequence, index_of(&model, "ref_aim_onehanded"));
    assert_eq!(walking.activity, 3);

    // Standing still: the idle gait.
    let idle = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().velocity = Vec3::ZERO;
        game.set_animation(eng, 0, PlayerAnim::Idle, 0);
    });
    assert_eq!(activity_of(&model, idle.gait), 1, "an ACT_IDLE sequence");

    // Ducking and moving: `crouchrun`; still: `crouch_idle`.
    let crouching = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.flags |= FL_DUCKING;
        v.velocity = Vec3::new(60.0, 0.0, 0.0);
        game.set_animation(eng, 0, PlayerAnim::Walk, 0);
    });
    assert_eq!(crouching.gait, index_of(&model, "crouchrun"));
    assert_eq!(crouching.sequence, index_of(&model, "crouch_aim_onehanded"));
    let crouched = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.flags |= FL_DUCKING;
        v.velocity = Vec3::ZERO;
        game.set_animation(eng, 0, PlayerAnim::Idle, 0);
    });
    assert_eq!(crouched.gait, index_of(&model, "crouch_idle"));

    // A shot's animation holds the walk back until it is finished: the sequence stays the
    // shot's, the gait follows the legs.
    let shooting = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.flags &= !FL_DUCKING;
        v.velocity = Vec3::new(100.0, 0.0, 0.0);
        game.set_animation(eng, 0, PlayerAnim::Attack1, 0);
        game.set_animation(eng, 0, PlayerAnim::Walk, 0);
    });
    assert_eq!(shooting.sequence, index_of(&model, "ref_shoot_onehanded"));
    assert_eq!(activity_of(&model, shooting.gait), 3);
}

#[test]
fn a_jump_is_the_hop_gait_and_death_depends_on_the_hit_group() {
    let Some((mut b, model)) = with_model() else {
        return;
    };
    let jump = animate(&mut b, |game, eng| {
        respawn(game, eng, "onehanded");
        game.set_animation(eng, 0, PlayerAnim::Jump, 0);
    });
    assert_eq!(activity_of(&model, jump.gait), 7, "ACT_HOP");
    assert_eq!(jump.activity, 7);

    // Death by a shot in the head: an `ACT_DIE_HEADSHOT` sequence, no gait, frame 0.
    let head = animate(&mut b, |game, eng| game.set_animation(eng, 0, PlayerAnim::Die, 1));
    assert_eq!(activity_of(&model, head.sequence), 66, "ACT_DIE_HEADSHOT");
    assert_eq!((head.gait, head.frame), (0, 0.0));
    assert_eq!(head.activity, 36, "m_Activity: ACT_DIESIMPLE");
    // The chest and the stomach have their own.
    for (group, activity) in [(2, 67), (3, 68)] {
        let died = animate(&mut b, move |game, eng| {
            respawn(game, eng, "onehanded");
            game.set_animation(eng, 0, PlayerAnim::Die, group);
        });
        assert_eq!(activity_of(&model, died.sequence), activity, "hit group {group}");
    }
    // The left arm: the sequence called `left`; the right arm `right`.
    for (group, label) in [(4, "left"), (5, "right")] {
        let died = animate(&mut b, move |game, eng| {
            respawn(game, eng, "onehanded");
            game.set_animation(eng, 0, PlayerAnim::Die, group);
        });
        assert_eq!(died.sequence, index_of(&model, label), "hit group {group}");
    }
    // Ducking: `crouch_die`, whatever the hit group.
    let crouch = animate(&mut b, |game, eng| {
        respawn(game, eng, "onehanded");
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().flags |= FL_DUCKING;
        game.set_animation(eng, 0, PlayerAnim::Die, 1);
        // A second death animation while dead changes nothing (`m_Activity == m_IdealActivity`).
        game.set_animation(eng, 0, PlayerAnim::Die, 3);
    });
    assert_eq!(crouch.sequence, index_of(&model, "crouch_die"));
}

#[test]
fn a_flinch_is_the_head_or_the_gut_flinch() {
    let Some((mut b, model)) = with_model() else {
        return;
    };
    for (group, label) in [(1, "head_flinch"), (2, "head_flinch"), (3, "gut_flinch"), (6, "gut_flinch")]
    {
        let flinch = animate(&mut b, move |game, eng| {
            respawn(game, eng, "onehanded");
            game.set_animation(eng, 0, PlayerAnim::Flinch, group);
        });
        assert_eq!(flinch.sequence, index_of(&model, label), "hit group {group}");
        assert_eq!(flinch.activity, 77, "m_Activity: ACT_FLINCH");
    }
}

#[test]
fn the_frame_advances_in_post_think_and_the_gait_blends_follow_the_view() {
    let Some((mut b, model)) = with_model() else {
        return;
    };
    let shot = animate(&mut b, |game, eng| {
        respawn(game, eng, "onehanded");
        game.set_animation(eng, 0, PlayerAnim::Attack1, 0);
    });
    assert!(model.sequences[shot.sequence as usize].fps > 0.0);
    // A few frames later (the probe's commands run `PostThink`, which advances the frame).
    b.step(10);
    let v = b.run(|_g, eng| {
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (v.frame, v.animtime, v.sequence)
    });
    assert!(v.0 != shot.frame || v.2 != shot.sequence as u16, "the frame moved: {v:?}");
    assert!(v.1 > shot.animtime, "the animation time moved: {v:?}");

    // Looking down 60 degrees, level, up 60: the pitch blend (`pev->blending[1]`) of
    // `StudioPlayerBlend`, from `angles.x`, which is a third of the view's pitch, negated.
    for (pitch, blend) in [(60.0, 255u8), (0.0, 127), (-60.0, 0)] {
        let blending = b.run(move |game, eng| {
            let cmd = GoldSrcUserCmd {
                msec: 10,
                viewangles: Vec3::new(pitch, 0.0, 0.0),
                ..Default::default()
            };
            common::run_cmd(game, eng, 0, &cmd, 0);
            eng.ent(eng.client_edict(0)).unwrap().blending
        });
        assert_eq!(blending[1], blend, "pitch {pitch}");
    }
}

/// `DEAD_DYING` and `DEAD_DEAD`.
const DEAD_DYING: i32 = 1;
const DEAD_DEAD: i32 = 2;
const SOLID_SLIDEBOX: u8 = 3;

/// The height of the highest point of the player of slot 0 that a shot straight down hits,
/// over a grid of shots around it (a lying body is not under its origin). `None`: none hits.
fn top_of_player(eng: &EngineApi<'_>) -> Option<f32> {
    let id = eng.client_edict(0);
    let origin = eng.ent(id).unwrap().origin;
    let mut top: Option<f32> = None;
    for ix in -8..=8 {
        for iy in -8..=8 {
            let at = origin + Vec3::new(ix as f32 * 8.0, iy as f32 * 8.0, 0.0);
            let t = eng.trace_line(
                at + Vec3::new(0.0, 0.0, 200.0),
                at - Vec3::new(0.0, 0.0, 50.0),
                cs_engine::MoveFilter::Normal,
                None,
            );
            if t.ent == Some(id) {
                top = Some(top.map_or(t.trace.end_pos.z, |z| z.max(t.trace.end_pos.z)));
            }
        }
    }
    top
}

/// Kills the player of slot 0 with a hit in the head (the world's damage).
fn kill_in_the_head(b: &mut Bench) {
    b.run(|game, eng| {
        game.set_last_hit_group(0, 1);
        game.damage_player(eng, 0, None, 1000.0, 32);
    });
}

#[test]
fn the_death_animation_plays_before_the_body_is_dead() {
    let Some((mut b, model)) = with_model() else {
        return;
    };
    kill_in_the_head(&mut b);
    // The first command after the kill: the death sequence, from its start.
    let died = b.run(|game, eng| {
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (i32::from(v.sequence), v.deadflag, v.frame, game.anim_state(0).unwrap())
    });
    // (`TakeDamage` of the world leaves `m_LastHitGroup` generic: one of the random picks.)
    let activity = activity_of(&model, died.0);
    assert!(matches!(activity, 36..=38 | 66..=69), "a death sequence: {activity}");
    assert_eq!(died.1, DEAD_DYING);
    // One second on: still dying, the frame is part of the way through the sequence (the
    // sequence is not finished, `PlayerDeathThink` goes on animating).
    b.step(100);
    let mid = b.run(|game, eng| {
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (v.deadflag, v.frame, v.framerate, game.anim_state(0).unwrap().2)
    });
    assert_eq!(mid.0, DEAD_DYING, "the death animation is not over after a second");
    assert!(mid.1 > 0.0 && mid.1 < 255.0, "mid-sequence: {}", mid.1);
    assert_eq!((mid.2, mid.3), (1.0, false));
    // The animation ends (the sequence is a second and a half long): the body is dead, the
    // frame is at its last and `StopAnimation` stopped it (`EF_NOINTERP` is cleared by the engine at the end of the frame).
    b.step(150);
    let dead = b.run(|game, eng| {
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (v.deadflag, v.frame, v.framerate, game.anim_state(0).unwrap().2)
    });
    assert_eq!(dead, (DEAD_DEAD, 255.0, 0.0, true));
    // Three seconds after it, the corpse is sent with the sequence and the time the animation
    // stopped at (`animtime - time`, in hundredths, so negative).
    b.step(450);
    let corpse = b
        .messages()
        .into_iter()
        .find_map(|m| match m {
            cs_proto48::usermsg::CsUserMsg::ClCorpse { sequence, time_diff, .. } => {
                Some((sequence, time_diff))
            }
            _ => None,
        })
        .expect("a ClCorpse was sent");
    assert_eq!(i32::from(corpse.0), died.0, "the corpse lies in the death sequence");
    assert!(corpse.1 < 0, "the animation stopped before the corpse was sent: {}", corpse.1);
}

#[test]
fn hitbox_pose_follows_the_player_animation() {
    let Some((mut b, _model)) = with_model() else {
        return;
    };
    // A shot straight down onto a standing player hits it high. Killed, the player plays its
    // death animation to the end and lies down; the same shot at the body (made solid again
    // by the test, a corpse is not) lands much lower: the engine poses the hitboxes from the
    // `sequence` and `frame` the animation advanced to, nothing written by hand.
    let standing_z = b
        .run(|game, eng| {
            respawn(game, eng, "onehanded");
            top_of_player(eng)
        })
        .expect("the standing player is hit");
    kill_in_the_head(&mut b);
    b.step(300);
    let lying_z = b.run(|_game, eng| {
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        assert_eq!(v.deadflag, DEAD_DEAD);
        v.solid = SOLID_SLIDEBOX;
        top_of_player(eng).expect("the body is hit")
    });
    assert!(
        lying_z < standing_z - 10.0,
        "the pose of the death animation's end is lower: {lying_z} against {standing_z}"
    );
}

// ---- a synthetic player model: runs without the game files ------------------------------------

/// One sequence of the synthetic model: `(label, activity, weight, frames, fps, flags)`.
type Seq = (&'static str, i32, i32, i32, f32, i32);

const SYNTHETIC: [Seq; 18] = [
    ("dummy", 0, 0, 61, 24.0, 1),
    ("idle1", 1, 1, 61, 15.0, 1),
    ("walk", 3, 1, 33, 30.0, 1),
    ("run", 4, 1, 37, 60.0, 1),
    ("crouchrun", 17, 1, 31, 30.0, 1),
    ("crouch_idle", 18, 1, 31, 10.0, 1),
    ("jump", 7, 1, 61, 30.0, 0),
    ("longjump", 8, 1, 61, 30.0, 0),
    ("ref_aim_onehanded", 0, 0, 2, 30.0, 1),
    ("ref_shoot_onehanded", 0, 0, 7, 15.0, 0),
    ("crouch_aim_onehanded", 0, 0, 2, 30.0, 1),
    ("crouch_shoot_onehanded", 0, 0, 7, 15.0, 0),
    ("gut_flinch", 0, 0, 2, 30.0, 0),
    ("head_flinch", 0, 0, 2, 30.0, 0),
    ("death2", 36, 2, 41, 30.0, 0),
    ("head", 66, 1, 56, 30.0, 0),
    ("crouch_die", 0, 0, 31, 30.0, 0),
    ("left", 36, 1, 66, 30.0, 0),
];

/// A `.mdl` (IDST v10) of one bone and the sequences of [`SYNTHETIC`]: what `StudioPoseData`
/// reads (labels, fps, flags, activities, weights, frames), nothing else.
fn synthetic_player_model() -> Vec<u8> {
    let (bone_off, seq_off) = (244usize, 244usize + 112);
    let mut b = vec![0u8; seq_off + SYNTHETIC.len() * 176];
    b[0..4].copy_from_slice(b"IDST");
    b[4..8].copy_from_slice(&10i32.to_le_bytes());
    b[8..8 + 6].copy_from_slice(b"player");
    let put = |b: &mut Vec<u8>, at: usize, v: i32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut b, 140, 1);
    put(&mut b, 144, bone_off as i32);
    put(&mut b, 164, SYNTHETIC.len() as i32);
    put(&mut b, 168, seq_off as i32);
    put(&mut b, bone_off + 32, -1);
    for c in 0..6 {
        put(&mut b, bone_off + 40 + c * 4, -1);
    }
    for (i, (label, activity, weight, frames, fps, flags)) in SYNTHETIC.iter().enumerate() {
        let at = seq_off + i * 176;
        b[at..at + label.len()].copy_from_slice(label.as_bytes());
        b[at + 32..at + 36].copy_from_slice(&fps.to_le_bytes());
        put(&mut b, at + 36, *flags);
        put(&mut b, at + 40, *activity);
        put(&mut b, at + 44, *weight);
        put(&mut b, at + 56, *frames);
        put(&mut b, at + 120, 1);
    }
    b
}

fn synthetic_index(label: &str) -> i32 {
    SYNTHETIC.iter().position(|s| s.0 == label).expect("a label of the fixture") as i32
}

/// The same checks as the real model's, on the fixture: they run whether the game is installed
/// or not.
#[test]
fn set_animation_on_a_synthetic_model() {
    let bytes = synthetic_player_model();
    let model = StudioPoseData::parse(&bytes).expect("the fixture parses");
    assert_eq!(model.sequences.len(), SYNTHETIC.len());
    assert_eq!(model.sequences[9].label, "ref_shoot_onehanded");
    let map = cs_engine::ServerMap::stub_with_entities("test", spawns(4, 4));
    let mut b = Bench::with_slots(map, 2);
    // The player's model on the server is the team's (`models/player/urban/urban.mdl` for the
    // class joined here), so every player model is the fixture.
    b.server.set_model_loader(Arc::new(move |name| {
        name.starts_with("models/player").then(|| bytes.clone())
    }));
    join(&mut b, 0, 2, 5);

    // The spawn: an `ACT_IDLE` sequence (the only one: `idle1`).
    let spawned = animate(&mut b, |game, eng| {
        game.respawn_player(eng, 0);
    });
    assert_eq!(spawned.sequence, synthetic_index("idle1"));

    // Shooting standing and ducking, reloading has no sequence in the fixture (falls to 0).
    let shot = animate(&mut b, |game, eng| {
        game.set_anim_extension(0, "onehanded");
        game.set_animation(eng, 0, PlayerAnim::Attack1, 0);
    });
    assert_eq!(shot.sequence, synthetic_index("ref_shoot_onehanded"));
    assert_eq!((shot.frame, shot.framerate, shot.activity), (0.0, 1.0, 28));
    let crouch = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().flags |= FL_DUCKING;
        game.set_animation(eng, 0, PlayerAnim::Attack1, 0);
    });
    assert_eq!(crouch.sequence, synthetic_index("crouch_shoot_onehanded"));

    // The gait by speed.
    let run = animate(&mut b, |game, eng| {
        game.respawn_player(eng, 0);
        game.set_anim_extension(0, "onehanded");
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.flags |= 1 << 9;
        v.velocity = Vec3::new(250.0, 0.0, 0.0);
        game.set_animation(eng, 0, PlayerAnim::Walk, 0);
    });
    assert_eq!(run.gait, synthetic_index("run"));
    let walk = animate(&mut b, |game, eng| {
        let id = eng.client_edict(0);
        eng.ent_mut(id).unwrap().velocity = Vec3::new(100.0, 0.0, 0.0);
        game.set_animation(eng, 0, PlayerAnim::Walk, 0);
    });
    assert_eq!(walk.gait, synthetic_index("walk"));
    assert_eq!(walk.sequence, synthetic_index("ref_aim_onehanded"));
    let jump = animate(&mut b, |game, eng| {
        game.respawn_player(eng, 0);
        game.set_animation(eng, 0, PlayerAnim::Idle, 0);
        game.set_animation(eng, 0, PlayerAnim::Jump, 0);
    });
    assert_eq!(jump.gait, synthetic_index("jump"));

    // Death by hit group: the head's sequence, and a generic hit picks among the weighted
    // `ACT_DIESIMPLE` ones (`death2` and `left`).
    let head = animate(&mut b, |game, eng| {
        game.respawn_player(eng, 0);
        game.set_animation(eng, 0, PlayerAnim::Idle, 0);
        game.set_animation(eng, 0, PlayerAnim::Die, 1);
    });
    assert_eq!(head.sequence, synthetic_index("head"));
    let flinch = animate(&mut b, |game, eng| {
        game.respawn_player(eng, 0);
        game.set_animation(eng, 0, PlayerAnim::Idle, 0);
        game.set_animation(eng, 0, PlayerAnim::Flinch, 3);
    });
    assert_eq!(flinch.sequence, synthetic_index("gut_flinch"));
}

#[test]
fn hostile_angles_and_speeds_never_panic_the_animation() {
    let Some((mut b, _model)) = with_model() else {
        return;
    };
    b.run(|game, eng| {
        respawn(game, eng, "onehanded");
        let id = eng.client_edict(0);
        for value in [f32::MAX, -f32::MAX, 1.0e30, -1.0e30, 720.5, f32::MIN_POSITIVE] {
            let v = eng.ent_mut(id).unwrap();
            v.angles = Vec3::new(value, value, value);
            v.velocity = Vec3::new(value, -value, value);
            v.frame = value;
            v.animtime = value;
            v.flags |= 1 << 9;
            let cmd = GoldSrcUserCmd {
                msec: 255,
                viewangles: Vec3::new(value, value, value),
                ..Default::default()
            };
            common::run_cmd(game, eng, 0, &cmd, 0);
            for anim in [PlayerAnim::Walk, PlayerAnim::Idle, PlayerAnim::Attack1, PlayerAnim::Jump] {
                game.set_animation(eng, 0, anim, 0);
            }
        }
    });
    b.step(3);
}
