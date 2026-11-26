//! Clean-room procedural **Instant Demo** asset pack (`dm_rusthouse`) — 100% original bytes,
//! zero Valve/Steam files required, built in memory in `< 150 ms`, and strictly mounted as a
//! lowest-priority fallback layer (`demo/...`) so real Steam `cstrike`/`valve` assets are never
//! shadowed.

pub mod bsp_builder;
pub mod mdl_builder;
pub mod nav_builder;
pub mod spr_audio;
pub mod textures;

use cs_vfs::{Vfs, VfsConfig};

use self::bsp_builder::build_dm_rusthouse_bsp;
use self::mdl_builder::{
    CT_PALETTE, T_PALETTE, WeaponStyle, build_player_mdl, build_viewmodel_mdl,
    build_world_or_player_weapon_mdl,
};
use self::nav_builder::build_dm_rusthouse_nav;
use self::spr_audio::{build_demo_audio, build_demo_sprites};
use self::textures::{
    build_arena_miptex_list, build_demo_decals_wad, build_demo_wad, build_skybox_tgas,
};

/// Default arena map shipped by the clean-room Instant Demo pack.
pub const DEMO_MAP_NAME: &str = "dm_rusthouse";

/// Lowest-priority VFS layer prefix recognized by `Vfs::from_layered_memory` (rank 0 < `platform`/`valve`/`cstrike`).
pub const DEMO_LAYER_PREFIX: &str = "demo";

/// Complete clean-room in-memory asset bundle for 1-click Instant Demo playback.
#[derive(Debug, Clone)]
pub struct DemoPack {
    /// Normalized relative GoldSrc paths (e.g. `"maps/dm_rusthouse.bsp"`) and original bytes.
    pub files: Vec<(String, Vec<u8>)>,
}

impl DemoPack {
    /// Number of generated virtual files in the pack.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Total uncompressed bytes across all generated files.
    pub fn total_bytes(&self) -> usize {
        self.files.iter().map(|(_, b)| b.len()).sum()
    }

    /// Return entries prefixed with `"demo/"` for lowest-priority insertion into `Vfs::from_layered_memory`.
    pub fn layered_entries(&self) -> Vec<(String, Vec<u8>)> {
        self.files
            .iter()
            .map(|(p, b)| (format!("{DEMO_LAYER_PREFIX}/{p}"), b.clone()))
            .collect()
    }

    /// Build a standalone in-memory [`Vfs`] containing only the clean-room demo pack.
    pub fn to_vfs(&self) -> Vfs {
        Vfs::from_layered_memory(self.layered_entries(), VfsConfig::default())
    }

    /// Mount this clean-room pack strictly BELOW all existing layers of `vfs`, without shadowing
    /// any real Steam `cstrike`/`valve`/`platform` files.
    pub fn mount_as_fallback(&self, vfs: &mut Vfs) -> usize {
        vfs.mount_fallback_assets(self.layered_entries())
    }
}

