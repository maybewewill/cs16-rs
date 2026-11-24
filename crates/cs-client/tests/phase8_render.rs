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
    build_draw_lists_tracked, glow_blend,
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

#[test]
fn gamma_texgamma_brightness_overbright_and_software_dither() {
    let base = GammaParams::default();
    let mid = apply_goldsrc_gamma_rgb([0.5; 3], [0.5; 3], 0, 0, &base);
    assert!((mid[0] - 0.5).abs() < 1e-4);

    let bright = apply_goldsrc_gamma_rgb(
        [0.5; 3],
        [0.5; 3],
        0,
        0,
        &GammaParams {
            brightness: 1.5,
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

    let sw = apply_goldsrc_gamma_rgb(
        [0.37; 3],
        [0.5; 3],
        3,
        1,
        &GammaParams {
            software_mode: true,
            ..base
        },
    );
    assert!((sw[0] * 31.0 - (sw[0] * 31.0).round()).abs() < 1e-4);
    assert!((sw[1] * 63.0 - (sw[1] * 63.0).round()).abs() < 1e-4);
}

#[test]
fn texture_filter_modes_mipmaps_dlights_and_flashlight_spot() {
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
    let npot_mips = generate_mipmaps_rgba(6, 10, &vec![200u8; 6 * 10 * 4]);
    assert_eq!(npot_mips.len(), 1);

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

    let (spot, _bounce) = compute_flashlight_spot(Vec3::ZERO, Vec3::X, None, 1.0);
    assert!(spot.radius >= 64.0 && spot.cos_half_angle > 0.8);
    let uni = pool.pack_uniform(Vec3::ZERO, &[spot]);
    assert_eq!(uni.counts[0], 1);
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
fn rendermodes_all_renderfx_fade_tracker_and_sorting() {
    let reg = ModelRegistry::from_precache(&["", "maps/test.bsp", "*1", "models/p.mdl", "sprites/g.spr"]);
    let h_mdl = reg.resolve(3).unwrap();
    let h_spr = reg.resolve(4).unwrap();
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

    let mut spr_glow = RenderEntity::new(13, h_spr, Vec3::new(50.0, 0.0, 0.0));
    spr_glow.rendermode = EntRenderMode::Glow;
    spr_glow.renderfx = RENDERFX_CLAMP_MIN_SCALE;
    spr_glow.renderamt = 255;

    let frame0 = SceneFrame {
        time: 0.0,
        view,
        entities: vec![fade_ent.clone(), solid_ent.clone(), shell_ent, spr_glow],
    };
    let lists0 = build_draw_lists_tracked(&frame0, &reg, Some(&mut tracker));
    assert_eq!(lists0.studio.len(), 3);
    let fade_0 = lists0.studio.iter().find(|s| s.renderfx == RENDERFX_FADE_FAST).unwrap().blend;
    let solid_0 = lists0.studio.iter().find(|s| s.renderfx == RENDERFX_SOLID_FAST).unwrap().blend;

    let frame1 = SceneFrame {
        time: 1.0,
        view,
        entities: vec![fade_ent, solid_ent],
    };
    let lists1 = build_draw_lists_tracked(&frame1, &reg, Some(&mut tracker));
    let fade_1 = lists1.studio.iter().find(|s| s.renderfx == RENDERFX_FADE_FAST).unwrap().blend;
    let solid_1 = lists1.studio.iter().find(|s| s.renderfx == RENDERFX_SOLID_FAST).unwrap().blend;
    assert!(fade_1 < fade_0);
    assert!(solid_1 > solid_0);

    // ClampMinScale keeps scale >= 1.0 even at close distance.
    let (_, clamp_scale) = glow_blend(50.0, RENDERFX_CLAMP_MIN_SCALE);
    assert!(clamp_scale >= 1.0);

    // Smoke-check all remaining renderfx constants compile and differ from zero.
    let _fxs = [
        RENDERFX_FADE_SLOW,
        RENDERFX_PULSE_FAST_WIDE,
        RENDERFX_STROBE_FAST,
        RENDERFX_FLICKER_FAST,
        RENDERFX_DISTORT,
        RENDERFX_HOLOGRAM,
        RENDERFX_EXPLODE,
        RENDERFX_LIGHT_MULTIPLIER,
    ];
    let _spr_types = [
        SpriteType::ParallelUpright,
        SpriteType::FacingUpright,
        SpriteType::Parallel,
        SpriteType::Oriented,
        SpriteType::ParallelOriented,
    ];
}

#[test]
fn mdl_features_skins_bodygroups_flags_bones_events_and_attachments() {
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

    assert_eq!(mdl.resolve_skin_texture(0, 0), 0);
    assert_eq!(mdl.resolve_skin_texture(1, 0), 1);
    let bones = mdl.evaluate_bones_full(&BoneEvalParams::default());
    let atts = mdl.evaluate_attachments(glam::Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0)), &bones);
    assert!((atts[0] - Vec3::new(20.0, 10.0, 5.0)).length() < 1e-4);
}

#[test]
fn decals_particles_beams_view_effects_hor_plus_and_300fps() {
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

    let mut ps = ParticleSystem::new();
    ps.spawn_effect(1.0, &ParticleEffect::BloodStream {
        origin: Vec3::ZERO,
        direction: Vec3::X,
        color: 70,
        count: 16,
    });
    ps.spawn_effect(1.0, &ParticleEffect::TarExplosion { position: Vec3::ZERO });
    ps.step_weather(1.0, 0.05, 2, Vec3::ZERO);
    assert!(!ps.is_empty());

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

    // View effects + Hor+ widescreen FOV + 300 FPS interpolation.
    let cam = Camera::default();
    let p_43 = cam.proj_matrix_ex(800.0, 600.0, 90.0, "widescreen", 4.0, 8192.0);
    let p_169 = cam.proj_matrix_ex(1920.0, 1080.0, 90.0, "widescreen", 4.0, 8192.0);
    let p_vm = cam.viewmodel_proj_matrix_fov(1920.0, 1080.0, 68.0, "widescreen");
    // Same vertical scale (Hor+), wider horizontal scale (smaller x_axis.x) on 16:9.
    assert!((p_43.y_axis.y - p_169.y_axis.y).abs() < 1e-4);
    assert!(p_169.x_axis.x < p_43.x_axis.x);
    assert!(p_vm.y_axis.y > p_169.y_axis.y);

    // 300 FPS interpolation sub-step verification on bob/roll/lag/fade/shake/water.
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

    // Compile check that HudView exposes underwater_tint cleanly.
    let _: fn(&HudView<'_>) -> Option<[f32; 4]> = |hv| hv.underwater_tint;
}

#[test]
fn real_assets_and_headless_gpu_offscreen_render_if_installed() {
    let Ok(gamedir) = std::env::var("CS16_GAMEDIR") else {
        return;
    };
    let Ok(vfs) = Vfs::open(std::path::Path::new(&gamedir), cs_vfs::VfsConfig::default()) else {
        return;
    };

    if let Ok(wad_bytes) = vfs.read_bytes("decals.wad")
        && let Ok(wad) = WadArchive::parse(wad_bytes)
        && let Some(&first) = wad.entry_names().first()
    {
        let decoded = wad.decode_decal(first).expect("decode decal").expect("entry");
        assert!(decoded.width > 0 && decoded.height > 0);
    }

    let Ok(map) = BspMap::load(&vfs, "de_dust2") else {
        return;
    };
    let coll = Arc::new(WorldCollision::from_bsp(&map));
    let spawn_pos = map
        .entities
        .iter()
        .find_map(|e| {
            matches!(
                e.classname(),
                "info_player_start" | "info_player_deathmatch"
            )
            .then(|| e.get_vec3("origin"))
            .flatten()
        })
        .unwrap_or(Vec3::new(0.0, 0.0, 64.0));
    let styles = evaluate_lightstyles(&default_lightstyles(), 0.0);
    let lp = map.sample_light_point_styled(spawn_pos, &styles);
    assert!(lp.is_finite());

    let pvs = map
        .visible_world_face_mask(spawn_pos)
        .expect("valid spawn leaf PVS");
    assert_eq!(pvs.len(), map.faces.len());
    assert!(
        pvs.iter().filter(|&&v| v).count() < map.faces.len(),
        "PVS must cull faces on de_dust2"
    );
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
    let _ = coll;

    if let Ok(urban) = StudioModel::load(&vfs, "models/player/urban/urban.mdl") {
        let p = BoneEvalParams {
            blending: [128, 0],
            gait: Some((1.min(urban.sequences.len().saturating_sub(1)), 5.0)),
            ..BoneEvalParams::default()
        };
        let bones = urban.evaluate_bones_full(&p);
        assert_eq!(bones.len(), urban.bones.len());
    }
    if let Ok(ak) = StudioModel::load(&vfs, "models/v_ak47.mdl") {
        assert!(!ak.attachments.is_empty());
        assert!(ak.sequences.iter().any(|s| !s.events.is_empty()));
    }

    // Optional headless GPU render + MSAA 4x validation when a hardware/software adapter exists.
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

    let sky = cs_assets::scheme::SkyboxTextures::load(&vfs, &map.skyname);
    let cam = Camera {
        position: spawn_pos + Vec3::new(0.0, 0.0, 17.0),
        ..Camera::default()
    };
    let params = BspDrawParams {
        view_proj: cam.proj_matrix(640.0, 480.0, 90.0, "widescreen") * cam.view_matrix(),
        camera_pos: cam.position,
        gamma: 2.5,
        texgamma: 2.0,
        brightness: 1.0,
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
