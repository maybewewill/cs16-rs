//! Phase 14 Task 5 integration tests:
//! 1. Classic player/hostage blob shadows (`cl_shadows 1/0`)
//! 2. `cl_minmodels 1/0` + `cl_min_ct` / `cl_min_t` + VIP `ScoreAttrib`
//! 3. Brush-entity local decals (`*N` door rotation + `func_breakable` eviction)
//! 4. 32-bit CS 1.6 smoke cloud (`events/createsmoke.sc`, `gas_puff_01.spr`, `max_smokepuffs`, `fastsprites`)

use std::collections::BTreeMap;
use std::sync::Arc;

use cs_assets::bsp::{
    BspMap, BspModel, Face, Leaf, LightmapAtlas, Node, Plane, TexInfo,
};
use cs_assets::wad::DecodedTexture;
use cs_client::render::decal_renderer::{
    DecalSpawn, build_brush_entity_lookup, project_decal_onto_faces,
    query_submodel_faces_in_sphere, transform_decal_vertices,
};
use cs_client::render::mdl_renderer::build_shadow_disc_vertices;
use cs_client::render::scene::{
    EF_NODRAW, ModelKind, ModelRegistry, RenderEntity, SOLID_BSP, brush_matrix,
    world_to_brush_local,
};
use cs_client::view::{ViewBuilder, ViewInputs, resolve_minmodel_path};
use cs_clientcore::cvars::ClientCvars;
use cs_clientcore::events::{EventFx, FxEnv, fev};
use cs_clientcore::hud::PlayerScore;
use cs_clientcore::interp::{EmitResult, VisibleEntity};
use cs_clientcore::tempents::{GAS_PUFF_SPRITE, ModelRef, RenderMode, TempEntClock};
use cs_clientcore::{ClientEffects, EntityKind, ModelKind as CoreModelKind, PrecacheEntry, PrecacheTables};
use cs_pm::collision::{CONTENTS_EMPTY, CONTENTS_SOLID, WorldCollision};
use cs_proto48::delta::GoldSrcEntityState;
use cs_proto48::event::EventArgs;
use glam::Vec3;

