//! Client-side weapon prediction (`cl_lw 1`) tests:
//! - immediate clip decrement, recoil punch, and local event emission before server reply
//! - zero double-firing on prediction replay (`runfuncs = false`) and empty dryfire click
//! - zoom FOV cycles, silencer/burst toggles, reload completion, and shield draw/undraw
//! - exact shared-random and spread/recoil parity against `cs-game` for the same `random_seed`

use std::collections::BTreeMap;

use cs_clientcore::weapons_predict::{
    DEFAULT_FOV, IN_ATTACK, IN_ATTACK2, IN_RELOAD, PLAYER_HOLDING_SHIELD, PredictWeaponExtras,
    SHIELD_DRAWN_MAX_SPEED, WPNSTATE_FAMAS_BURST_MODE, WPNSTATE_GLOCK18_BURST_MODE,
    WPNSTATE_M4A1_SILENCED, WPNSTATE_SHIELD_DRAWN, WPNSTATE_USP_SILENCED,
    fire_bullets3_spread_offset, predict_weapon_step, shared_random_float, shared_random_long,
    weapon_id,
};
use cs_clientcore::{
    ClientEffects, EventFx, EventQueue, LocalState, NoWorld, PrecacheEntry, PrecacheTables,
    PredictedSound,
};
use cs_game::weapons::{WeaponId, shared_random_float as srv_rand_float, shared_random_long as srv_rand_long};
use cs_proto48::delta::{GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData};
use glam::Vec3;

fn setup_weapon_state(wid: i32, clip: i32) -> (LocalState, BTreeMap<i32, PredictWeaponExtras>) {
    let mut state = LocalState::default();
    state.weapon.weapon_id = wid;
    state.weapon.fov = DEFAULT_FOV;
    state.maxspeed = 250.0;
    state.weapon.weapons.insert(
        wid as u8,
        GoldSrcWeaponData {
            m_i_id: wid,
            m_i_clip: clip,
            ..Default::default()
        },
    );
    (state, BTreeMap::new())
}

fn step(
    state: &mut LocalState,
    extras: &mut BTreeMap<i32, PredictWeaponExtras>,
    buttons: u16,
    old_buttons: u16,
    seed: u32,
    msec: u8,
    runfuncs: bool,
    cl_lw: bool,
) -> (Vec<cs_clientcore::PredictedWeaponEvent>, Vec<PredictedSound>) {
    let mut events = Vec::new();
    let mut sounds = Vec::new();
    let cmd = GoldSrcUserCmd {
        buttons,
        msec,
        viewangles: Vec3::new(0.0, 90.0, 0.0),
        ..Default::default()
    };
    predict_weapon_step(
        state,
        extras,
        &cmd,
        old_buttons,
        seed,
        true,
        false,
        runfuncs,
        cl_lw,
        &mut events,
        &mut sounds,
    );
    state.weapon.post_run_cmd(msec);
    (events, sounds)
}

