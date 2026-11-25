//! Headless UI tests for Phase 14 Task 6:
//! Authentic GoldSrc `net_graph 1/2/3`, `cl_showpos 1`, and `r_speeds 1`.

use cs_assets::scheme::VguiColors;
use cs_client::ui::vgui_theme::VguiPalette;
use cs_client::ui::{
    FrameTelemetry, draw_net_graph_with_telemetry, draw_r_speeds, draw_showpos,
};
use cs_clientcore::{NetGraphTelemetry, PacketSampleStatus};
use cs_cvar::CVarRegistry;
use egui::epaint::{ClippedShape, Shape};
use egui::{Color32, Pos2, Rect};

fn palette() -> VguiPalette {
    VguiPalette::from(VguiColors::default())
}

fn telemetry() -> FrameTelemetry<'static> {
    FrameTelemetry {
        fps: 99.5,
        frame_ms: 10.05,
        map_name: "de_dust2",
        pos: [128.25, -256.50, 64.00],
        yaw_deg: 135.5,
        pitch_deg: -12.25,
        speed: 250.0,
        draw_calls: 77,
        triangles: 24500,
    }
}

fn sample_net_graph_telemetry() -> NetGraphTelemetry {
    let mut ng = NetGraphTelemetry::new();
    // Outgoing commands across interpolation & extrapolation range.
    for seq in 1..=20u32 {
        let cmd_lerp = if seq % 5 == 0 { 1.4 } else { 0.45 };
        ng.record_outgoing(seq, 80 + (seq as usize) * 2, 1.0 + f64::from(seq) * 0.01, true, cmd_lerp, 50, 100.0);
    }
    // Incoming packets: valid, dropped gap (seq 5), choked (seq 8), invalid delta (seq 10).
    ng.record_incoming(1, 0, 1, 210, 1.040, 60.0);
    ng.record_incoming(2, 1, 2, 220, 1.055, 60.0);
    ng.record_incoming(3, 2, 3, 230, 1.070, 60.0);
    ng.record_incoming(4, 3, 4, 240, 1.085, 60.0);
    // Gap: seq 5 skipped -> Dropped
    ng.record_incoming(6, 4, 6, 250, 1.115, 60.0);
    ng.record_incoming(7, 6, 7, 260, 1.130, 60.0);
    ng.record_incoming(8, 7, 8, 270, 1.145, 60.0);
    ng.mark_choked(8);
    ng.record_incoming(9, 8, 9, 280, 1.160, 60.0);
    ng.record_incoming(10, 9, 10, 290, 1.175, 60.0);
    ng.mark_invalid_delta(10);
    ng.record_incoming(11, 10, 11, 312, 1.190, 60.0);
    assert_eq!(ng.incoming_sample(5).status, PacketSampleStatus::Dropped);
    ng
}

struct CapturedOutput {
    texts: Vec<(String, Pos2)>,
    rects: Vec<(Rect, Color32)>,
}

fn collect_shape(shape: &Shape, out: &mut CapturedOutput) {
    match shape {
        Shape::Text(ts) => {
            out.texts.push((ts.galley.text().to_string(), ts.pos));
        }
        Shape::Rect(rs) => {
            out.rects.push((rs.rect, rs.fill));
        }
        Shape::Vec(children) => {
            for child in children {
                collect_shape(child, out);
            }
        }
        _ => {}
    }
}

fn run_frame(mut draw: impl FnMut(&egui::Context)) -> CapturedOutput {
    let ctx = egui::Context::default();
    let mut final_out = CapturedOutput {
        texts: Vec::new(),
        rects: Vec::new(),
    };
    for _ in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1024.0, 768.0))),
            ..Default::default()
        };
        ctx.begin_pass(input);
        draw(&ctx);
        let mut full = ctx.end_pass();
        full.textures_delta.clear();
        final_out.texts.clear();
        final_out.rects.clear();
        for ClippedShape { shape, .. } in &full.shapes {
            collect_shape(shape, &mut final_out);
        }
    }
    final_out
}

#[test]
fn net_graph_3_renders_compact_four_row_text_only() {
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("net_graph", "3");
    let ng = sample_net_graph_telemetry();
    let t = telemetry();

    let out = run_frame(|ctx| {
        draw_net_graph_with_telemetry(ctx, &cvars, &t, Some(&ng), palette());
    });

    assert!(out.rects.is_empty(), "net_graph 3 must not draw graph bar rects");
    let joined: String = out.texts.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join("\n");
    assert!(joined.contains("fps") && joined.contains("ms"), "row 1 missing fps/ms: {joined}");
    assert!(joined.contains("in :") && joined.contains("k/s") && joined.contains("/s"), "row 2 missing in stats: {joined}");
    assert!(joined.contains("out:") && joined.contains("k/s") && joined.contains("/s"), "row 3 missing out stats: {joined}");
    assert!(joined.contains("loss:") && joined.contains("choke:"), "row 4 missing loss/choke: {joined}");
    assert!(joined.contains("312"), "expected last_in_bytes 312 in {joined}");
}

