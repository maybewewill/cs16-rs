//! Phase 14 Task 11 client integration tests:
//! 1. `spec_xray 1` anti-cheat gate (zero draws when alive; team colors in spectator & `.dem`)
//! 2. `spec_hud_bars 1` CT & T player cards built and rendered from `HudState`

use std::collections::BTreeMap;

use cs_client::ui::screens::{
    SpectatorRenderCtx, SpectatorVguiUi, collect_spec_sidebar_players,
};
use cs_client::ui::vgui2::VguiRuntime;
use cs_client::view::{ViewBuilder, ViewInputs};
use cs_clientcore::cvars::ClientCvars;
use cs_clientcore::interp::{EmitResult, VisibleEntity};
use cs_clientcore::{
    ClientEffects, EntityKind, ModelKind as CoreModelKind, PrecacheEntry, PrecacheTables,
};
use cs_cvar::CVarRegistry;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState};
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

fn um(name: &str, decoded: CsUserMsg) -> SvcMessage {
    SvcMessage::UserMsg {
        id: 64,
        name: name.to_string(),
        decoded,
    }
}

fn make_player_ent(number: u16, origin: Vec3, modelindex: u16, weaponmodel: u16) -> VisibleEntity {
    let state = GoldSrcEntityState {
        number,
        origin,
        modelindex,
        weaponmodel,
        solid: 3,
        ..Default::default()
    };
    VisibleEntity {
        number,
        kind: EntityKind::Player,
        model: CoreModelKind::Studio,
        modelindex,
        origin,
        angles: Vec3::ZERO,
        frame: 0.0,
        sequence: 1,
        rendermode: 0,
        renderamt: 255,
        rendercolor: [0, 0, 0],
        renderfx: 0,
        local_player: false,
        studio_lerp: false,
        state,
        latched: Default::default(),
    }
}

fn test_precache() -> PrecacheTables {
    let mut p = PrecacheTables::default();
    for (idx, name) in [
        (1u16, "maps/de_dust2.bsp"),
        (2, "models/player/gign/gign.mdl"),
        (3, "models/p_m4a1.mdl"),
        (4, "models/p_ak47.mdl"),
    ] {
        p.models.insert(
            idx,
            PrecacheEntry {
                name: name.to_string(),
                download_size: 0,
                flags: 0,
            },
        );
    }
    p
}

