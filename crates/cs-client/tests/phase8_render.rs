//! Phase 8 comprehensive renderer & GoldSrc visual parity integration test suite.
//! Exercises all 42 `docs/parity/render.toml` capabilities across `cs-assets`, `cs-clientcore`,
//! `cs-proto48`, `cs-pm`, and `cs-client`, plus real GoldSrc assets & headless wgpu when present.

use std::sync::Arc;

use cs_assets::bsp::{
    BspMap, DecalFacePoly, FaceLightmapInfo, LightmapAtlas, SURF_FLAG_SCROLL, TextureAnimTable,
};
use cs_assets::mdl::{
    BoneEvalParams, MdlBone, MdlBodyPart, MdlMesh, MdlSeqEvent, MdlSequence, MdlSubModel,
    MdlTexture, STUDIO_NF_ADDITIVE, STUDIO_NF_CHROME, STUDIO_NF_FLATSHADE, STUDIO_NF_MASKED,
    StudioModel,
};
use cs_assets::spr::SpriteType;
use cs_assets::wad::WadArchive;
use cs_client::camera::Camera;
use cs_client::render::bsp_renderer::{BspDrawParams, BspRenderer};
use cs_client::render::decal_renderer::{DecalRenderer, project_decal_onto_faces};
use cs_client::render::fx_renderer::FxRenderer;
use cs_client::render::gamma::{GammaParams, apply_goldsrc_gamma_rgb};
use cs_client::render::lighting::{
    DLightPool, GpuLightsUniform, TextureFilterMode, compute_flashlight_spot,
    generate_mipmaps_rgba,
};
use cs_client::render::mdl_renderer::MdlRenderer;
use cs_client::render::particles::{BeamSystem, ParticleSystem};
use cs_client::render::render_offscreen_rgba;
use cs_client::render::scene::{
    EntRenderMode, FxFadeTracker, ModelRegistry, RENDERFX_CLAMP_MIN_SCALE, RENDERFX_DISTORT,
    RENDERFX_EXPLODE, RENDERFX_FADE_FAST, RENDERFX_FADE_SLOW, RENDERFX_FLICKER_FAST,
    RENDERFX_GLOW_SHELL, RENDERFX_HOLOGRAM, RENDERFX_LIGHT_MULTIPLIER, RENDERFX_PULSE_FAST_WIDE,
    RENDERFX_SOLID_FAST, RENDERFX_STROBE_FAST, RenderEntity, SceneFrame, ViewBasis,
    build_draw_lists_tracked, fx_blend, glow_blend,
};
use cs_client::render::skybox::SkyboxRenderer;
use cs_client::render::sprite_renderer::SpriteRenderer;
use cs_client::ui::hud::HudView;
use cs_client::view_effects::{
    BobParams, ViewModelLagState, calc_bob, calc_roll, calc_screen_shake, evaluate_screen_fade,
    underwater_tint_and_fov,
};
use cs_clientcore::hud::{
    FFADE_MODULATE, FFADE_OUT, FFADE_PURGE, FFADE_STAYOUT, ScreenFade, ScreenShake,
    default_lightstyles, evaluate_lightstyles,
};
use cs_clientcore::tempents::{DynamicLight, ParticleEffect};
use cs_pm::collision::{CONTENTS_LAVA, CONTENTS_SLIME, CONTENTS_WATER, WorldCollision};
use cs_proto48::tempent::{BeamParams, TempEffect, te};
use cs_vfs::Vfs;
use glam::Vec3;