#[test]
fn net_graph_1_and_2_render_timelane_bars_above_four_text_rows() {
    let ng = sample_net_graph_telemetry();
    let t = telemetry();

    for mode in ["1", "2"] {
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("net_graph", mode);
        cvars.set("net_graphwidth", "192");
        cvars.set("net_graphheight", "64");
        cvars.set("net_graphsolid", "1");
        cvars.set("net_scale", "5");

        let out = run_frame(|ctx| {
            draw_net_graph_with_telemetry(ctx, &cvars, &t, Some(&ng), palette());
        });

        assert!(!out.rects.is_empty(), "net_graph {mode} must emit graph bar rects");

        // Verify red drop spike, yellow choke bar, and blue cl_cmdrate dot are all emitted.
        let has_red_drop = out.rects.iter().any(|(_, c)| *c == Color32::from_rgb(255, 0, 0));
        let has_yellow_choke = out.rects.iter().any(|(_, c)| *c == Color32::from_rgb(255, 255, 0));
        let has_blue_cmd = out.rects.iter().any(|(_, c)| *c == Color32::from_rgb(70, 150, 255));
        assert!(has_red_drop, "net_graph {mode} missing red drop spike");
        assert!(has_yellow_choke, "net_graph {mode} missing yellow choke bar");
        assert!(has_blue_cmd, "net_graph {mode} missing blue cl_cmdrate dot");

        // Verify the graph bars sit strictly above the 4 text rows.
        let text_top = out
            .texts
            .iter()
            .filter(|(s, _)| s.contains("fps") || s.contains("in :") || s.contains("out:") || s.contains("loss:"))
            .map(|(_, p)| p.y)
            .fold(f32::INFINITY, f32::min);
        let max_graph_bottom = out
            .rects
            .iter()
            .map(|(r, _)| r.bottom())
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max_graph_bottom <= text_top,
            "graph rects (bottom={max_graph_bottom}) must be above 4 text rows (top={text_top})"
        );
    }
}

#[test]
fn net_graphpos_aligns_right_center_and_left() {
    let ng = sample_net_graph_telemetry();
    let t = telemetry();

    let min_x_for_pos = |pos_val: &str| -> f32 {
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("net_graph", "1");
        cvars.set("net_graphpos", pos_val);
        cvars.set("net_graphwidth", "192");
        let out = run_frame(|ctx| {
            draw_net_graph_with_telemetry(ctx, &cvars, &t, Some(&ng), palette());
        });
        out.rects.iter().map(|(r, _)| r.left()).fold(f32::INFINITY, f32::min)
    };

    let x_right = min_x_for_pos("1");
    let x_center = min_x_for_pos("2");
    let x_left = min_x_for_pos("3");
    assert!(x_left < 20.0, "net_graphpos 3 should be left-aligned, got {x_left}");
    assert!((x_center - (1024.0 - 192.0) * 0.5).abs() < 2.0, "net_graphpos 2 should be centered, got {x_center}");
    assert!(x_right > 800.0, "net_graphpos 1 should be right-aligned, got {x_right}");
}

#[test]
fn cl_showpos_and_r_speeds_render_separate_telemetry() {
    let t = telemetry();

    let mut cv_pos = CVarRegistry::new_with_defaults();
    cv_pos.set("cl_showpos", "1");
    let out_pos = run_frame(|ctx| {
        draw_showpos(ctx, &cv_pos, &t);
        draw_r_speeds(ctx, &cv_pos, &t);
    });
    let joined_pos: String = out_pos.texts.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join("\n");
    assert!(joined_pos.contains("pos: 128.25 -256.50 64.00"), "missing pos: {joined_pos}");
    assert!(joined_pos.contains("ang: -12.25 135.50 0.00"), "missing ang: {joined_pos}");
    assert!(joined_pos.contains("vel: 250.00"), "missing vel: {joined_pos}");
    assert!(!joined_pos.contains("batches:"), "r_speeds should be off: {joined_pos}");

    let mut cv_rs = CVarRegistry::new_with_defaults();
    cv_rs.set("r_speeds", "1");
    let out_rs = run_frame(|ctx| {
        draw_showpos(ctx, &cv_rs, &t);
        draw_r_speeds(ctx, &cv_rs, &t);
    });
    let joined_rs: String = out_rs.texts.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join("\n");
    assert!(joined_rs.contains("batches: 77"), "missing batches: {joined_rs}");
    assert!(joined_rs.contains("tris: 24500"), "missing tris: {joined_rs}");
    assert!(!joined_rs.contains("pos:"), "cl_showpos should be off: {joined_rs}");
}

#[test]
fn net_graph_honors_width_height_solid_and_net_scale_cvars() {
    let ng = sample_net_graph_telemetry();
    let t = telemetry();

    let mut cv_solid = CVarRegistry::new_with_defaults();
    cv_solid.set("net_graph", "2");
    cv_solid.set("net_graphwidth", "128");
    cv_solid.set("net_graphheight", "96");
    cv_solid.set("net_graphsolid", "1");
    cv_solid.set("net_scale", "5");

    let out_solid = run_frame(|ctx| {
        draw_net_graph_with_telemetry(ctx, &cv_solid, &t, Some(&ng), palette());
    });

    cv_solid.set("net_graphsolid", "0");
    let out_hollow = run_frame(|ctx| {
        draw_net_graph_with_telemetry(ctx, &cv_solid, &t, Some(&ng), palette());
    });

    let max_bar_h_solid = out_solid
        .rects
        .iter()
        .map(|(r, _)| r.height())
        .fold(0.0f32, f32::max);
    let max_bar_h_hollow = out_hollow
        .rects
        .iter()
        .map(|(r, _)| r.height())
        .fold(0.0f32, f32::max);
    assert!(max_bar_h_solid > 4.0, "solid bars should have >4px height, got {max_bar_h_solid}");
    assert!(max_bar_h_hollow <= 2.5, "non-solid mode should emit <=2px dots, got {max_bar_h_hollow}");
}

