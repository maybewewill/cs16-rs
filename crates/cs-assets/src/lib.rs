//! GoldSrc binary & text asset parsers (`WAD3`, `BSP v30`, `MDL v10`, `SPR v2`, `materials.txt`,
//! `TrackerScheme.res`, `BackgroundLayout.txt`, `hud.txt`, Skybox TGA).

pub mod bsp;
pub mod materials;
pub mod mdl;
pub mod scheme;
pub mod spr;
pub mod wad;

#[cfg(test)]
mod tests {
    use cs_vfs::{Vfs, VfsConfig};

    use crate::bsp::{BspMap, BrushVolumeKind, SpawnTeam};
    use crate::materials::{MaterialTable, SurfaceMaterial};
    use crate::mdl::StudioModel;
    use crate::scheme::{SkyboxTextures, VguiColors, load_hud_txt, load_menu_background};
    use crate::spr::GoldSrcSprite;
    use crate::wad::WadArchive;

    fn local_vfs() -> Option<Vfs> {
        Vfs::discover(None, VfsConfig::default()).ok()
    }

    #[test]
    fn parses_real_wad_and_miptex_if_installed() {
        let Some(vfs) = local_vfs() else { return };
        let bytes = vfs.read_bytes("cs_dust.wad").expect("cs_dust.wad");
        let wad = WadArchive::parse(bytes).expect("parse WAD3");
        assert!(wad.len() > 20, "expected textures in cs_dust.wad");

        let hl_bytes = vfs.read_bytes("halflife.wad").expect("halflife.wad");
        let hl_wad = WadArchive::parse(hl_bytes).expect("parse halflife.wad");
        assert!(hl_wad.len() > 500);
    }

    #[test]
    fn parses_real_cs16_maps_if_installed() {
        let Some(vfs) = local_vfs() else { return };
        for map_name in ["de_dust2", "de_inferno", "de_nuke", "cs_assault"] {
            if !vfs.exists(&format!("maps/{map_name}.bsp")) {
                continue;
            }
            let map = BspMap::load(&vfs, map_name).expect("parse BSP v30");
            assert!(!map.render_vertices.is_empty(), "{map_name} has vertices");
            assert!(!map.render_indices.is_empty(), "{map_name} has indices");
            assert!(!map.batches.is_empty(), "{map_name} has render batches");
            assert!(
                map.lightmap_atlas.packed_faces > 50,
                "{map_name} packed lightmaps"
            );
            assert!(
                map.spawns
                    .iter()
                    .any(|s| s.team == SpawnTeam::CounterTerrorist),
                "{map_name} has CT spawns"
            );
            assert!(
                map.spawns.iter().any(|s| s.team == SpawnTeam::Terrorist),
                "{map_name} has T spawns"
            );
            if map_name.starts_with("de_") {
                assert!(
                    map.brush_entities
                        .iter()
                        .any(|b| b.kind == BrushVolumeKind::BombTarget),
                    "{map_name} has bomb target zones"
                );
            }
            let sky = SkyboxTextures::load(&vfs, &map.skyname);
            assert_eq!(sky.faces.len(), 6);
            assert!(sky.faces[0].width >= 64);
        }
    }

    #[test]
    fn parses_scheme_background_and_hud_if_installed() {
        let Some(vfs) = local_vfs() else { return };

        let colors = VguiColors::load(&vfs);
        assert_eq!(colors.control_bg, [76, 88, 68, 255]);
        assert_eq!(colors.window_bg, [62, 70, 55, 255]);
        assert_eq!(colors.bright_control_text, [196, 181, 80, 255]);

        let bg = load_menu_background(&vfs).expect("stitch menu background");
        assert_eq!(bg.width, 800);
        assert_eq!(bg.height, 600);
        assert_eq!(bg.rgba.len(), 800 * 600 * 4);

        let hud = load_hud_txt(&vfs).expect("parse hud.txt");
        assert!(
            hud.len() >= 180,
            "expected ~193 HUD sprite entries, got {}",
            hud.len()
        );
        assert!(hud.iter().any(|e| e.name == "number_0"));
        assert!(hud.iter().any(|e| e.name == "c4"));
    }

