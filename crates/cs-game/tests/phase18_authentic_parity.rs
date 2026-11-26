//! Phase 18 Stage 1A authentic CS 1.6 / CZ parity integration tests:
//! 1. Tick-0 `SIGNAL_BUY` (immediate spawn + freeze-time `B-4-2`, `ak47`, `m4a1`, `autobuy`,
//!    `nighthawk`, `nvg`, and full Build 10210 alias/drop/price/sound audit).
//! 2. `M4A1` & `USP` silencer `m_iWeaponState` persistence across fire/reload/switch, attach/detach
//!    attack lock, `AUG`/`SG552` `fov=55` + `0.135s` zoomed fire rate, `AWP` `40/10` & `Scout` `40/15`,
//!    and `models/p_*.mdl` 3rd-person weapon models.
//! 3. Footstep `MSG_PAS` echo suppression: stepping mover receives `0` `pl_step` `svc_sound`
//!    packets while nearby observers/enemies DO receive them.
//! 4. Crimson `TE_BLOODSPRITE` (`color == 247`), `{blood` decals, DeathCam (`DEAD_DYING`,
//!    `view_ofs.z == -8`, killer `iuser3`), `ClCorpse`, and Observer mode/target cycling.
//! 5. Official `.nav` loading + auto-nav connectivity on `dm_rusthouse`, `de_dust2`, and `de_inferno`.
//! 6. 5v5 ZBot round verifying freeze-time calm & meta buys, `250 u/s` knife rollout, pre-contact
//!    rifle switch, smooth continuous velocity vectors (non-45°-quantized), kills with crimson
//!    blood + radio callouts, and full C4 plant/defuse loop.

mod common;

use std::sync::Arc;

use common::{Bench, discover, entity, map_named, run_cmd, spawn_at, stub_spawns};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_bot::path::{PathCostParams, PathObstacles, find_path};
use cs_engine::consts::FL_ONGROUND;
use cs_engine::{EngineApi, GameDll, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::buy::SIGNAL_BUY;
use cs_game::observer::{OBS_CHASE_FREE, OBS_IN_EYE, OBS_ROAMING};
use cs_game::player::{ArmorType, DEAD_DYING, Team};
use cs_game::rules::WinStatus;
use cs_game::weapons::{
    WPNSTATE_M4A1_SILENCED, WPNSTATE_USP_SILENCED, WeaponId, is_primary_weapon, weapon_info,
};
use cs_proto48::delta::{GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::messages::SvcMessage;
use cs_proto48::tempent::{TempEffect, te};
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pm::consts::{IN_ATTACK, IN_ATTACK2, IN_FORWARD, IN_JUMP, IN_RELOAD};
use cs_vfs::Vfs;
use glam::Vec3;

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;

fn minimal_bsp_raw() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

/// Real flat floor BSP (`z < 0` solid across all 4 hulls) for deterministic movement, footsteps,
/// and bot rounds.
fn named_floor_bsp(name: &str, entities: Vec<Entity>) -> ServerMap {
    let leaf = |contents: i32| Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    };
    let planes = vec![
        Plane {
            normal: Vec3::Z,
            dist: 0.0,
            plane_type: 2,
        },
        Plane {
            normal: Vec3::Z,
            dist: 36.0,
            plane_type: 2,
        },
        Plane {
            normal: Vec3::Z,
            dist: 32.0,
            plane_type: 2,
        },
        Plane {
            normal: Vec3::Z,
            dist: 18.0,
            plane_type: 2,
        },
    ];
    let nodes = vec![
        Node {
            plane: 0,
            children: [-2, -1],
            mins: [0; 3],
            maxs: [0; 3],
            first_face: 0,
            num_faces: 0,
        },
        Node {
            plane: 0,
            children: [-2, -1],
            mins: [0; 3],
            maxs: [0; 3],
            first_face: 0,
            num_faces: 0,
        },
    ];
    let clipnodes = vec![
        ClipNode {
            plane: 1,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        },
        ClipNode {
            plane: 2,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        },
        ClipNode {
            plane: 3,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        },
        ClipNode {
            plane: 3,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        },
    ];
    let leafs = vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)];
    let models = vec![BspModel {
        mins: Vec3::new(-4096.0, -4096.0, -64.0),
        maxs: Vec3::new(4096.0, 4096.0, 4096.0),
        origin: Vec3::ZERO,
        head_nodes: [0, 0, 1, 2],
        vis_leafs: 1,
        first_face: 0,
        num_faces: 0,
    }];

    let bsp = BspMap {
        name: name.into(),
        skyname: String::new(),
        entities,
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes,
        vertices_raw: Vec::new(),
        visibility: Vec::new(),
        nodes,
        texinfos: Vec::new(),
        faces: Vec::new(),
        lighting: Vec::new(),
        clipnodes,
        leafs,
        mark_surfaces: Vec::new(),
        edges: Vec::new(),
        surf_edges: Vec::new(),
        models,
        textures: Vec::new(),
        lightmap_atlas: LightmapAtlas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            packed_faces: 0,
        },
        render_vertices: Vec::new(),
        render_indices: Vec::new(),
        batches: Vec::new(),
    };
    ServerMap::from_bsp(name, &minimal_bsp_raw(), Arc::new(bsp)).expect("BSP header")
}

fn size_zones(eng: &mut EngineApi<'_>, class: &str, half: Vec3) {
    let mut after = None;
    while let Some(id) = eng.find_by_classname(after, class) {
        after = Some(id);
        let origin = eng.ent(id).unwrap().origin;
        eng.set_size(id, -half, half);
        eng.set_origin(id, origin);
    }
}

fn bot_slots(eng: &EngineApi<'_>) -> Vec<u8> {
    (0..eng.max_clients())
        .filter(|&s| eng.is_fake_client(s))
        .collect()
}

fn weapon_data_for(b: &mut Bench, slot: u8, id: WeaponId) -> GoldSrcWeaponData {
    b.run(move |game, eng| {
        let mut table = [GoldSrcWeaponData::default(); 64];
        assert!(game.get_weapon_data(eng, slot, &mut table));
        table[id as i32 as usize]
    })
}

fn sound_index(b: &mut Bench, sample: &str) -> u16 {
    let owned = sample.to_string();
    b.run(move |_, eng| eng.sound_index(&owned))
        .unwrap_or_else(|| panic!("sound {sample} must be precached"))
}

/// Runs one player command as the engine's `SV_RunCmd` does: populates `v.button`, clears
/// `fixangle`, writes `v_angle`, then executes `run_usercmd` + `post_usercmd`.
fn drive_cmd(
    game: &mut CsGame,
    eng: &mut EngineApi<'_>,
    slot: u8,
    cmd: &GoldSrcUserCmd,
    seed: u32,
) {
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.button = i32::from(cmd.buttons);
        v.fixangle = 0;
        v.v_angle = cmd.viewangles;
    }
    run_cmd(game, eng, slot, cmd, seed);
}

// -----------------------------------------------------------------------------------------------
// 1. Server Task 1: Instant Tick-0 Buy (`SIGNAL_BUY` at spawn + freeze time) & Complete Aliases
// -----------------------------------------------------------------------------------------------