/// Generate the complete clean-room `dm_rusthouse` Instant Demo asset pack in memory.
pub fn build_instant_demo_pack() -> DemoPack {
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(240);

    // 1. Procedural BSP v30 courtyard (`maps/dm_rusthouse.bsp`) + matching ZBot v5 `.nav`
    let bsp_bytes = build_dm_rusthouse_bsp();
    let nav_bytes = build_dm_rusthouse_nav(bsp_bytes.len() as u32);
    files.push((format!("maps/{DEMO_MAP_NAME}.bsp"), bsp_bytes));
    files.push((format!("maps/{DEMO_MAP_NAME}.nav"), nav_bytes));

    // 2. WAD3 archives (`demo.wad`, `decals.wad`) + 6-face procedural TGA skybox
    let miptex = build_arena_miptex_list();
    files.push(("demo.wad".to_string(), build_demo_wad(&miptex)));
    files.push(("decals.wad".to_string(), build_demo_decals_wad()));
    files.extend(build_skybox_tgas("rusthouse"));

    // 3. Articulated humanoid CT & T MDL v10 player models
    let ct_mdl = build_player_mdl("models/player/urban/urban.mdl", CT_PALETTE);
    let t_mdl = build_player_mdl("models/player/terror/terror.mdl", T_PALETTE);
    files.push(("models/player.mdl".to_string(), ct_mdl.clone()));
    for ct_name in ["urban", "gsg9", "sas", "gign", "spetsnaz", "vip"] {
        files.push((
            format!("models/player/{ct_name}/{ct_name}.mdl"),
            ct_mdl.clone(),
        ));
    }
    for t_name in ["terror", "leet", "arctic", "guerilla", "militia"] {
        files.push((
            format!("models/player/{t_name}/{t_name}.mdl"),
            t_mdl.clone(),
        ));
    }

    // 4. Clean-room weapon viewmodels (`v_*`), third-person (`p_*`), and worldmodels (`w_*`)
    let v_ak47 = build_viewmodel_mdl("models/v_ak47.mdl", WeaponStyle::Ak47);
    let v_m4a1 = build_viewmodel_mdl("models/v_m4a1.mdl", WeaponStyle::M4a1);
    let v_sniper = build_viewmodel_mdl("models/v_awp.mdl", WeaponStyle::Sniper);
    let v_smg = build_viewmodel_mdl("models/v_mp5.mdl", WeaponStyle::Smg);
    let v_shotgun = build_viewmodel_mdl("models/v_m3.mdl", WeaponStyle::Shotgun);
    let v_pistol = build_viewmodel_mdl("models/v_usp.mdl", WeaponStyle::Pistol);
    let v_knife = build_viewmodel_mdl("models/v_knife.mdl", WeaponStyle::Knife);
    let v_grenade = build_viewmodel_mdl("models/v_hegrenade.mdl", WeaponStyle::Grenade);
    let v_c4 = build_viewmodel_mdl("models/v_c4.mdl", WeaponStyle::C4);

    let p_rifle = build_world_or_player_weapon_mdl("models/p_ak47.mdl", WeaponStyle::Ak47, true);
    let p_sniper = build_world_or_player_weapon_mdl("models/p_awp.mdl", WeaponStyle::Sniper, true);
    let p_pistol = build_world_or_player_weapon_mdl("models/p_usp.mdl", WeaponStyle::Pistol, true);
    let p_knife = build_world_or_player_weapon_mdl("models/p_knife.mdl", WeaponStyle::Knife, true);
    let p_nade =
        build_world_or_player_weapon_mdl("models/p_hegrenade.mdl", WeaponStyle::Grenade, true);
    let p_c4 = build_world_or_player_weapon_mdl("models/p_c4.mdl", WeaponStyle::C4, true);

    let w_rifle = build_world_or_player_weapon_mdl("models/w_ak47.mdl", WeaponStyle::Ak47, false);
    let w_sniper = build_world_or_player_weapon_mdl("models/w_awp.mdl", WeaponStyle::Sniper, false);
    let w_pistol = build_world_or_player_weapon_mdl("models/w_usp.mdl", WeaponStyle::Pistol, false);
    let w_knife = build_world_or_player_weapon_mdl("models/w_knife.mdl", WeaponStyle::Knife, false);
    let w_nade =
        build_world_or_player_weapon_mdl("models/w_hegrenade.mdl", WeaponStyle::Grenade, false);
    let w_c4 = build_world_or_player_weapon_mdl("models/w_c4.mdl", WeaponStyle::C4, false);

    type WeaponBundle<'a> = (&'a str, &'a [u8], &'a [u8], &'a [u8]);
    let weapon_models: [WeaponBundle<'_>; 29] = [
        ("ak47", &v_ak47, &p_rifle, &w_rifle),
        ("galil", &v_ak47, &p_rifle, &w_rifle),
        ("sg552", &v_ak47, &p_rifle, &w_rifle),
        ("m249", &v_ak47, &p_rifle, &w_rifle),
        ("m4a1", &v_m4a1, &p_rifle, &w_rifle),
        ("famas", &v_m4a1, &p_rifle, &w_rifle),
        ("aug", &v_m4a1, &p_rifle, &w_rifle),
        ("awp", &v_sniper, &p_sniper, &w_sniper),
        ("scout", &v_sniper, &p_sniper, &w_sniper),
        ("g3sg1", &v_sniper, &p_sniper, &w_sniper),
        ("sg550", &v_sniper, &p_sniper, &w_sniper),
        ("mp5", &v_smg, &p_rifle, &w_rifle),
        ("ump45", &v_smg, &p_rifle, &w_rifle),
        ("p90", &v_smg, &p_rifle, &w_rifle),
        ("mac10", &v_smg, &p_rifle, &w_rifle),
        ("tmp", &v_smg, &p_rifle, &w_rifle),
        ("m3", &v_shotgun, &p_rifle, &w_rifle),
        ("xm1014", &v_shotgun, &p_rifle, &w_rifle),
        ("usp", &v_pistol, &p_pistol, &w_pistol),
        ("glock18", &v_pistol, &p_pistol, &w_pistol),
        ("deagle", &v_pistol, &p_pistol, &w_pistol),
        ("p228", &v_pistol, &p_pistol, &w_pistol),
        ("fiveseven", &v_pistol, &p_pistol, &w_pistol),
        ("elite", &v_pistol, &p_pistol, &w_pistol),
        ("knife", &v_knife, &p_knife, &w_knife),
        ("hegrenade", &v_grenade, &p_nade, &w_nade),
        ("flashbang", &v_grenade, &p_nade, &w_nade),
        ("smokegrenade", &v_grenade, &p_nade, &w_nade),
        ("c4", &v_c4, &p_c4, &w_c4),
    ];
    for (stem, v_bytes, p_bytes, w_bytes) in weapon_models {
        files.push((format!("models/v_{stem}.mdl"), v_bytes.to_vec()));
        files.push((format!("models/p_{stem}.mdl"), p_bytes.to_vec()));
        files.push((format!("models/w_{stem}.mdl"), w_bytes.to_vec()));
    }
    files.push(("models/w_weaponbox.mdl".to_string(), w_rifle.clone()));
    files.push(("models/w_backpack.mdl".to_string(), w_c4.clone()));
    files.push(("models/w_kevlar.mdl".to_string(), w_c4.clone()));
    files.push(("models/w_assault.mdl".to_string(), w_c4.clone()));
    files.push(("models/w_thighpack.mdl".to_string(), w_c4.clone()));
    for misc_w in [
        "models/w_battery.mdl",
        "models/w_antidote.mdl",
        "models/w_security.mdl",
        "models/w_longjump.mdl",
        "models/w_9mmclip.mdl",
        "models/w_shotbox.mdl",
        "models/w_shield.mdl",
        "models/p_shield.mdl",
        "models/hgibs.mdl",
        "models/agibs.mdl",
    ] {
        files.push((misc_w.to_string(), w_c4.clone()));
    }
    files.push(("models/grenade.mdl".to_string(), w_nade));
    for (stem, v_bytes, p_bytes) in [
        ("usp", &v_pistol, &p_pistol),
        ("fiveseven", &v_pistol, &p_pistol),
        ("deagle", &v_pistol, &p_pistol),
        ("p228", &v_pistol, &p_pistol),
        ("glock18", &v_pistol, &p_pistol),
        ("knife", &v_knife, &p_knife),
        ("flashbang", &v_grenade, &p_nade),
        ("hegrenade", &v_grenade, &p_nade),
        ("smokegrenade", &v_grenade, &p_nade),
    ] {
        files.push((
            format!("models/shield/v_shield_{stem}.mdl"),
            v_bytes.clone(),
        ));
        files.push((
            format!("models/shield/p_shield_{stem}.mdl"),
            p_bytes.clone(),
        ));
    }
    files.push(("models/pshell.mdl".to_string(), w_pistol.clone()));
    files.push(("models/rshell.mdl".to_string(), w_pistol.clone()));
    files.push(("models/rshell_big.mdl".to_string(), w_pistol.clone()));
    files.push(("models/shotgunshell.mdl".to_string(), w_pistol));

    // 5. SPR v2 HUD, radar, scope, muzzleflash, smoke & pain sprites + hud.txt
    files.extend(build_demo_sprites());

    // 6. Synthesized retro PCM `.wav` cues
    files.extend(build_demo_audio());

    // 7. Clean-room text/config files (materials, mapcycle, MOTD, map briefing, titles, schemes)
    files.push((
        "sound/materials.txt".to_string(),
        b"C RH_FLOOR\nC RH_SANDWALL\nC RH_SPAWNCT\nC RH_SPAWNT\nW RH_CRATE\nM RH_RUSTTRIM\nM RH_DECK\n"
            .to_vec(),
    ));
    files.push((
        "mapcycle.txt".to_string(),
        format!("{DEMO_MAP_NAME}\n").into_bytes(),
    ));
    files.push((
        "motd.txt".to_string(),
        b"cs16-rs Instant Demo - dm_rusthouse (Clean-Room Procedural Arena)\n\n\
          Press B in spawn to buy weapons, WASD to move, Left-Click to fire.\n\
          Mount your Steam Half-Life folder anytime for full retail Counter-Strike 1.6!\n"
            .to_vec(),
    ));
    files.push((
        format!("maps/{DEMO_MAP_NAME}.txt"),
        b"Rusthouse - Clean-Room Tactical Arena\n\n\
          Symmetric open-sky courtyard with four tactical cover crates,\n\
          flank boxes, and a central elevated steel walkway.\n"
            .to_vec(),
    ));
    files.push((
        "titles.txt".to_string(),
        b"$position -1 0.35\n$effect 0\n$color 255 180 30\n$fadein 0.01\n$fadeout 0.5\n$holdtime 2.5\n\n\
          Terrorists_Win\n{\nTerrorists Win!\n}\n\n\
          CTs_Win\n{\nCounter-Terrorists Win!\n}\n\n\
          Round_Draw\n{\nRound Draw!\n}\n\n\
          Game_Commencing\n{\nGame Commencing!\n}\n\n\
          Game_will_restart_in\n{\nThe game will restart in %s1 %s2\n}\n"
            .to_vec(),
    ));
    files.push((
        "resource/TrackerScheme.res".to_string(),
        b"Scheme\n{\nColors\n{\n\
          \"ControlBG\" \"76 88 68 255\"\n\
          \"ControlDarkBG\" \"90 106 80 255\"\n\
          \"WindowBG\" \"62 70 55 255\"\n\
          \"SelectionBG\" \"149 136 49 255\"\n\
          \"SelectionBG2\" \"40 46 34 255\"\n\
          \"BorderBright\" \"136 145 128 255\"\n\
          \"BorderDark\" \"40 46 34 255\"\n\
          \"BaseText\" \"216 222 211 255\"\n\
          \"BrightBaseText\" \"255 255 255 255\"\n\
          \"DimBaseText\" \"160 170 149 255\"\n\
          \"BrightControlText\" \"196 181 80 255\"\n\
          \"DisabledText1\" \"117 128 111 255\"\n\
          }\n}\n"
            .to_vec(),
    ));
    files.push((
        "resource/cstrike_english.txt".to_string(),
        br#""lang"
{
"Language" "English"
"Tokens"
{
"Game_connected" "%s1 connected"
"Game_disconnected" "%s1 disconnected"
"Game_join_ct" "%s1 is joining the Counter-Terrorist force"
"Game_join_terrorist" "%s1 is joining the Terrorist force"
"Game_join_ct_auto" "%s1 is joining the Counter-Terrorist force (auto)"
"Game_join_terrorist_auto" "%s1 is joining the Terrorist force (auto)"
"Game_scoring" "Scoring will not start until both teams have players"
"Game_Commencing" "Game Commencing!"
"Game_will_restart_in" "The game will restart in %s1 %s2"
"Terrorists_Win" "Terrorists Win!"
"CTs_Win" "Counter-Terrorists Win!"
"Round_Draw" "Round Draw!"
"Hint_press_buy_to_purchase" "Press the BUY key (B) to purchase weapons and equipment."
"Hint_you_have_the_bomb" "You have the C4 explosive."
"Not_Enough_Money" "You have insufficient funds!"
"Cannot_Carry_Anymore" "You cannot carry anymore!"
"Cant_buy" "%s1 seconds have passed. You can't buy anything now!"
"Got_defuser" "You picked up a defusal kit."
"Cstrike_Chat_All" "%s1 :  %s2"
"Cstrike_Chat_AllDead" "*DEAD* %s1 :  %s2"
"Cstrike_Chat_CT" "(Counter-Terrorist) %s1 :  %s2"
"Cstrike_Chat_T" "(Terrorist) %s1 :  %s2"
"Cstrike_Chat_Spec" "(Spectator) %s1 :  %s2"
}
}
"#
        .to_vec(),
    ));
    files.push((
        "resource/ClientScheme.res".to_string(),
        b"Scheme\n{\nColors\n{\n\
          \"BaseText\" \"255 176 0 255\"\n\
          \"TitleText\" \"255 174 0 255\"\n\
          \"WindowBG\" \"0 0 0 200\"\n\
          \"BorderBright\" \"188 112 0 128\"\n\
          \"Team0\" \"204 204 204 255\"\n\
          \"Team1\" \"255 64 64 255\"\n\
          \"Team2\" \"153 204 255 255\"\n\
          }\n}\n"
            .to_vec(),
    ));

    DemoPack { files }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::{BrushVolumeKind, BspMap, SpawnTeam};
    use crate::mdl::{StudioModel, StudioPoseData, studio_bounds};
    use crate::nav::{NAV_VERSION, NavFile};
    use crate::scheme::{SkyboxTextures, load_hud_txt};
    use crate::spr::GoldSrcSprite;
    use crate::wad::WadArchive;

    #[test]
    fn demo_pack_parses_all_goldsrc_formats_and_never_shadows_steam() {
        let t0 = std::time::Instant::now();
        let pack = build_instant_demo_pack();
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let bsp_bytes = pack
            .files
            .iter()
            .find(|(p, _)| p == "maps/dm_rusthouse.bsp")
            .map(|(_, b)| b.len())
            .unwrap_or(0);
        let nav_bytes = pack
            .files
            .iter()
            .find(|(p, _)| p == "maps/dm_rusthouse.nav")
            .map(|(_, b)| b.len())
            .unwrap_or(0);
        eprintln!(
            "demo_pack stats: files={}, total_bytes={} ({:.1} KiB), bsp={} B, nav={} B, build_ms={:.2}",
            pack.file_count(),
            pack.total_bytes(),
            pack.total_bytes() as f64 / 1024.0,
            bsp_bytes,
            nav_bytes,
            elapsed_ms
        );
        assert!(
            pack.file_count() > 120,
            "expected full clean-room pack, got {}",
            pack.file_count()
        );
        let max_ms = if cfg!(debug_assertions) {
            2000.0
        } else {
            150.0
        };
        assert!(
            elapsed_ms < max_ms,
            "pack generation exceeded {max_ms:.0} ms budget: {elapsed_ms:.2} ms"
        );

        let vfs = pack.to_vfs();
        assert!(vfs.list_maps().contains(&DEMO_MAP_NAME.to_string()));

        // 1. BSP v30 parses cleanly with embedded textures, lightmaps, spawns, buyzones, and vis
        let bsp = BspMap::load(&vfs, DEMO_MAP_NAME).expect("parse dm_rusthouse.bsp");
        assert!(!bsp.render_vertices.is_empty());
        assert!(!bsp.render_indices.is_empty());
        assert!(!bsp.batches.is_empty());
        assert!(
            bsp.lightmap_atlas.packed_faces >= 50,
            "baked lightmaps packed"
        );
        assert_eq!(
            bsp.spawns
                .iter()
                .filter(|s| s.team == SpawnTeam::CounterTerrorist)
                .count(),
            8
        );
        assert_eq!(
            bsp.spawns
                .iter()
                .filter(|s| s.team == SpawnTeam::Terrorist)
                .count(),
            8
        );
        assert_eq!(
            bsp.brush_entities
                .iter()
                .filter(|b| matches!(b.kind, BrushVolumeKind::BuyZone { .. }))
                .count(),
            2
        );
        assert_eq!(bsp.skyname, "rusthouse");
        let sky = SkyboxTextures::load(&vfs, &bsp.skyname);
        assert_eq!(sky.skyname, "rusthouse");
        assert_eq!(sky.faces[0].width, 128);

        // 2. WAD3 + decals.wad parse cleanly with accurate neutral RGB palette quantization
        let demo_wad = WadArchive::parse(vfs.read_bytes("demo.wad").unwrap()).unwrap();
        assert!(demo_wad.len() >= 7);
        let floor_tex = demo_wad.decode_texture("RH_FLOOR").unwrap().unwrap();
        let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
        for px in floor_tex.rgba.as_chunks::<4>().0 {
            sr += u64::from(px[0]);
            sg += u64::from(px[1]);
            sb += u64::from(px[2]);
        }
        let n = (floor_tex.width * floor_tex.height) as u64;
        let (mr, mg, mb) = (sr / n, sg / n, sb / n);
        assert!(
            mr.abs_diff(mg) < 16 && mr >= mb && mb > 110,
            "RH_FLOOR must decode to warm neutral concrete, got rgb=({mr},{mg},{mb})"
        );
        let decals_wad = WadArchive::parse(vfs.read_bytes("decals.wad").unwrap()).unwrap();
        let shot = decals_wad.decode_decal("{shot1").unwrap().unwrap();
        assert_eq!((shot.width, shot.height), (16, 16));
        let loc = crate::scheme::LocalizationTable::load(&vfs);
        assert!(loc.resolve("#Hint_press_buy_to_purchase").contains("BUY"));

        // 3. ZBot v5 .nav parses cleanly
        let nav = NavFile::load(&vfs, DEMO_MAP_NAME).expect("parse dm_rusthouse.nav");
        assert_eq!(nav.version, NAV_VERSION);
        assert!(nav.areas.len() > 200, "rich walkable nav grid");
        assert!(nav.areas.iter().any(|a| !a.hiding_spots.is_empty()));

        // 4. Articulated humanoid player models + weapon viewmodels parse with valid hitboxes & bounds
        for p in [
            "models/player.mdl",
            "models/player/urban/urban.mdl",
            "models/player/terror/terror.mdl",
            "models/v_ak47.mdl",
            "models/v_m4a1.mdl",
            "models/v_awp.mdl",
            "models/v_usp.mdl",
            "models/v_glock18.mdl",
            "models/v_deagle.mdl",
            "models/v_knife.mdl",
            "models/p_ak47.mdl",
            "models/w_ak47.mdl",
        ] {
            let raw = vfs.read_bytes(p).unwrap_or_else(|e| panic!("{p}: {e}"));
            let mdl = StudioModel::parse(&raw).unwrap_or_else(|e| panic!("{p}: {e}"));
            let pose = StudioPoseData::parse(&raw).unwrap_or_else(|e| panic!("{p} pose: {e}"));
            assert!(!mdl.bones.is_empty());
            assert!(!mdl.textures.is_empty());
            assert!(!mdl.bodyparts.is_empty());
            assert!(!mdl.sequences.is_empty());
            let bounds = studio_bounds(&raw).unwrap_or_else(|| panic!("{p} studio_bounds"));
            assert!(bounds.0[0] < bounds.1[0]);
            if p.contains("player") {
                assert!(mdl.hitboxes.iter().any(|h| h.group == 1), "{p} head hitbox");
                assert!(pose.hitboxes.len() >= 7);
                let (bmin, bmax) = mdl.skinned_bounds(0, 0.0);
                assert!(
                    bmin.z < -30.0 && bmax.z > 20.0,
                    "{p} standing bounds: {bmin:?}..{bmax:?}"
                );
            }
        }

        // 5. SPR v2 HUD/world sprites + hud.txt parse cleanly
        let hud_entries = load_hud_txt(&vfs).expect("parse demo hud.txt");
        assert!(hud_entries.iter().any(|e| e.name == "number_0"));
        assert!(hud_entries.iter().any(|e| e.name == "cross"));
        assert!(hud_entries.iter().any(|e| e.name == "buyzone"));
        for sp in [
            "sprites/640hud7.spr",
            "sprites/640hud1.spr",
            "sprites/radar640.spr",
            "sprites/sniper_scope.spr",
            "sprites/muzzleflash1.spr",
            "sprites/gas_puff_01.spr",
            "sprites/640_pain.spr",
        ] {
            let spr = GoldSrcSprite::load(&vfs, sp).unwrap_or_else(|e| panic!("{sp}: {e}"));
            assert!(!spr.frames.is_empty());
        }

        // 6. Demo layer MUST NEVER shadow real Steam `cstrike`/`valve` files!
        let sentinel = b"REAL_STEAM_ASSET_BYTES".to_vec();
        let mut combined = pack.layered_entries();
        combined.push(("cstrike/models/v_ak47.mdl".to_string(), sentinel.clone()));
        combined.push(("valve/sprites/hud.txt".to_string(), sentinel.clone()));
        let layered = Vfs::from_layered_memory(combined, VfsConfig::default());
        assert_eq!(layered.read_bytes("models/v_ak47.mdl").unwrap(), sentinel);
        assert_eq!(layered.read_bytes("sprites/hud.txt").unwrap(), sentinel);
        assert!(layered.exists("maps/dm_rusthouse.bsp"));

        // And `mount_as_fallback` on an existing VFS never overwrites existing entries!
        let mut existing = Vfs::from_layered_memory(
            vec![("cstrike/models/player.mdl".to_string(), sentinel.clone())],
            VfsConfig::default(),
        );
        let added = pack.mount_as_fallback(&mut existing);
        assert_eq!(added, pack.file_count() - 1);
        assert_eq!(existing.read_bytes("models/player.mdl").unwrap(), sentinel);
        assert!(existing.exists("maps/dm_rusthouse.bsp"));
    }
}
