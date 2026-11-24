//! 1-to-1 Counter-Strike 1.6 `TAB` Scoreboard matching `Resource/UI/ScoreBoard.res`
//! and `ClientScheme.res` (translucent rounded black frame, 1px amber border & dividers,
//! amber hostname & headers, CT `153 204 255` and T `255 64 64` rows).

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Vec2};

use crate::ui::vgui2::controls::{ListColumn, VguiListPanelState};
use crate::ui::vgui_assets::ClientPalette;

/// One row of the board, from `ScoreInfo`, `TeamInfo`, `ScoreAttrib` and the userinfo name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreRow {
    pub slot: u8,
    pub name: String,
    pub frags: i16,
    pub deaths: i16,
    /// `TERRORIST`, `CT`, `SPECTATOR`, `UNASSIGNED` (the `TeamInfo` text).
    pub team: String,
    pub dead: bool,
    pub bomb: bool,
    pub vip: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn show_scoreboard(
    ctx: &egui::Context,
    hostname: &str,
    _map_name: &str,
    ct_score: i16,
    t_score: i16,
    players: &[ScoreRow],
    local_slot: Option<u8>,
    cp: ClientPalette,
) {
    let screen = ctx.content_rect();
    let scale = (screen.height() / 480.0).min(screen.width() / 640.0) * 0.92;
    let w = (520.0 * scale).clamp(480.0, 820.0);
    let h = (340.0 * scale).clamp(320.0, 560.0);
    let rect = Rect::from_center_size(
        Pos2::new(screen.center().x, screen.center().y - 12.0 * scale),
        Vec2::new(w, h),
    );

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_vgui_scoreboard"),
    ));

    let border_col = Color32::from_rgba_unmultiplied(
        cp.border_bright.r(),
        cp.border_bright.g(),
        cp.border_bright.b(),
        180,
    );
    let bg_col = Color32::from_rgba_unmultiplied(0, 0, 0, 178);
    let r = (7.0 * scale).round().clamp(4.0, 14.0) as u8;

    // 1. Rounded translucent black scoreboard container (`ClientScheme.res` ListBG/WindowBG + amber border)
    painter.rect(
        rect,
        CornerRadius::same(r),
        bg_col,
        Stroke::new(1.0, border_col),
        egui::StrokeKind::Inside,
    );

    let pad_x = 14.0 * scale;
    let pad_top = 10.0 * scale;
    let font_title = FontId::proportional((11.5 * scale).clamp(12.5, 19.0));
    let font_hdr = FontId::proportional((9.5 * scale).clamp(11.0, 16.0));
    let font_row = FontId::proportional((9.8 * scale).clamp(11.0, 16.0));

    // Column anchor X positions (exact CS 1.6 proportions)
    let x_name = rect.left() + pad_x;
    let x_status = rect.right() - 210.0 * scale;
    let x_score = rect.right() - 120.0 * scale;
    let x_deaths = rect.right() - 68.0 * scale;
    let x_ping = rect.right() - pad_x;

    // 2. Top bar: Server hostname (left) + Score / Deaths / Latency column titles (right)
    let top_y = rect.top() + pad_top;
    painter.text(
        Pos2::new(x_name, top_y),
        egui::Align2::LEFT_TOP,
        hostname,
        font_title,
        cp.title_text,
    );
    let col_hdr_y = top_y + 2.0 * scale;
    painter.text(
        Pos2::new(x_score, col_hdr_y),
        egui::Align2::RIGHT_TOP,
        "Score",
        font_hdr.clone(),
        cp.base_text,
    );
    painter.text(
        Pos2::new(x_deaths, col_hdr_y),
        egui::Align2::RIGHT_TOP,
        "Deaths",
        font_hdr.clone(),
        cp.base_text,
    );
    painter.text(
        Pos2::new(x_ping, col_hdr_y),
        egui::Align2::RIGHT_TOP,
        "Latency",
        font_hdr.clone(),
        cp.base_text,
    );

    let mut y = top_y + 22.0 * scale;
    painter.line_segment(
        [Pos2::new(x_name, y), Pos2::new(x_ping, y)],
        Stroke::new(1.0, border_col),
    );
    y += 10.0 * scale;

    // 3. Render both teams (`Counter-Terrorists` + `Terrorists`) in exact `ScoreBoard.res` style
    let row_h = (14.5 * scale).clamp(16.0, 24.0);
    let teams = [
        ("Counter-Terrorists", "CT", ct_score, cp.team_ct),
        ("Terrorists", "TERRORIST", t_score, cp.team_t),
    ];

    for (team_idx, (title, team, team_rounds, col)) in teams.iter().enumerate() {
        let mut team_players: Vec<&ScoreRow> =
            players.iter().filter(|p| p.team == *team).collect();
        team_players.sort_by(|a, b| b.frags.cmp(&a.frags).then(a.deaths.cmp(&b.deaths)));

        // Team header row
        painter.text(
            Pos2::new(x_name, y),
            egui::Align2::LEFT_TOP,
            format!("{title}   -   {} players", team_players.len()),
            font_hdr.clone(),
            *col,
        );
        painter.text(
            Pos2::new(x_score, y),
            egui::Align2::RIGHT_TOP,
            team_rounds.to_string(),
            font_hdr.clone(),
            *col,
        );
        y += row_h + 1.0 * scale;

        // Crisp 1px divider under team banner
        painter.line_segment(
            [Pos2::new(x_name, y), Pos2::new(x_ping, y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 130)),
        );
        y += 4.0 * scale;

        for p in team_players {
            if y + row_h > rect.bottom() - 8.0 * scale {
                break;
            }
            // Subtle translucent white highlight band behind the local player's row
            if Some(p.slot) == local_slot {
                let hi_rect = Rect::from_min_size(
                    Pos2::new(x_name - 4.0, y - 1.0),
                    Vec2::new((x_ping - x_name) + 8.0, row_h),
                );
                painter.rect_filled(hi_rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 28));
            }

            let row_col = if !p.dead {
                *col
            } else {
                Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 155)
            };

            painter.text(
                Pos2::new(x_name + 4.0 * scale, y),
                egui::Align2::LEFT_TOP,
                &p.name,
                font_row.clone(),
                row_col,
            );

            let status = if p.dead {
                "Dead"
            } else if p.bomb {
                "Bomb"
            } else if p.vip {
                "VIP"
            } else {
                ""
            };
            if !status.is_empty() {
                painter.text(
                    Pos2::new(x_status, y),
                    egui::Align2::LEFT_TOP,
                    status,
                    font_row.clone(),
                    row_col,
                );
            }

            painter.text(
                Pos2::new(x_score, y),
                egui::Align2::RIGHT_TOP,
                p.frags.to_string(),
                font_row.clone(),
                row_col,
            );
            painter.text(
                Pos2::new(x_deaths, y),
                egui::Align2::RIGHT_TOP,
                p.deaths.to_string(),
                font_row.clone(),
                row_col,
            );
            painter.text(
                Pos2::new(x_ping, y),
                egui::Align2::RIGHT_TOP,
                "",
                font_row.clone(),
                row_col,
            );
            y += row_h;
        }

        if team_idx == 0 {
            y += 14.0 * scale;
        }
    }

    let specs: Vec<&ScoreRow> = players
        .iter()
        .filter(|p| p.team != "CT" && p.team != "TERRORIST")
        .collect();
    if !specs.is_empty() && y + row_h * 2.0 <= rect.bottom() {
        y += 8.0 * scale;
        let spec_col = Color32::from_rgb(204, 204, 204);
        painter.text(
            Pos2::new(x_name, y),
            egui::Align2::LEFT_TOP,
            format!("Spectators ({}): {}", specs.len(), specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")),
            font_row,
            spec_col,
        );
    }
}

/// Build a sortable VGUI2 `ListPanel` model matching `ScoreBoard.res` columns.
pub fn scoreboard_to_list_panel(players: &[ScoreRow], muted_mask: u32) -> VguiListPanelState {
    let mut state = VguiListPanelState {
        columns: vec![
            ListColumn { name: "Player".into(), width: 180 },
            ListColumn { name: "Team".into(), width: 90 },
            ListColumn { name: "Score".into(), width: 56 },
            ListColumn { name: "Deaths".into(), width: 56 },
            ListColumn { name: "Voice".into(), width: 64 },
        ],
        ..Default::default()
    };
    state.rows = players
        .iter()
        .map(|p| {
            let muted = p.slot < 32 && (muted_mask & (1u32 << p.slot)) != 0;
            vec![
                p.name.clone(),
                p.team.clone(),
                p.frags.to_string(),
                p.deaths.to_string(),
                if muted { "Muted".into() } else { "OK".into() },
            ]
        })
        .collect();
    state
}