#[test]
fn tick_zero_buy_and_complete_aliases() {
    let mut ents = stub_spawns();
    ents.push(entity(&[
        ("classname", "func_buyzone"),
        ("origin", "200 0 0"),
        ("team", "2"),
    ]));
    ents.push(entity(&[
        ("classname", "func_buyzone"),
        ("origin", "-200 0 0"),
        ("team", "1"),
    ]));
    ents.push(entity(&[
        ("classname", "info_bomb_target"),
        ("origin", "0 600 0"),
    ]));

    let mut b = Bench::with_userinfo(
        ServerMap::stub_with_entities("tick0_buy", ents),
        4,
        &[("_vgui_menus", "0")],
    );
    b.idle_cmds = true;
    b.step(4);
    b.run(|_, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_freezetime", "6");
        size_zones(eng, "func_buyzone", Vec3::splat(160.0));
    });

    // Tick-0 T spawn: BEFORE any tick/settle elapses, `SIGNAL_BUY` must ALREADY be set and
    // classic `B-4-2` (`buy` -> `menuselect 4` -> `menuselect 2` -> AK-47) must succeed instantly!
    let (sig_t, active_t, money_t) = b.run(|game, eng| {
        game.restart_round_now(eng);
        assert!(game.spawn_player(eng, 0, Team::Terrorist, 1));
        game.state_mut().players[0].as_mut().unwrap().money = 16_000;
        let sig = game.state().players[0]
            .as_ref()
            .unwrap()
            .test_zone_signals();
        game.client_command(eng, 0, &["buy".into()]);
        game.client_command(eng, 0, &["menuselect".into(), "4".into()]);
        game.client_command(eng, 0, &["menuselect".into(), "2".into()]);
        let p = game.state().players[0].as_ref().unwrap();
        (sig, p.active_item(), p.money)
    });
    assert_ne!(
        sig_t & SIGNAL_BUY,
        0,
        "SIGNAL_BUY must be populated immediately on tick 0 at spawn"
    );
    assert_eq!(
        active_t,
        Some(WeaponId::Ak47),
        "tick-0 B-4-2 for Terrorist must immediately buy and auto-equip AK-47"
    );
    assert_eq!(money_t, 16_000 - 2500);

    // Tick-0 CT spawn during freeze time (after previous T life ended): `autobuy` (F1) buys M4A1
    // immediately on tick 0!
    let (sig_ct, active_ct, money_ct) = b.run(|game, eng| {
        game.damage_player(eng, 0, None, 500.0, cs_game::player::dmg::GENERIC);
        assert!(game.spawn_player(eng, 0, Team::Ct, 1));
        game.state_mut().players[0].as_mut().unwrap().money = 16_000;
        let sig = game.state().players[0]
            .as_ref()
            .unwrap()
            .test_zone_signals();
        game.client_command(eng, 0, &["autobuy".into()]);
        let p = game.state().players[0].as_ref().unwrap();
        (sig, p.active_item(), p.money)
    });
    assert_ne!(sig_ct & SIGNAL_BUY, 0);
    assert_eq!(
        active_ct,
        Some(WeaponId::M4a1),
        "tick-0 autobuy (F1) for CT must immediately buy and equip M4A1"
    );
    assert!(
        money_ct < 16_000 - 3100,
        "autobuy should also purchase armor/defuser/ammo"
    );

    // Verify `SIGNAL_BUY` remains active throughout `mp_freezetime` (100 ticks = 1.0s frozen).
    b.step(100);
    let frozen_buy_ok = b.run(|game, eng| {
        let sig = game.state().players[0]
            .as_ref()
            .unwrap()
            .test_zone_signals();
        game.state_mut().players[0].as_mut().unwrap().money = 16_000;
        game.client_command(eng, 0, &["nighthawk".into()]);
        game.client_command(eng, 0, &["nvg".into()]);
        let p = game.state().players[0].as_ref().unwrap();
        (
            sig & SIGNAL_BUY != 0,
            game.carried_weapons(0).contains(&WeaponId::Deagle),
            p.test_has_night_vision(),
        )
    });
    assert_eq!(
        frozen_buy_ok,
        (true, true, true),
        "SIGNAL_BUY must persist through mp_freezetime and accept nighthawk & nvg aliases"
    );

    // Audit EVERY Build 10210 weapon alias for exact price, auto-equip, weaponbox drop (`models/w_*.mdl`),
    // and pickup sound (`items/gunpickup2.wav`) + ammo (`items/9mmclip1.wav`).
    let gun_pickup = sound_index(&mut b, "items/gunpickup2.wav");
    let clip_pickup = sound_index(&mut b, "items/9mmclip1.wav");

    let t_aliases: &[(&str, WeaponId)] = &[
        ("galil", WeaponId::Galil),
        ("defender", WeaponId::Galil),
        ("ak47", WeaponId::Ak47),
        ("cv47", WeaponId::Ak47),
        ("scout", WeaponId::Scout),
        ("sg552", WeaponId::Sg552),
        ("krieg552", WeaponId::Sg552),
        ("awp", WeaponId::Awp),
        ("magnum", WeaponId::Awp),
        ("g3sg1", WeaponId::G3sg1),
        ("d3au1", WeaponId::G3sg1),
        ("usp", WeaponId::Usp),
        ("km45", WeaponId::Usp),
        ("p228", WeaponId::P228),
        ("228compact", WeaponId::P228),
        ("deagle", WeaponId::Deagle),
        ("nighthawk", WeaponId::Deagle),
        ("elites", WeaponId::Elite),
        ("m3", WeaponId::M3),
        ("12gauge", WeaponId::M3),
        ("xm1014", WeaponId::Xm1014),
        ("autoshotgun", WeaponId::Xm1014),
        ("mac10", WeaponId::Mac10),
        ("mp5", WeaponId::Mp5n),
        ("smg", WeaponId::Mp5n),
        ("ump45", WeaponId::Ump45),
        ("p90", WeaponId::P90),
        ("c90", WeaponId::P90),
        ("m249", WeaponId::M249),
    ];
    let ct_aliases: &[(&str, WeaponId)] = &[
        ("glock", WeaponId::Glock18),
        ("famas", WeaponId::Famas),
        ("clarion", WeaponId::Famas),
        ("m4a1", WeaponId::M4a1),
        ("aug", WeaponId::Aug),
        ("bullpup", WeaponId::Aug),
        ("sg550", WeaponId::Sg550),
        ("krieg550", WeaponId::Sg550),
        ("fn57", WeaponId::FiveSeven),
        ("tmp", WeaponId::Tmp),
        ("mp", WeaponId::Tmp),
    ];

    for (team, aliases) in [(Team::Terrorist, t_aliases), (Team::Ct, ct_aliases)] {
        for &(alias, expected_id) in aliases {
            let expected_cost = weapon_info(expected_id).expect("weapon info").cost;
            b.probe.clear_messages();
            let (carried, spent, dropped_models) = b.run(move |game, eng| {
                // Kill previous life so `spawn_player` resets primary/secondary cleanly.
                game.damage_player(eng, 0, None, 500.0, cs_game::player::dmg::GENERIC);
                assert!(game.spawn_player(eng, 0, team, 1));
                game.state_mut().players[0].as_mut().unwrap().money = 16_000;
                game.client_command(eng, 0, &[alias.into()]);
                let carried = game.carried_weapons(0);
                let spent = 16_000 - game.state().players[0].as_ref().unwrap().money;
                let mut wboxes = Vec::new();
                let mut cur = None;
                while let Some(wb) = eng.find_by_classname(cur, "weaponbox") {
                    cur = Some(wb);
                    if let Some(v) = eng.ent(wb) {
                        wboxes.push(v.model.clone());
                    }
                }
                (carried, spent, wboxes)
            });
            // Pump one frame for sound delivery if needed.
            b.step(2);
            assert!(
                carried.contains(&expected_id),
                "alias `{alias}` for {team:?} must grant {expected_id:?}"
            );
            assert_eq!(
                spent, expected_cost,
                "alias `{alias}` must deduct exact Build 10210 cost"
            );
            let heard_gun = b
                .probe
                .messages()
                .iter()
                .any(|m| matches!(m, SvcMessage::Sound(s) if s.sound_index == gun_pickup));
            assert!(heard_gun, "buying `{alias}` must emit items/gunpickup2.wav");
            // Secondary aliases replace the spawn pistol and drop a `models/w_*.mdl` weaponbox.
            if matches!(
                expected_id,
                WeaponId::Deagle | WeaponId::P228 | WeaponId::Elite | WeaponId::FiveSeven
            ) {
                assert!(
                    dropped_models
                        .iter()
                        .any(|m| m.starts_with("models/w_") && m.ends_with(".mdl")),
                    "buying `{alias}` must drop previous weapon as models/w_*.mdl weaponbox"
                );
            }
        }
    }

    // Buying a second primary drops the first primary as `models/w_ak47.mdl` and auto-equips the new one.
    let (active_after_swap, dropped_ak) = b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Terrorist, 1));
        game.state_mut().players[0].as_mut().unwrap().money = 16_000;
        game.client_command(eng, 0, &["ak47".into()]);
        game.client_command(eng, 0, &["awp".into()]);
        let active = game.state().players[0].as_ref().unwrap().active_item();
        let mut has_ak_box = false;
        let mut cur = None;
        while let Some(wb) = eng.find_by_classname(cur, "weaponbox") {
            cur = Some(wb);
            if eng.ent(wb).is_some_and(|v| v.model == "models/w_ak47.mdl") {
                has_ak_box = true;
            }
        }
        (active, has_ak_box)
    });
    assert_eq!(active_after_swap, Some(WeaponId::Awp));
    assert!(
        dropped_ak,
        "buying AWP while holding AK-47 must drop models/w_ak47.mdl"
    );

    // Equipment + ammo aliases (`primammo`, `secammo`, `buyammo1`, `buyammo2`, `vest`, `vesthelm`,
    // `flash`, `hegren`, `sgren`, `nvgs`, `defuser`, `shield`).
    b.probe.clear_messages();
    let equip_ok = b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 1));
        game.state_mut().players[0].as_mut().unwrap().money = 16_000;
        for cmd in [
            "m4a1", "primammo", "secammo", "vesthelm", "flash", "hegren", "sgren", "nvgs",
            "defuser",
        ] {
            game.client_command(eng, 0, &[cmd.into()]);
        }
        let p = game.state().players[0].as_ref().unwrap();
        let ok_base = p.kevlar == ArmorType::VestHelm
            && p.has_defuser
            && p.test_has_night_vision()
            && game.carried_weapons(0).contains(&WeaponId::HeGrenade)
            && game.carried_weapons(0).contains(&WeaponId::Flashbang)
            && game.carried_weapons(0).contains(&WeaponId::SmokeGrenade);
        game.client_command(eng, 0, &["shield".into()]);
        let owns_shield = game.state().players[0].as_ref().unwrap().owns_shield;
        (ok_base, owns_shield)
    });
    b.step(2);
    assert_eq!(equip_ok, (true, true));
    assert!(
        b.probe
            .messages()
            .iter()
            .any(|m| matches!(m, SvcMessage::Sound(s) if s.sound_index == clip_pickup)),
        "primammo/secammo must emit items/9mmclip1.wav"
    );
}

