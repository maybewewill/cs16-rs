//! Phase 14 Task 9 tests: Opt-In Practice, Wallbang Lab, Warmup/Buy Anywhere,
//! Damage Print, Live-on-Three (`lo3`), and Knife Round (`mp_knife_round`).

mod common;

use std::sync::Arc;

use common::{Bench, spawns};
use cs_assets::bsp::{BspMap, BspModel, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_cvar::CVarRegistry;
use cs_engine::collision::{CONTENTS_EMPTY, CONTENTS_SOLID};
use cs_engine::{GameDll, ServerMap};
use cs_game::bullets::Bullet;
use cs_game::player::{ArmorType, Team};
use cs_game::rules::{KnifeRoundState, Lo3State};
use cs_game::weapons::{BulletType, WEAPON_NOCLIP, WeaponId, ammo_index, new_weapon};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::tempent::te;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;

fn minimal_bsp_raw() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

fn leaf(contents: i32) -> Leaf {
    Leaf {
        contents,
        vis_offset: -1,
        mins: [-4096, -4096, -4096],
        maxs: [4096, 4096, 4096],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    }
}

/// Builds a synthetic BSP with a solid floor at `z = 0` and a `24u`-thick vertical wall
/// spanning `x in [128, 152]`.
fn wallbang_bsp_map(entities: Vec<Entity>) -> ServerMap {
    let planes = vec![
        // Plane 0: wall front face at x = 128
        Plane { normal: Vec3::X, dist: 128.0, plane_type: 0 },
        // Plane 1: wall back face at x = 152
        Plane { normal: Vec3::X, dist: 152.0, plane_type: 0 },
        // Plane 2: floor at z = 0
        Plane { normal: Vec3::Z, dist: 0.0, plane_type: 2 },
    ];
    let nodes = vec![
        // Node 0: split by x = 128; front (x>=128) -> Node 1, back (x<128) -> Node 2
        Node {
            plane: 0,
            children: [1, 2],
            mins: [-4096, -4096, -4096],
            maxs: [4096, 4096, 4096],
            first_face: 0,
            num_faces: 0,
        },
        // Node 1 (x >= 128): split by x = 152; front (x>=152) -> Node 2 (floor), back (128<=x<152) -> Solid (leaf 0)
        Node {
            plane: 1,
            children: [2, -1],
            mins: [-4096, -4096, -4096],
            maxs: [4096, 4096, 4096],
            first_face: 0,
            num_faces: 0,
        },
        // Node 2: floor z = 0; front (z>=0) -> Empty (leaf 1), back (z<0) -> Solid (leaf 0)
        Node {
            plane: 2,
            children: [-2, -1],
            mins: [-4096, -4096, -4096],
            maxs: [4096, 4096, 4096],
            first_face: 0,
            num_faces: 0,
        },
    ];
    let leafs = vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)];
    let models = vec![BspModel {
        mins: Vec3::new(-4096.0, -4096.0, -64.0),
        maxs: Vec3::new(4096.0, 4096.0, 4096.0),
        origin: Vec3::ZERO,
        head_nodes: [0, 0, 0, 0],
        vis_leafs: 2,
        first_face: 0,
        num_faces: 0,
    }];
    let bsp = BspMap {
        name: "wallbang_test".into(),
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
        clipnodes: Vec::new(),
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
    ServerMap::from_bsp("wallbang_test", &minimal_bsp_raw(), Arc::new(bsp)).expect("valid BSP")
}

fn collected_prints(b: &Bench) -> Vec<String> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Print(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn collected_temp_types(b: &Bench) -> Vec<u8> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::TempEntity { te_type, .. } => Some(*te_type),
            _ => None,
        })
        .collect()
}

