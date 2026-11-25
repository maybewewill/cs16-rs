//! Phase 14 Task 12 integration tests: Zero-Config ZBot Support for Custom & Arena Maps:
//! - Automatic v5 `.nav` generation & `nav_cache` roundtrip keyed by `bsp_crc32` (native fs + `ConfigStorage`)
//! - Synthetic `fy_pool_day` floor rifle scavenging (`armoury_entity` + `FetchGroundWeapon`) and combat
//! - Synthetic `35hp_2` knife-only arena combat (`game_player_equip`, no gun buys, close-range knife kills)
//! - Synthetic `awp_india` (`game_player_equip` AWP + knife skips buy logic)

mod common;

use std::sync::Arc;

use common::{Bench, entity, spawn_at};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane};
use cs_assets::nav::{NAV_VERSION, NavFile};
use cs_engine::{ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::weapons::WeaponId;
use cs_platform::MemoryStorage;
use cs_proto48::crypto::crc_map_bsp;
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;

fn synthetic_bsp_raw(seed_byte: u8) -> Vec<u8> {
    // BSP v30 header: 4-byte version + 15 lumps (offset, length), plus a 16-byte lump 1 payload
    // so `crc_map_bsp` hashes distinct geometry per `seed_byte`.
    let header_len = 4 + 15 * 8;
    let mut raw = vec![0u8; header_len + 16];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    let lump1_base = 4 + 8;
    raw[lump1_base..lump1_base + 4].copy_from_slice(&(header_len as i32).to_le_bytes());
    raw[lump1_base + 4..lump1_base + 8].copy_from_slice(&16i32.to_le_bytes());
    for (i, b) in raw[header_len..header_len + 16].iter_mut().enumerate() {
        *b = seed_byte.wrapping_add(i as u8);
    }
    raw
}

/// Real flat floor BSP (`z < 0` solid across all 4 hulls) for deterministic arena simulation.
fn named_floor_bsp(name: &str, raw: &[u8], entities: Vec<Entity>) -> ServerMap {
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
    ServerMap::from_bsp(name, raw, Arc::new(bsp)).expect("valid synthetic BSP header")
}

fn bot_slots(eng: &cs_engine::EngineApi<'_>) -> Vec<u8> {
    (0..eng.max_clients())
        .filter(|&s| eng.is_fake_client(s))
        .collect()
}

#[test]
fn nav_cache_roundtrip_keyed_by_bsp_crc32() {
    let raw_a = synthetic_bsp_raw(0x11);
    let raw_b = synthetic_bsp_raw(0x77);
    let crc_a = crc_map_bsp(&raw_a).expect("crc_a");
    let crc_b = crc_map_bsp(&raw_b).expect("crc_b");
    assert_ne!(crc_a, crc_b);

    let cache_dir = std::env::temp_dir().join(format!(
        "cs16_rs_phase14_nav_cache_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cache_dir);

    let make_entities = || {
        vec![
            entity(&[("classname", "worldspawn")]),
            spawn_at("info_player_start", "240 0 36"),
            spawn_at("info_player_deathmatch", "-240 0 36"),
            entity(&[
                ("classname", "armoury_entity"),
                ("origin", "0 0 36"),
                ("item", "4"),
                ("count", "1"),
            ]),
        ]
    };

    // 1. First load: no .nav in VFS and no cache file -> auto-generates v5 .nav and writes cache.
    let vfs_a1 = Vfs::from_layered_memory(
        vec![("cstrike/maps/fy_pool_day.bsp".to_string(), raw_a.clone())],
        VfsConfig::default(),
    );
    let mut game1 = CsGame::new();
    game1.load_bot_assets(&vfs_a1);
    cs_game::bots::set_nav_cache_dir(game1.state_mut(), Some(cache_dir.clone()));

    let mut b1 = Bench::with_game(
        named_floor_bsp("fy_pool_day", &raw_a, make_entities()),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[],
        true,
        &[],
        game1,
    );
    let _ = b1.server.console_command("bot_add_t");
    b1.step(5);

    let (built1, from_cache1, areas1) = b1.run(|g, _| {
        (
            g.bot_nav_built(),
            cs_game::bots::nav_loaded_from_cache(g.state()),
            g.bot_nav_mesh().map_or(0, |m| m.areas().len()),
        )
    });
    assert!(built1);
    assert!(!from_cache1);
    assert!(areas1 > 0);

    let cached_file_a = cache_dir.join(format!("fy_pool_day_{crc_a:08x}.nav"));
    assert!(cached_file_a.is_file(), "expected cached file {}", cached_file_a.display());
    let parsed_a = NavFile::parse(&std::fs::read(&cached_file_a).unwrap()).unwrap();
    assert_eq!(parsed_a.version, NAV_VERSION);
    assert_eq!(parsed_a.areas.len(), areas1);

    // 2. Second load with matching bsp_crc32 -> mounts directly from nav_cache.
    let vfs_a2 = Vfs::from_layered_memory(
        vec![("cstrike/maps/fy_pool_day.bsp".to_string(), raw_a.clone())],
        VfsConfig::default(),
    );
    let mut game2 = CsGame::new();
    game2.load_bot_assets(&vfs_a2);
    cs_game::bots::set_nav_cache_dir(game2.state_mut(), Some(cache_dir.clone()));

    let mut b2 = Bench::with_game(
        named_floor_bsp("fy_pool_day", &raw_a, make_entities()),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[],
        true,
        &[],
        game2,
    );
    let _ = b2.server.console_command("bot_add_t");
    b2.step(5);
    let (built2, from_cache2, areas2) = b2.run(|g, _| {
        (
            g.bot_nav_built(),
            cs_game::bots::nav_loaded_from_cache(g.state()),
            g.bot_nav_mesh().map_or(0, |m| m.areas().len()),
        )
    });
    assert!(built2);
    assert!(from_cache2, "matching bsp_crc32 must hit nav_cache");
    assert_eq!(areas2, areas1);

    // 3. Different bsp_crc32 -> cache miss, regenerates and writes new <map>_<crc_b>.nav.
    let vfs_b = Vfs::from_layered_memory(
        vec![("cstrike/maps/fy_pool_day.bsp".to_string(), raw_b.clone())],
        VfsConfig::default(),
    );
    let mut game3 = CsGame::new();
    game3.load_bot_assets(&vfs_b);
    cs_game::bots::set_nav_cache_dir(game3.state_mut(), Some(cache_dir.clone()));

    let mut b3 = Bench::with_game(
        named_floor_bsp("fy_pool_day", &raw_b, make_entities()),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[],
        true,
        &[],
        game3,
    );
    let _ = b3.server.console_command("bot_add_t");
    b3.step(5);
    assert!(!b3.run(|g, _| cs_game::bots::nav_loaded_from_cache(g.state())));
    let cached_file_b = cache_dir.join(format!("fy_pool_day_{crc_b:08x}.nav"));
    assert!(cached_file_b.is_file(), "expected new cache file {}", cached_file_b.display());

    // 4. ConfigStorage backend (wasm32 path) roundtrip.
    let mut game4 = CsGame::new();
    game4.load_bot_assets(&vfs_a1);
    cs_game::bots::set_nav_cache_storage(game4.state_mut(), Some(Box::new(MemoryStorage::default())));
    let mut b4 = Bench::with_game(
        named_floor_bsp("fy_pool_day", &raw_a, make_entities()),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[],
        true,
        &[],
        game4,
    );
    let _ = b4.server.console_command("bot_add_t");
    b4.step(5);
    assert!(b4.run(|g, _| g.bot_nav_built()));
    // Clearing mounted mesh while keeping `ConfigStorage` reloads directly from storage cache.
    b4.run(|g, _| cs_game::bots::clear_nav_cache_mesh(g.state_mut()));
    b4.step(5);
    assert!(
        b4.run(|g, _| cs_game::bots::nav_loaded_from_cache(g.state())),
        "ConfigStorage nav_cache should serve subsequent reload"
    );
    let _ = std::fs::remove_dir_all(&cache_dir);
}

#[test]
fn fy_pool_day_floor_rifle_scavenging_and_combat() {
    let raw = synthetic_bsp_raw(0x22);
    // No func_buyzone; armoury_entity rifles lie between spawns and pool edges.
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "420 0 36"),
        spawn_at("info_player_deathmatch", "-420 0 36"),
        entity(&[
            ("classname", "armoury_entity"),
            ("origin", "-240 0 36"),
            ("item", "4"), // AK47
            ("count", "2"),
        ]),
        entity(&[
            ("classname", "armoury_entity"),
            ("origin", "240 0 36"),
            ("item", "6"), // M4A1
            ("count", "2"),
        ]),
    ];
    let mut b = Bench::with_slots(named_floor_bsp("fy_pool_day", &raw, entities), 6);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_startmoney 800");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");

    let mut saw_rifle_pickup = false;
    let mut saw_kill = false;
    for _ in 0..240 {
        b.step(5);
        let (has_rifle, any_dead) = b.run(|g, eng| {
            let slots = bot_slots(eng);
            let has_rifle = slots.iter().any(|&s| {
                g.carried_weapons(s)
                    .iter()
                    .any(|w| matches!(w, WeaponId::Ak47 | WeaponId::M4a1))
            });
            let any_dead = slots.iter().any(|&s| {
                g.state().players[usize::from(s)]
                    .as_ref()
                    .is_some_and(|p| !p.alive)
            });
            (has_rifle, any_dead)
        });
        saw_rifle_pickup |= has_rifle;
        saw_kill |= any_dead;
        if saw_rifle_pickup && saw_kill {
            break;
        }
    }

    let summary = b.run(|g, eng| {
        let mut s = g.debug_bot_summary(eng);
        for slot in bot_slots(eng) {
            if let Some((alive, health)) = g.state().players[usize::from(slot)]
                .as_ref()
                .map(|p| (p.alive, p.health))
            {
                let clips: Vec<_> = g
                    .carried_weapons(slot)
                    .into_iter()
                    .map(|w| (w, g.weapon_mut(slot, w).map(|x| x.clip)))
                    .collect();
                let v = eng.ent(eng.client_edict(slot)).unwrap();
                s.push_str(&format!(
                    "  p[{slot}] alive={alive} hp={health} v_angle={:?} button={} state={:?} clips={:?}\n",
                    v.v_angle,
                    v.button,
                    g.player_weapon_state(slot),
                    clips,
                ));
            }
        }
        s
    });
    assert!(saw_rifle_pickup, "bots on fy_pool_day must scavenge floor rifles\n{summary}");
    assert!(saw_kill, "bots must fight and eliminate opponent after scavenging floor rifles\n{summary}");
}