// -----------------------------------------------------------------------------------------------
// 2. Server Task 2/3: M4A1/USP Silencer Persistence, AUG=55 vs AWP=40/10, 3rd-Person Models
// -----------------------------------------------------------------------------------------------

#[test]
fn m4a1_and_usp_silencer_state_persists_and_zoom_fovs_match_regamedll() {
    let mut b = Bench::with_userinfo(
        ServerMap::stub_with_entities("weapons_parity", stub_spawns()),
        2,
        &[("cl_lw", "1")],
    );
    b.run(|game, eng| {
        eng.set_cvar("sv_gravity", "0");
        eng.set_cvar("mp_freezetime", "0");
        game.restart_round_now(eng);
        // Clear `not_killed` so `spawn_player` runs `GiveDefaultItems` (Knife + USP).
        game.test_set_not_killed(0, false);
        assert!(game.spawn_player(eng, 0, Team::Ct, 1));
        game.end_freeze_period(eng);
        game.state_mut().players[0].as_mut().unwrap().money = 16_000;
        game.client_command(eng, 0, &["m4a1".into()]);
        game.client_command(eng, 0, &["primammo".into()]);
        game.client_command(eng, 0, &["secammo".into()]);
    });
    b.step(5);

    // Verify 3rd-person weapon models (`models/p_*.mdl`) are precached and applied on deploy.
    let (p_m4_idx, active_wm) = b.run(|_, eng| {
        let p_m4 = eng.model_index("models/p_m4a1.mdl").unwrap_or(0);
        let p_usp = eng.model_index("models/p_usp.mdl").unwrap_or(0);
        let p_shield_usp = eng
            .model_index("models/shield/p_shield_usp.mdl")
            .unwrap_or(0);
        assert!(p_m4 != 0 && p_usp != 0 && p_shield_usp != 0);
        (p_m4, eng.ent(eng.client_edict(0)).unwrap().weaponmodel)
    });
    assert_eq!(
        active_wm, p_m4_idx,
        "deployed M4A1 must set pev->weaponmodel to p_m4a1.mdl"
    );

    // Wait out M4A1 deploy delay (1.0s).
    b.run(|game, eng| {
        for _ in 0..110 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                0,
            );
        }
    });

    // Toggle M4A1 silencer via IN_ATTACK2 -> WPNSTATE_M4A1_SILENCED immediately replicates and
    // locks primary fire during attach animation (`2.0s` per ReGameDLL `wpn_m4a1.cpp`).
    let (clip_before, clip_locked, clip_after_attach) = b.run(|game, eng| {
        let before = game.weapon_mut(0, WeaponId::M4a1).unwrap().clip;
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_ATTACK2 as u16,
                ..Default::default()
            },
            1,
        );
        // Attempt firing 0.5s into the 2.0s silencer attach: must remain locked!
        for _ in 0..50 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    buttons: IN_ATTACK as u16,
                    ..Default::default()
                },
                2,
            );
        }
        let locked = game.weapon_mut(0, WeaponId::M4a1).unwrap().clip;
        // Finish the remaining 1.6s of attach lock, then fire!
        for _ in 0..170 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                3,
            );
        }
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_ATTACK as u16,
                ..Default::default()
            },
            4,
        );
        let after = game.weapon_mut(0, WeaponId::M4a1).unwrap().clip;
        (before, locked, after)
    });
    assert_eq!(clip_before, 30);
    assert_eq!(
        clip_locked, 30,
        "M4A1 must not fire during silencer attach lock"
    );
    assert_eq!(
        clip_after_attach, 29,
        "M4A1 must fire once attach lock completes"
    );
    assert_ne!(
        weapon_data_for(&mut b, 0, WeaponId::M4a1).m_i_weapon_state & WPNSTATE_M4A1_SILENCED,
        0,
        "M4A1 silencer bit must replicate in WeaponData"
    );

    // Reload M4A1, holster to Knife, re-deploy M4A1 -> silencer state MUST persist!
    let m4_reloaded_clip = b.run(|game, eng| {
        // Wait out the 0.0875s shot cycle so `next_primary_attack < 0` accepts IN_RELOAD.
        for _ in 0..15 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                5,
            );
        }
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_RELOAD as u16,
                ..Default::default()
            },
            5,
        );
        for _ in 0..330 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                6,
            );
        }
        let reloaded = game.weapon_mut(0, WeaponId::M4a1).unwrap().clip;
        assert!(game.select_weapon(eng, 0, WeaponId::Knife));
        for _ in 0..20 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                7,
            );
        }
        assert!(game.select_weapon(eng, 0, WeaponId::M4a1));
        for _ in 0..110 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                8,
            );
        }
        reloaded
    });
    assert_eq!(m4_reloaded_clip, 30, "M4A1 reload must refill clip to 30");
    assert_ne!(
        weapon_data_for(&mut b, 0, WeaponId::M4a1).m_i_weapon_state & WPNSTATE_M4A1_SILENCED,
        0,
        "M4A1 silencer must persist across fire, reload, holster, and re-deploy"
    );

    // Switch to USP, attach silencer, verify 3.0s lock, fire, reload, switch to knife and back.
    let (usp_locked, usp_after) = b.run(|game, eng| {
        assert!(game.select_weapon(eng, 0, WeaponId::Usp));
        for _ in 0..110 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                9,
            );
        }
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_ATTACK2 as u16,
                ..Default::default()
            },
            10,
        );
        for _ in 0..150 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    buttons: IN_ATTACK as u16,
                    ..Default::default()
                },
                11,
            );
        }
        let locked = game.weapon_mut(0, WeaponId::Usp).unwrap().clip;
        for _ in 0..170 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                12,
            );
        }
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_ATTACK as u16,
                ..Default::default()
            },
            13,
        );
        let after = game.weapon_mut(0, WeaponId::Usp).unwrap().clip;
        // Reload + switch away and back.
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_RELOAD as u16,
                ..Default::default()
            },
            14,
        );
        for _ in 0..290 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                15,
            );
        }
        assert!(game.select_weapon(eng, 0, WeaponId::Knife));
        assert!(game.select_weapon(eng, 0, WeaponId::Usp));
        (locked, after)
    });
    assert_eq!(
        usp_locked, 12,
        "USP must not fire during 3.0s silencer attach lock"
    );
    assert_eq!(
        usp_after, 11,
        "USP must fire once silencer attach completes"
    );
    assert_ne!(
        weapon_data_for(&mut b, 0, WeaponId::Usp).m_i_weapon_state & WPNSTATE_USP_SILENCED,
        0,
        "USP silencer must persist across fire, reload, holster, and deploy"
    );

    // AUG zoom: `fov = 55`, zoomed fire interval `0.135s` (vs `0.0825s` unzoomed).
    let (aug_zoom_fov, aug_unzoom_fov, aug_zoom_delay) = b.run(|game, eng| {
        game.state_mut().players[0].as_mut().unwrap().money = 16_000;
        game.client_command(eng, 0, &["aug".into()]);
        for _ in 0..110 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                20,
            );
        }
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_ATTACK2 as u16,
                ..Default::default()
            },
            21,
        );
        let z_fov = eng.ent(eng.client_edict(0)).unwrap().fov;
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_ATTACK as u16,
                ..Default::default()
            },
            22,
        );
        let z_delay = game
            .weapon_mut(0, WeaponId::Aug)
            .unwrap()
            .next_primary_attack;
        for _ in 0..35 {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                23,
            );
        }
        drive_cmd(
            game,
            eng,
            0,
            &GoldSrcUserCmd {
                msec: 10,
                buttons: IN_ATTACK2 as u16,
                ..Default::default()
            },
            24,
        );
        let u_fov = eng.ent(eng.client_edict(0)).unwrap().fov;
        (z_fov, u_fov, z_delay)
    });
    assert_eq!(aug_zoom_fov, 55.0, "AUG zoom must set fov=55");
    assert_eq!(aug_unzoom_fov, 90.0, "AUG unzoom must restore fov=90");
    assert!(
        (aug_zoom_delay - 0.135).abs() < 0.02,
        "AUG zoomed fire rate must slow to ~0.135s, got {aug_zoom_delay}"
    );

    // AWP zoom: `90 -> 40 -> 10 -> 90`; Scout zoom: `90 -> 40 -> 15 -> 90`.
    let (awp_fovs, scout_fovs) = b.run(|game, eng| {
        let mut cycle = |alias: &str| -> [f32; 3] {
            game.state_mut().players[0].as_mut().unwrap().money = 16_000;
            game.client_command(eng, 0, &[alias.into()]);
            for _ in 0..160 {
                drive_cmd(
                    game,
                    eng,
                    0,
                    &GoldSrcUserCmd {
                        msec: 10,
                        ..Default::default()
                    },
                    30,
                );
            }
            let mut fovs = [0.0; 3];
            for f in &mut fovs {
                drive_cmd(
                    game,
                    eng,
                    0,
                    &GoldSrcUserCmd {
                        msec: 10,
                        buttons: IN_ATTACK2 as u16,
                        ..Default::default()
                    },
                    31,
                );
                *f = eng.ent(eng.client_edict(0)).unwrap().fov;
                for _ in 0..35 {
                    drive_cmd(
                        game,
                        eng,
                        0,
                        &GoldSrcUserCmd {
                            msec: 10,
                            ..Default::default()
                        },
                        32,
                    );
                }
            }
            fovs
        };
        (cycle("awp"), cycle("scout"))
    });
    assert_eq!(
        awp_fovs,
        [40.0, 10.0, 90.0],
        "AWP zoom cycle must be 40 -> 10 -> 90"
    );
    assert_eq!(
        scout_fovs,
        [40.0, 15.0, 90.0],
        "Scout zoom cycle must be 40 -> 15 -> 90"
    );
}

