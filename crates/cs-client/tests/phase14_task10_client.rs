//! Phase 14 Task 10 client tests: `cl_ljstats` & `cl_ljstats_min` HUD rendering and filtering.

use cs_client::ui::draw_ljstats;
use cs_cvar::CVarRegistry;
use cs_pm::ljstats::{JumpKind, JumpStats};

fn render_lj_hud(cvars: &CVarRegistry, stats: Option<&JumpStats>) -> usize {
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        },
        |ui| {
            draw_ljstats(ui.ctx(), cvars, stats);
        },
    );
    let shapes = out.shapes.len();
    out.textures_delta.clear();
    shapes
}

#[test]
fn cl_ljstats_hud_renders_and_respects_min_threshold() {
    let sample = JumpStats {
        kind: JumpKind::Lj,
        distance: 252.4,
        prestrafe: 274.2,
        max_speed: 312.8,
        strafes: 6,
        sync_pct: 92.0,
        height_diff: 0.0,
    };

    let mut cvars = CVarRegistry::new_with_defaults();
    assert_eq!(cvars.get_str("cl_ljstats"), "0");
    assert_eq!(cvars.get_str("cl_ljstats_min"), "215");
    assert_eq!(render_lj_hud(&cvars, Some(&sample)), 0);

    cvars.set("cl_ljstats", "1");
    assert!(render_lj_hud(&cvars, Some(&sample)) > 0);

    cvars.set("cl_ljstats_min", "260");
    assert_eq!(render_lj_hud(&cvars, Some(&sample)), 0);
}