fn make_shadow_and_door_bsp() -> BspMap {
    // Model 0 (world): floor plane z = 0 (normal +Z, children[0] = -2 EMPTY (z >= 0), children[1] = -1 SOLID (z < 0)).
    // Model 1 (*1): brush box [-16, 16] x [-32, 32] x [0, 64] with plane x = 16 (normal +X, face 0) and z = 48 (normal +Z).
    // For BSP model 1: node 1 splits on plane x = 16? Let's make a simple slab z <= 48 in model 1 for shadow test,
    // and also include face 0 on x = 16 for decal clipping!
    BspMap {
        name: "task5_test".into(),
        skyname: String::new(),
        entities: Vec::new(),
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes: vec![
            // Plane 0: z = 0 (+Z)
            Plane {
                normal: Vec3::Z,
                dist: 0.0,
                plane_type: 2,
            },
            // Plane 1: z = 48 (+Z) for brush *1 top surface
            Plane {
                normal: Vec3::Z,
                dist: 48.0,
                plane_type: 2,
            },
            // Plane 2: x = 16 (+X) for brush *1 front face decal
            Plane {
                normal: Vec3::X,
                dist: 16.0,
                plane_type: 0,
            },
        ],
        // Box vertices on x = 16 plane: y in [-32, 32], z in [0, 64]
        vertices_raw: vec![
            Vec3::new(16.0, -32.0, 0.0),
            Vec3::new(16.0, 32.0, 0.0),
            Vec3::new(16.0, 32.0, 64.0),
            Vec3::new(16.0, -32.0, 64.0),
        ],
        visibility: Vec::new(),
        nodes: vec![
            // Node 0 (world): z >= 0 -> leaf -2 (empty), z < 0 -> leaf -1 (solid)
            Node {
                plane: 0,
                children: [-2, -1],
                mins: [-512, -512, -512],
                maxs: [512, 512, 512],
                first_face: 0,
                num_faces: 0,
            },
            // Node 1 (*1): z >= 48 -> leaf -2 (empty), z < 48 -> leaf -1 (solid)
            Node {
                plane: 1,
                children: [-2, -1],
                mins: [-64, -64, 0],
                maxs: [64, 64, 64],
                first_face: 0,
                num_faces: 1,
            },
        ],
        texinfos: vec![TexInfo {
            s_vec: Vec3::Y,
            s_shift: 0.0,
            t_vec: Vec3::Z,
            t_shift: 0.0,
            miptex: 0,
            flags: 0,
        }],
        faces: vec![Face {
            plane: 2,
            plane_side: 0,
            first_edge: 0,
            num_edges: 4,
            texinfo: 0,
            styles: [0, 255, 255, 255],
            lightmap_offset: -1,
        }],
        lighting: Vec::new(),
        clipnodes: Vec::new(),
        leafs: vec![
            Leaf {
                contents: CONTENTS_SOLID,
                vis_offset: -1,
                mins: [-512; 3],
                maxs: [512; 3],
                first_mark_surface: 0,
                num_mark_surfaces: 0,
                ambient_levels: [0; 4],
            },
            Leaf {
                contents: CONTENTS_EMPTY,
                vis_offset: -1,
                mins: [-512; 3],
                maxs: [512; 3],
                first_mark_surface: 0,
                num_mark_surfaces: 0,
                ambient_levels: [0; 4],
            },
        ],
        mark_surfaces: Vec::new(),
        edges: vec![[0, 1], [1, 2], [2, 3], [3, 0]],
        surf_edges: vec![0, 1, 2, 3],
        models: vec![
            BspModel {
                mins: Vec3::splat(-512.0),
                maxs: Vec3::splat(512.0),
                origin: Vec3::ZERO,
                head_nodes: [0, -1, -1, -1],
                vis_leafs: 2,
                first_face: 0,
                num_faces: 0,
            },
            BspModel {
                mins: Vec3::new(-64.0, -64.0, 0.0),
                maxs: Vec3::new(64.0, 64.0, 64.0),
                origin: Vec3::ZERO,
                head_nodes: [1, -1, -1, -1],
                vis_leafs: 2,
                first_face: 0,
                num_faces: 1,
            },
        ],
        textures: vec![DecodedTexture {
            name: "door_metal".into(),
            width: 64,
            height: 64,
            rgba: vec![255; 64 * 64 * 4],
            alpha_test: false,
        }],
        lightmap_atlas: LightmapAtlas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            packed_faces: 0,
        },
        render_vertices: Vec::new(),
        render_indices: Vec::new(),
        batches: Vec::new(),
    }
}

fn make_precache() -> PrecacheTables {
    let mut precache = PrecacheTables::default();
    for (idx, name) in [
        (1u16, "maps/task5_test.bsp"),
        (2, "*1"),
        (3, "models/player.mdl"),
        (4, "models/scientist.mdl"),
    ] {
        precache.models.insert(
            idx,
            PrecacheEntry {
                name: name.to_string(),
                download_size: 0,
                flags: 0,
            },
        );
    }
    precache
}

fn vis_player(number: u16, origin: Vec3, local_player: bool) -> VisibleEntity {
    VisibleEntity {
        number,
        kind: EntityKind::Player,
        model: CoreModelKind::Studio,
        modelindex: 3,
        origin,
        angles: Vec3::ZERO,
        frame: 0.0,
        sequence: 0,
        rendermode: 0,
        renderamt: 255,
        renderfx: 0,
        rendercolor: [255, 255, 255],
        local_player,
        studio_lerp: false,
        latched: Default::default(),
        state: GoldSrcEntityState {
            solid: 3,
            movetype: 3,
            ..Default::default()
        },
    }
}

