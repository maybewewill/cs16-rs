//! 1-to-1 Counter-Strike 1.6 `TAB` Scoreboard driven by `Resource/UI/ScoreBoard.res`,
//! `ClientScheme.res`, localization tokens, and `VguiFontAtlas`.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::ui::vgui2::controls::{ListColumn, VguiListPanelState};
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
    pub short_headers: bool,
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

fn loc_header_or(loc: Option<&LocalizationTable>, tokens: &[&str], fallback: &str) -> String {
    for &tok in tokens {
        let resolved = loc_or(loc, tok, "");
        if !resolved.is_empty() {
            if resolved.is_ascii()
                && resolved.len() > 1
                && resolved.chars().all(|c| !c.is_ascii_lowercase())
            {
                let mut chars = resolved.chars();
                if let Some(first) = chars.next() {
                    return format!(
                        "{}{}",
                        first.to_ascii_uppercase(),
                        chars.as_str().to_ascii_lowercase()
                    );
                }
            }
            return resolved;
        }
    }
    fallback.to_string()
}

/// Centered `CClientScoreBoardDialog` (`520 x 340` at 640x480 scaled by `H / 480.0`).
/// Measured across 1280x720 `(250,105) 780x510`, 1280x800 `(207,117) 866x566`,
/// and 1280x1024 `(85,149) 1109x725`.
pub fn scoreboard_rect(screen: Rect) -> Rect {
    let s_h = (screen.height() / 480.0).max(0.5);
    let w = (520.0 * s_h).round().min((screen.width() - 16.0).max(200.0));
    let h = (340.0 * s_h).round().min((screen.height() - 16.0).max(160.0));
    let x = screen.left() + ((screen.width() - w) * 0.5).floor();
    let y = screen.top() + ((screen.height() - h) * 0.5).floor();
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
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
    let s_h = (screen.height() / 480.0).max(0.5);
    let rect = scoreboard_rect(screen);

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_vgui_scoreboard"),
    ));

    let scheme = &runtime.client_scheme;
    let border_col = scheme_color_or(
        scheme,
        "BorderBright",
        Color32::from_rgba_unmultiplied(188, 112, 0, 128),
    );
    let bg_col = scheme_color_or(
        scheme,
        "ListBG",
        Color32::from_rgba_unmultiplied(0, 0, 0, 128),
    );
    let title_col = scheme_color_or(scheme, "TitleText", s.fallback_palette.title_text);
    let base_col = scheme_color_or(scheme, "BaseText", s.fallback_palette.base_text);
    let ct_col = scheme_color_or(scheme, "team2", s.fallback_palette.team_ct);
    let t_col = scheme_color_or(scheme, "team1", s.fallback_palette.team_t);
    let spec_col = scheme_color_or(scheme, "team0", Color32::from_rgb(204, 204, 204));
    let client_fonts = scheme.fonts.clone();

    let corner = (8.0 * s_h).round().clamp(4.0, 16.0) as u8;
    painter.rect(
        rect,
        CornerRadius::same(corner),
        bg_col,
        Stroke::new(1.0, border_col),
        StrokeKind::Inside,
    );

    let line_x0 = rect.left() + 6.0;
    let line_x1 = rect.right() - 6.0;
    let x_name = rect.left() + (38.0 * s_h).round();
    let (x_score, x_deaths, x_ping) = if s.short_headers {
        (
            rect.right() - (88.0 * s_h).round(),
            rect.right() - (50.0 * s_h).round(),
            rect.right() - (11.5 * s_h).round(),
        )
    } else {
        (
            rect.right() - (107.0 * s_h).round(),
            rect.right() - (61.0 * s_h).round(),
            rect.right() - (11.5 * s_h).round(),
        )
    };
    let x_status = x_score - (76.0 * s_h).round();
    let top_y = rect.top() + (2.5 * s_h).round();
    let hdr_line_y = rect.top() + (13.2 * s_h).round();
    let row_h = (13.5 * s_h).round().max(14.0);
    let local_row_bg = Color32::from_rgba_unmultiplied(255, 255, 255, 18);

    let srv_rect = Rect::from_min_size(
        Pos2::new(rect.left() + (6.5 * s_h).round(), top_y),
        Vec2::new((x_score - rect.left() - 64.0).max(120.0), row_h),
    );
    runtime.font_atlas.paint_text_with_scale(
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
        TextAlignment::NorthWest,
        false,
        1.0,
    );

    let (score_hdr, deaths_hdr, latency_hdr) = if s.short_headers {
        ("S".to_string(), "D".to_string(), "Ping".to_string())
    } else {
        (
            loc_header_or(s.loc, &["#Cstrike_ScoreBoard_Score", "#Cstrike_TitlesTXT_SCORE"], "Score"),
            loc_header_or(s.loc, &["#Cstrike_ScoreBoard_Deaths", "#Cstrike_TitlesTXT_DEATHS"], "Deaths"),
            loc_header_or(s.loc, &["#Cstrike_ScoreBoard_Latency", "#Cstrike_TitlesTXT_LATENCY"], "Latency"),
        )
    };

    for (rx, hdr) in [
        (x_score, &score_hdr),
        (x_deaths, &deaths_hdr),
        (x_ping, &latency_hdr),
    ] {
        let hr = Rect::from_min_size(Pos2::new(rx - 80.0, top_y), Vec2::new(80.0, row_h));
        runtime.font_atlas.paint_text_with_scale(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "Default",
            screen_h,
            hr,
            hdr,
            base_col,
            TextAlignment::NorthEast,
            false,
            1.0,
        );
    }

    painter.rect_filled(
        Rect::from_min_size(Pos2::new(line_x0, hdr_line_y), Vec2::new(line_x1 - line_x0, 1.0)),
        0.0,
        border_col,
    );
    let mut y = hdr_line_y + (6.5 * s_h).round();

    let ct_name = loc_or(s.loc, "#Cstrike_ScoreBoard_CT", "Counter-Terrorists");
    let t_name = loc_or(s.loc, "#Cstrike_ScoreBoard_Ter", "Terrorists");
    let dead_lbl = loc_or(s.loc, "#Cstrike_DEAD", "Dead");
    let bomb_lbl = loc_or(s.loc, "#Cstrike_BOMB", "Bomb");
    let vip_lbl = loc_or(s.loc, "#Cstrike_VIP", "VIP");

    let teams = [
        (ct_name, "CT", s.ct_score, ct_col),
        (t_name, "TERRORIST", s.t_score, t_col),
    ];

    for (title, team, team_rounds, col) in teams {
        let mut team_players: Vec<&ScoreRow> = s.players.iter().filter(|p| p.team == team).collect();
        if team_players.is_empty() {
            continue;
        }
        team_players.sort_by(|a, b| b.frags.cmp(&a.frags).then(a.deaths.cmp(&b.deaths)));

        let header = format_team_count(s.loc, &title, team_players.len());
        let avg_ping = (team_players.iter().map(|p| u32::from(p.ping)).sum::<u32>()
            / team_players.len() as u32)
            .to_string();

        runtime.font_atlas.paint_text_with_scale(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "Default",
            screen_h,
            Rect::from_min_size(Pos2::new(x_name, y), Vec2::new((x_score - x_name - 16.0).max(120.0), row_h)),
            &header,
            col,
            TextAlignment::NorthWest,
            false,
            1.0,
        );
        runtime.font_atlas.paint_text_with_scale(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "Default",
            screen_h,
            Rect::from_min_size(Pos2::new(x_score - 56.0, y), Vec2::new(56.0, row_h)),
            &team_rounds.to_string(),
            base_col,
            TextAlignment::NorthEast,
            false,
            1.0,
        );
        runtime.font_atlas.paint_text_with_scale(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "Default",
            screen_h,
            Rect::from_min_size(Pos2::new(x_ping - 56.0, y), Vec2::new(56.0, row_h)),
            &avg_ping,
            base_col,
            TextAlignment::NorthEast,
            false,
            1.0,
        );

        let team_line_y = y + (9.7 * s_h).round();
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(line_x0, team_line_y), Vec2::new(line_x1 - line_x0, 1.0)),
            0.0,
            col,
        );
        y = team_line_y + (7.5 * s_h).round();

        for p in team_players {
            if y + row_h > rect.bottom() - 8.0 * s_h {
                break;
            }
            if s.local_slot == Some(p.slot) {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(line_x0, y), Vec2::new(line_x1 - line_x0, row_h)),
                    0.0,
                    local_row_bg,
                );
            }
            let row_col = if !p.dead {
                col
            } else {
                Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 155)
            };

            runtime.font_atlas.paint_text_with_scale(
                &painter,
                ctx,
                s.vfs,
                &client_fonts,
                "client",
                "Default",
                screen_h,
                Rect::from_min_size(Pos2::new(x_name, y), Vec2::new((x_status - x_name - 8.0).max(80.0), row_h)),
                &p.name,
                row_col,
                TextAlignment::NorthWest,
                false,
                1.0,
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
                runtime.font_atlas.paint_text_with_scale(
                    &painter,
                    ctx,
                    s.vfs,
                    &client_fonts,
                    "client",
                    "Default",
                    screen_h,
                    Rect::from_min_size(Pos2::new(x_status, y), Vec2::new(56.0, row_h)),
                    status,
                    row_col,
                    TextAlignment::NorthWest,
                    false,
                    1.0,
                );
            }

            for (rx, val) in [
                (x_score, p.frags.to_string()),
                (x_deaths, p.deaths.to_string()),
                (x_ping, p.ping.to_string()),
            ] {
                runtime.font_atlas.paint_text_with_scale(
                    &painter,
                    ctx,
                    s.vfs,
                    &client_fonts,
                    "client",
                    "Default",
                    screen_h,
                    Rect::from_min_size(Pos2::new(rx - 52.0, y), Vec2::new(52.0, row_h)),
                    &val,
                    row_col,
                    TextAlignment::NorthEast,
                    false,
                    1.0,
                );
            }
            y += row_h;
        }
        y += (10.0 * s_h).round();
    }

    let specs: Vec<&ScoreRow> = s
        .players
        .iter()
        .filter(|p| p.team != "CT" && p.team != "TERRORIST")
        .collect();
    if !specs.is_empty() && y + row_h * 2.0 <= rect.bottom() - 4.0 {
        let spec_hdr = loc_or(s.loc, "#Cstrike_TitlesTXT_Spectators", "Spectators");
        let spec_title = format_team_count(s.loc, &spec_hdr, specs.len());
        runtime.font_atlas.paint_text_with_scale(
            &painter,
            ctx,
            s.vfs,
            &client_fonts,
            "client",
            "Default",
            screen_h,
            Rect::from_min_size(Pos2::new(x_name, y), Vec2::new((line_x1 - x_name).max(120.0), row_h)),
            &spec_title,
            base_col,
            TextAlignment::NorthWest,
            false,
            1.0,
        );
        let spec_line_y = y + (9.7 * s_h).round();
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(line_x0, spec_line_y), Vec2::new(line_x1 - line_x0, 1.0)),
            0.0,
            border_col,
        );
        y = spec_line_y + (7.5 * s_h).round();
        for p in specs {
            if y + row_h > rect.bottom() - 4.0 {
                break;
            }
            if s.local_slot == Some(p.slot) {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(line_x0, y), Vec2::new(line_x1 - line_x0, row_h)),
                    0.0,
                    local_row_bg,
                );
            }
            runtime.font_atlas.paint_text_with_scale(
                &painter,
                ctx,
                s.vfs,
                &client_fonts,
                "client",
                "Default",
                screen_h,
                Rect::from_min_size(Pos2::new(x_name, y), Vec2::new((x_ping - x_name - 56.0).max(120.0), row_h)),
                &p.name,
                spec_col,
                TextAlignment::NorthWest,
                false,
                1.0,
            );
            runtime.font_atlas.paint_text_with_scale(
                &painter,
                ctx,
                s.vfs,
                &client_fonts,
                "client",
                "Default",
                screen_h,
                Rect::from_min_size(Pos2::new(x_ping - 52.0, y), Vec2::new(52.0, row_h)),
                &p.ping.to_string(),
                spec_col,
                TextAlignment::NorthEast,
                false,
                1.0,
            );
            y += row_h;
        }
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
    let s_h = (screen.height() / 480.0).max(0.5);
    let rect = scoreboard_rect(screen);

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_vgui_scoreboard"),
    ));

    let border_col = Color32::from_rgba_unmultiplied(
        cp.border_bright.r(),
        cp.border_bright.g(),
        cp.border_bright.b(),
        128,
    );
    let bg_col = Color32::from_rgba_unmultiplied(0, 0, 0, 128);
    let r = (8.0 * s_h).round().clamp(4.0, 16.0) as u8;

    painter.rect(
        rect,
        CornerRadius::same(r),
        bg_col,
        Stroke::new(1.0, border_col),
        StrokeKind::Inside,
    );

    let font_row = FontId::proportional((9.5 * s_h).clamp(11.0, 16.0));
    let line_x0 = rect.left() + 6.0;
    let line_x1 = rect.right() - 6.0;
    let x_name = rect.left() + (38.0 * s_h).round();
    let x_score = rect.right() - (107.0 * s_h).round();
    let x_deaths = rect.right() - (61.0 * s_h).round();
    let x_ping = rect.right() - (11.5 * s_h).round();
    let x_status = x_score - (76.0 * s_h).round();

    let top_y = rect.top() + (3.5 * s_h).round();
    painter.text(Pos2::new(rect.left() + (6.5 * s_h).round(), top_y), egui::Align2::LEFT_TOP, hostname, font_row.clone(), cp.title_text);
    painter.text(Pos2::new(x_score, top_y), egui::Align2::RIGHT_TOP, "Score", font_row.clone(), cp.base_text);
    painter.text(Pos2::new(x_deaths, top_y), egui::Align2::RIGHT_TOP, "Deaths", font_row.clone(), cp.base_text);
    painter.text(Pos2::new(x_ping, top_y), egui::Align2::RIGHT_TOP, "Latency", font_row.clone(), cp.base_text);

    let hdr_line_y = rect.top() + (13.2 * s_h).round();
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(line_x0, hdr_line_y), Vec2::new(line_x1 - line_x0, 1.0)),
        0.0,
        border_col,
    );
    let mut y = hdr_line_y + (6.5 * s_h).round();
    let local_row_bg = Color32::from_rgba_unmultiplied(255, 255, 255, 18);

    let row_h = (13.5 * s_h).round().max(14.0);
    let teams = [
        ("Counter-Terrorists", "CT", ct_score, cp.team_ct),
        ("Terrorists", "TERRORIST", t_score, cp.team_t),
    ];

    for (title, team, team_rounds, col) in teams {
        let mut team_players: Vec<&ScoreRow> = players.iter().filter(|p| p.team == team).collect();
        if team_players.is_empty() {
            continue;
        }
        team_players.sort_by(|a, b| b.frags.cmp(&a.frags).then(a.deaths.cmp(&b.deaths)));

        let noun = if team_players.len() == 1 { "player" } else { "players" };
        painter.text(
            Pos2::new(x_name, y),
            egui::Align2::LEFT_TOP,
            format!("{title}    -   {} {noun}", team_players.len()),
            font_row.clone(),
            col,
        );
        painter.text(Pos2::new(x_score, y), egui::Align2::RIGHT_TOP, team_rounds.to_string(), font_row.clone(), cp.base_text);
        let team_line_y = y + (9.7 * s_h).round();
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(line_x0, team_line_y), Vec2::new(line_x1 - line_x0, 1.0)),
            0.0,
            col,
        );
        y = team_line_y + (7.5 * s_h).round();

        for p in team_players {
            if y + row_h > rect.bottom() - 8.0 * s_h {
                break;
            }
            if local_slot == Some(p.slot) {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(line_x0, y), Vec2::new(line_x1 - line_x0, row_h)),
                    0.0,
                    local_row_bg,
                );
            }
            let row_col = if !p.dead {
                col
            } else {
                Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 155)
            };
            painter.text(Pos2::new(x_name, y), egui::Align2::LEFT_TOP, &p.name, font_row.clone(), row_col);
            let status = if p.dead { "Dead" } else if p.bomb { "Bomb" } else if p.vip { "VIP" } else { "" };
            if !status.is_empty() {
                painter.text(Pos2::new(x_status, y), egui::Align2::LEFT_TOP, status, font_row.clone(), row_col);
            }
            painter.text(Pos2::new(x_score, y), egui::Align2::RIGHT_TOP, p.frags.to_string(), font_row.clone(), row_col);
            painter.text(Pos2::new(x_deaths, y), egui::Align2::RIGHT_TOP, p.deaths.to_string(), font_row.clone(), row_col);
            painter.text(Pos2::new(x_ping, y), egui::Align2::RIGHT_TOP, p.ping.to_string(), font_row.clone(), row_col);
            y += row_h;
        }
        y += (10.0 * s_h).round();
    }

    let specs: Vec<&ScoreRow> = players.iter().filter(|p| p.team != "CT" && p.team != "TERRORIST").collect();
    if !specs.is_empty() && y + row_h * 2.0 <= rect.bottom() {
        let spec_col = Color32::from_rgb(204, 204, 204);
        let noun = if specs.len() == 1 { "player" } else { "players" };
        painter.text(
            Pos2::new(x_name, y),
            egui::Align2::LEFT_TOP,
            format!("Spectators    -   {} {noun}", specs.len()),
            font_row.clone(),
            cp.base_text,
        );
        let spec_line_y = y + (9.7 * s_h).round();
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(line_x0, spec_line_y), Vec2::new(line_x1 - line_x0, 1.0)),
            0.0,
            border_col,
        );
        y = spec_line_y + (7.5 * s_h).round();
        for p in specs {
            if y + row_h > rect.bottom() - 4.0 {
                break;
            }
            if local_slot == Some(p.slot) {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(line_x0, y), Vec2::new(line_x1 - line_x0, row_h)),
                    0.0,
                    local_row_bg,
                );
            }
            painter.text(Pos2::new(x_name, y), egui::Align2::LEFT_TOP, &p.name, font_row.clone(), spec_col);
            painter.text(Pos2::new(x_ping, y), egui::Align2::RIGHT_TOP, p.ping.to_string(), font_row.clone(), spec_col);
            y += row_h;
        }
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