fn collected_texts(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

#[test]
fn task9_cvars_default_to_vanilla_disabled() {
    let reg = CVarRegistry::new_with_defaults();
    for name in [
        "sv_showimpacts",
        "sv_grenade_trajectory",
        "sv_showhitboxes",
        "mp_buy_anywhere",
        "mp_warmup_mode",
        "cl_showspeed",
        "cl_showkeys",
        "cl_crosshair_dot",
        "cl_crosshair_outline",
        "cl_crosshair_t",
        "cl_crosshair_gap",
        "cl_mute_enemy_team",
        "sv_damage_print",
        "mp_knife_round",
    ] {
        assert_eq!(reg.get_str(name), "0", "{name} must default to 0");
    }
    assert_eq!(reg.get_str("sv_grenade_trajectory_time"), "8");
}

#[test]
fn cheats_gating_and_practice_aids() {
    let mut b = Bench::on(wallbang_bsp_map(spawns(2, 2)));
    b.run(|game, eng| {
        eng.set_cvar("sv_cheats", "0");
        eng.set_cvar("sv_showimpacts", "1");
        eng.set_cvar("sv_grenade_trajectory", "1");
        eng.set_cvar("sv_showhitboxes", "1");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        eng.set_origin(eng.client_edict(0), Vec3::new(0.0, 0.0, 36.0));
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::Ak47).unwrap(), 30, 90));
        assert!(game.select_weapon(eng, 0, WeaponId::Ak47));

        // sv_cheats 0 refuses rethrow, give_all, ammo_full, and sv_showimpacts recording.
        game.client_command(eng, 0, &["rethrow".into()]);
        game.client_command(eng, 0, &["give_all".into()]);
        game.client_command(eng, 0, &["ammo_full".into()]);
        let bullet = Bullet {
            src: Vec3::new(0.0, 0.0, 53.0),
            dir: Vec3::X,
            spread: 0.0,
            distance: 8192.0,
            penetration: 2,
            kind: BulletType::Player762mm,
            damage: 36,
            range_modifier: 0.98,
            pistol: false,
            shared_rand: 0,
        };
        game.test_fire_bullets3(eng, 0, Some(WeaponId::Ak47), Vec3::ZERO, &bullet);
        assert!(game.state().practice_impacts.is_empty());
        assert_eq!(game.state().players[0].as_ref().unwrap().armor, 0.0);
    });
    b.step(2);
    let prints = collected_prints(&b).join("\n");
    assert!(prints.contains("sv_cheats"), "expected refusal print: {prints}");
    assert!(!prints.contains("[wallbang]"));

    // Now enable sv_cheats 1 and verify wallbang entry/exit, give_all, ammo_full, and sv_showhitboxes.
    b.probe.clear_messages();
    b.run(|game, eng| {
        eng.set_cvar("sv_cheats", "1");
        game.client_command(eng, 0, &["give_all".into()]);
        let p = game.state().players[0].as_ref().unwrap();
        assert_eq!(p.armor, 100.0);
        assert_eq!(p.kevlar, ArmorType::VestHelm);
        assert!(p.has_defuser);
        assert_eq!(game.player_ammo(0, ammo_index("HEGrenade") as usize), Some(1));
        assert_eq!(game.player_ammo(0, ammo_index("Flashbang") as usize), Some(2));
        assert_eq!(game.player_ammo(0, ammo_index("SmokeGrenade") as usize), Some(1));

        let bullet = Bullet {
            src: Vec3::new(0.0, 0.0, 53.0),
            dir: Vec3::X,
            spread: 0.0,
            distance: 8192.0,
            penetration: 2,
            kind: BulletType::Player762mm,
            damage: 36,
            range_modifier: 0.98,
            pistol: false,
            shared_rand: 0,
        };
        game.test_fire_bullets3(eng, 0, Some(WeaponId::Ak47), Vec3::ZERO, &bullet);
        let wb = game
            .state()
            .practice_impacts
            .iter()
            .find(|i| i.wallbang_line.is_some())
            .expect("wallbang recorded");
        assert!((wb.origin.x - 128.0).abs() < 0.5, "entry at x=128: {:?}", wb.origin);
        let exit = wb.exit.expect("exit recorded");
        assert!((exit.x - 152.0).abs() < 0.5, "exit at x=152: {exit:?}");
    });
    b.step(2);
    let prints = collected_prints(&b).join("\n");
    assert!(prints.contains("[wallbang] ak47 penetrated 1 wall (24."), "{prints}");
    let tes = collected_temp_types(&b);
    assert!(tes.contains(&te::BOX));
    assert!(tes.contains(&te::LINE));
}