    #[test]
    fn parses_real_viewmodels_and_player_models_if_installed() {
        let Some(vfs) = local_vfs() else { return };

        for path in [
            "models/v_ak47.mdl",
            "models/v_m4a1.mdl",
            "models/v_awp.mdl",
            "models/v_deagle.mdl",
            "models/v_usp.mdl",
            "models/v_glock18.mdl",
            "models/v_knife.mdl",
            "models/v_c4.mdl",
            "models/player/urban/urban.mdl",
            "models/player/terror/terror.mdl",
            "models/player/sas/sas.mdl",
            "models/player/leet/leet.mdl",
        ] {
            let mdl = StudioModel::load(&vfs, path).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(!mdl.bones.is_empty(), "{path} has bones");
            assert!(!mdl.textures.is_empty(), "{path} has textures");
            assert!(!mdl.bodyparts.is_empty(), "{path} has bodyparts");
            assert!(!mdl.sequences.is_empty(), "{path} has sequences");
            let bones = mdl.evaluate_bones(0, 0.5);
            assert_eq!(bones.len(), mdl.bones.len());
            if path.contains("/player/") {
                assert!(!mdl.hitboxes.is_empty(), "{path} has hitboxes");
                assert!(
                    mdl.hitboxes.iter().any(|h| h.group == 1),
                    "{path} has head hitbox (group 1)"
                );
                let idle_idx = mdl.find_sequence("idle").unwrap_or(0);
                let (bmin, bmax) = mdl.skinned_bounds(idle_idx, 0.0);
                assert!(
                    bmin.z < -30.0 && bmax.z > 20.0,
                    "{path} standing bounds centered on hull origin: {bmin:?}..{bmax:?}"
                );
            }
        }
    }

    #[test]
    fn parses_real_sprites_and_materials_if_installed() {
        let Some(vfs) = local_vfs() else { return };

        for path in [
            "sprites/640hud2.spr",
            "sprites/640hud7.spr",
            "sprites/radar640.spr",
            "sprites/muzzleflash1.spr",
            "sprites/smokepuff.spr",
        ] {
            let spr = GoldSrcSprite::load(&vfs, path).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(!spr.frames.is_empty(), "{path} has frames");
            assert!(spr.frames[0].width > 0 && spr.frames[0].height > 0);
        }

        let mats = MaterialTable::load(&vfs);
        assert!(mats.len() > 50);
        assert_eq!(mats.lookup("A_Snow"), SurfaceMaterial::Snow);

        let client_colors = crate::scheme::ClientVguiColors::load(&vfs);
        assert_eq!(client_colors.base_text[0], 255);
        assert_eq!(client_colors.base_text[1], 176);

        let loc = crate::scheme::LocalizationTable::load(&vfs);
        assert!(loc.len() > 500, "cstrike_english.txt tokens loaded");
        assert!(loc.resolve("#Cstrike_AK47Price").contains("2500"));

        let ak_panel = crate::scheme::ClassResPanel::load(&vfs, "ak47", &loc).expect("ak47.res");
        assert_eq!(ak_panel.image_path.as_deref(), Some("gfx/vgui/ak47.tga"));
        assert!(ak_panel.labels.len() >= 10);

        let urban_panel = crate::scheme::ClassResPanel::load(&vfs, "urban", &loc).expect("urban.res");
        assert_eq!(urban_panel.image_path.as_deref(), Some("gfx/vgui/urban.tga"));

        let cs_logo = crate::scheme::load_tga_image(&vfs, "gfx/vgui/CS_logo.tga").expect("CS_logo.tga");
        assert_eq!((cs_logo.width, cs_logo.height), (64, 64));

        let dust2_brief = crate::scheme::load_map_briefing(&vfs, "de_dust2").expect("de_dust2.txt");
        assert!(dust2_brief.contains("Dust"));
    }

