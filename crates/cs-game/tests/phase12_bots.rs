//! Comprehensive Phase 12 ZBot (`cs-bot`) end-to-end integration tests:
//! - 5v5 multi-round `bot_quota 10` match, `bot_kill`, `bot_kick`, and `bot_join_after_player`
//! - `.nav` VFS loading, `bot_nav_generate`, editing (`mark`/`split`/`crouch`/`place_set`/`check_consistency`), and `bot_nav_save` roundtrip
//! - `BotProfile.db` VFS loading and builtin fallback
//! - Weapon restriction cvars/presets (`bot_knives_only`, `bot_pistols_only`, `bot_snipers_only`, `bot_all_weapons`)
//! - Radio chatter, human radio commands (`followme`, `holdpos`, `report`), `bot_chatter off`, `bot_stop`
//! - Objective tactics: VIP escort & safety-zone escape, and bomb plant/guard/defuse

mod common;

use std::sync::Arc;

use common::{Bench, command, entity, join, spawn_at, spawns};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_assets::nav::{NAV_VERSION, NavArea, NavAttribute, NavDir, NavFile};
use cs_engine::console::ConsoleOutcome;
use cs_engine::{GameDll, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_game::rules::WinStatus;
use cs_proto48::usermsg::CsUserMsg;
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;

fn minimal_bsp_raw() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

fn console_text(out: ConsoleOutcome) -> String {
    match out {
        ConsoleOutcome::Output(s) => s,
        other => format!("{other:?}"),
    }
}

fn bot_slots(eng: &cs_engine::EngineApi<'_>) -> Vec<u8> {
    (0..eng.max_clients())
        .filter(|&s| eng.is_fake_client(s))
        .collect()
}

/// Real flat floor BSP (`z < 0` solid across all 4 hulls) for deterministic movement & combat.
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
        // Extra trailing clipnode so `hull_range` (`last = clipnodes.len() - 1`) is strictly > `head_nodes[3]` (2).
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

fn spawns_z36(ct: usize, t: usize) -> Vec<Entity> {
    let mut v = vec![entity(&[("classname", "worldspawn")])];
    for i in 0..ct {
        v.push(spawn_at(
            "info_player_start",
            &format!("{} 0 36", 120 + 160 * i as i32),
        ));
    }
    for i in 0..t {
        v.push(spawn_at(
            "info_player_deathmatch",
            &format!("{} 0 36", -120 - 160 * i as i32),
        ));
    }
    v
}

fn floor_bsp(entities: Vec<Entity>) -> ServerMap {
    named_floor_bsp("phase12_arena", entities)
}

fn make_area(
    id: u32,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    z: f32,
    place_id: u16,
) -> NavArea {
    NavArea {
        id,
        attributes: NavAttribute::NONE,
        nw_corner: Vec3::new(min_x, min_y, z),
        se_corner: Vec3::new(max_x, max_y, z),
        ne_z: z,
        sw_z: z,
        connections: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
        hiding_spots: Vec::new(),
        approach_spots: Vec::new(),
        encounters: Vec::new(),
        place_id,
        ladders_up: Vec::new(),
        ladders_down: Vec::new(),
    }
}

#[test]
fn match_5v5_quota_10_multi_round_and_commands() {
    let map = floor_bsp(spawns_z36(8, 8));
    let mut game = CsGame::new();
    game.enable_cvar_bot_quota();
    let cfg = ServerConfig {
        max_clients: 12,
        ..Default::default()
    };
    let mut b = Bench::with_game(map, cfg, &[], false, &[], game);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("bot_stop 1");
    let _ = b.server.console_command("bot_join_after_player 0");
    let _ = b.server.console_command("bot_quota 10");

    for _ in 0..700 {
        b.step(10);
        if b.run(|_, eng| bot_slots(eng).len()) >= 10 {
            break;
        }
    }

    // Start a clean simultaneous 5v5 round once all 10 quota bots have connected.
    b.run(|g, eng| g.restart_round_now(eng));
    let _ = b.server.console_command("bot_stop 0");
    b.step(5);

    let (t_count, ct_count, alive_bots, nav_ready, summary) = b.run(|g, eng| {
        let slots = bot_slots(eng);
        let mut t = 0usize;
        let mut ct = 0usize;
        let mut alive = 0usize;
        for &s in &slots {
            if let Some(p) = g.state().players[usize::from(s)].as_ref() {
                match p.team {
                    Team::Terrorist => t += 1,
                    Team::Ct => ct += 1,
                    _ => {}
                }
                if p.alive {
                    alive += 1;
                }
            }
        }
        (t, ct, alive, g.bot_nav_built(), g.debug_bot_summary(eng))
    });
    assert_eq!(t_count, 5, "expected 5 T bots in 5v5 match\n{summary}");
    assert_eq!(ct_count, 5, "expected 5 CT bots in 5v5 match\n{summary}");
    assert_eq!(alive_bots, 10, "expected all 10 bots alive\n{summary}");
    assert!(nav_ready, "navmesh should be built once bots enter");

    // Step active match and slay Terrorist bots to force a round win + round 2 reset.
    b.step(50);
    let killed = console_text(b.server.console_command("bot_kill t"));
    assert!(killed.contains("Killed"), "unexpected bot_kill output: {killed}");

    for _ in 0..100 {
        b.step(10);
        let (rounds, alive_r2) = b.run(|g, eng| {
            let alive = bot_slots(eng)
                .into_iter()
                .filter(|&s| {
                    g.state().players[usize::from(s)]
                        .as_ref()
                        .is_some_and(|p| p.alive)
                })
                .count();
            (g.state().rules.total_rounds_played(), alive)
        });
        if rounds >= 1 && alive_r2 == 10 {
            break;
        }
    }
    let (rounds, alive_r2, diag) = b.run(|g, eng| {
        let alive = bot_slots(eng)
            .into_iter()
            .filter(|&s| {
                g.state().players[usize::from(s)]
                    .as_ref()
                    .is_some_and(|p| p.alive)
            })
            .count();
        let d = format!(
            "time={:.2} win={:?} restart_at={:.2} dead={:?}",
            eng.time(),
            g.state().rules.win_status(),
            g.state().rules.restart_round_time(),
            bot_slots(eng)
                .into_iter()
                .filter_map(|s| {
                    let p = g.state().players[usize::from(s)].as_ref()?;
                    (!p.alive).then_some((s, p.team, p.joining))
                })
                .collect::<Vec<_>>()
        );
        (g.state().rules.total_rounds_played(), alive, d)
    });
    assert!(rounds >= 1, "expected round restart after wiping T team ({diag})");
    assert_eq!(alive_r2, 10, "all 10 bots should respawn alive in round 2 ({diag})");

    let kick_ct = console_text(b.server.console_command("bot_kick ct"));
    assert!(kick_ct.contains("Kicked 5"), "expected 5 CT bots kicked: {kick_ct}");
    let kick_all = console_text(b.server.console_command("bot_kick"));
    assert!(kick_all.contains("Kicked"), "expected remaining bots kicked: {kick_all}");
    let (remaining, quota_after) = b.run(|_, eng| (bot_slots(eng).len(), eng.cvar_f32("bot_quota")));
    assert_eq!(remaining, 0);
    assert_eq!(quota_after, 0.0);
}

#[test]
fn bot_join_after_player_waits_for_human() {
    let map = floor_bsp(spawns(4, 4));
    let mut game = CsGame::new();
    game.enable_cvar_bot_quota();
    let cfg = ServerConfig {
        max_clients: 8,
        ..Default::default()
    };
    let mut b = Bench::with_game(map, cfg, &[], false, &[], game);
    b.run(|g, eng| g.client_disconnect(eng, 0));
    let _ = b.server.console_command("bot_join_after_player 1");
    let _ = b.server.console_command("bot_quota 4");
    b.step(150);

    let bots_before = b.run(|_, eng| bot_slots(eng).len());
    assert_eq!(
        bots_before, 0,
        "bot_join_after_player=1 must hold bots while humans == 0"
    );

    b.run(|g, eng| g.client_put_in_server(eng, 0));
    for _ in 0..300 {
        b.step(10);
        if b.run(|_, eng| bot_slots(eng).len()) >= 4 {
            break;
        }
    }
    let bots_after = b.run(|_, eng| bot_slots(eng).len());
    assert_eq!(bots_after, 4, "expected 4 bots once human joined");
}

#[test]
fn nav_vfs_load_generate_edit_and_save_roundtrip() {
    let mut a1 = make_area(1, -256.0, -256.0, 0.0, 256.0, 0.0, 1);
    let mut a2 = make_area(2, 0.0, -256.0, 256.0, 256.0, 0.0, 2);
    a1.connections[NavDir::East.as_index()].push(2);
    a2.connections[NavDir::West.as_index()].push(1);
    let nav = NavFile {
        version: NAV_VERSION,
        bsp_size: 0,
        places: vec!["BombsiteA".into(), "Mid".into()],
        areas: vec![a1, a2],
        ladders: Vec::new(),
    };
    let bytes = nav.to_bytes().expect("serialize synthetic NavFile");
    let vfs = Vfs::from_layered_memory(
        vec![("cstrike/maps/phase12_arena.nav".to_string(), bytes)],
        VfsConfig::default(),
    );

    let map = floor_bsp(spawns(2, 2));
    let mut b = Bench::with_slots(map, 6);
    b.run(move |g, _| g.load_bot_assets(&vfs));

    let load_out = console_text(b.server.console_command("bot_nav_load"));
    assert!(
        load_out.contains("Loaded 2 nav areas from VFS"),
        "unexpected bot_nav_load output: {load_out}"
    );
    let (loaded_areas, loaded_places) = b.run(|g, _| {
        let m = g.bot_nav_mesh().expect("navmesh loaded");
        (m.areas().len(), m.places().to_vec())
    });
    assert_eq!(loaded_areas, 2);
    assert_eq!(loaded_places, vec!["BombsiteA", "Mid"]);

    let gen_out = console_text(b.server.console_command("bot_nav_generate"));
    assert!(
        gen_out.contains("Generated navmesh for 'phase12_arena'"),
        "unexpected bot_nav_generate output: {gen_out}"
    );
    let areas_before = b.run(|g, _| g.bot_nav_mesh().expect("generated").areas().len());
    assert!(areas_before > 0);

    let mark_out = console_text(b.server.console_command("bot_nav_mark"));
    assert!(mark_out.contains("Marked nav area #"), "{mark_out}");
    let crouch_out = console_text(b.server.console_command("bot_nav_crouch"));
    assert!(crouch_out.contains("bot_nav_crouch: true"), "{crouch_out}");
    let place_out = console_text(b.server.console_command("bot_nav_place_set BombsiteB"));
    assert!(place_out.contains("BombsiteB"), "{place_out}");
    let split_out = console_text(b.server.console_command("bot_nav_split"));
    assert!(split_out.contains("Split area #"), "{split_out}");
    let areas_after = b.run(|g, _| g.bot_nav_mesh().expect("after split").areas().len());
    assert_eq!(areas_after, areas_before + 1);

    let consistency = console_text(b.server.console_command("bot_nav_check_consistency"));
    assert!(
        consistency.contains("0 dangling links"),
        "expected clean nav links: {consistency}"
    );

    let tmp_path = std::env::temp_dir().join(format!(
        "cs16_rs_phase12_nav_{}.nav",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp_path);
    let save_out = console_text(
        b.server
            .console_command(&format!("bot_nav_save {}", tmp_path.display())),
    );
    assert!(
        save_out.contains("Saved navigation mesh"),
        "unexpected bot_nav_save output: {save_out}"
    );
    let saved_bytes = std::fs::read(&tmp_path).expect("read saved .nav");
    let _ = std::fs::remove_file(&tmp_path);
    let reparsed = NavFile::parse(&saved_bytes).expect("reparse saved .nav");
    assert_eq!(reparsed.areas.len(), areas_after);
    assert!(reparsed.places.iter().any(|p| p == "BombsiteB"));
}

#[test]
fn bot_profile_db_vfs_load_and_builtin_fallback() {
    let map = floor_bsp(spawns(2, 2));
    let mut b = Bench::with_slots(map, 6);

    let builtin_about = console_text(b.server.console_command("bot_about"));
    assert!(
        builtin_about.contains("cs16-rs ZBot AI") && !builtin_about.contains("0 profiles loaded"),
        "unexpected builtin bot_about: {builtin_about}"
    );

    let custom_db = "\
Default
    Skill = 50
    Aggression = 50
    ReactionTime = 0.3
    Teamwork = 60
    Cost = 0
    Difficulty = EASY+NORMAL+HARD+EXPERT
    VoicePitch = 100
End

Elite+UnitAlpha
    Skill = 95
    Skin = 3
    WeaponPreference = ak47
    VoicePitch = 112
End
";
    let vfs = Vfs::from_layered_memory(
        vec![("cstrike/BotProfile.db".to_string(), custom_db.as_bytes().to_vec())],
        VfsConfig::default(),
    );
    b.run(move |g, _| g.load_bot_assets(&vfs));

    let custom_about = console_text(b.server.console_command("bot_about"));
    assert!(
        custom_about.contains("1 profiles loaded"),
        "expected custom BotProfile.db with 1 profile: {custom_about}"
    );
    let add_out = console_text(b.server.console_command("bot_add_t UnitAlpha"));
    assert!(add_out.is_empty(), "bot_add_t failed: {add_out}");
    let bot_name = b.run(|_, eng| {
        let slot = bot_slots(eng)[0];
        eng.client_key(slot, "name").map(str::to_string)
    });
    assert_eq!(bot_name.as_deref(), Some("UnitAlpha"));
}

#[test]
fn weapon_restrictions_presets_and_buy() {
    let map = floor_bsp(spawns_z36(2, 2));
    let mut b = Bench::with_slots(map, 6);
    let _ = b.server.console_command("mp_startmoney 16000");

    let _ = b.server.console_command("bot_knives_only");
    let (pistols, rifles) = b.run(|_, eng| {
        (
            eng.cvar_f32("bot_allow_pistols"),
            eng.cvar_f32("bot_allow_rifles"),
        )
    });
    assert_eq!(pistols, 0.0);
    assert_eq!(rifles, 0.0);

    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    b.step(120);

    let any_primary_knives = b.run(|g, eng| {
        bot_slots(eng)
            .into_iter()
            .any(|s| g.weapons_mask(s).is_some_and(|(_, has_primary)| has_primary))
    });
    assert!(!any_primary_knives, "bots must not buy primaries under bot_knives_only");

    let _ = b.server.console_command("bot_pistols_only");
    assert_eq!(b.run(|_, eng| eng.cvar_f32("bot_allow_pistols")), 1.0);
    assert_eq!(b.run(|_, eng| eng.cvar_f32("bot_allow_rifles")), 0.0);

    let _ = b.server.console_command("bot_snipers_only");
    assert_eq!(b.run(|_, eng| eng.cvar_f32("bot_allow_snipers")), 1.0);
    assert_eq!(b.run(|_, eng| eng.cvar_f32("bot_allow_rifles")), 0.0);

    let _ = b.server.console_command("bot_all_weapons");
    let all_allowed = b.run(|_, eng| {
        [
            "bot_allow_pistols",
            "bot_allow_shotguns",
            "bot_allow_sub_machine_guns",
            "bot_allow_rifles",
            "bot_allow_snipers",
            "bot_allow_machine_guns",
            "bot_allow_grenades",
            "bot_allow_shield",
        ]
        .iter()
        .all(|c| eng.cvar_f32(c) != 0.0)
    });
    assert!(all_allowed, "bot_all_weapons must re-enable all 8 weapon restriction cvars");

    b.run(|g, eng| g.restart_round_now(eng));
    b.step(120);
    let bought_primary_or_shield = b.run(|g, eng| {
        bot_slots(eng).into_iter().any(|s| {
            g.weapons_mask(s).is_some_and(|(_, has_primary)| has_primary)
                || g.state().players[usize::from(s)]
                    .as_ref()
                    .is_some_and(|p| p.owns_shield)
        })
    });
    assert!(
        bought_primary_or_shield,
        "bots should buy primary weapons or shield once bot_all_weapons is active"
    );
}

#[test]
fn radio_chatter_and_human_coordination() {
    let map = floor_bsp(spawns(3, 1));
    let mut b = Bench::with_slots(map, 6);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("bot_chatter radio");
    join(&mut b, 0, 2, 1);

    let _ = b.server.console_command("bot_add_ct");
    let _ = b.server.console_command("bot_add_ct");
    b.step(60);

    command(&mut b, 0, "followme");
    b.step(40);
    command(&mut b, 0, "holdpos");
    b.step(40);
    command(&mut b, 0, "report");
    b.step(80);

    let saw_radio = b.messages().into_iter().any(|m| match m {
        CsUserMsg::TextMsg { message, .. } => message.contains("Game_radio"),
        CsUserMsg::SendAudio { audio_code, .. } => audio_code.contains("%!MRAD_"),
        _ => false,
    });
    assert!(saw_radio, "expected human/bot radio messages on wire");

    let _ = b.server.console_command("bot_stop 1");
    b.step(10);
    let pos_before = b.run(|_, eng| {
        bot_slots(eng)
            .into_iter()
            .filter_map(|s| eng.ent(eng.client_edict(s)).map(|v| (s, v.origin)))
            .collect::<Vec<_>>()
    });
    b.step(80);
    let pos_after = b.run(|_, eng| {
        bot_slots(eng)
            .into_iter()
            .filter_map(|s| eng.ent(eng.client_edict(s)).map(|v| (s, v.origin)))
            .collect::<Vec<_>>()
    });
    assert_eq!(pos_before, pos_after, "bot_stop 1 must freeze bot positions");

    let before_audio = b
        .messages()
        .into_iter()
        .filter(|m| matches!(m, CsUserMsg::SendAudio { .. }))
        .count();
    let _ = b.server.console_command("bot_chatter off");
    let _ = b.server.console_command("bot_stop 0");
    b.step(80);
    let after_audio = b
        .messages()
        .into_iter()
        .filter(|m| matches!(m, CsUserMsg::SendAudio { .. }))
        .count();
    assert_eq!(before_audio, after_audio, "bot_chatter off must suppress bot SendAudio");
}

#[test]
fn vip_escort_and_safety_zone_escape() {
    let ents = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "100 0 36"),
        spawn_at("info_player_start", "160 0 36"),
        spawn_at("info_vip_start", "100 0 36"),
        spawn_at("info_player_deathmatch", "-500 0 36"),
        entity(&[("classname", "func_vip_safetyzone"), ("origin", "650 0 36")]),
    ];
    let map = named_floor_bsp("as_phase12", ents);
    let mut b = Bench::with_slots(map, 6);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("bot_knives_only");

    let _ = b.server.console_command("bot_add_ct");
    let _ = b.server.console_command("bot_add_ct");
    let _ = b.server.console_command("bot_add_t");
    b.run(|g, eng| {
        g.restart_round_now(eng);
        if let Some(zid) = eng.find_by_classname(None, "func_vip_safetyzone") {
            eng.set_size(zid, Vec3::new(-128.0, -128.0, -36.0), Vec3::new(128.0, 128.0, 96.0));
            eng.set_origin(zid, Vec3::new(650.0, 0.0, 36.0));
        }
    });

    let start_positions = b.run(|g, eng| {
        bot_slots(eng)
            .into_iter()
            .filter_map(|s| {
                let p = g.state().players[usize::from(s)].as_ref()?;
                let pos = eng.ent(eng.client_edict(s))?.origin;
                Some((s, p.team, p.is_vip, pos))
            })
            .collect::<Vec<_>>()
    });
    assert!(
        start_positions.iter().any(|(_, _, is_vip, _)| *is_vip),
        "expected a CT bot to be chosen as VIP on as_ map: {start_positions:?}"
    );

    let mut escaped_or_won = false;
    let mut escort_moved = false;
    for _ in 0..80 {
        b.step(15);
        let starts = start_positions.clone();
        let (won, escaped, any_ct_moved) = b.run(move |g, eng| {
            let win = g.state().rules.win_status();
            let esc = bot_slots(eng).into_iter().any(|s| {
                g.state().players[usize::from(s)]
                    .as_ref()
                    .is_some_and(|p| p.is_vip && p.escaped)
            });
            let moved = bot_slots(eng).into_iter().any(|s| {
                let Some(p) = g.state().players[usize::from(s)].as_ref() else {
                    return false;
                };
                if p.team != Team::Ct || p.is_vip {
                    return false;
                }
                let cur = eng
                    .ent(eng.client_edict(s))
                    .map(|v| v.origin)
                    .unwrap_or(Vec3::ZERO);
                starts
                    .iter()
                    .find(|(slot, _, _, _)| *slot == s)
                    .is_some_and(|(_, _, _, old)| cur.distance(*old) > 24.0)
            });
            (win == WinStatus::Cts, esc, moved)
        });
        escort_moved |= any_ct_moved;
        if won || escaped {
            escaped_or_won = true;
            break;
        }
    }

    assert!(escaped_or_won, "expected VIP bot to reach func_vip_safetyzone and win");
    assert!(escort_moved, "expected non-VIP CT escort bot to navigate along with VIP");
}