#[test]
fn spec_xray_blocked_when_alive_and_emits_team_colors_in_spec_and_demo() {
    let precache = test_precache();
    let mut fx = ClientEffects::default();
    for (id, team, team_id, attrib) in [
        (1u8, "CT", 2i16, 0u8),         // local slot 0 (or spec)
        (2u8, "CT", 2i16, 0u8),         // CT teammate
        (3u8, "TERRORIST", 1i16, 0u8),  // T enemy
        (4u8, "TERRORIST", 1i16, 1u8),  // Dead T (ScoreAttrib dead bit)
    ] {
        fx.hud.handle(
            &um(
                "TeamInfo",
                CsUserMsg::TeamInfo {
                    player_id: id,
                    team_name: team.into(),
                },
            ),
            0.1,
        );
        fx.hud.handle(
            &um(
                "ScoreInfo",
                CsUserMsg::ScoreInfo {
                    player_id: id,
                    frags: 1,
                    deaths: 0,
                    class_id: 1,
                    team_id,
                },
            ),
            0.1,
        );
        fx.hud.handle(
            &um(
                "ScoreAttrib",
                CsUserMsg::ScoreAttrib {
                    player_id: id,
                    flags: attrib,
                },
            ),
            0.1,
        );
    }

    let userinfos = BTreeMap::new();
    let entities = vec![
        make_player_ent(2, Vec3::new(100.0, 0.0, 0.0), 2, 3),
        make_player_ent(3, Vec3::new(300.0, 0.0, 0.0), 2, 4),
        make_player_ent(4, Vec3::new(400.0, 0.0, 0.0), 2, 0),
    ];
    let emitted = || EmitResult {
        entities: entities.clone(),
        ..Default::default()
    };
    let mut cvars = ClientCvars::default();
    cvars.set("spec_xray", "1");
    let inputs = ViewInputs {
        now: 1.0,
        spec_xray: true,
        ..ViewInputs::default()
    };
    let mut vb = ViewBuilder::new();

    // 1. Live & alive player (`deadflag == 0`, `iuser1 == 0`) -> ZERO X-Ray draws!
    let live_cd = GoldSrcClientData {
        health: 100.0,
        deadflag: 0,
        iuser1: 0,
        ..Default::default()
    };
    let (frame_alive, _) = vb.build_parts_with_is_demo(
        &precache,
        &mut fx,
        &userinfos,
        Some(live_cd),
        Some(0),
        None,
        emitted(),
        Vec::new(),
        Vec3::ZERO,
        &inputs,
        Some(&cvars),
        false,
    );
    assert_eq!(
        frame_alive
            .scene
            .entities
            .iter()
            .filter(|e| e.xray_color.is_some())
            .count(),
        0,
        "spec_xray 1 must emit zero X-Ray draws while local player is alive"
    );

    // 2. Dead CT observer with `ForceCam(1, 0, 0)` -> CT teammate ONLY (no enemy T)!
    fx.hud.handle(
        &um(
            "ForceCam",
            CsUserMsg::ForceCam {
                forcecam: 1,
                forcechasecam: 0,
                fadetoblack: 0,
            },
        ),
        0.2,
    );
    let dead_cd = GoldSrcClientData {
        health: 0.0,
        deadflag: 2,
        iuser1: 2, // OBS_CHASE_FREE
        iuser2: 2,
        ..Default::default()
    };
    let (frame_team_only, _) = vb.build_parts_with_is_demo(
        &precache,
        &mut fx,
        &userinfos,
        Some(dead_cd.clone()),
        Some(0),
        None,
        emitted(),
        Vec::new(),
        Vec3::ZERO,
        &inputs,
        Some(&cvars),
        false,
    );
    let ct_ent = frame_team_only
        .scene
        .entities
        .iter()
        .find(|e| e.index == 2)
        .unwrap();
    let t_ent = frame_team_only
        .scene
        .entities
        .iter()
        .find(|e| e.index == 3)
        .unwrap();
    assert_eq!(ct_ent.xray_color, Some([0.25, 0.55, 1.0]));
    assert_eq!(t_ent.xray_color, None);

    // 3. Pure Spectator (`TeamInfo = SPECTATOR`) -> BOTH CT & T get team colors; dead T gets None!
    fx.hud.handle(
        &um(
            "TeamInfo",
            CsUserMsg::TeamInfo {
                player_id: 1,
                team_name: "SPECTATOR".into(),
            },
        ),
        0.3,
    );
    let (frame_spec, _) = vb.build_parts_with_is_demo(
        &precache,
        &mut fx,
        &userinfos,
        Some(dead_cd),
        Some(0),
        None,
        emitted(),
        Vec::new(),
        Vec3::ZERO,
        &inputs,
        Some(&cvars),
        false,
    );
    assert_eq!(
        frame_spec
            .scene
            .entities
            .iter()
            .find(|e| e.index == 2)
            .unwrap()
            .xray_color,
        Some([0.25, 0.55, 1.0])
    );
    assert_eq!(
        frame_spec
            .scene
            .entities
            .iter()
            .find(|e| e.index == 3)
            .unwrap()
            .xray_color,
        Some([1.0, 0.35, 0.20])
    );
    assert_eq!(
        frame_spec
            .scene
            .entities
            .iter()
            .find(|e| e.index == 4)
            .unwrap()
            .xray_color,
        None
    );

    // 4. Demo playback (`is_demo == true`) -> BOTH live teams get X-Ray even if POV client_data is alive!
    let demo_cd = GoldSrcClientData {
        health: 100.0,
        deadflag: 0,
        iuser1: 0,
        ..Default::default()
    };
    let (frame_demo, _) = vb.build_parts_with_is_demo(
        &precache,
        &mut fx,
        &userinfos,
        Some(demo_cd),
        Some(0),
        None,
        emitted(),
        Vec::new(),
        Vec3::ZERO,
        &inputs,
        None,
        true,
    );
    assert_eq!(
        frame_demo
            .scene
            .entities
            .iter()
            .find(|e| e.index == 2)
            .unwrap()
            .xray_color,
        Some([0.25, 0.55, 1.0])
    );
    assert_eq!(
        frame_demo
            .scene
            .entities
            .iter()
            .find(|e| e.index == 3)
            .unwrap()
            .xray_color,
        Some([1.0, 0.35, 0.20])
    );
    assert_eq!(
        frame_demo.player_weapons.get(&2).map(String::as_str),
        Some("m4a1")
    );
    assert_eq!(
        frame_demo.player_weapons.get(&3).map(String::as_str),
        Some("ak47")
    );
}