// -----------------------------------------------------------------------------------------------
// 3. Server Task 4: Footstep `MSG_PAS` Filtering (0 echo for mover, broadcast to nearby listener)
// -----------------------------------------------------------------------------------------------

#[test]
fn footstep_pas_skips_stepping_client_and_reaches_nearby_listener() {
    let ents = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "0 0 36"),
        spawn_at("info_player_start", "160 0 36"),
        spawn_at("info_player_deathmatch", "400 0 36"),
    ];
    let map = named_floor_bsp("step_pas", ents);
    let mut b = Bench::with_userinfo(map, 4, &[("cl_lw", "1")]);
    let mut listener = b.add_probe("listener");

    b.run(|game, eng| {
        eng.set_cvar("mp_freezetime", "0");
        eng.set_cvar("mp_footsteps", "1");
        game.restart_round_now(eng);
        assert!(game.spawn_player(eng, 0, Team::Ct, 1));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 1));
        assert!(game.place_player(eng, 0, Vec3::new(0.0, 0.0, 36.0)));
        assert!(game.place_player(eng, 1, Vec3::new(140.0, 80.0, 36.0)));
    });
    b.step_both(&mut listener, 10);
    b.probe.clear_messages();
    listener.clear_messages();

    let step_indices: Vec<u16> = (1..=4)
        .filter_map(|i| {
            let name = format!("player/pl_step{i}.wav");
            b.run(move |_, eng| eng.sound_index(&name))
        })
        .collect();
    assert_eq!(
        step_indices.len(),
        4,
        "all four pl_step1..4.wav must be precached"
    );

    // Drive real movement commands for slot 0 at full running speed across the floor so
    // `PM_Move` -> `PM_PlayStepSound` fires footsteps repeatedly.
    for _ in 0..140 {
        b.probe.queue_cmd(GoldSrcUserCmd {
            msec: 10,
            forwardmove: 250.0,
            buttons: IN_FORWARD as u16,
            ..Default::default()
        });
        listener.queue_cmd(GoldSrcUserCmd {
            msec: 10,
            ..Default::default()
        });
        b.step_both(&mut listener, 1);
    }

    let mover_step_echoes = b
        .probe
        .messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::Sound(s) if step_indices.contains(&s.sound_index)))
        .count();
    let listener_steps_from_mover = listener
        .messages()
        .iter()
        .filter(|m| {
            matches!(
                m,
                SvcMessage::Sound(s) if s.entity == 1 && step_indices.contains(&s.sound_index)
            )
        })
        .count();

    assert_eq!(
        mover_step_echoes, 0,
        "stepping player must receive ZERO echo pl_step svc_sound packets"
    );
    assert!(
        listener_steps_from_mover >= 2,
        "nearby listener must receive mover's pl_step svc_sound packets (got {listener_steps_from_mover})"
    );
}