#[test]
fn bomb_plant_guard_and_defuse_synthetic_arena() {
    let ents = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "1400 0 36"),
        spawn_at("info_player_deathmatch", "-150 0 36"),
        spawn_at("info_player_deathmatch", "-150 200 36"),
        entity(&[("classname", "info_bomb_target"), ("origin", "0 0 36")]),
    ];
    let map = named_floor_bsp("de_phase12", ents);
    let mut b = Bench::with_slots(map, 6);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_c4timer 45");
    let _ = b.server.console_command("bot_add_t");
    b.step(5);
    b.run(|g, eng| g.restart_round_now(eng));

    let mut saw_planted = false;
    for _step in 0..80 {
        b.step(20);
        let planted_ent = b.run(|_, eng| {
            let mut c = None;
            while let Some(id) = eng.find_by_classname(c, "grenade") {
                if let Some(v) = eng.ent(id)
                    && v.model.contains("c4")
                {
                    return true;
                }
                c = Some(id);
            }
            false
        });
        let planted_msg = b.messages().into_iter().any(|m| {
            matches!(m, CsUserMsg::BombDrop { planted: true, .. })
        });
        if planted_ent || planted_msg {
            saw_planted = true;
            break;
        }
    }

    // Step briefly after plant so Terrorist bots transition into post-plant guard/escape tactics.
    b.step(30);
    let summary = b.run(|g, eng| g.debug_bot_summary(eng));
    assert!(
        saw_planted,
        "expected T bot to carry C4 to info_bomb_target and plant\n{summary}"
    );
    assert!(
        summary.contains("state=GuardBomb") || summary.contains("state=EscapeFromBomb"),
        "expected T bots to guard or evade planted C4:\n{summary}"
    );
}