#[test]
fn cl_shadows_projects_blob_shadow_for_live_players_and_hostages_and_respects_cvar() {
    let bsp = make_shadow_and_door_bsp();
    let coll = Arc::new(WorldCollision::from_bsp(&bsp));
    let precache = make_precache();

    let mut builder = ViewBuilder::new();
    builder.set_world_context(Some(coll), None);

    let mut fx = ClientEffects::default();
    // Player 2 is live, Player 3 is dead (ScoreAttrib bit 0 = 1).
    fx.hud.players.insert(
        2,
        PlayerScore {
            team: "CT".into(),
            attrib: 0,
            ..Default::default()
        },
    );
    fx.hud.players.insert(
        3,
        PlayerScore {
            team: "TERRORIST".into(),
            attrib: 1,
            ..Default::default()
        },
    );

    let make_emitted = || EmitResult {
        entities: vec![
            // Local player 1: excluded from third-person shadows.
            vis_player(1, Vec3::new(-50.0, 0.0, 36.0), true),
            // Live remote player 2 standing on world floor z = 0 (origin z = 36).
            vis_player(2, Vec3::new(0.0, 0.0, 36.0), false),
            // Dead remote player 3: must not cast live shadow.
            vis_player(3, Vec3::new(-100.0, 0.0, 36.0), false),
            // Solid brush platform *1 at (150, 0, 0), top surface at z = 48.
            VisibleEntity {
                number: 20,
                kind: EntityKind::Normal,
                model: CoreModelKind::Brush,
                modelindex: 2,
                origin: Vec3::new(150.0, 0.0, 0.0),
                angles: Vec3::ZERO,
                frame: 0.0,
                sequence: 0,
                rendermode: 0,
                renderamt: 255,
                renderfx: 0,
                rendercolor: [255, 255, 255],
                local_player: false,
                studio_lerp: false,
                latched: Default::default(),
                state: GoldSrcEntityState {
                    solid: SOLID_BSP as u8,
                    ..Default::default()
                },
            },
            // Live hostage standing on top of brush platform *1 (origin z = 48).
            VisibleEntity {
                number: 21,
                kind: EntityKind::Normal,
                model: CoreModelKind::Studio,
                modelindex: 4,
                origin: Vec3::new(150.0, 0.0, 48.0),
                angles: Vec3::ZERO,
                frame: 0.0,
                sequence: 0,
                rendermode: 0,
                renderamt: 255,
                renderfx: 0,
                rendercolor: [255, 255, 255],
                local_player: false,
                studio_lerp: false,
                latched: Default::default(),
                state: GoldSrcEntityState {
                    solid: 3,
                    movetype: 4,
                    ..Default::default()
                },
            },
        ],
        ..Default::default()
    };

    let mut cvars = ClientCvars::new();
    assert_eq!(cvars.float("cl_shadows"), 1.0);
    let (frame, _) = builder.build_parts_with_cvars(
        &precache,
        &mut fx,
        &BTreeMap::new(),
        None,
        Some(0),
        None,
        make_emitted(),
        Vec::new(),
        Vec3::ZERO,
        &ViewInputs::default(),
        Some(&cvars),
    );
    assert_eq!(frame.scene.shadows.len(), 2, "live player + live hostage");
    // BSP Hull::Point traces back off planes by DIST_EPSILON (0.03125 u), then +0.15 u along normal.
    let p_sh = frame.scene.shadows[0];
    assert!(
        (p_sh.origin.z - 0.18125).abs() < 1e-4,
        "player shadow offset DIST_EPSILON + 0.15u: {:?}",
        p_sh
    );
    assert!((p_sh.alpha - 0.5 * (1.0 - 36.0 / 256.0)).abs() < 1e-3);

    let h_sh = frame.scene.shadows[1];
    assert!(
        (h_sh.origin.z - 48.18125).abs() < 1e-4,
        "hostage shadow on brush *1: {:?}",
        h_sh
    );
    assert!((h_sh.alpha - 0.5).abs() < 1e-3);

    // Feathered octagonal mesh geometry check: 8 triangles (24 verts), center alpha = shadow.alpha, rim alpha = 0.
    let disc = build_shadow_disc_vertices(&p_sh);
    assert_eq!(disc.len(), 24);
    assert!((disc[0].color[3] - p_sh.alpha).abs() < 1e-6);
    assert_eq!(disc[1].color[3], 0.0);

    // Disabling cl_shadows suppresses all blob shadows.
    cvars.set("cl_shadows", "0");
    let (frame_off, _) = builder.build_parts_with_cvars(
        &precache,
        &mut fx,
        &BTreeMap::new(),
        None,
        Some(0),
        None,
        make_emitted(),
        Vec::new(),
        Vec3::ZERO,
        &ViewInputs::default(),
        Some(&cvars),
    );
    assert!(frame_off.scene.shadows.is_empty());
}

