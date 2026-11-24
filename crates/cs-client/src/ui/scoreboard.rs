//! 1-to-1 Counter-Strike 1.6 `TAB` Scoreboard driven by `Resource/UI/ScoreBoard.res`,
//! `ClientScheme.res`, localization tokens, and `VguiFontAtlas`.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::{TextAlignment, VguiCoordMode};
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::ui::vgui2::controls::{ListColumn, VguiListPanelState};
use crate::ui::vgui2::font_atlas::VguiTextDrawOpts;
use crate::ui::vgui2::runtime::{VguiRuntime, scheme_color_or};
use crate::ui::vgui_assets::ClientPalette;

/// One row of the board, from `ScoreInfo`, `TeamInfo`, `ScoreAttrib` and the userinfo name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreRow {
    pub slot: u8,
    pub name: String,
    pub frags: i16,
    pub deaths: i16,
    pub ping: u16,
    /// `TERRORIST`, `CT`, `SPECTATOR`, `UNASSIGNED` (the `TeamInfo` text).
    pub team: String,
    pub dead: bool,
    pub bomb: bool,
    pub vip: bool,
}

pub struct ScoreboardRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub hostname: &'a str,
    pub map_name: &'a str,
    pub ct_score: i16,
    pub t_score: i16,
    pub players: &'a [ScoreRow],
    pub local_slot: Option<u8>,
    pub fallback_palette: ClientPalette,
}

fn loc_or(loc: Option<&LocalizationTable>, token: &str, fallback: &str) -> String {
    if let Some(l) = loc {
        let res = l.resolve(token);
        if !res.is_empty() && !res.starts_with('#') {
            return res.to_string();
        }
    }
    fallback.to_string()
}

fn format_team_count(loc: Option<&LocalizationTable>, title: &str, count: usize) -> String {
    let key = if count == 1 {
        "#Cstrike_ScoreBoard_Player"
    } else {
        "#Cstrike_ScoreBoard_Players"
    };
    if let Some(l) = loc {
        let tmpl = l.resolve(key);
        if !tmpl.is_empty() && !tmpl.starts_with('#') {
            return tmpl
                .replace("%s1", title)
                .replace("%s2", &count.to_string());
        }
    }
    let noun = if count == 1 { "player" } else { "players" };
    format!("{title}    -   {count} {noun}")
}