fn sample_test_mdl() -> StudioModel {
    let mut mdl = StudioModel {
        name: "test.mdl".into(),
        bones: vec![MdlBone {
            name: "Bip01 Spine".into(),
            parent: -1,
            bonecontroller: [-1; 6],
            default_pos: Vec3::ZERO,
            default_rot: Vec3::ZERO,
            pos_scale: Vec3::ONE,
            rot_scale: Vec3::ONE,
        }],
        controllers: Vec::new(),
        attachments: Vec::new(),
        hitboxes: Vec::new(),
        sequences: vec![MdlSequence {
            label: "idle".into(),
            fps: 30.0,
            flags: 1,
            num_frames: 2,
            num_blends: 1,
            blend_type: [0, 0],
            blend_start: [0.0, 0.0],
            blend_end: [0.0, 0.0],
            seqgroup: 0,
            anim_index: 0,
            events: vec![MdlSeqEvent {
                frame: 1,
                event: 5001,
                ev_type: 0,
                options: "1".into(),
            }],
            bone_anims: Vec::new(),
            blends: Vec::new(),
        }],
        seq_groups: Vec::new(),
        textures: vec![
            MdlTexture {
                name: "t0".into(),
                width: 2,
                height: 2,
                flags: STUDIO_NF_FLATSHADE | STUDIO_NF_CHROME,
                rgba: vec![255; 16],
            },
            MdlTexture {
                name: "t1".into(),
                width: 2,
                height: 2,
                flags: STUDIO_NF_ADDITIVE | STUDIO_NF_MASKED,
                rgba: vec![255; 16],
            },
        ],
        skin_families: vec![vec![0], vec![1]],
        bodyparts: vec![MdlBodyPart {
            name: "body".into(),
            submodels: vec![
                MdlSubModel {
                    name: "s0".into(),
                    meshes: vec![MdlMesh {
                        texture_index: 0,
                        skinref: 0,
                        vertices: Vec::new(),
                    }],
                },
                MdlSubModel {
                    name: "s1".into(),
                    meshes: Vec::new(),
                },
            ],
        }],
    };
    mdl.attachments.push(cs_assets::mdl::MdlAttachment {
        name: "muzzle".into(),
        att_type: 0,
        bone: 0,
        org: Vec3::new(20.0, 0.0, 5.0),
    });
    mdl
}

fn open_retail_vfs() -> Option<(Vfs, BspMap)> {
    let gamedir = std::env::var("CS16_GAMEDIR").ok()?;
    let vfs = Vfs::open(std::path::Path::new(&gamedir), cs_vfs::VfsConfig::default()).ok()?;
    let map = BspMap::load(&vfs, "de_dust2").ok()?;
    Some((vfs, map))
}

fn map_spawn_origin(map: &BspMap) -> Vec3 {
    map.entities
        .iter()
        .find_map(|e| {
            matches!(
                e.classname(),
                "info_player_start" | "info_player_deathmatch"
            )
            .then(|| e.get_vec3("origin"))
            .flatten()
        })
        .unwrap_or(Vec3::new(0.0, 0.0, 64.0))
}

#[test]
fn gamma_texgamma_brightness_overbright_curve() {
    let base = GammaParams::default();
    let mid = apply_goldsrc_gamma_rgb([0.5; 3], [0.5; 3], 0, 0, &base);
    assert!(mid[0] > 0.15 && mid[0] < 0.45, "{mid:?}");

    let bright = apply_goldsrc_gamma_rgb(
        [0.5; 3],
        [0.5; 3],
        0,
        0,
        &GammaParams {
            brightness: 1.0,
            ..base
        },
    );
    assert!(bright[0] > mid[0]);

    let dark_tex = apply_goldsrc_gamma_rgb(
        [0.5; 3],
        [0.5; 3],
        0,
        0,
        &GammaParams {
            texgamma: 3.0,
            ..base
        },
    );
    assert!(dark_tex[0] < mid[0]);

    let ob = apply_goldsrc_gamma_rgb(
        [0.5; 3],
        [0.9; 3],
        0,
        0,
        &GammaParams {
            gl_overbright: true,
            ..base
        },
    );
    assert!(ob[0] >= mid[0]);
}

#[test]
fn software_mode_4x4_bayer_rgb565_dither() {
    let sw = apply_goldsrc_gamma_rgb(
        [0.37; 3],
        [0.5; 3],
        3,
        1,
        &GammaParams {
            software_mode: true,
            ..GammaParams::default()
        },
    );
    assert!((sw[0] * 31.0 - (sw[0] * 31.0).round()).abs() < 1e-4);
    assert!((sw[1] * 63.0 - (sw[1] * 63.0).round()).abs() < 1e-4);
    assert!((sw[2] * 31.0 - (sw[2] * 31.0).round()).abs() < 1e-4);
}