#[test]
fn cl_minmodels_overrides_team_and_vip_models_and_respects_min_cvars() {
    assert_eq!(
        resolve_minmodel_path("TERRORIST", 1, 0, 2, 1),
        Some("models/player/leet/leet.mdl")
    );
    assert_eq!(
        resolve_minmodel_path("CT", 2, 0, 2, 1),
        Some("models/player/gign/gign.mdl")
    );
    assert_eq!(
        resolve_minmodel_path("CT", 2, 4, 7, 1),
        Some("models/player/vip/vip.mdl")
    );
    assert_eq!(
        resolve_minmodel_path("CT", 2, 0, 7, 1),
        Some("models/player/sas/sas.mdl")
    );
    assert_eq!(
        resolve_minmodel_path("TERRORIST", 1, 0, 2, 6),
        Some("models/player/arctic/arctic.mdl")
    );

    let precache = make_precache();
    let mut builder = ViewBuilder::new();
    let mut fx = ClientEffects::default();
    fx.hud.players.insert(
        2,
        PlayerScore {
            team: "CT".into(),
            attrib: 0,
            ..Default::default()
        },
    );
    fx.hud.players.insert(
        3,
        PlayerScore {
            team: "CT".into(),
            attrib: 4, // VIP
            ..Default::default()
        },
    );
    fx.hud.players.insert(
        4,
        PlayerScore {
            team: "TERRORIST".into(),
            attrib: 0,
            ..Default::default()
        },
    );

    let mut userinfos = BTreeMap::new();
    userinfos.insert(1, "\\name\\ct_reg\\model\\urban".to_string());
    userinfos.insert(2, "\\name\\ct_vip\\model\\urban".to_string());
    userinfos.insert(3, "\\name\\t_reg\\model\\guerilla".to_string());

    let make_emitted = || EmitResult {
        entities: vec![
            vis_player(2, Vec3::new(50.0, 0.0, 0.0), false),
            vis_player(3, Vec3::new(100.0, 0.0, 0.0), false),
            vis_player(4, Vec3::new(150.0, 0.0, 0.0), false),
        ],
        ..Default::default()
    };

    let mut cvars = ClientCvars::new();
    // cl_minmodels = 0 -> userinfo models kept.
    let (v_off, _) = builder.build_parts_with_cvars(
        &precache,
        &mut fx,
        &userinfos,
        None,
        Some(0),
        None,
        make_emitted(),
        Vec::new(),
        Vec3::ZERO,
        &ViewInputs::default(),
        Some(&cvars),
    );
    let path_off = |id: u16| -> &str {
        let e = v_off.scene.entities.iter().find(|e| e.index == id).unwrap();
        builder.models().get(e.model).unwrap().name.as_str()
    };
    assert_eq!(path_off(2), "models/player/urban/urban.mdl");
    assert_eq!(path_off(3), "models/player/urban/urban.mdl");
    assert_eq!(path_off(4), "models/player/guerilla/guerilla.mdl");

    // Enable cl_minmodels 1 -> overrides to gign, vip, leet on the same frame!
    cvars.set("cl_minmodels", "1");
    let (v_on, reg_opt) = builder.build_parts_with_cvars(
        &precache,
        &mut fx,
        &userinfos,
        None,
        Some(0),
        None,
        make_emitted(),
        Vec::new(),
        Vec3::ZERO,
        &ViewInputs::default(),
        Some(&cvars),
    );
    let reg = reg_opt.expect("cl_minmodels adds/updates client model registry immediately");
    let path_of = |id: u16| -> &str {
        let e = v_on.scene.entities.iter().find(|e| e.index == id).unwrap();
        reg.get(e.model).unwrap().name.as_str()
    };
    assert_eq!(path_of(2), "models/player/gign/gign.mdl");
    assert_eq!(path_of(3), "models/player/vip/vip.mdl");
    assert_eq!(path_of(4), "models/player/leet/leet.mdl");

    // Override via cl_min_ct = 7 (sas) and cl_min_t = 6 (arctic); VIP stays vip.mdl.
    cvars.set("cl_min_ct", "7");
    cvars.set("cl_min_t", "6");
    let (v_custom, reg_opt2) = builder.build_parts_with_cvars(
        &precache,
        &mut fx,
        &userinfos,
        None,
        Some(0),
        None,
        make_emitted(),
        Vec::new(),
        Vec3::ZERO,
        &ViewInputs::default(),
        Some(&cvars),
    );
    let reg2 = reg_opt2.expect("custom minmodels registry update");
    let path_of2 = |id: u16| -> &str {
        let e = v_custom.scene.entities.iter().find(|e| e.index == id).unwrap();
        reg2.get(e.model).unwrap().name.as_str()
    };
    assert_eq!(path_of2(2), "models/player/sas/sas.mdl");
    assert_eq!(path_of2(3), "models/player/vip/vip.mdl");
    assert_eq!(path_of2(4), "models/player/arctic/arctic.mdl");
}