/// Draw the `.res`-driven CS 1.6 scoreboard using `Resource/UI/ScoreBoard.res`, `ClientScheme.res`,
/// `VguiFontAtlas`, and localized column/team/status labels.
pub fn show_scoreboard_vgui(
    ctx: &egui::Context,
    runtime: &mut VguiRuntime,
    s: &ScoreboardRenderCtx<'_>,
) {
    if let Some(v) = s.vfs {
        runtime.load_res(v, "resource/UI/ScoreBoard.res");
    }
    let screen = ctx.content_rect();
    let screen_h = screen.height().round().max(480.0) as u32;
    let vp = [screen.left(), screen.top(), screen.width(), screen.height()];
    let nodes = runtime.compute_layout_mode(
        "resource/UI/ScoreBoard.res",
        vp,
        VguiCoordMode::Proportional640,
        s.loc,
    );

    let sx = (screen.width() / 640.0).max(0.5);
    let sy = (screen.height() / 480.0).max(0.5);
    let scale = sx.min(sy);
    let rect = nodes
        .iter()
        .find(|n| n.field_name.eq_ignore_ascii_case("ClientScoreBoard"))
        .map_or_else(
            || {
                let w = (520.0 * scale).clamp(480.0, 900.0);
                let h = (340.0 * scale).clamp(320.0, 600.0);
                Rect::from_center_size(screen.center(), Vec2::new(w, h))
            },
            |n| {
                Rect::from_min_size(
                    Pos2::new(n.rect[0], n.rect[1]),
                    Vec2::new(n.rect[2].max(420.0), n.rect[3].max(280.0)),
                )
            },
        );

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_vgui_scoreboard"),
    ));

    let scheme = &runtime.client_scheme;
    let border_col = scheme_color_or(
        scheme,
        "BorderBright",
        Color32::from_rgba_unmultiplied(188, 112, 0, 180),
    );
    let bg_col = scheme_color_or(
        scheme,
        "ListBG",
        Color32::from_rgba_unmultiplied(0, 0, 0, 178),
    );
    let title_col = scheme_color_or(scheme, "TitleText", s.fallback_palette.title_text);
    let base_col = scheme_color_or(scheme, "BaseText", s.fallback_palette.base_text);
    let ct_col = scheme_color_or(scheme, "team2", s.fallback_palette.team_ct);
    let t_col = scheme_color_or(scheme, "team1", s.fallback_palette.team_t);
    let spec_col = scheme_color_or(scheme, "team0", Color32::from_rgb(204, 204, 204));
    let client_fonts = scheme.fonts.clone();

    let corner = (7.0 * scale).round().clamp(4.0, 14.0) as u8;
    painter.rect(
        rect,
        CornerRadius::same(corner),
        bg_col,
        Stroke::new(1.0, border_col),
        StrokeKind::Inside,
    );

    let pad_x = (14.0 * scale).round();
    let pad_top = (10.0 * scale).round();
    let row_h = ((13.0 * sy).max(16.0)).round();

    let x_name = rect.left() + pad_x;
    let x_status = rect.right() - (210.0 * scale).round();
    let x_score = rect.right() - (120.0 * scale).round();
    let x_deaths = rect.right() - (68.0 * scale).round();
    let x_ping = rect.right() - pad_x;
    let top_y = rect.top() + pad_top;

    let srv_rect = Rect::from_min_size(
        Pos2::new(x_name, top_y),
        Vec2::new((x_status - x_name - 8.0).max(120.0), row_h + 4.0),
    );
    runtime.font_atlas.paint_text(
        &painter,
        ctx,
        s.vfs,
        &client_fonts,
        "client",
        "Default",
        screen_h,
        srv_rect,
        s.hostname,
        title_col,
        VguiTextDrawOpts {
            align: TextAlignment::NorthWest,
            process_hotkey: false,
            proportional_scale: false,
        },
    );

    let score_hdr = loc_or(s.loc, "#Cstrike_TitlesTXT_SCORE", "SCORE");
    let deaths_hdr = loc_or(s.loc, "#Cstrike_TitlesTXT_DEATHS", "DEATHS");
    let latency_hdr = loc_or(s.loc, "#Cstrike_TitlesTXT_LATENCY", "LATENCY");

    for (rx, hdr) in [
        (x_score, &score_hdr),
        (x_deaths, &deaths_hdr),
        (x_ping, &latency_hdr),
    ] {
        let hr = Rect::from_min_size(Pos2::new(rx - 64.0, top_y), Vec2::new(64.0, row_h + 2.0));
        runtime.font_atlas.paint_text(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "DefaultSmall",
            screen_h,
            hr,
            hdr,
            base_col,
            VguiTextDrawOpts {
                align: TextAlignment::NorthEast,
                process_hotkey: false,
                proportional_scale: false,
            },
        );
    }

    let mut y = top_y + row_h + (4.0 * scale).round();
    painter.line_segment([Pos2::new(x_name, y), Pos2::new(x_ping, y)], Stroke::new(1.0, border_col));
    y += (8.0 * scale).round();

    let ct_name = loc_or(s.loc, "#Cstrike_ScoreBoard_CT", "Counter-Terrorists");
    let t_name = loc_or(s.loc, "#Cstrike_ScoreBoard_Ter", "Terrorists");
    let dead_lbl = loc_or(s.loc, "#Cstrike_DEAD", "Dead");
    let bomb_lbl = loc_or(s.loc, "#Cstrike_BOMB", "Bomb");
    let vip_lbl = loc_or(s.loc, "#Cstrike_VIP", "VIP");

    let teams = [
        (ct_name, "CT", s.ct_score, ct_col),
        (t_name, "TERRORIST", s.t_score, t_col),
    ];

    for (team_idx, (title, team, team_rounds, col)) in teams.iter().enumerate() {
        let mut team_players: Vec<&ScoreRow> = s.players.iter().filter(|p| p.team == *team).collect();
        team_players.sort_by(|a, b| b.frags.cmp(&a.frags).then(a.deaths.cmp(&b.deaths)));

        let header = format_team_count(s.loc, title, team_players.len());
        runtime.font_atlas.paint_text(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "Default",
            screen_h,
            Rect::from_min_size(Pos2::new(x_name, y), Vec2::new((x_score - x_name - 16.0).max(120.0), row_h)),
            &header,
            *col,
            VguiTextDrawOpts {
                align: TextAlignment::NorthWest,
                process_hotkey: false,
                proportional_scale: false,
            },
        );
        runtime.font_atlas.paint_text(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "Default",
            screen_h,
            Rect::from_min_size(Pos2::new(x_score - 56.0, y), Vec2::new(56.0, row_h)),
            &team_rounds.to_string(),
            *col,
            VguiTextDrawOpts {
                align: TextAlignment::NorthEast,
                process_hotkey: false,
                proportional_scale: false,
            },
        );
        y += row_h + 2.0;

        painter.line_segment(
            [Pos2::new(x_name, y), Pos2::new(x_ping, y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 130)),
        );
        y += 4.0;

        for p in team_players {
            if y + row_h > rect.bottom() - 8.0 * scale {
                break;
            }
            if Some(p.slot) == s.local_slot {
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

            runtime.font_atlas.paint_text(
                &painter,
                ctx,
                s.vfs,
                &client_fonts,
                "client",
                "DefaultSmall",
                screen_h,
                Rect::from_min_size(Pos2::new(x_name + 4.0, y), Vec2::new((x_status - x_name - 10.0).max(80.0), row_h)),
                &p.name,
                row_col,
                VguiTextDrawOpts {
                    align: TextAlignment::NorthWest,
                    process_hotkey: false,
                    proportional_scale: false,
                },
            );

            let status = if p.dead {
                dead_lbl.as_str()
            } else if p.bomb {
                bomb_lbl.as_str()
            } else if p.vip {
                vip_lbl.as_str()
            } else {
                ""
            };
            if !status.is_empty() {
                runtime.font_atlas.paint_text(
                    &painter,
                    ctx,
                    s.vfs,
                    &client_fonts,
                    "client",
                    "DefaultSmall",
                    screen_h,
                    Rect::from_min_size(Pos2::new(x_status, y), Vec2::new(56.0, row_h)),
                    status,
                    row_col,
                    VguiTextDrawOpts {
                        align: TextAlignment::NorthWest,
                        process_hotkey: false,
                        proportional_scale: false,
                    },
                );
            }

            for (rx, val) in [
                (x_score, p.frags.to_string()),
                (x_deaths, p.deaths.to_string()),
                (x_ping, p.ping.to_string()),
            ] {
                runtime.font_atlas.paint_text(
                    &painter,
                    ctx,
                    s.vfs,
                    &client_fonts,
                    "client",
                    "DefaultSmall",
                    screen_h,
                    Rect::from_min_size(Pos2::new(rx - 52.0, y), Vec2::new(52.0, row_h)),
                    &val,
                    row_col,
                    VguiTextDrawOpts {
                        align: TextAlignment::NorthEast,
                        process_hotkey: false,
                        proportional_scale: false,
                    },
                );
            }
            y += row_h;
        }

        if team_idx == 0 {
            y += (12.0 * scale).round();
        }
    }

    let specs: Vec<&ScoreRow> = s
        .players
        .iter()
        .filter(|p| p.team != "CT" && p.team != "TERRORIST")
        .collect();
    if !specs.is_empty() && y + row_h <= rect.bottom() - 4.0 {
        y += (6.0 * scale).round();
        let spec_hdr = loc_or(s.loc, "#Cstrike_TitlesTXT_Spectators", "Spectators");
        let spec_line = format!(
            "{spec_hdr} ({}): {}",
            specs.len(),
            specs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ")
        );
        runtime.font_atlas.paint_text(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "DefaultSmall",
            screen_h,
            Rect::from_min_size(Pos2::new(x_name, y), Vec2::new((x_ping - x_name).max(120.0), row_h)),
            &spec_line,
            spec_col,
            VguiTextDrawOpts {
                align: TextAlignment::NorthWest,
                process_hotkey: false,
                proportional_scale: false,
            },
        );
    }
}