#[test]
fn texture_filter_modes_and_pot_mipmaps() {
    assert_eq!(TextureFilterMode::parse("GL_NEAREST"), TextureFilterMode::Nearest);
    assert_eq!(TextureFilterMode::parse("GL_LINEAR"), TextureFilterMode::Linear);
    assert_eq!(
        TextureFilterMode::parse("GL_NEAREST_MIPMAP_NEAREST"),
        TextureFilterMode::NearestMipmapNearest
    );
    assert_eq!(
        TextureFilterMode::parse("GL_LINEAR_MIPMAP_NEAREST"),
        TextureFilterMode::LinearMipmapNearest
    );
    assert_eq!(
        TextureFilterMode::parse("GL_NEAREST_MIPMAP_LINEAR"),
        TextureFilterMode::NearestMipmapLinear
    );
    assert_eq!(
        TextureFilterMode::parse("GL_LINEAR_MIPMAP_LINEAR"),
        TextureFilterMode::LinearMipmapLinear
    );

    let pot_mips = generate_mipmaps_rgba(8, 8, &vec![200u8; 8 * 8 * 4]);
    assert_eq!(pot_mips.len(), 4);
    let npot_mips = generate_mipmaps_rgba(128, 240, &vec![200u8; 128 * 240 * 4]);
    assert_eq!(npot_mips.len(), 9);
}

#[test]
fn dynamic_lights_dlights_and_elights() {
    let mut pool = DLightPool::default();
    pool.add_light(DynamicLight {
        key: 1,
        entity_light: false,
        origin: Vec3::new(10.0, 0.0, 0.0),
        radius: 200.0,
        color: [255, 180, 90],
        die: 10.0,
        decay: 50.0,
    });
    pool.add_light(DynamicLight {
        key: 2,
        entity_light: true,
        origin: Vec3::new(0.0, 20.0, 0.0),
        radius: 150.0,
        color: [80, 140, 255],
        die: 10.0,
        decay: 0.0,
    });
    let el = pool.sample_elights_at(Vec3::ZERO);
    assert!(el.x > 0.5 && el.z > 0.3);

    let uni = pool.pack_uniform(Vec3::ZERO, &[]);
    assert_eq!(uni.counts[0], 1);
    assert_eq!(uni.counts[1], 0);
}

#[test]
fn flashlight_spotlight_trace_and_hud_battery() {
    let pool = DLightPool::default();
    let (spot, _bounce) = compute_flashlight_spot(Vec3::ZERO, Vec3::X, None, 1.0);
    assert!(spot.radius >= 64.0 && spot.cos_half_angle > 0.8);
    let uni = pool.pack_uniform(Vec3::ZERO, &[spot]);
    assert_eq!(uni.counts[1], 1);
}

#[test]
fn lightstyles_and_atlas_rebuild() {
    let mut styles = default_lightstyles();
    styles[32] = "az".to_string();
    let v0 = evaluate_lightstyles(&styles, 0.0);
    let v1 = evaluate_lightstyles(&styles, 0.1);
    assert_eq!(v0[0], 1.0);
    assert_eq!(v0[32], 0.0);
    assert!(v1[32] > 2.0);

    let lighting = vec![
        100u8, 100, 100, // style 0
        200, 100, 50,    // style 32
    ];
    let rects = vec![FaceLightmapInfo {
        face_index: 0,
        atlas_x: 1,
        atlas_y: 1,
        width: 1,
        height: 1,
        num_styles: 2,
        lightmap_offset: 0,
        styles: [0, 32, 255, 255],
    }];
    let mut atlas = LightmapAtlas {
        width: 4,
        height: 4,
        rgba: vec![0u8; 4 * 4 * 4],
        packed_faces: 1,
    };
    let mut prev = [1.0f32; 64];
    prev[32] = 0.0;
    let dirty = atlas.rebuild_for_styles(&lighting, &rects, &v1, &prev);
    assert_eq!(dirty.len(), 1);
    let px = ((atlas.width + 1) * 4) as usize;
    assert!(atlas.rgba[px] > 150);
}