// -----------------------------------------------------------------------------------------------
// 4. Server Task 3: Crimson Blood + Decals + DeathCam -> Observer Cycling
// -----------------------------------------------------------------------------------------------

#[test]
fn death_cam_crimson_blood_decals_and_observer_mode_cycling() {
    let ents = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "0 0 36"),
        spawn_at("info_player_start", "300 200 36"),
        spawn_at("info_player_deathmatch", "-120 0 36"),
    ];
    let map = named_floor_bsp("deathcam_blood", ents);
    let mut b = Bench::with_userinfo(map, 4, &[("cl_lw", "1")]);
    b.server
        .set_decals((1..=6).map(|i| format!("{{blood{i}")).collect());
    b.idle_cmds = true;

    b.run(|_, eng| {
        eng.set_cvar("mp_freezetime", "0");
        eng.set_cvar("mp_fadetoblack", "0");
    });
    // Join all 3 slots through the real team/class join path (so `#Game_Commencing` fires and
    // finishes BEFORE the kill, never restarting the round mid-deathcam!).
    b.step(6);
    common::join(&mut b, 0, 2, 1);
    for (slot, team_choice) in [(1, 2), (2, 1)] {
        common::enter(&mut b, slot);
        common::join(&mut b, slot, team_choice, 1);
    }
    b.step(360);
    b.run(|game, eng| {
        game.end_freeze_period(eng);
        assert!(game.place_player(eng, 0, Vec3::new(0.0, 0.0, 36.0)));
        assert!(game.place_player(eng, 1, Vec3::new(300.0, 200.0, 36.0)));
        assert!(game.place_player(eng, 2, Vec3::new(-75.0, 0.0, 36.0)));
        game.state_mut().players[2].as_mut().unwrap().money = 16_000;
        game.client_command(eng, 2, &["ak47".into()]);
        // Wait out AK-47 deploy delay.
        for _ in 0..110 {
            drive_cmd(
                game,
                eng,
                2,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                0,
            );
        }
    });
    b.step(5);
    b.probe.clear_messages();

    // Attacker (slot 2) aims straight +X and downward (+22 deg pitch) so TraceBleed hits the z=0
    // floor well within 172u behind the unarmored victim!
    b.run(|game, eng| {
        for i in 0..16 {
            let cmd = GoldSrcUserCmd {
                msec: 10,
                viewangles: Vec3::new(22.0, 0.0, 0.0),
                buttons: IN_ATTACK as u16,
                ..Default::default()
            };
            drive_cmd(game, eng, 2, &cmd, 100 + i);
            for _ in 0..12 {
                drive_cmd(
                    game,
                    eng,
                    2,
                    &GoldSrcUserCmd {
                        msec: 10,
                        viewangles: Vec3::new(22.0, 0.0, 0.0),
                        ..Default::default()
                    },
                    200 + i,
                );
            }
            if !game.state().players[0].as_ref().unwrap().alive {
                break;
            }
        }
    });
    b.step(3);

    // Verify crimson TE_BLOODSPRITE (palette color == 247) and blood decal temp entity.
    let mut blood_sprites_crimson = 0usize;
    let mut blood_decals = 0usize;
    for msg in b.probe.messages() {
        if let SvcMessage::TempEntity { te_type, raw } = msg {
            if *te_type == te::BLOODSPRITE
                && let Ok(TempEffect::BloodSprite { color, .. }) = TempEffect::parse(*te_type, raw)
                && color == 247
            {
                blood_sprites_crimson += 1;
            } else if matches!(
                *te_type,
                te::WORLDDECAL | te::WORLDDECALHIGH | te::DECAL | te::DECALHIGH
            ) {
                blood_decals += 1;
            }
        }
    }
    assert!(
        blood_sprites_crimson >= 1,
        "bullet hits on unarmored player must emit crimson TE_BLOODSPRITE (color 247)"
    );
    assert!(
        blood_decals >= 1,
        "TraceBleed must emit wall/floor blood decal temp entities"
    );

    // Verify DeathCam state right after death (`DEAD_DYING`/`DEAD_DEAD`, `view_ofs = -8`, `iuser3 = killer`).
    let (alive, view_ofs_z, iuser1, iuser3) = b.run(|game, eng| {
        let p = game.state().players[0].as_ref().unwrap();
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (p.alive, v.view_ofs.z, v.iuser1, v.iuser3)
    });
    assert!(!alive);
    assert_eq!(view_ofs_z, -8.0);
    assert_eq!(iuser1, OBS_CHASE_FREE);
    assert_eq!(
        iuser3, 3,
        "death cam iuser3 must point at killer edict (slot 2 -> edict 3)"
    );
    let _ = DEAD_DYING;

    // Advance past dying animation + DEATH_ANIMATION_TIME (3.0s) -> ClCorpse + Observer transition.
    for _ in 0..700 {
        b.run(|game, eng| {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                },
                0,
            );
        });
    }
    b.step(3);
    let has_corpse = b
        .messages()
        .iter()
        .any(|m| matches!(m, CsUserMsg::ClCorpse { .. }));
    assert!(
        has_corpse,
        "ClCorpse must be emitted before transitioning to Observer"
    );
    let (obs_view_ofs, mode_initial) = b.run(|_, eng| {
        let v = eng.ent(eng.client_edict(0)).unwrap();
        (v.view_ofs, v.iuser1)
    });
    assert_eq!(
        obs_view_ofs,
        Vec3::ZERO,
        "dead player must transition into Observer after DEATH_ANIMATION_TIME"
    );
    assert_ne!(mode_initial, 0);

    // Cycle observer modes via +jump (`OBS_ROAMING -> OBS_CHASE_LOCKED -> OBS_CHASE_FREE -> OBS_IN_EYE`)
    // and verify `OBS_IN_EYE` and `OBS_ROAMING` are reached.
    let mut seen_modes = vec![mode_initial];
    for _ in 0..6 {
        b.run(|game, eng| {
            drive_cmd(
                game,
                eng,
                0,
                &GoldSrcUserCmd {
                    msec: 10,
                    buttons: IN_JUMP as u16,
                    ..Default::default()
                },
                0,
            );
        });
        for _ in 0..35 {
            b.run(|game, eng| {
                drive_cmd(
                    game,
                    eng,
                    0,
                    &GoldSrcUserCmd {
                        msec: 10,
                        ..Default::default()
                    },
                    0,
                );
            });
        }
        let m = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().iuser1);
        seen_modes.push(m);
    }
    assert!(
        seen_modes.contains(&OBS_IN_EYE) && seen_modes.contains(&OBS_ROAMING),
        "pressing +jump as observer must cycle through OBS_IN_EYE and OBS_ROAMING (saw {seen_modes:?})"
    );
}