#[test]
fn rethrow_spawns_identical_grenade_trajectory() {
    let mut b = Bench::on(wallbang_bsp_map(spawns(2, 2)));
    let (first_origin, first_vel) = b.run(|game, eng| {
        eng.set_cvar("sv_cheats", "1");
        eng.set_cvar("sv_grenade_trajectory", "1");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        let id = eng.client_edict(0);
        eng.set_origin(id, Vec3::new(-100.0, 0.0, 36.0));
        assert!(game.give_weapon(eng, 0, new_weapon(WeaponId::HeGrenade).unwrap(), WEAPON_NOCLIP, 1));
        assert!(game.select_weapon(eng, 0, WeaponId::HeGrenade));
        game.set_next_attack(0, 0.0);
        let w = game.weapon_mut(0, WeaponId::HeGrenade).unwrap();
        w.next_primary_attack = 0.0;
        w.time_weapon_idle = 0.0;
        eng.ent_mut(id).unwrap().button = i32::from(IN_ATTACK);
        common::run_cmd(game, eng, 0, &GoldSrcUserCmd { msec: 10, buttons: IN_ATTACK, ..Default::default() }, 0);
        game.weapon_mut(0, WeaponId::HeGrenade).unwrap().time_weapon_idle = 0.0;
        eng.ent_mut(id).unwrap().button = 0;
        common::run_cmd(game, eng, 0, &GoldSrcUserCmd { msec: 10, ..Default::default() }, 0);

        let g = eng.find_by_classname(None, "grenade").expect("thrown grenade");
        let gv = eng.ent(g).unwrap();
        (gv.origin, gv.velocity)
    });

    // Step flight past HE fuse + Smoke3C removal (~2.15s) so only the rethrown grenade exists.
    b.step(250);
    assert!(collected_temp_types(&b).contains(&te::LINE));

    let (re_origin, re_vel) = b.run(|game, eng| {
        game.client_command(eng, 0, &["rethrow".into()]);
        let g = eng.find_by_classname(None, "grenade").expect("rethrown grenade");
        let gv = eng.ent(g).unwrap();
        (gv.origin, gv.velocity)
    });
    assert!((re_origin - first_origin).length() < 0.01);
    assert!((re_vel - first_vel).length() < 0.01);
}

#[test]
fn warmup_and_buy_anywhere_behavior() {
    let mut b = Bench::stub_without_moves(spawns(2, 2));
    b.run(|game, eng| {
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        game.end_freeze_period(eng);
        // Outside buyzone -> cannot buy by default.
        game.client_command(eng, 0, &["m4a1".into()]);
        assert!(game.weapon_mut(0, WeaponId::M4a1).is_none());

        // Enable mp_buy_anywhere 1 -> can buy anywhere on the map regardless of buyzone/buytime.
        eng.set_cvar("mp_buy_anywhere", "1");
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
        game.client_command(eng, 0, &["m4a1".into()]);
        assert!(game.weapon_mut(0, WeaponId::M4a1).is_some());

        // Enable mp_warmup_mode 1, kill player, verify rapid auto-respawn + $16000 refill.
        eng.set_cvar("mp_warmup_mode", "1");
        assert!(!game.damage_player(eng, 0, None, 200.0, 0));
        assert!(!game.state().players[0].as_ref().unwrap().alive);
    });
    b.step(70);
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.alive, "warmup player auto-respawned");
        assert_eq!(p.money, 16000, "warmup player refilled to $16000");
    });
}