    #[test]
    fn parses_real_phase8_bsp_decal_and_mdl_features_if_installed() {
        let Some(vfs) = local_vfs() else { return };

        // 1. decals.wad index-alpha decoding
        if let Ok(bytes) = vfs.read_bytes("decals.wad") {
            let wad = WadArchive::parse(bytes).expect("parse decals.wad");
            let shot = wad.decode_decal("{shot1").expect("decode").expect("{shot1");
            assert_eq!((shot.width, shot.height), (16, 16));
            let blood = wad.decode_decal("{blood1").expect("decode").expect("{blood1");
            assert!(blood.alpha_test);
            assert!(blood.rgba.chunks_exact(4).any(|px| px[3] > 0 && px[0] > 0));
        }

        // 2. de_dust2 PVS mask, lightmap rects, world face spans, styled lightpoint, decal sphere query
        if vfs.exists("maps/de_dust2.bsp") {
            let mut map = BspMap::load(&vfs, "de_dust2").expect("de_dust2");
            let rects = map.build_face_lightmap_rects();
            assert_eq!(rects.len(), map.lightmap_atlas.packed_faces);
            let spawn = map.spawns[0].position + glam::Vec3::new(0.0, 0.0, 32.0);
            let mask = map.visible_world_face_mask(spawn).expect("PVS mask");
            let vis_count = mask.iter().filter(|&&v| v).count();
            assert!(vis_count > 10 && vis_count < map.faces.len());
            let (wv, wi, spans) = map.build_world_face_meshes();
            assert!(!wv.is_empty() && !wi.is_empty());
            assert!(spans.iter().flatten().count() > 100);

            let mut styles = [1.0f32; 64];
            let lit = map.sample_light_point_styled(spawn, &styles);
            assert!(lit.is_finite() && lit.min_element() >= 0.12);
            styles[0] = 0.0;
            let _ = map.build_texture_anim_table();
            let near = map.query_faces_in_sphere(map.spawns[0].position, 64.0);
            assert!(!near.is_empty(), "expected floor/wall faces near spawn");
            let mut prev = [1.0f32; 64];
            let dirty = map.lightmap_atlas.rebuild_for_styles(&map.lighting, &rects, &styles, &prev);
            assert!(!dirty.is_empty());
            prev = styles;
            assert!(map.lightmap_atlas.rebuild_for_styles(&map.lighting, &rects, &styles, &prev).is_empty());
        }

        // 3. urban.mdl 9-way blending, controllers, attachments, spine gait split; v_ak47.mdl events
        let urban = StudioModel::load(&vfs, "models/player/urban/urban.mdl").expect("urban.mdl");
        assert!(!urban.controllers.is_empty());
        assert!(!urban.attachments.is_empty());
        let aim_seq = urban.find_sequence("crouch_aim_carbine").expect("aim seq");
        assert!(urban.sequences[aim_seq].num_blends >= 9);
        let bones_lo = urban.evaluate_bones_full(&crate::mdl::BoneEvalParams {
            sequence: aim_seq,
            frame: 0.0,
            loop_anim: false,
            blending: [0, 0],
            controller: [127; 4],
            mouth: 0,
            prev_sequence: None,
            gait: Some((0, 0.0)),
        });
        let bones_hi = urban.evaluate_bones_full(&crate::mdl::BoneEvalParams {
            sequence: aim_seq,
            frame: 0.0,
            loop_anim: false,
            blending: [255, 0],
            controller: [127; 4],
            mouth: 0,
            prev_sequence: None,
            gait: Some((0, 0.0)),
        });
        assert_ne!(
            urban.evaluate_attachments(glam::Mat4::IDENTITY, &bones_lo)[0],
            urban.evaluate_attachments(glam::Mat4::IDENTITY, &bones_hi)[0]
        );

        let ak = StudioModel::load(&vfs, "models/v_ak47.mdl").expect("v_ak47.mdl");
        assert!(ak.sequences.iter().any(|s| !s.events.is_empty()));
    }
}