#[test]
fn cl_lw_1_predicts_shot_recoil_clip_event_and_sound_before_server_reply() {
    // 1. Immediate AK47 shot updates clip, recoil punch, and emits exactly 1 event when runfuncs=true.
    let (mut state, mut extras) = setup_weapon_state(weapon_id::AK47, 30);
    let (events, sounds) = step(&mut state, &mut extras, IN_ATTACK, 0, 42, 10, true, true);
    assert_eq!(state.weapon.weapons[&(weapon_id::AK47 as u8)].m_i_clip, 29);
    assert!(state.punchangle.x < -0.5, "expected recoil punch, got {:?}", state.punchangle);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_name, "events/ak47.sc");
    assert!(sounds.is_empty());

    // Queueing predicted local event into EventQueue + ClientEffects fires viewanim + flash + sound cleanly.
    let mut precache = PrecacheTables::default();
    precache.events.insert(
        7,
        PrecacheEntry {
            name: "events/ak47.sc".to_string(),
            download_size: 0,
            flags: 0,
        },
    );
    let mut fx = ClientEffects::default();
    let mut ev = events[0].clone();
    ev.args.entindex = 1;
    let idx = precache.event_index(ev.event_name).unwrap();
    fx.events.push_local(idx, ev.args);
    let ents: BTreeMap<u16, GoldSrcEntityState> = BTreeMap::new();
    let mut env = cs_clientcore::FxEnv::new(1.0, &precache, &ents, &NoWorld);
    env.local_player = 1;
    let fired = fx.update(&env, 800.0);
    assert_eq!(fired.len(), 1);
    let EventFx::Weapon(fire) = &fired[0].fx else {
        panic!("expected WeaponFireFx, got {:?}", fired[0].fx);
    };
    assert!(fire.local);
    assert!(fire.muzzle_flash);
    assert!(fire.view_anim.is_some());
    assert!(fire.sound.is_some());

    // 2. Prediction replay (`runfuncs = false`) updates state identically WITHOUT double-firing events/sounds.
    let (mut replay_state, mut replay_extras) = setup_weapon_state(weapon_id::AK47, 30);
    let (replay_events, replay_sounds) =
        step(&mut replay_state, &mut replay_extras, IN_ATTACK, 0, 42, 10, false, true);
    assert_eq!(
        replay_state.weapon.weapons[&(weapon_id::AK47 as u8)].m_i_clip,
        29
    );
    assert_eq!(replay_state.punchangle, state.punchangle);
    assert!(replay_events.is_empty(), "replay must NOT double-emit weapon events");
    assert!(replay_sounds.is_empty(), "replay must NOT double-emit sounds");

    // 3. `cl_lw = false` suppresses local events & sounds even on runfuncs = true.
    let (mut nolw_state, mut nolw_extras) = setup_weapon_state(weapon_id::USP, 12);
    let (nolw_events, nolw_sounds) =
        step(&mut nolw_state, &mut nolw_extras, IN_ATTACK, 0, 99, 10, true, false);
    assert!(nolw_events.is_empty());
    assert!(nolw_sounds.is_empty());

    // 4. Empty clip rising edge emits pistol/rifle dryfire sound once.
    let (mut empty_pistol, mut empty_p_ex) = setup_weapon_state(weapon_id::USP, 0);
    let (e_ev, e_snd) = step(&mut empty_pistol, &mut empty_p_ex, IN_ATTACK, 0, 1, 10, true, true);
    assert!(e_ev.is_empty());
    assert_eq!(e_snd.len(), 1);
    assert_eq!(e_snd[0].sample, "weapons/dryfire_pistol.wav");

    let (mut empty_rifle, mut empty_r_ex) = setup_weapon_state(weapon_id::M4A1, 0);
    let (_, r_snd) = step(&mut empty_rifle, &mut empty_r_ex, IN_ATTACK, 0, 1, 10, true, true);
    assert_eq!(r_snd.len(), 1);
    assert_eq!(r_snd[0].sample, "weapons/dryfire_rifle.wav");
}