#[test]
fn sv_damage_print_reports_damage_on_death_and_round_end() {
    let mut b = Bench::stub_without_moves(spawns(2, 2));
    b.run(|game, eng| {
        eng.set_cvar("sv_damage_print", "1");
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        game.end_freeze_period(eng);
        // Slot 0 deals 45 damage to slot 1, then slot 1 kills slot 0 with a headshot (112 dmg).
        game.test_damage_with_weapon(eng, 1, Some(0), Some(WeaponId::Ak47), 45.0, cs_game::player::dmg::BULLET);
        game.set_last_hit_group(0, 1);
        game.test_damage_with_weapon(eng, 0, Some(1), Some(WeaponId::Ak47), 112.0, cs_game::player::dmg::BULLET);
    });
    b.step(2);
    let prints = collected_prints(&b).join("\n");
    assert!(prints.contains("Damage Given to"), "{prints}");
    assert!(prints.contains("45 in 1 hit"), "{prints}");
    assert!(prints.contains("Damage Taken"), "{prints}");
    assert!(prints.contains("(headshot)"), "{prints}");
}

#[test]
fn lo3_runs_three_restarts_and_announces_live() {
    let mut b = Bench::stub_without_moves(spawns(2, 2));
    let out = b.server.console_command("lo3");
    assert!(format!("{out:?}").contains("LIVE ON THREE"));
    b.run(|game, _| {
        assert_eq!(game.state().rules.lo3_state(), Lo3State::Restart1);
    });
    // Restart 1 (1s) + Restart 2 (1s) + Restart 3 (3s) = ~5.2s -> 530 frames.
    b.step(530);
    b.run(|game, _| {
        assert_eq!(game.state().rules.lo3_state(), Lo3State::Idle);
    });
    let texts = collected_texts(&b).join("\n");
    assert!(texts.contains("GL & HF! MATCH IS LIVE!"), "{texts}");
}

#[test]
fn mp_knife_round_stay_and_switch_trigger_lo3() {
    let mut b = Bench::stub_without_moves(spawns(2, 2));
    b.run(|game, eng| {
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
    });
    // Let initial Game_Commencing restart settle before enabling knife round.
    b.step(310);
    let _ = b.server.console_command("mp_knife_round 1");
    b.step(2);
    b.run(|game, eng| {
        assert_eq!(game.state().rules.knife_round(), KnifeRoundState::Active);
        game.end_freeze_period(eng);
        let p0 = game.state().players[0].as_ref().unwrap();
        assert_eq!(p0.money, 0);
        assert_eq!(p0.armor, 100.0);
        assert_eq!(p0.kevlar, ArmorType::Vest);
        assert!(game.weapon_mut(0, WeaponId::Knife).is_some());
        assert!(game.weapon_mut(0, WeaponId::Usp).is_none());
        // CT (slot 0) kills T (slot 1) to win the knife round.
        let alive_still = game.damage_player(eng, 1, Some(0), 500.0, cs_game::player::dmg::BULLET);
        let p1 = game.state().players[1].as_ref().unwrap();
        assert!(!alive_still && !p1.alive, "p1 survived? hp={} armor={} takes_dmg={} knife={:?}", p1.health, p1.armor, p1.takes_damage, game.state().rules.knife_round());
    });
    b.step(2);
    b.run(|game, eng| {
        assert_eq!(
            game.state().rules.knife_round(),
            KnifeRoundState::AwaitingChoice { winner: Team::Ct },
            "game_started={} rws={:?} team0={:?} team1={:?}",
            game.state().rules.game_started(),
            game.state().rules.win_status(),
            game.state().players[0].as_ref().map(|p| (p.team, p.alive, p.menu)),
            game.state().players[1].as_ref().map(|p| (p.team, p.alive, p.menu)),
        );
        // Winner CT types `.switch` in chat -> swaps sides and fires `lo3`!
        game.client_command(eng, 0, &["say".into(), ".switch".into()]);
        assert_eq!(game.state().players[0].as_ref().unwrap().team, Team::Terrorist);
        assert_eq!(game.state().players[1].as_ref().unwrap().team, Team::Ct);
        assert_eq!(game.state().rules.knife_round(), KnifeRoundState::Inactive);
        assert_eq!(game.state().rules.lo3_state(), Lo3State::Restart1);
    });
}