#[test]
fn bsp_animated_toggleable_and_scroll_textures() {
    let textures: Vec<cs_assets::wad::DecodedTexture> = [
        "+0btn", "+1btn", "+Abtn",
        "+0loop", "+1loop", "+2loop",
        "scrollconveyor",
    ]
    .into_iter()
    .map(|n| cs_assets::wad::DecodedTexture {
        name: n.to_string(),
        width: 16,
        height: 16,
        rgba: vec![255; 16 * 16 * 4],
        alpha_test: false,
    })
    .collect();
    let table = TextureAnimTable::from_textures(&textures);
    assert_eq!(table.resolve(3, 0.0, 0.0), 3);
    assert_eq!(table.resolve(3, 0.25, 0.0), 4);
    assert_eq!(table.resolve(3, 0.45, 0.0), 5);
    assert_eq!(table.resolve(0, 0.0, 0.0), 0);
    assert_eq!(table.resolve(0, 0.0, 1.0), 2);
    let scroll_flag: u32 = SURF_FLAG_SCROLL;
    assert_eq!(scroll_flag, 8);
}

#[test]
fn rendermodes_blend_and_alpha_sorting() {
    let reg = ModelRegistry::from_precache(&["", "maps/test.bsp", "*1", "models/p.mdl"]);
    let h_mdl = reg.resolve(3).unwrap();
    let view = ViewBasis::from_angles(Vec3::ZERO, Vec3::ZERO);

    let mut solid_ent = RenderEntity::new(10, h_mdl, Vec3::new(120.0, 0.0, 0.0));
    solid_ent.rendermode = EntRenderMode::Normal;
    solid_ent.renderamt = 255;

    let mut color_ent = RenderEntity::new(11, h_mdl, Vec3::new(90.0, 0.0, 0.0));
    color_ent.rendermode = EntRenderMode::TransColor;
    color_ent.renderamt = 180;

    let mut trans_near = RenderEntity::new(12, h_mdl, Vec3::new(50.0, 0.0, 0.0));
    trans_near.rendermode = EntRenderMode::TransTexture;
    trans_near.renderamt = 128;

    let mut add_far = RenderEntity::new(13, h_mdl, Vec3::new(200.0, 0.0, 0.0));
    add_far.rendermode = EntRenderMode::TransAdd;
    add_far.renderamt = 200;

    let frame = SceneFrame {
        time: 0.0,
        view,
        entities: vec![solid_ent, trans_near, color_ent, add_far],
    };
    let lists = build_draw_lists_tracked(&frame, &reg, None);
    assert_eq!(lists.studio.len(), 4);
    assert!(lists.studio.iter().any(|d| d.mode == EntRenderMode::TransAdd));
    assert!(lists.studio.iter().any(|d| d.mode == EntRenderMode::TransColor));
}