#[test]
fn bullet_impact_onto_solid_brush_entity_sets_entity_index_and_model_index() {
    use cs_clientcore::events::{BulletFire, BulletType, FiredEvent, WeaponFireFx};

    let bsp = make_shadow_and_door_bsp();
    let coll = Arc::new(WorldCollision::from_bsp(&bsp));
    let precache = make_precache();
    let mut builder = ViewBuilder::new();
    builder.set_world_context(Some(coll), None);
    builder.set_map_bsp(Some(Arc::new(bsp)));

    let mut fx = ClientEffects::default();
    // Solid brush entity 10 (*1) at (100, 0, 0): top plane is z = 48.
    let emitted = EmitResult {
        entities: vec![VisibleEntity {
            number: 10,
            kind: EntityKind::Normal,
            model: CoreModelKind::Brush,
            modelindex: 2,
            origin: Vec3::new(100.0, 0.0, 0.0),
            angles: Vec3::ZERO,
            frame: 0.0,
            sequence: 0,
            rendermode: 0,
            renderamt: 255,
            renderfx: 0,
            rendercolor: [255, 255, 255],
            local_player: false,
            studio_lerp: false,
            latched: Default::default(),
            state: GoldSrcEntityState {
                solid: SOLID_BSP as u8,
                ..Default::default()
            },
        }],
        ..Default::default()
    };
    let fired = vec![FiredEvent {
        index: 1,
        name: Some("events/ak47.sc".into()),
        flags: 0,
        args: EventArgs::default(),
        velocity: Vec3::ZERO,
        fx: EventFx::Weapon(WeaponFireFx {
            weapon: "ak47",
            entity: 1,
            local: true,
            view_anim: None,
            muzzle_flash: false,
            shell: None,
            sound: None,
            bullets: Some(BulletFire {
                entity: 1,
                origin: Vec3::new(100.0, 0.0, 100.0),
                forward: Vec3::NEG_Z,
                right: Vec3::Y,
                up: Vec3::X,
                shots: 1,
                spread: [0.0, 0.0],
                distance: 256.0,
                bullet: BulletType::Mm762,
                penetration: 2,
            }),
        }),
    }];

    let (view, _) = builder.build_parts(
        &precache,
        &mut fx,
        &BTreeMap::new(),
        None,
        Some(0),
        None,
        emitted,
        fired,
        Vec3::ZERO,
        &ViewInputs::default(),
    );
    assert_eq!(view.decals.len(), 1);
    assert_eq!(view.decals[0].entity_index, 10);
    assert_eq!(view.decals[0].model_index, 2);
    assert!((view.decals[0].origin.z - 48.0).abs() < 0.1);
}