#[test]
fn knife_arena_35hp_bots_close_in_and_kill() {
    let raw = synthetic_bsp_raw(0x35);
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "160 0 36"),
        spawn_at("info_player_deathmatch", "-160 0 36"),
        entity(&[
            ("classname", "game_player_equip"),
            ("origin", "0 0 36"),
            ("weapon_knife", "1"),
        ]),
    ];
    let mut b = Bench::with_slots(named_floor_bsp("35hp_2", &raw, entities), 6);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_startmoney 16000");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    b.step(5);

    // Simulate 35hp_2 map trigger setting both bots to 35 HP.
    b.run(|g, eng| {
        for s in bot_slots(eng) {
            if let Some(p) = g.state_mut().players[usize::from(s)].as_mut() {
                p.health = 35.0;
            }
            if let Some(v) = eng.ent_mut(eng.client_edict(s)) {
                v.health = 35.0;
            }
        }
    });

    let mut bought_any_gun = false;
    let mut saw_knife_kill = false;
    for _ in 0..220 {
        b.step(5);
        let (has_gun, any_dead) = b.run(|g, eng| {
            let slots = bot_slots(eng);
            let has_gun = slots.iter().any(|&s| {
                g.carried_weapons(s)
                    .iter()
                    .any(|&w| w != WeaponId::Knife)
            });
            let any_dead = slots.iter().any(|&s| {
                g.state().players[usize::from(s)]
                    .as_ref()
                    .is_some_and(|p| !p.alive)
            });
            (has_gun, any_dead)
        });
        bought_any_gun |= has_gun;
        saw_knife_kill |= any_dead;
        if saw_knife_kill {
            break;
        }
    }

    assert!(!bought_any_gun, "35hp_2 bots must skip buying guns and carry only knife");
    assert!(saw_knife_kill, "35hp_2 knife bots must close distance and score a knife kill");
}