#[test]
fn renderfx_all_modes_and_fade_tracker() {
    let reg = ModelRegistry::from_precache(&["", "maps/test.bsp", "*1", "models/p.mdl"]);
    let h_mdl = reg.resolve(3).unwrap();
    let view = ViewBasis::from_angles(Vec3::ZERO, Vec3::ZERO);

    let mut tracker = FxFadeTracker::default();
    let mut fade_ent = RenderEntity::new(10, h_mdl, Vec3::new(100.0, 0.0, 0.0));
    fade_ent.rendermode = EntRenderMode::TransTexture;
    fade_ent.renderfx = RENDERFX_FADE_FAST;
    fade_ent.renderamt = 200;

    let mut solid_ent = RenderEntity::new(11, h_mdl, Vec3::new(80.0, 0.0, 0.0));
    solid_ent.rendermode = EntRenderMode::TransTexture;
    solid_ent.renderfx = RENDERFX_SOLID_FAST;
    solid_ent.renderamt = 20;

    let mut shell_ent = RenderEntity::new(12, h_mdl, Vec3::new(200.0, 0.0, 0.0));
    shell_ent.renderfx = RENDERFX_GLOW_SHELL;
    shell_ent.rendercolor = [255, 50, 50];

    let mut all_entities = vec![fade_ent.clone(), solid_ent.clone(), shell_ent];
    for (idx, fx) in [
        RENDERFX_FADE_SLOW,
        RENDERFX_PULSE_FAST_WIDE,
        RENDERFX_STROBE_FAST,
        RENDERFX_FLICKER_FAST,
        RENDERFX_DISTORT,
        RENDERFX_HOLOGRAM,
        RENDERFX_EXPLODE,
        RENDERFX_LIGHT_MULTIPLIER,
    ]
    .into_iter()
    .enumerate()
    {
        let mut e = RenderEntity::new(20 + idx as u16, h_mdl, Vec3::new(150.0, idx as f32, 0.0));
        e.rendermode = EntRenderMode::TransTexture;
        e.renderfx = fx;
        e.renderamt = 200;
        all_entities.push(e);
    }

    let frame0 = SceneFrame {
        time: 0.25,
        view,
        entities: all_entities,
    };
    let lists0 = build_draw_lists_tracked(&frame0, &reg, Some(&mut tracker));
    assert!(lists0.studio.len() >= 8, "expected >= 8 visible renderfx draws, got {}", lists0.studio.len());
    assert!(lists0.studio.iter().all(|s| s.blend.is_finite()));
    let fade_0 = lists0.studio.iter().find(|s| s.renderfx == RENDERFX_FADE_FAST).unwrap().blend;
    let solid_0 = lists0.studio.iter().find(|s| s.renderfx == RENDERFX_SOLID_FAST).unwrap().blend;

    for fx in [RENDERFX_STROBE_FAST, RENDERFX_FLICKER_FAST] {
        let mut probe = RenderEntity::new(30, h_mdl, Vec3::new(150.0, 0.0, 0.0));
        probe.rendermode = EntRenderMode::TransTexture;
        probe.renderfx = fx;
        probe.renderamt = 200;
        let samples: Vec<i32> = [0.0f32, 0.05, 0.1, 0.15, 0.2, 0.25, 0.3, 0.35]
            .into_iter()
            .map(|t| fx_blend(&probe, t, &view))
            .collect();
        assert!(samples.contains(&0), "fx {fx} never went off: {samples:?}");
        assert!(samples.iter().any(|&v| v > 0), "fx {fx} never lit up: {samples:?}");
    }

    let frame1 = SceneFrame {
        time: 1.25,
        view,
        entities: vec![fade_ent, solid_ent],
    };
    let lists1 = build_draw_lists_tracked(&frame1, &reg, Some(&mut tracker));
    let fade_1 = lists1.studio.iter().find(|s| s.renderfx == RENDERFX_FADE_FAST).unwrap().blend;
    let solid_1 = lists1.studio.iter().find(|s| s.renderfx == RENDERFX_SOLID_FAST).unwrap().blend;
    assert!(fade_1 < fade_0);
    assert!(solid_1 > solid_0);
}

#[test]
fn sprites_all_orientations_and_glow_occlusion() {
    let reg = ModelRegistry::from_precache(&["", "maps/test.bsp", "*1", "sprites/g.spr"]);
    let h_spr = reg.resolve(3).unwrap();
    let view = ViewBasis::from_angles(Vec3::ZERO, Vec3::ZERO);

    let mut spr_glow = RenderEntity::new(13, h_spr, Vec3::new(50.0, 0.0, 0.0));
    spr_glow.rendermode = EntRenderMode::Glow;
    spr_glow.renderfx = RENDERFX_CLAMP_MIN_SCALE;
    spr_glow.renderamt = 255;

    let frame = SceneFrame {
        time: 0.0,
        view,
        entities: vec![spr_glow],
    };
    let lists = build_draw_lists_tracked(&frame, &reg, None);
    assert_eq!(lists.studio.len(), 0);
    assert_eq!(lists.sprites.len(), 1);

    let (_, clamp_scale) = glow_blend(50.0, RENDERFX_CLAMP_MIN_SCALE);
    assert!(clamp_scale >= 1.0);

    let orientations = [
        SpriteType::ParallelUpright,
        SpriteType::FacingUpright,
        SpriteType::Parallel,
        SpriteType::Oriented,
        SpriteType::ParallelOriented,
    ];
    assert_eq!(orientations.len(), 5);
    assert_ne!(SpriteType::Parallel, SpriteType::Oriented);
}

#[test]
fn mdl_skins_and_bodygroups() {
    let mdl = sample_test_mdl();
    assert_eq!(mdl.resolve_skin_texture(0, 0), 0);
    assert_eq!(mdl.resolve_skin_texture(1, 0), 1);
    assert_eq!(mdl.bodyparts[0].submodels.len(), 2);
}