/// Standalone scoreboard entry point (used by `cs-web` and headless tests without `VguiRuntime`).
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

    painter.rect(
        rect,
        CornerRadius::same(r),
        bg_col,
        Stroke::new(1.0, border_col),
        StrokeKind::Inside,
    );

    let pad_x = 14.0 * scale;
    let pad_top = 10.0 * scale;
    let font_title = FontId::proportional((11.5 * scale).clamp(12.5, 19.0));
    let font_hdr = FontId::proportional((9.5 * scale).clamp(11.0, 16.0));
    let font_row = FontId::proportional((9.8 * scale).clamp(11.0, 16.0));

    let x_name = rect.left() + pad_x;
    let x_status = rect.right() - 210.0 * scale;
    let x_score = rect.right() - 120.0 * scale;
    let x_deaths = rect.right() - 68.0 * scale;
    let x_ping = rect.right() - pad_x;

    let top_y = rect.top() + pad_top;
    painter.text(Pos2::new(x_name, top_y), egui::Align2::LEFT_TOP, hostname, font_title, cp.title_text);
    let col_hdr_y = top_y + 2.0 * scale;
    painter.text(Pos2::new(x_score, col_hdr_y), egui::Align2::RIGHT_TOP, "Score", font_hdr.clone(), cp.base_text);
    painter.text(Pos2::new(x_deaths, col_hdr_y), egui::Align2::RIGHT_TOP, "Deaths", font_hdr.clone(), cp.base_text);
    painter.text(Pos2::new(x_ping, col_hdr_y), egui::Align2::RIGHT_TOP, "Latency", font_hdr.clone(), cp.base_text);

    let mut y = top_y + 22.0 * scale;
    painter.line_segment([Pos2::new(x_name, y), Pos2::new(x_ping, y)], Stroke::new(1.0, border_col));
    y += 10.0 * scale;

    let row_h = (14.5 * scale).clamp(16.0, 24.0);
    let teams = [
        ("Counter-Terrorists", "CT", ct_score, cp.team_ct),
        ("Terrorists", "TERRORIST", t_score, cp.team_t),
    ];

    for (team_idx, (title, team, team_rounds, col)) in teams.iter().enumerate() {
        let mut team_players: Vec<&ScoreRow> = players.iter().filter(|p| p.team == *team).collect();
        team_players.sort_by(|a, b| b.frags.cmp(&a.frags).then(a.deaths.cmp(&b.deaths)));

        painter.text(
            Pos2::new(x_name, y),
            egui::Align2::LEFT_TOP,
            format!("{title}   -   {} players", team_players.len()),
            font_hdr.clone(),
            *col,
        );
        painter.text(Pos2::new(x_score, y), egui::Align2::RIGHT_TOP, team_rounds.to_string(), font_hdr.clone(), *col);
        y += row_h + 1.0 * scale;

        painter.line_segment(
            [Pos2::new(x_name, y), Pos2::new(x_ping, y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 130)),
        );
        y += 4.0 * scale;

        for p in team_players {
            if y + row_h > rect.bottom() - 8.0 * scale {
                break;
            }
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
            painter.text(Pos2::new(x_name + 4.0 * scale, y), egui::Align2::LEFT_TOP, &p.name, font_row.clone(), row_col);
            let status = if p.dead { "Dead" } else if p.bomb { "Bomb" } else if p.vip { "VIP" } else { "" };
            if !status.is_empty() {
                painter.text(Pos2::new(x_status, y), egui::Align2::LEFT_TOP, status, font_row.clone(), row_col);
            }
            painter.text(Pos2::new(x_score, y), egui::Align2::RIGHT_TOP, p.frags.to_string(), font_row.clone(), row_col);
            painter.text(Pos2::new(x_deaths, y), egui::Align2::RIGHT_TOP, p.deaths.to_string(), font_row.clone(), row_col);
            painter.text(Pos2::new(x_ping, y), egui::Align2::RIGHT_TOP, p.ping.to_string(), font_row.clone(), row_col);
            y += row_h;
        }
        if team_idx == 0 {
            y += 14.0 * scale;
        }
    }

    let specs: Vec<&ScoreRow> = players.iter().filter(|p| p.team != "CT" && p.team != "TERRORIST").collect();
    if !specs.is_empty() && y + row_h * 2.0 <= rect.bottom() {
        y += 8.0 * scale;
        let spec_col = Color32::from_rgb(204, 204, 204);
        painter.text(
            Pos2::new(x_name, y),
            egui::Align2::LEFT_TOP,
            format!(
                "Spectators ({}): {}",
                specs.len(),
                specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
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
