//! Phase 14 Task 9 client QoL, crosshair extensions, and demo console controls tests.

use cs_client::ui::{
    CrosshairInputs, CrosshairState, KeyDisplayState, draw_crosshair_vgui, draw_showkeys,
    draw_showspeed,
};
use cs_clientcore::DemoPlayer;
use cs_cvar::{CVarRegistry, EngineAction};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::demo::{
    DemoDirectoryEntry, DemoFile, DemoFrame, DemoFrameData, DemoHeader, DemoNetMsgFrame,
    DemoNetMsgInfo, write_demo,
};
use cs_proto48::messages::write_svc_time;

fn demo_bytes_with_duration(secs: f32) -> Vec<u8> {
    let mut w = MsgWriter::new();
    write_svc_time(&mut w, 1.0);
    let msg = w.into_bytes();
    let mut frames = Vec::new();
    let steps = (secs * 10.0).round() as usize;
    for i in 0..=steps {
        let t = (i as f32) * 0.1;
        frames.push(DemoFrame {
            frame_type_raw: 1,
            time: t,
            frame: i as i32,
            data: DemoFrameData::NetMsg(Box::new(DemoNetMsgFrame {
                info: DemoNetMsgInfo::default(),
                incoming_sequence: 0,
                incoming_acknowledged: 0,
                incoming_reliable_acknowledged: 0,
                incoming_reliable_sequence: 0,
                outgoing_sequence: 0,
                reliable_sequence: 0,
                last_reliable_sequence: 0,
                msg: msg.clone(),
            })),
        });
    }
    let demo = DemoFile {
        header: DemoHeader {
            demo_protocol: 5,
            net_protocol: 48,
            map_name: "de_dust2".into(),
            game_dir: "cstrike".into(),
            map_crc: 0,
            directory_offset: 0,
        },
        directory: vec![DemoDirectoryEntry {
            entry_type: 1,
            description: "PLAYBACK".into(),
            flags: 0,
            cd_track: -1,
            track_time: secs,
            frame_count: frames.len() as i32,
            offset: 0,
            file_length: 0,
            frames,
        }],
    };
    write_demo(&demo).expect("encode demo")
}

#[test]
fn qol_hud_crosshair_variants_and_showspeed_showkeys_render() {
    let ctx = egui::Context::default();
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("cl_showspeed", "1");
    cvars.set("cl_showkeys", "1");
    cvars.set("cl_crosshair_dot", "1");
    cvars.set("cl_crosshair_outline", "1");
    cvars.set("cl_crosshair_t", "1");
    cvars.set("cl_crosshair_gap", "4");

    let mut state = CrosshairState::default();
    let inputs = CrosshairInputs {
        weapon_id: 28,
        on_ground: true,
        ..Default::default()
    };
    let mut out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        },
        |ui| {
            let ctx = ui.ctx();
            draw_crosshair_vgui(ctx, &cvars, &mut state, &inputs);
            draw_showspeed(ctx, &cvars, 276.4);
            draw_showkeys(
                ctx,
                &cvars,
                &KeyDisplayState {
                    forward: true,
                    jump: true,
                    ..Default::default()
                },
            );
        },
    );
    assert!(!out.shapes.is_empty());
    out.textures_delta.clear();
}

#[test]
fn demo_console_commands_drive_demo_player() {
    let mut reg = CVarRegistry::new_with_defaults();
    let actions = reg.execute_script(
        "demo_timescale 2.0; demo_pause; demo_resume; demo_togglepause; demo_goto 3.5",
    );
    assert_eq!(
        actions,
        vec![
            EngineAction::DemoTimescale(2.0),
            EngineAction::DemoPause,
            EngineAction::DemoResume,
            EngineAction::DemoTogglePause,
            EngineAction::DemoGoto(3.5),
        ]
    );

    let bytes = demo_bytes_with_duration(6.0);
    let mut player = DemoPlayer::from_bytes(&bytes).expect("valid demo");
    for act in actions {
        match act {
            EngineAction::DemoTimescale(s) => player.set_speed(s),
            EngineAction::DemoPause => player.set_paused(true),
            EngineAction::DemoResume => player.set_paused(false),
            EngineAction::DemoTogglePause => player.set_paused(!player.is_paused()),
            EngineAction::DemoGoto(t) => player.seek(t),
            _ => {}
        }
    }
    assert_eq!(player.speed(), 2.0);
    assert!(player.is_paused());
    assert!((player.current_time() - 3.5).abs() < 0.15);
}