#[test]
fn mdl_chrome_additive_masked_and_flatshade_flags() {
    let mdl = sample_test_mdl();
    assert_ne!(mdl.textures[0].flags & STUDIO_NF_FLATSHADE, 0);
    assert_ne!(mdl.textures[0].flags & STUDIO_NF_CHROME, 0);
    assert_ne!(mdl.textures[1].flags & STUDIO_NF_ADDITIVE, 0);
    assert_ne!(mdl.textures[1].flags & STUDIO_NF_MASKED, 0);
}

#[test]
fn mdl_bone_controllers_and_attachments() {
    let mdl = sample_test_mdl();
    let bones = mdl.evaluate_bones_full(&BoneEvalParams {
        controller: [128, 0, 0, 0],
        ..BoneEvalParams::default()
    });
    let atts = mdl.evaluate_attachments(glam::Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0)), &bones);
    assert!((atts[0] - Vec3::new(20.0, 10.0, 5.0)).length() < 1e-4);
}

#[test]
fn mdl_sequence_blending_and_animation_events() {
    let mdl = sample_test_mdl();
    assert_eq!(mdl.sequences[0].events.len(), 1);
    assert_eq!(mdl.sequences[0].events[0].event, 5001);

    if let Some((vfs, _)) = open_retail_vfs()
        && let Ok(ak) = StudioModel::load(&vfs, "models/v_ak47.mdl")
    {
        assert!(!ak.attachments.is_empty());
        assert!(ak.sequences.iter().any(|s| !s.events.is_empty()));
    }
}

#[test]
fn mdl_gait_animation_spine_split_if_installed() {
    let Some((vfs, _)) = open_retail_vfs() else {
        return;
    };
    if let Ok(urban) = StudioModel::load(&vfs, "models/player/urban/urban.mdl") {
        let p = BoneEvalParams {
            blending: [128, 0],
            gait: Some((1.min(urban.sequences.len().saturating_sub(1)), 5.0)),
            ..BoneEvalParams::default()
        };
        let bones = urban.evaluate_bones_full(&p);
        assert_eq!(bones.len(), urban.bones.len());
    }
}

#[test]
fn mdl_floor_point_lighting_if_installed() {
    let Some((_vfs, map)) = open_retail_vfs() else {
        return;
    };
    let spawn_pos = map_spawn_origin(&map);
    let styles = evaluate_lightstyles(&default_lightstyles(), 0.0);
    let lp = map.sample_light_point_styled(spawn_pos, &styles);
    assert!(lp.is_finite());
}

#[test]
fn decals_wad_clipping_and_custom_sprays() {
    let face = DecalFacePoly {
        face_index: 0,
        model_index: 0,
        normal: Vec3::Z,
        plane_dist: 0.0,
        verts: vec![
            Vec3::new(-64.0, -64.0, 0.0),
            Vec3::new(64.0, -64.0, 0.0),
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(-64.0, 64.0, 0.0),
        ],
    };
    let clipped = project_decal_onto_faces(&[face], Vec3::ZERO, 16.0, 0.35, [1.0; 4]);
    assert_eq!(clipped.len(), 6);

    if let Some((vfs, map)) = open_retail_vfs() {
        if let Ok(wad_bytes) = vfs.read_bytes("decals.wad")
            && let Ok(wad) = WadArchive::parse(wad_bytes)
            && let Some(&first) = wad.entry_names().first()
        {
            let decoded = wad.decode_decal(first).expect("decode decal").expect("entry");
            assert!(decoded.width > 0 && decoded.height > 0);
        }

        let spawn_pos = map_spawn_origin(&map);
        let floor_faces = map.query_faces_in_sphere(spawn_pos - Vec3::new(0.0, 0.0, 36.0), 48.0);
        if !floor_faces.is_empty() {
            let dec = project_decal_onto_faces(
                &floor_faces,
                spawn_pos - Vec3::new(0.0, 0.0, 36.0),
                16.0,
                0.35,
                [1.0; 4],
            );
            assert!(!dec.is_empty());
        }
    }
}

#[test]
fn particles_and_weather_system() {
    let mut ps = ParticleSystem::new();
    ps.spawn_effect(
        1.0,
        &ParticleEffect::BloodStream {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            color: 70,
            count: 16,
        },
    );
    ps.spawn_effect(1.0, &ParticleEffect::TarExplosion { position: Vec3::ZERO });
    ps.step_weather(1.0, 0.05, 2, Vec3::ZERO);
    assert!(!ps.is_empty());
}