// -----------------------------------------------------------------------------------------------
// 5. NavMesh Official `.nav` Loading & Auto-Nav Connectivity (`dm_rusthouse`, `de_dust2`, `de_inferno`)
// -----------------------------------------------------------------------------------------------

#[test]
fn navmesh_official_loading_and_connectivity_on_rusthouse_dust2_and_inferno() {
    // 5a. Clean-room `dm_rusthouse`: official `.nav` loads directly from VFS.
    let pack = cs_assets::demo_pack::build_instant_demo_pack();
    let vfs_with_nav = Vfs::from_memory(pack.files.clone());
    let rusthouse_map = map_named(&vfs_with_nav, "dm_rusthouse");
    let mut game_nav = CsGame::new();
    game_nav.load_bot_assets(&vfs_with_nav);
    let mut b_nav = Bench::with_game(
        rusthouse_map,
        ServerConfig {
            max_clients: 6,
            ..Default::default()
        },
        &[],
        false,
        &[],
        game_nav,
    );
    let _ = b_nav.server.console_command("bot_add_ct");
    b_nav.step(5);
    let loaded_from_file = b_nav.run(|g, _| {
        let areas = g.bot_nav_mesh().map_or(0, |m| m.areas().len());
        (g.bot_nav_built(), areas)
    });
    assert!(
        loaded_from_file.0 && loaded_from_file.1 > 0,
        "dm_rusthouse official .nav must load cleanly"
    );

    // 5b. `dm_rusthouse` WITHOUT `.nav` in VFS: auto-nav connects CT spawn <-> T spawn.
    let files_no_nav: Vec<(String, Vec<u8>)> = pack
        .files
        .into_iter()
        .filter(|(p, _)| !p.ends_with(".nav"))
        .collect();
    let vfs_no_nav = Vfs::from_memory(files_no_nav);
    let rusthouse_map2 = map_named(&vfs_no_nav, "dm_rusthouse");
    let mut game_auto = CsGame::new();
    game_auto.load_bot_assets(&vfs_no_nav);
    let mut b_auto = Bench::with_game(
        rusthouse_map2,
        ServerConfig {
            max_clients: 6,
            ..Default::default()
        },
        &[],
        false,
        &[],
        game_auto,
    );
    let _ = b_auto.server.console_command("bot_add_ct");
    b_auto.step(5);
    let rusthouse_connected = b_auto.run(|g, eng| {
        let mesh = g
            .bot_nav_mesh()
            .expect("auto-generated dm_rusthouse navmesh");
        let ct = eng
            .ent(eng.find_by_classname(None, "info_player_start").unwrap())
            .unwrap()
            .origin;
        let t = eng
            .ent(
                eng.find_by_classname(None, "info_player_deathmatch")
                    .unwrap(),
            )
            .unwrap()
            .origin;
        let obs = PathObstacles::default();
        let params = PathCostParams::default();
        find_path(mesh, ct, t, &params, &obs).is_some()
            && find_path(mesh, t, ct, &params, &obs).is_some()
    });
    assert!(
        rusthouse_connected,
        "auto-generated dm_rusthouse navmesh must bidirectionally connect CT and T spawns"
    );

    // 5c. Retail Steam `de_dust2` & `de_inferno` (runs when Steam assets are installed).
    let Some(vfs) = discover() else {
        return;
    };
    for map_name in ["de_dust2", "de_inferno"] {
        if !vfs.exists(&format!("maps/{map_name}.bsp")) {
            continue;
        }
        let srv_map = map_named(&vfs, map_name);
        let mut game = CsGame::new();
        game.load_bot_assets(&vfs);
        // Force fresh auto-generation into a temp cache dir (never touching user cache).
        let tmp_dir = std::env::temp_dir().join(format!("cs16rs_p18_nav_{map_name}"));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        cs_game::bots::set_nav_cache_dir(game.state_mut(), Some(tmp_dir.clone()));

        let mut b = Bench::with_game(
            srv_map,
            ServerConfig {
                max_clients: 6,
                ..Default::default()
            },
            &[],
            false,
            &[],
            game,
        );
        let _ = b.server.console_command("bot_add_ct");
        b.step(5);

        let (area_count, ct_to_t, t_to_ct, site_paths) = b.run(|g, eng| {
            let mesh = g.bot_nav_mesh().expect("auto-generated navmesh");
            let ct = eng
                .ent(eng.find_by_classname(None, "info_player_start").unwrap())
                .unwrap()
                .origin;
            let t = eng
                .ent(
                    eng.find_by_classname(None, "info_player_deathmatch")
                        .unwrap(),
                )
                .unwrap()
                .origin;
            let mut sites = Vec::new();
            let mut cur = None;
            while let Some(id) = eng.find_by_classname(cur, "func_bomb_target") {
                cur = Some(id);
                if let Some(v) = eng.ent(id) {
                    let c = Vec3::new(
                        (v.absmin.x + v.absmax.x) * 0.5,
                        (v.absmin.y + v.absmax.y) * 0.5,
                        v.absmin.z + ((v.absmax.z - v.absmin.z) * 0.5).min(36.0),
                    );
                    sites.push(c);
                }
            }
            let obs = PathObstacles::default();
            let params = PathCostParams::default();
            let ct_to_t = find_path(mesh, ct, t, &params, &obs).is_some();
            let t_to_ct = find_path(mesh, t, ct, &params, &obs).is_some();
            let mut ok_sites = Vec::new();
            for s in &sites {
                let from_t = find_path(mesh, t, *s, &params, &obs).is_some();
                let from_ct = find_path(mesh, ct, *s, &params, &obs).is_some();
                ok_sites.push((from_t, from_ct));
            }
            (mesh.areas().len(), ct_to_t, t_to_ct, ok_sites)
        });
        let _ = std::fs::remove_dir_all(&tmp_dir);

        assert!(
            area_count > 100,
            "{map_name} navmesh must cover the map (areas={area_count})"
        );
        assert!(
            ct_to_t && t_to_ct,
            "{map_name} navmesh must bidirectionally connect CT <-> T spawns"
        );
        assert!(
            site_paths.len() >= 2 && site_paths.iter().all(|&(ft, fc)| ft && fc),
            "{map_name} navmesh must connect CT & T spawns to BOTH bombsites A & B: {site_paths:?}"
        );
    }
}