#[test]
fn brush_entity_decals_follow_door_rotation_and_evict_when_breakable_breaks() {
    let bsp = make_shadow_and_door_bsp();
    let reg = ModelRegistry::from_precache(&["", "maps/task5_test.bsp", "*1"]);
    let h_brush = reg.resolve(2).unwrap();
    assert_eq!(reg.get(h_brush).unwrap().kind, ModelKind::Brush(1));

    // Door pivot at (100, 0, 0), closed (angles = 0).
    let mut door = RenderEntity::new(10, h_brush, Vec3::new(100.0, 0.0, 0.0));
    door.solid = SOLID_BSP;
    let lookup_closed = build_brush_entity_lookup(std::slice::from_ref(&door), &reg);
    let tf_closed = lookup_closed.get(&10).expect("active door in lookup");

    // Bullet hits world pos (116, 0, 32) on face x=16 of *1.
    let spawn = DecalSpawn {
        origin: Vec3::new(116.0, 0.0, 32.0),
        decal_name: "{shot1".into(),
        half_size: 4.0,
        permanent: false,
        player_spray: None,
        entity_index: 10,
        model_index: 2,
    };
    let local_origin = world_to_brush_local(spawn.origin, tf_closed.origin, tf_closed.angles);
    assert!((local_origin - Vec3::new(16.0, 0.0, 32.0)).length() < 1e-4);

    let faces = query_submodel_faces_in_sphere(&bsp, tf_closed.submodel, local_origin, 6.0);
    assert_eq!(faces.len(), 1, "clipped against *1 local face");
    let local_verts = project_decal_onto_faces(&faces, local_origin, 4.0, 0.35, [1.0; 4]);
    assert_eq!(local_verts.len(), 6);
    for v in &local_verts {
        assert!((v.position[0] - 16.35).abs() < 1e-4);
    }

    // Rotate the door 90 degrees yaw: local +X (16.35) becomes world +Y (16.35) around pivot (100, 0, 0),
    // while local y in [-4, 4] maps to world x = 100 - y in [96, 104].
    door.angles = Vec3::new(0.0, 90.0, 0.0);
    let world_verts = transform_decal_vertices(&local_verts, brush_matrix(door.origin, door.angles));
    for v in &world_verts {
        assert!((95.9..=104.1).contains(&v.position[0]), "x={}", v.position[0]);
        assert!((v.position[1] - 16.35).abs() < 1e-3, "y={}", v.position[1]);
    }
    let avg_x: f32 = world_verts.iter().map(|v| v.position[0]).sum::<f32>() / world_verts.len() as f32;
    assert!((avg_x - 100.0).abs() < 1e-3, "avg_x={avg_x}");

    // Breakable shatters: SOLID_NOT (solid = 0) or EF_NODRAW -> evicted from lookup!
    let mut broken_solid = door.clone();
    broken_solid.solid = 0;
    assert!(build_brush_entity_lookup(&[broken_solid], &reg).is_empty());

    let mut broken_nodraw = door.clone();
    broken_nodraw.effects = EF_NODRAW;
    assert!(build_brush_entity_lookup(&[broken_nodraw], &reg).is_empty());
}