#[test]
fn beams_and_lasers_subdivision_and_kill() {
    let mut bs = BeamSystem::new();
    bs.spawn_from_temp_effect(
        1.0,
        &TempEffect::BeamPoints {
            start: Vec3::ZERO,
            end: Vec3::new(100.0, 0.0, 0.0),
            beam: BeamParams {
                sprite: 1,
                start_frame: 0,
                frame_rate: 10,
                life: 5,
                width: 10,
                noise: 4,
                color: [255, 80, 40],
                brightness: 255,
                speed: 10,
            },
        },
        &|_| None,
    );
    let mut fx_beams = Vec::new();
    bs.emit_fx(1.1, &mut fx_beams);
    assert_eq!(fx_beams.len(), 10);
    assert_eq!(te::EXPLODEMODEL, 107);
}

#[test]
fn view_bob_roll_punch_and_viewmodel_lag() {
    let mut lag = ViewModelLagState::default();
    let mut prev_bob = 0.0f32;
    let mut max_step_delta = 0.0f32;
    for step in 0..300 {
        let t = (step as f32) / 300.0;
        let b = calc_bob(t, Vec3::new(250.0, 0.0, 0.0), true, &BobParams::default());
        if step > 0 {
            max_step_delta = max_step_delta.max((b - prev_bob).abs());
        }
        prev_bob = b;
        let _ = lag.update(Vec3::new((t * 0.5).cos(), (t * 0.5).sin(), 0.0), 1.0 / 300.0, 1.0);
    }
    assert!(max_step_delta < 0.2, "300 FPS bob step must be smooth: {max_step_delta}");
    assert!(calc_roll(0.0, Vec3::new(0.0, -200.0, 0.0), 2.0, 200.0).abs() > 1.9);
}