// -----------------------------------------------------------------------------------------------
// 6. 5v5 ZBot Round: Freeze-Time Calm & Buy, 250 u/s Knife Rollout, Rifle Switch, Continuous Steering
// -----------------------------------------------------------------------------------------------

#[test]
fn zbot_5v5_freeze_buy_knife_rollout_rifle_switch_smooth_steering_and_blood() {
    // Use real `de_dust2` when Steam assets are installed, and ALWAYS run on a full-size 3000u
    // flat bomb arena too so CI without Steam BSPs validates every single invariant.
    // Place spawns 3300u apart (> 2800u bot sight range) so bots begin out of visual contact and
    // pull out Knife (`250 u/s`) for rollout before switching back to primary rifles as they close.
    let mut big_ents = vec![entity(&[("classname", "worldspawn")])];
    for i in 0..6 {
        big_ents.push(spawn_at(
            "info_player_start",
            &format!("{} {} 36", 1650 + (i % 3) * 90, -100 + (i / 3) * 180),
        ));
        big_ents.push(spawn_at(
            "info_player_deathmatch",
            &format!("{} {} 36", -1650 - (i % 3) * 90, -100 + (i / 3) * 180),
        ));
    }
    big_ents.push(entity(&[
        ("classname", "info_bomb_target"),
        ("origin", "350 420 0"),
    ]));
    big_ents.push(entity(&[
        ("classname", "info_bomb_target"),
        ("origin", "-150 -460 0"),
    ]));

    let synthetic_map = named_floor_bsp("p18_big_bomb", big_ents);
    run_5v5_zbot_parity_assertions(synthetic_map, None, "p18_big_bomb", false);

    if let Some(vfs) = discover()
        && vfs.exists("maps/de_dust2.bsp")
    {
        let dust2_map = map_named(&vfs, "de_dust2");
        run_5v5_zbot_parity_assertions(dust2_map, Some(&vfs), "de_dust2", false);
    }
}

fn run_5v5_zbot_parity_assertions(
    map: ServerMap,
    vfs: Option<&Vfs>,
    label: &str,
    size_mid_wall: bool,
) {
    let mut game = CsGame::new();
    if let Some(v) = vfs {
        game.load_bot_assets(v);
        let tmp_dir = std::env::temp_dir().join(format!("cs16rs_p18_round_{label}"));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        cs_game::bots::set_nav_cache_dir(game.state_mut(), Some(tmp_dir));
    }
    game.enable_cvar_bot_quota();

    let mut b = Bench::with_game(
        map,
        ServerConfig {
            max_clients: 12,
            ..Default::default()
        },
        &[],
        false,
        &[],
        game,
    );
    if size_mid_wall {
        b.run(|_, eng| {
            if let Some(wall) = eng.find_by_classname(None, "func_wall") {
                if let Some(v) = eng.ent_mut(wall) {
                    v.solid = cs_engine::consts::SOLID_BBOX;
                }
                eng.set_size(
                    wall,
                    Vec3::new(-32.0, -520.0, 0.0),
                    Vec3::new(32.0, 520.0, 220.0),
                );
                eng.set_origin(wall, Vec3::ZERO);
            }
        });
    }
    let _ = b.server.console_command("mp_startmoney 10000");
    let _ = b.server.console_command("mp_freezetime 2");
    let _ = b.server.console_command("bot_join_after_player 0");
    let _ = b.server.console_command("bot_stop 1");
    let _ = b.server.console_command("bot_quota 10");

    // Wait for all 10 bots to join.
    for _ in 0..700 {
        b.step(10);
        if b.run(|_, eng| bot_slots(eng).len()) >= 10 {
            break;
        }
    }
    assert_eq!(
        b.run(|_, eng| bot_slots(eng).len()),
        10,
        "{label}: 10 bots must connect"
    );

    // Restart into a clean freeze-time round (`mp_freezetime 2`, `$10000`).
    b.run(|g, eng| g.restart_round_now(eng));
    let _ = b.server.console_command("bot_stop 0");
    b.step(20);

    // Record freeze-time angles after initial spawn settle and verify ZERO twitching/spinning
    // during the rest of freeze time, PLUS verify bots bought primaries + armor!
    let freeze_snap_1: Vec<(u8, Vec3, bool, f32)> = b.run(|g, eng| {
        bot_slots(eng)
            .into_iter()
            .map(|s| {
                let p = g.state().players[usize::from(s)].as_ref().unwrap();
                let v = eng.ent(eng.client_edict(s)).unwrap();
                let has_prim =
                    p.owns_shield || g.carried_weapons(s).into_iter().any(is_primary_weapon);
                (s, v.v_angle, has_prim, p.armor)
            })
            .collect()
    });
    b.step(120); // 1.2s later, still inside `mp_freezetime 2`.
    let freeze_snap_2: Vec<(u8, Vec3, bool, f32, bool)> = b.run(|g, eng| {
        bot_slots(eng)
            .into_iter()
            .map(|s| {
                let p = g.state().players[usize::from(s)].as_ref().unwrap();
                let v = eng.ent(eng.client_edict(s)).unwrap();
                let has_prim =
                    p.owns_shield || g.carried_weapons(s).into_iter().any(is_primary_weapon);
                (s, v.v_angle, has_prim, p.armor, p.has_defuser)
            })
            .collect()
    });

    let mut max_freeze_angle_delta = 0.0_f32;
    let mut bought_primaries = 0usize;
    let mut bought_armor = 0usize;
    for (s1, s2) in freeze_snap_1.iter().zip(freeze_snap_2.iter()) {
        max_freeze_angle_delta = max_freeze_angle_delta
            .max((s1.1.x - s2.1.x).abs())
            .max((s1.1.y - s2.1.y).abs());
        if s2.2 {
            bought_primaries += 1;
        }
        if s2.3 > 0.0 {
            bought_armor += 1;
        }
    }
    assert!(
        max_freeze_angle_delta < 1.0,
        "{label}: bots must stand calm during mp_freezetime (max angle delta={max_freeze_angle_delta})"
    );
    assert!(
        bought_primaries >= 8 && bought_armor >= 8,
        "{label}: bots must execute meta buys during freeze time (primaries={bought_primaries}, armor={bought_armor})"
    );

    // Step past FreezeTimeEnd and observe live rollout, rifle switch, continuous velocity angles,
    // combat kills + crimson blood, and radio calls.
    b.probe.clear_messages();
    let mut saw_knife_rollout_250 = false;
    let mut saw_rifle_switch_from_knife = false;
    let mut knife_equipped_bots = std::collections::HashSet::new();
    let mut non_quantized_vel_samples = 0usize;
    let mut total_moving_vel_samples = 0usize;
    let mut saw_kill = false;

    let max_samples = if label == "de_dust2" { 900 } else { 360 };
    for _ in 0..max_samples {
        b.step(5); // 50ms per sample (up to 45s on full de_dust2, breaks early once all checks pass)
        let frame_info: Vec<(u8, bool, Option<WeaponId>, Vec3, bool)> = b.run(|g, eng| {
            bot_slots(eng)
                .into_iter()
                .map(|s| {
                    let p = g.state().players[usize::from(s)].as_ref().unwrap();
                    let v = eng.ent(eng.client_edict(s)).unwrap();
                    (
                        s,
                        p.alive,
                        p.active_item(),
                        v.velocity,
                        (v.flags & FL_ONGROUND) != 0,
                    )
                })
                .collect()
        });
        for (slot, alive, active, vel, on_ground) in frame_info {
            if !alive {
                saw_kill = true;
                continue;
            }
            let spd = vel.truncate().length();
            if active == Some(WeaponId::Knife) {
                knife_equipped_bots.insert(slot);
                if spd >= 235.0 {
                    saw_knife_rollout_250 = true;
                }
            } else if active.is_some_and(|w| !matches!(w, WeaponId::Knife | WeaponId::C4))
                && knife_equipped_bots.contains(&slot)
            {
                saw_rifle_switch_from_knife = true;
            }
            if on_ground && spd > 120.0 {
                total_moving_vel_samples += 1;
                let deg = vel.y.atan2(vel.x).to_degrees().rem_euclid(45.0);
                let dist_to_45 = deg.min(45.0 - deg);
                if dist_to_45 > 4.0 {
                    non_quantized_vel_samples += 1;
                }
            }
        }
        if saw_knife_rollout_250
            && saw_rifle_switch_from_knife
            && non_quantized_vel_samples >= 20
            && saw_kill
        {
            break;
        }
    }

    assert!(
        saw_knife_rollout_250,
        "{label}: bots must pull out Knife and sprint at ~250 u/s during spawn rollout"
    );
    assert!(
        saw_rifle_switch_from_knife,
        "{label}: bots that rolled out with Knife must switch back to primary rifle before contact"
    );
    assert!(
        total_moving_vel_samples > 20 && non_quantized_vel_samples >= 10,
        "{label}: bots must produce continuous analog velocity headings, not 8-way quantized 45° steps \
         (non_quantized={non_quantized_vel_samples}/{total_moving_vel_samples})"
    );
    assert!(
        saw_kill,
        "{label}: 5v5 bots must engage and score kills during live round"
    );
}