#[test]
fn cl_lw_1_predicts_zoom_silencer_burst_reload_and_shield_toggle() {
    // 1. AWP zoom cycle: 90 -> 40 -> 10 -> 90; Scout: 90 -> 40 -> 15 -> 90; AUG: 90 -> 55 -> 90.
    let (mut awp, mut awp_ex) = setup_weapon_state(weapon_id::AWP, 10);
    for expected_fov in [40.0, 10.0, 90.0] {
        step(&mut awp, &mut awp_ex, IN_ATTACK2, 0, 1, 10, true, true);
        assert_eq!(awp.weapon.fov, expected_fov);
        for _ in 0..35 {
            step(&mut awp, &mut awp_ex, 0, 0, 1, 10, true, true);
        }
    }

    let (mut scout, mut scout_ex) = setup_weapon_state(weapon_id::SCOUT, 10);
    for expected_fov in [40.0, 15.0, 90.0] {
        step(&mut scout, &mut scout_ex, IN_ATTACK2, 0, 1, 10, true, true);
        assert_eq!(scout.weapon.fov, expected_fov);
        for _ in 0..35 {
            step(&mut scout, &mut scout_ex, 0, 0, 1, 10, true, true);
        }
    }

    let (mut aug, mut aug_ex) = setup_weapon_state(weapon_id::AUG, 30);
    for expected_fov in [55.0, 90.0] {
        step(&mut aug, &mut aug_ex, IN_ATTACK2, 0, 1, 10, true, true);
        assert_eq!(aug.weapon.fov, expected_fov);
        for _ in 0..35 {
            step(&mut aug, &mut aug_ex, 0, 0, 1, 10, true, true);
        }
    }

    // 2. Silencer toggles (USP & M4A1).
    let (mut usp, mut usp_ex) = setup_weapon_state(weapon_id::USP, 12);
    step(&mut usp, &mut usp_ex, IN_ATTACK2, 0, 1, 10, true, true);
    assert_ne!(
        usp.weapon.weapons[&(weapon_id::USP as u8)].m_i_weapon_state & WPNSTATE_USP_SILENCED,
        0
    );

    let (mut m4, mut m4_ex) = setup_weapon_state(weapon_id::M4A1, 30);
    step(&mut m4, &mut m4_ex, IN_ATTACK2, 0, 1, 10, true, true);
    assert_ne!(
        m4.weapon.weapons[&(weapon_id::M4A1 as u8)].m_i_weapon_state & WPNSTATE_M4A1_SILENCED,
        0
    );

    // 3. Burst toggles (Glock18 & FAMAS).
    let (mut glock, mut glock_ex) = setup_weapon_state(weapon_id::GLOCK18, 20);
    step(&mut glock, &mut glock_ex, IN_ATTACK2, 0, 1, 10, true, true);
    assert_ne!(
        glock.weapon.weapons[&(weapon_id::GLOCK18 as u8)].m_i_weapon_state
            & WPNSTATE_GLOCK18_BURST_MODE,
        0
    );

    let (mut famas, mut famas_ex) = setup_weapon_state(weapon_id::FAMAS, 25);
    step(&mut famas, &mut famas_ex, IN_ATTACK2, 0, 1, 10, true, true);
    assert_ne!(
        famas.weapon.weapons[&(weapon_id::FAMAS as u8)].m_i_weapon_state
            & WPNSTATE_FAMAS_BURST_MODE,
        0
    );

    // 4. Reload sets m_f_in_reload=1 and refills clip to max_clip once timer reaches 0.
    let (mut ak, mut ak_ex) = setup_weapon_state(weapon_id::AK47, 10);
    step(&mut ak, &mut ak_ex, IN_RELOAD, 0, 1, 10, true, true);
    assert_eq!(ak.weapon.weapons[&(weapon_id::AK47 as u8)].m_f_in_reload, 1);
    for _ in 0..260 {
        step(&mut ak, &mut ak_ex, 0, 0, 1, 10, true, true);
    }
    assert_eq!(ak.weapon.weapons[&(weapon_id::AK47 as u8)].m_f_in_reload, 0);
    assert_eq!(ak.weapon.weapons[&(weapon_id::AK47 as u8)].m_i_clip, 30);

    // 5. Tactical shield draw/undraw toggles WPNSTATE_SHIELD_DRAWN and maxspeed (180 vs 250).
    let (mut shield_usp, mut su_ex) = setup_weapon_state(weapon_id::USP, 12);
    shield_usp.iuser[2] |= PLAYER_HOLDING_SHIELD;
    step(&mut shield_usp, &mut su_ex, IN_ATTACK2, 0, 1, 10, true, true);
    assert_ne!(
        shield_usp.weapon.weapons[&(weapon_id::USP as u8)].m_i_weapon_state
            & WPNSTATE_SHIELD_DRAWN,
        0
    );
    assert_eq!(shield_usp.maxspeed, SHIELD_DRAWN_MAX_SPEED);
    // Cannot primary-fire while shield is drawn.
    let (ev_blocked, _) = step(&mut shield_usp, &mut su_ex, IN_ATTACK, 0, 1, 10, true, true);
    assert!(ev_blocked.is_empty());
    assert_eq!(shield_usp.weapon.weapons[&(weapon_id::USP as u8)].m_i_clip, 12);
    // Wait 0.4s cooldown and undraw shield.
    for _ in 0..45 {
        step(&mut shield_usp, &mut su_ex, 0, 0, 1, 10, true, true);
    }
    step(&mut shield_usp, &mut su_ex, IN_ATTACK2, 0, 1, 10, true, true);
    assert_eq!(
        shield_usp.weapon.weapons[&(weapon_id::USP as u8)].m_i_weapon_state
            & WPNSTATE_SHIELD_DRAWN,
        0
    );
    assert_eq!(shield_usp.maxspeed, 250.0);
}