#[test]
fn awp_india_game_player_equip_skips_buy_and_keeps_awp() {
    let raw = synthetic_bsp_raw(0x44);
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "500 0 36"),
        spawn_at("info_player_deathmatch", "-500 0 36"),
        entity(&[
            ("classname", "game_player_equip"),
            ("origin", "0 0 36"),
            ("weapon_awp", "1"),
            ("weapon_knife", "1"),
            ("ammo_338magnum", "3"),
        ]),
    ];
    let mut b = Bench::with_slots(named_floor_bsp("awp_india", &raw, entities), 6);
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_startmoney 16000");
    let _ = b.server.console_command("bot_add_t");
    let _ = b.server.console_command("bot_add_ct");
    b.step(30);

    let all_awp_only = b.run(|g, eng| {
        let slots = bot_slots(eng);
        assert_eq!(slots.len(), 2);
        slots.iter().all(|&s| {
            let weapons = g.carried_weapons(s);
            let money = g.state().players[usize::from(s)]
                .as_ref()
                .map_or(0, |p| p.money);
            weapons.contains(&WeaponId::Awp)
                && weapons.contains(&WeaponId::Knife)
                && !weapons.iter().any(|w| !matches!(w, WeaponId::Awp | WeaponId::Knife))
                && money == 16000
        })
    });
    assert!(
        all_awp_only,
        "awp_india bots must skip buy logic and keep game_player_equip AWP + knife"
    );
}