// -----------------------------------------------------------------------------------------------
// 7. Full ZBot C4 Plant -> Guard -> Rotate -> Defuse Tactical Loop + Radio Verification
// -----------------------------------------------------------------------------------------------

#[test]
fn zbot_c4_plant_beep_rotate_and_defuse_loop() {
    let ents = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "520 0 36"),
        spawn_at("info_player_deathmatch", "-220 0 36"),
        entity(&[("classname", "info_bomb_target"), ("origin", "-180 0 0")]),
    ];
    let map = named_floor_bsp("p18_c4_loop", ents);
    let mut b = Bench::with_config(
        map,
        ServerConfig {
            max_clients: 4,
            max_update_rate: 102,
            ..Default::default()
        },
        &[("cl_updaterate", "102")],
        false,
    );
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_c4timer 45");
    let _ = b.server.console_command("bot_stop 1");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    // Step past `#Game_Commencing` (3s) with bots paused so both teams are registered cleanly.
    b.step(360);

    // Park CT bot far away (> 2800u sight range) while the T carrier plants on site cleanly, then
    // eliminate T and place CT near the site with a defuser to rotate and defuse (`%!MRAD_BOMBDEF`)!
    let (t_slot, ct_slot) = b.run(|g, eng| {
        g.restart_round_now(eng);
        g.end_freeze_period(eng);
        let bots = bot_slots(eng);
        let t = *bots
            .iter()
            .find(|&&s| g.state().players[usize::from(s)].as_ref().unwrap().team == Team::Terrorist)
            .unwrap();
        let ct = *bots
            .iter()
            .find(|&&s| g.state().players[usize::from(s)].as_ref().unwrap().team == Team::Ct)
            .unwrap();
        // Keep CT alive at x=3900 (> 4100u away, well beyond 2800u sight range even while running
        // for 4s) so T finishes the 3s C4 plant undisturbed before CT rotates in to defuse.
        g.place_player(eng, ct, Vec3::new(3900.0, 0.0, 36.0));
        (t, ct)
    });
    let _ = b.server.console_command("bot_stop 0");
    b.probe.clear_messages();

    let mut planted = false;
    for _ in 0..140 {
        b.step(10);
        if b.messages()
            .iter()
            .any(|m| matches!(m, CsUserMsg::BombDrop { planted: true, .. }))
        {
            planted = true;
            break;
        }
    }
    let t_dbg = b.run(move |g, eng| {
        let p = g.state().players[usize::from(t_slot)].as_ref().unwrap();
        let v = eng.ent(eng.client_edict(t_slot)).unwrap();
        (
            p.alive,
            p.has_c4,
            p.active_item(),
            v.origin,
            g.state().rules.win_status(),
        )
    });
    assert!(
        planted,
        "T bot carrier must navigate to bombsite and plant C4 (t_dbg={t_dbg:?})"
    );

    // Verify `%!MRAD_BOMBPL` broadcast on plant, eliminate T, and bring CT near the site with a kit.
    let saw_bombpl = b.messages().iter().any(|m| {
        matches!(
            m,
            CsUserMsg::SendAudio { audio_code, .. }
                if audio_code.to_ascii_uppercase().contains("BOMBPL")
        )
    });
    assert!(saw_bombpl, "planting C4 must emit %!MRAD_BOMBPL SendAudio");

    b.run(move |g, eng| {
        g.state_mut().players[usize::from(ct_slot)]
            .as_mut()
            .unwrap()
            .has_defuser = true;
        g.place_player(eng, ct_slot, Vec3::new(140.0, 40.0, 36.0));
        g.damage_player(eng, t_slot, None, 500.0, cs_game::player::dmg::GENERIC);
    });

    let mut defused = false;
    for _ in 0..180 {
        b.step(10);
        if b.run(|g, _| {
            g.state().rules.bomb_defused && g.state().rules.win_status() == WinStatus::Cts
        }) {
            defused = true;
            break;
        }
    }
    assert!(
        defused,
        "CT bot must rotate to planted C4, aim smoothly via look spring, hold +use, and defuse it"
    );
    // Pump 3 more frames so the loopback probe drains the reliable/broadcast `%!MRAD_BOMBDEF` packet.
    b.step(3);
    let saw_bombdef = b.messages().iter().any(|m| {
        matches!(
            m,
            CsUserMsg::SendAudio { audio_code, .. }
                if audio_code.to_ascii_uppercase().contains("BOMBDEF")
        )
    });
    assert!(
        saw_bombdef,
        "defusing C4 must emit %!MRAD_BOMBDEF SendAudio"
    );
}