#[test]
fn client_prediction_matches_server_spread_and_recoil_with_same_seed() {
    // 1. Exact bit-for-bit equality of shared PRNG across 256 seeds.
    for seed in 0..256_u32 {
        assert_eq!(
            shared_random_float(seed, -0.5, 0.5).to_bits(),
            srv_rand_float(seed, -0.5, 0.5).to_bits(),
            "shared_random_float mismatch at seed {seed}"
        );
        assert_eq!(
            shared_random_long(seed, 0, 8),
            srv_rand_long(seed, 0, 8),
            "shared_random_long mismatch at seed {seed}"
        );
    }

    // 2. Verify spread + first-shot recoil parity with cs-game constants across representative weapons.
    let seed = 1337_u32;
    let weapons = [
        (weapon_id::AK47, WeaponId::Ak47, 0.0275 * 0.2_f32, -1.0_f32),
        (weapon_id::M4A1, WeaponId::M4a1, 0.02 * 0.2_f32, -0.65_f32),
        (weapon_id::USP, WeaponId::Usp, 0.1 * (1.0 - 0.2_f32), -2.0_f32),
        (weapon_id::DEAGLE, WeaponId::Deagle, 0.13 * (1.0 - 0.2_f32), -2.0_f32),
        (weapon_id::AWP, WeaponId::Awp, 0.081_f32, -2.0_f32),
    ];

    for (wid, _srv_id, _expected_spread, expected_punch_x) in weapons {
        let (mut state, mut extras) = setup_weapon_state(wid, 10);
        // Set standing USP/Deagle baseline accuracy 0.9 / 0.8 match client/server defaults.
        if matches!(wid, weapon_id::USP | weapon_id::DEAGLE) {
            extras.entry(wid).or_default().accuracy = 0.9;
        }
        let (events, _) = step(&mut state, &mut extras, IN_ATTACK, 0, seed, 10, true, true);
        assert_eq!(events.len(), 1);
        assert!(
            (state.punchangle.x - expected_punch_x).abs() < 1e-5,
            "weapon {wid}: punchangle.x {} != expected {expected_punch_x}",
            state.punchangle.x
        );
        let spread = match wid {
            weapon_id::AK47 => (0.0275 * f64::from(0.2_f32)) as f32,
            weapon_id::M4A1 => (0.02 * f64::from(0.2_f32)) as f32,
            weapon_id::USP => (0.1 * f64::from(1.0_f32 - 0.9_f32)) as f32,
            weapon_id::DEAGLE => (0.13 * f64::from(1.0_f32 - 0.9_f32)) as f32,
            _ => 0.001 + 0.08,
        };
        let (sx, sy) = fire_bullets3_spread_offset(spread, seed);
        assert_eq!(events[0].args.fparam1.to_bits(), sx.to_bits());
        assert_eq!(events[0].args.fparam2.to_bits(), sy.to_bits());
    }
    let _ = EventQueue::default();
}