#[test]
fn smoke_cloud_createsmoke_event_lifecycle_and_cvars() {
    let mut precache = PrecacheTables::default();
    precache.add(&[cs_proto48::messages::ResourceEntry {
        res_type: cs_clientcore::res_type::EVENTSCRIPT,
        filename: "events/createsmoke.sc".into(),
        index: 1,
        download_size: 0,
        flags: 0,
        md5: None,
        reserved: None,
    }]);

    let ents = BTreeMap::new();
    let mut fx = ClientEffects::with_seed(42);
    fx.set_smoke_cvars(120, 0);

    let burst_origin = Vec3::new(200.0, -100.0, 10.0);
    let env0 = FxEnv::new(10.0, &precache, &ents, &cs_clientcore::NoWorld);
    // Detonation event (`iparam2 = 1`, `bparam2 = 0`).
    fx.events.queue(
        fev::SERVER,
        1,
        0.0,
        10.0,
        EventArgs {
            origin: burst_origin,
            iparam2: 1,
            bparam1: 0,
            bparam2: 0,
            ..Default::default()
        },
        Vec3::ZERO,
    );
    let fired0 = fx.update(&env0, 800.0);
    assert!(matches!(fired0[0].fx, EventFx::CreateSmoke { first: true, .. }));
    assert_eq!(fx.tempents.active_smoke_puff_count(), 20);

    // Follow-up SG_Smoke event (`iparam2 = 4`, `bparam2 = 0` on wire) must NOT respawn 20 clouds!
    let env1 = FxEnv::new(11.0, &precache, &ents, &cs_clientcore::NoWorld);
    fx.events.queue(
        fev::SERVER,
        1,
        0.0,
        11.0,
        EventArgs {
            origin: burst_origin,
            iparam1: 200,
            iparam2: 4,
            bparam2: 0,
            ..Default::default()
        },
        Vec3::ZERO,
    );
    let fired1 = fx.update(&env1, 800.0);
    assert!(matches!(fired1[0].fx, EventFx::CreateSmoke { first: false, .. }));
    assert_eq!(fx.tempents.active_smoke_puff_count(), 20);

    // Verify 32-bit grey alpha layering (not additive white) and ~115-130 u effective cloud radius.
    for t in fx.tempents.active() {
        assert_eq!(t.model, ModelRef::Client(GAS_PUFF_SPRITE));
        assert_eq!(t.render_mode, RenderMode::TransAlpha);
        assert!(t.render_color[0] >= 100 && t.render_color[0] <= 145);
        let horiz = Vec3::new(t.origin.x - burst_origin.x, t.origin.y - burst_origin.y, 0.0).length();
        assert!(horiz <= 75.0, "puff center within ~72 u scatter: {horiz}");
        assert!(t.scale >= 2.5 && t.scale <= 3.3, "scale={}", t.scale);
    }

    // Hold for 25s (at t = 34.5 -> 24.5s elapsed, still full 255), fade over 25..30s (at t = 37.5 -> ~half).
    fx.tempents.update(
        TempEntClock { time: 34.5, frametime: 1.0, gravity: 800.0 },
        &cs_clientcore::NoWorld,
        &mut fx.rng,
        &mut fx.sounds,
    );
    assert_eq!(fx.tempents.active_smoke_puff_count(), 20);
    assert!(fx.tempents.active().iter().all(|t| (t.render_amt - 255.0).abs() < 1e-3));

    fx.tempents.update(
        TempEntClock { time: 37.5, frametime: 2.5, gravity: 800.0 },
        &cs_clientcore::NoWorld,
        &mut fx.rng,
        &mut fx.sounds,
    );
    assert!(fx.tempents.active().iter().all(|t| t.render_amt > 100.0 && t.render_amt < 150.0));

    // Expire after 30s.
    fx.tempents.update(
        TempEntClock { time: 40.1, frametime: 0.1, gravity: 800.0 },
        &cs_clientcore::NoWorld,
        &mut fx.rng,
        &mut fx.sounds,
    );
    assert_eq!(fx.tempents.active_smoke_puff_count(), 0);

    // Check max_smokepuffs and fastsprites caps.
    fx.set_smoke_cvars(12, 0);
    let env_cap = FxEnv::new(50.0, &precache, &ents, &cs_clientcore::NoWorld);
    fx.events.queue(
        fev::SERVER,
        1,
        0.0,
        50.0,
        EventArgs { origin: burst_origin, iparam2: 1, ..Default::default() },
        Vec3::ZERO,
    );
    fx.update(&env_cap, 800.0);
    assert_eq!(fx.tempents.active_smoke_puff_count(), 12);
}