#[test]
fn spec_hud_bars_renders_ct_and_t_cards_from_hud_state() {
    let mut fx = ClientEffects::default();
    // CT player 1 (observed), T player 2 (has C4), dead CT player 3.
    for (id, team, team_id, frags, deaths, attrib, hp, money) in [
        (1u8, "CT", 2i16, 12i16, 4i16, 0u8, 84i32, 4750i32),
        (2u8, "TERRORIST", 1i16, 9i16, 6i16, 2u8, 52i32, 3100i32),
        (3u8, "CT", 2i16, 3i16, 8i16, 1u8, 0i32, 1400i32),
    ] {
        fx.hud.handle(
            &um(
                "TeamInfo",
                CsUserMsg::TeamInfo {
                    player_id: id,
                    team_name: team.into(),
                },
            ),
            0.1,
        );
        fx.hud.handle(
            &um(
                "ScoreInfo",
                CsUserMsg::ScoreInfo {
                    player_id: id,
                    frags,
                    deaths,
                    class_id: 1,
                    team_id,
                },
            ),
            0.1,
        );
        fx.hud.handle(
            &um(
                "ScoreAttrib",
                CsUserMsg::ScoreAttrib {
                    player_id: id,
                    flags: attrib,
                },
            ),
            0.1,
        );
        fx.hud.handle(
            &um(
                "HealthInfo",
                CsUserMsg::HealthInfo {
                    player_id: id,
                    health: hp,
                },
            ),
            0.1,
        );
        fx.hud.handle(
            &um(
                "Account",
                CsUserMsg::Account {
                    player_id: id,
                    amount: money,
                },
            ),
            0.1,
        );
    }
    fx.hud.handle(&um("Battery", CsUserMsg::Battery(100)), 0.1);
    fx.hud.handle(&um("ArmorType", CsUserMsg::ArmorType(1)), 0.1);
    fx.hud.handle(&um("ItemStatus", CsUserMsg::ItemStatus(2)), 0.1);

    let mut names = BTreeMap::new();
    names.insert(0, "AlphaCT".into());
    names.insert(1, "BravoT".into());
    names.insert(2, "CharlieCT".into());

    let mut weapons = BTreeMap::new();
    weapons.insert(1, "m4a1".into());
    weapons.insert(2, "ak47".into());

    let cards = collect_spec_sidebar_players(&fx.hud, &names, &weapons, 1);
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0].name, "AlphaCT");
    assert_eq!(cards[0].team, "CT");
    assert_eq!(cards[0].health, 84);
    assert_eq!(cards[0].money, Some(4750));
    assert!(cards[0].observed);
    assert!(cards[0].has_helmet);
    assert!(cards[0].has_defuser);
    assert_eq!(cards[0].weapon.as_deref(), Some("m4a1"));

    assert_eq!(cards[1].name, "CharlieCT");
    assert!(!cards[1].alive);
    assert_eq!(cards[1].health, 0);

    assert_eq!(cards[2].name, "BravoT");
    assert_eq!(cards[2].team, "TERRORIST");
    assert_eq!(cards[2].health, 52);
    assert_eq!(cards[2].money, Some(3100));
    assert!(cards[2].has_c4);
    assert_eq!(cards[2].weapon.as_deref(), Some("ak47"));

    // Render one egui frame with `spec_hud_bars 1`.
    let mut spec = SpectatorVguiUi::default();
    let mut rt = VguiRuntime::new();
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("spec_hud_bars", "1");
    let spec_players = vec![(1, "AlphaCT".into()), (2, "BravoT".into())];
    let ctx = egui::Context::default();
    let mut shapes = 0usize;
    for _ in 0..2 {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1280.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| {
                let _ = spec.show(
                    ui.ctx(),
                    &mut rt,
                    &mut cvars,
                    &SpectatorRenderCtx {
                        ct_score: 7,
                        t_score: 5,
                        round_timer_secs: 92,
                        players: &spec_players,
                        spec_health: Some((84, 1)),
                        sidebar_players: &cards,
                        ..Default::default()
                    },
                );
            },
        );
        out.textures_delta.clear();
        shapes = shapes.max(out.shapes.len());
    }
    assert!(shapes > 10, "expected CT/T sidebar cards to emit egui shapes, got {shapes}");
}