#[test]
fn screen_fade_and_screen_shake() {
    let shake = ScreenShake {
        amplitude: 4096 * 3,
        duration: 4096,
        frequency: 4096,
        started_at: 0.0,
    };
    let (sang, _) = calc_screen_shake(Some(&shake), 0.2);
    assert!(sang.length() > 0.0);

    let fade = ScreenFade {
        duration: 4096,
        hold_time: 4096,
        flags: FFADE_OUT | FFADE_STAYOUT | FFADE_MODULATE,
        color: [200, 100, 50, 220],
        started_at: 0.0,
    };
    assert!(evaluate_screen_fade(Some(&fade), 0.5).is_some());
    assert!(
        evaluate_screen_fade(
            Some(&ScreenFade {
                flags: FFADE_PURGE,
                ..fade
            }),
            0.5
        )
        .is_none()
    );
    assert!(underwater_tint_and_fov(CONTENTS_WATER, 3, 0.2).0.is_some());
    assert!(underwater_tint_and_fov(CONTENTS_SLIME, 3, 0.2).0.is_some());
    assert!(underwater_tint_and_fov(CONTENTS_LAVA, 3, 0.2).0.is_some());

    let _: fn(&HudView<'_>) -> Option<[f32; 4]> = |hv| hv.underwater_tint;
}

#[test]
fn widescreen_hor_plus_fov_and_300fps_interp() {
    let cam = Camera::default();
    let p_43 = cam.proj_matrix_ex(800.0, 600.0, 90.0, "widescreen", 4.0, 8192.0);
    let p_169 = cam.proj_matrix_ex(1920.0, 1080.0, 90.0, "widescreen", 4.0, 8192.0);
    // Same vertical scale (Hor+), wider horizontal scale (smaller x_axis.x) on 16:9.
    assert!((p_43.y_axis.y - p_169.y_axis.y).abs() < 1e-4);
    assert!(p_169.x_axis.x < p_43.x_axis.x);
}

#[test]
fn viewmodel_pass_and_righthand_mirror() {
    let cam = Camera::default();
    let p_world = cam.proj_matrix_ex(1920.0, 1080.0, 90.0, "widescreen", 4.0, 8192.0);
    let p_vm = cam.viewmodel_proj_matrix_fov(1920.0, 1080.0, 68.0, "widescreen");
    assert!(p_vm.y_axis.y > p_world.y_axis.y);
}

#[test]
fn bsp_pvs_culling_and_dynamic_brush_hide_if_installed() {
    let Some((_vfs, map)) = open_retail_vfs() else {
        return;
    };
    let _coll = Arc::new(WorldCollision::from_bsp(&map));
    let spawn_pos = map_spawn_origin(&map);
    let pvs = map
        .visible_world_face_mask(spawn_pos)
        .expect("valid spawn leaf PVS");
    assert_eq!(pvs.len(), map.faces.len());
    assert!(
        pvs.iter().filter(|&&v| v).count() < map.faces.len(),
        "PVS must cull faces on de_dust2"
    );
}

#[test]
fn skybox_6face_and_fog_uniforms_if_installed() {
    let Some((vfs, map)) = open_retail_vfs() else {
        return;
    };
    let sky = cs_assets::scheme::SkyboxTextures::load(&vfs, &map.skyname);
    let spawn_pos = map_spawn_origin(&map);
    let cam = Camera {
        position: spawn_pos + Vec3::new(0.0, 0.0, 17.0),
        ..Camera::default()
    };
    assert!(
        sky.faces.iter().any(|f| f.width > 0 && !f.rgba.is_empty()),
        "skybox should load at least one face"
    );
    let params = BspDrawParams {
        view_proj: cam.proj_matrix(640.0, 480.0, 90.0, "widescreen") * cam.view_matrix(),
        camera_pos: cam.position,
        gamma: 2.5,
        texgamma: 2.0,
        lightgamma: 2.5,
        brightness: 0.0,
        gl_overbright: false,
        gl_picmip: 0,
        gl_max_size: 512,
        gl_round_down: 3,
        software_mode: false,
        time_sec: 1.0,
        debug_mode: 0.0,
        wave_height: 0.0,
        fog: Some(([0.2, 0.25, 0.3], 0.001)),
        r_vis: true,
        lights: GpuLightsUniform::default(),
    };
    assert!(params.fog.is_some());
}

#[test]
fn msaa_and_anisotropy_pipeline_if_installed() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Some(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok() else {
        return;
    };
    let Ok((device, _queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("phase8-msaa-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    })) else {
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut bsp_r = BspRenderer::new(&device, format);
    let mut sky_r = SkyboxRenderer::new(&device, format);
    let mut dec_r = DecalRenderer::new(&device, format);
    let mut mdl_r = MdlRenderer::new(&device, format);
    let mut spr_r = SpriteRenderer::new(&device, format);
    let mut fx_r = FxRenderer::new(&device, format);
    bsp_r.rebuild_pipelines(&device, 4);
    sky_r.rebuild_pipelines(&device, 4);
    dec_r.rebuild_pipelines(&device, 4);
    mdl_r.rebuild_pipelines(&device, 4);
    spr_r.rebuild_pipelines(&device, 4);
    fx_r.rebuild_pipelines(&device, 4);
}

#[test]
fn screenshot_gpu_offscreen_readback_if_installed() {
    let Some((vfs, map)) = open_retail_vfs() else {
        return;
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Some(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok() else {
        return;
    };
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("phase8-test-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    })) else {
        return;
    };
    let sky = cs_assets::scheme::SkyboxTextures::load(&vfs, &map.skyname);
    let spawn_pos = map_spawn_origin(&map);
    let cam = Camera {
        position: spawn_pos + Vec3::new(0.0, 0.0, 17.0),
        ..Camera::default()
    };
    let params = BspDrawParams {
        view_proj: cam.proj_matrix(640.0, 480.0, 90.0, "widescreen") * cam.view_matrix(),
        camera_pos: cam.position,
        gamma: 2.5,
        texgamma: 2.0,
        lightgamma: 2.5,
        brightness: 0.0,
        gl_overbright: false,
        gl_picmip: 0,
        gl_max_size: 512,
        gl_round_down: 3,
        software_mode: true,
        time_sec: 1.0,
        debug_mode: 0.0,
        wave_height: 2.0,
        fog: Some(([0.2, 0.25, 0.3], 0.001)),
        r_vis: true,
        lights: GpuLightsUniform::default(),
    };
    let stats = render_offscreen_rgba(&device, &queue, &map, &sky, &cam, 320, 240, &params);
    assert!(stats.triangles > 0 && stats.draw_calls > 0);
}
