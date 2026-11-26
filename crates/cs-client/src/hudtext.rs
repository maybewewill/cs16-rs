//! Text the server sends for the HUD: `TextMsg` and `SayText` templates with `%s1`..`%s4`
//! slots and `#Token` names, and the MOTD's HTML.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_scripts::TitlesTable;

/// Built-in CS 1.6 / CZ English fallback templates used ONLY when a `#Token` is missing from
/// both `cstrike_<lang>.txt` and `titles.txt` (e.g. `#Game_radio_location`, radio phrases,
/// round/bomb announcements, or headless/demo mode without Steam localization packs).
fn fallback_token(token: &str) -> Option<&'static str> {
    let bare = token.trim_start_matches('#');
    let stripped = bare
        .strip_prefix("Cstrike_TitlesTXT_")
        .or_else(|| bare.strip_prefix("cstrike_titlestxt_"))
        .unwrap_or(bare);
    match stripped.to_ascii_lowercase().as_str() {
        "game_radio" => Some("\x02%s1 (RADIO): %s2"),
        "game_radio_location" => Some("\x02%s1 @ %s2 (RADIO): %s3"),
        "cover_me" => Some("Cover Me!"),
        "you_take_the_point" => Some("You Take the Point."),
        "hold_this_position" => Some("Hold This Position."),
        "regroup_team" => Some("Regroup Team."),
        "follow_me" => Some("Follow Me."),
        "taking_fire" => Some("Taking Fire...Need Assistance!"),
        "go_go_go" => Some("Go go go!"),
        "team_fall_back" => Some("Team, fall back!"),
        "stick_together_team" => Some("Stick together, team."),
        "get_in_position_and_wait" => Some("Get in position and wait for my go."),
        "storm_the_front" => Some("Storm the Front!"),
        "report_in_team" => Some("Report in, team."),
        "affirmative" => Some("Affirmative."),
        "roger_that" => Some("Roger that."),
        "enemy_spotted" => Some("Enemy spotted."),
        "need_backup" => Some("Need backup."),
        "sector_clear" => Some("Sector clear."),
        "in_position" => Some("I'm in position."),
        "reporting_in" => Some("Reporting in."),
        "get_out_of_there" => Some("Get out of there, it's gonna blow!"),
        "negative" => Some("Negative."),
        "enemy_down" => Some("Enemy down."),
        "fire_in_the_hole" => Some("Fire in the hole!"),
        "hostage_down" => Some("Hostage down."),
        "bomb_planted" => Some("The bomb has been planted!"),
        "bomb_defused" => Some("The bomb has been defused!"),
        "cts_win" => Some("Counter-Terrorists Win!"),
        "terrorists_win" => Some("Terrorists Win!"),
        "round_draw" => Some("Round Draw!"),
        "target_bombed" => Some("Target Successfully Bombed!"),
        "target_saved" => Some("Target has been saved!"),
        "game_commencing" => Some("Game Commencing!"),
        "switch_to_burstfire" => Some("Switched to burst-fire mode"),
        "switch_to_semiauto" => Some("Switched to semi-automatic"),
        "switch_to_fullauto" => Some("Switched to automatic"),
        "not_enough_money" => Some("You have insufficient funds!"),
        "cannot_buy_this" => Some("You cannot buy this item!"),
        "cant_buy" => Some("%s1 seconds have passed.\nYou can't buy anything now!"),
        "c4_plant_at_bomb_spot" => Some("C4 must be planted at a bomb site!"),
        "defusing_bomb_with_defuse_kit" => Some("Defusing bomb WITH Defuse kit."),
        "defusing_bomb_without_defuse_kit" => Some("Defusing bomb WITHOUT Defuse kit."),
        _ => None,
    }
}

/// Resolve `#Token` (including trailing whitespace/newlines) via `LocalizationTable`, `TitlesTable`,
/// and CS 1.6/CZ built-in fallback templates.
fn resolve_token<'a>(
    raw: &'a str,
    loc: Option<&'a LocalizationTable>,
    titles: Option<&'a TitlesTable>,
) -> &'a str {
    let trimmed = raw.trim_end_matches(['\r', '\n', ' ', '\t']);
    if !trimmed.starts_with('#') {
        return raw;
    }
    if let Some(l) = loc {
        let resolved = l.resolve_with_titles(trimmed, titles);
        if resolved != trimmed {
            return resolved;
        }
    }
    if let Some(t) = titles
        && let Some(entry) = t.get(trimmed.trim_start_matches('#'))
    {
        let text = entry.text.as_str();
        if text.starts_with('#')
            && let Some(l) = loc
        {
            let resolved = l.resolve_with_titles(text, titles);
            if resolved != text {
                return resolved;
            }
        }
        if !text.starts_with('#') {
            return text;
        }
        if let Some(fb) = fallback_token(text) {
            return fb;
        }
        return text;
    }
    if let Some(fb) = fallback_token(trimmed) {
        return fb;
    }
    raw
}

fn resolve_inline_tokens(
    text: &str,
    loc: Option<&LocalizationTable>,
    titles: Option<&TitlesTable>,
) -> String {
    if !text.contains('#') {
        return text.to_string();
    }
    let stage1 = if let Some(l) = loc {
        l.resolve_inline(text, titles)
    } else if let Some(t) = titles {
        LocalizationTable::default().resolve_inline(text, Some(t))
    } else {
        text.to_string()
    };
    if !stage1.contains('#') {
        return stage1;
    }
    let mut out = String::with_capacity(stage1.len());
    let bytes = stage1.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let tok = &stage1[start..i];
            if let Some(fb) = fallback_token(tok) {
                out.push_str(fb);
            } else {
                out.push_str(tok);
            }
        } else {
            let ch = stage1[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Fills a message `text` with its `params`: a `#Token` is looked up first, `%s1`..`%s4` take
/// the parameters in order, a bare `%s` takes the next one. Control bytes (`\x01` to `\x04`,
/// the colour codes of chat) are dropped.
pub fn format_message(text: &str, params: &[String], loc: Option<&LocalizationTable>) -> String {
    format_message_with_titles(text, params, loc, None)
}

/// Fills a HUD message using both `LocalizationTable` (`cstrike_<lang>.txt`) and `TitlesTable` (`titles.txt`),
/// stripping `\x01`..`\x04` chat color control bytes.
pub fn format_message_with_titles(
    text: &str,
    params: &[String],
    loc: Option<&LocalizationTable>,
    titles: Option<&TitlesTable>,
) -> String {
    format_with_titles_inner(text, params, loc, titles, false)
}

/// Fills a `SayText` chat message using `LocalizationTable` and `TitlesTable`, preserving `\x01`..`\x04`
/// color control bytes for `CHudSayText` span coloring.
pub fn format_saytext_with_titles(
    text: &str,
    params: &[String],
    loc: Option<&LocalizationTable>,
    titles: Option<&TitlesTable>,
) -> String {
    format_with_titles_inner(text, params, loc, titles, true)
}

fn format_with_titles_inner(
    text: &str,
    params: &[String],
    loc: Option<&LocalizationTable>,
    titles: Option<&TitlesTable>,
    keep_color_codes: bool,
) -> String {
    let stripped = text.trim_start_matches(['\u{1}', '\u{2}', '\u{3}', '\u{4}']);
    let prefix_len = text.len() - stripped.len();
    let trimmed = stripped.trim_end_matches(['\r', '\n', ' ', '\t']);
    let direct = resolve_token(trimmed, loc, titles);
    let template_owned = if direct != trimmed {
        resolve_inline_tokens(direct, loc, titles)
    } else if stripped.contains('#') {
        resolve_inline_tokens(stripped, loc, titles)
    } else {
        stripped.to_string()
    };
    let template = template_owned.as_str();
    let mut out = String::new();
    if keep_color_codes
        && prefix_len > 0
        && !template.starts_with(['\u{1}', '\u{2}', '\u{3}', '\u{4}'])
    {
        out.push_str(&text[..prefix_len]);
    }
    let mut next = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1}'..='\u{4}' => {
                if keep_color_codes {
                    out.push(c);
                }
            }
            '%' if chars.peek() == Some(&'s') => {
                chars.next();
                let slot = match chars.peek().and_then(|d| d.to_digit(10)) {
                    Some(d) if (1..=4).contains(&d) => {
                        chars.next();
                        d as usize - 1
                    }
                    _ => {
                        next += 1;
                        next - 1
                    }
                };
                let p = params.get(slot).map_or("", String::as_str);
                // A parameter may itself be a token (`#Cstrike_Name`) or contain one inline.
                let resolved_p = resolve_token(p, loc, titles);
                if resolved_p.contains('#') {
                    out.push_str(&resolve_inline_tokens(resolved_p, loc, titles));
                } else {
                    out.push_str(resolved_p);
                }
            }
            _ => out.push(c),
        }
    }
    // A template with no slot and parameters left: show them after it (a raw text message).
    if !template.contains('%') && !params.is_empty() && out == template {
        for p in params {
            out.push(' ');
            let resolved_p = resolve_token(p, loc, titles);
            if resolved_p.contains('#') {
                out.push_str(&resolve_inline_tokens(resolved_p, loc, titles));
            } else {
                out.push_str(resolved_p);
            }
        }
    }
    out.trim_matches(['\n', '\r']).to_string()
}

/// The visible text of a MOTD page: tags dropped, entities of the common few decoded, blank
/// runs squeezed.
pub fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut in_skipped = false;
    let mut tag = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                let name = name.to_ascii_lowercase();
                if matches!(name.as_str(), "style" | "script") {
                    in_skipped = !tag.starts_with('/');
                }
                if matches!(
                    name.as_str(),
                    "br" | "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3"
                ) {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(c),
            _ if in_skipped => {}
            _ => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");
    let mut squeezed = String::new();
    let mut blank = 0;
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        squeezed.push_str(line);
        squeezed.push('\n');
    }
    squeezed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn slots_take_their_parameters() {
        assert_eq!(
            format_message("%s1 :  %s2", &p(&["Bob", "hello"]), None),
            "Bob :  hello"
        );
        assert_eq!(
            format_message("\u{2}%s1: %s2", &p(&["a", "b"]), None),
            "a: b"
        );
        assert_eq!(
            format_message("%s killed %s", &p(&["a", "b"]), None),
            "a killed b"
        );
    }

    #[test]
    fn a_plain_message_keeps_its_parameters() {
        assert_eq!(
            format_message("#Game_connected", &p(&["cs-probe"]), None),
            "#Game_connected cs-probe"
        );
        assert_eq!(format_message("Hello", &[], None), "Hello");
    }

    #[test]
    fn html_becomes_text() {
        let page = "<html><head><style>p{color:red}</style></head><body><h1>Welcome</h1><p>Play &amp; enjoy</p><br><br><br>Bye</body></html>";
        assert_eq!(strip_html(page), "Welcome\n\nPlay & enjoy\n\nBye");
    }

    #[test]
    fn resolves_game_connected_and_inline_tokens_from_loc_and_titles() {
        let mut loc = LocalizationTable::default();
        loc.insert_text(
            "\"Tokens\"\n{\n\"Game_join_terrorist\" \"%s1 is joining the Terrorist force\"\n}",
        );
        let titles = TitlesTable::parse("Game_connected\n{\n%s connected.\n}\n");
        let joined = format_message_with_titles(
            "\u{1}#Game_connected",
            &p(&["Player"]),
            Some(&loc),
            Some(&titles),
        );
        assert_eq!(joined, "Player connected.");
        let team = format_message_with_titles(
            "#Game_join_terrorist",
            &p(&["Player"]),
            Some(&loc),
            Some(&titles),
        );
        assert_eq!(team, "Player is joining the Terrorist force");

        let trailing = format_message_with_titles(
            "\u{1}#Game_connected\n",
            &p(&["Bob"]),
            Some(&loc),
            Some(&titles),
        );
        assert_eq!(trailing, "Bob connected.");

        let inline = format_message_with_titles(
            "* #Game_connected",
            &p(&["Bob"]),
            Some(&loc),
            Some(&titles),
        );
        assert_eq!(inline, "* Bob connected.");
    }

    #[test]
    fn format_saytext_preserves_leading_and_inline_color_controls() {
        let mut loc = LocalizationTable::default();
        loc.insert_text(
            "\"Tokens\"\n{\n\"Cstrike_Chat_All\" \"\x02%s1 :  %s2\"\n\"Cstrike_Chat_CT\" \"\x01(Counter-Terrorist) \x03%s1\x01 :  %s2\"\n\"Game_radio\" \"\x02%s1 (RADIO): %s2\"\n}",
        );
        assert_eq!(
            format_saytext_with_titles("#Cstrike_Chat_All", &p(&["Alice", "hi"]), Some(&loc), None),
            "\x02Alice :  hi"
        );
        assert_eq!(
            format_saytext_with_titles(
                "#Cstrike_Chat_CT",
                &p(&["Bob", "rush a"]),
                Some(&loc),
                None
            ),
            "\x01(Counter-Terrorist) \x03Bob\x01 :  rush a"
        );
        assert_eq!(
            format_saytext_with_titles("\x01* \x03Bob\x01 : \x04bombticking", &[], None, None),
            "\x01* \x03Bob\x01 : \x04bombticking"
        );
        assert_eq!(
            format_message_with_titles("#Cstrike_Chat_All", &p(&["Alice", "hi"]), Some(&loc), None),
            "Alice :  hi"
        );
    }

    #[test]
    fn resolves_game_radio_location_and_round_tokens_without_raw_placeholders() {
        let radio_loc = format_saytext_with_titles(
            "#Game_radio_location",
            &p(&["BotAlpha", "Bombsite A", "#Cover_me"]),
            None,
            None,
        );
        assert_eq!(radio_loc, "\x02BotAlpha @ Bombsite A (RADIO): Cover Me!");
        assert!(!radio_loc.contains('#') && !radio_loc.contains("%s"));

        let radio = format_message_with_titles(
            "#Game_radio",
            &p(&["BotBravo", "#Fire_in_the_hole"]),
            None,
            None,
        );
        assert_eq!(radio, "BotBravo (RADIO): Fire in the hole!");

        for tok in [
            "#Cstrike_TitlesTXT_Bomb_Planted",
            "#Cstrike_TitlesTXT_Bomb_Defused",
            "#Cstrike_TitlesTXT_CTs_Win",
            "#Cstrike_TitlesTXT_Terrorists_Win",
            "#Switch_To_BurstFire",
            "#Not_Enough_Money",
        ] {
            let s = format_message_with_titles(tok, &[], None, None);
            assert!(
                !s.is_empty() && !s.contains('#') && !s.contains("%s"),
                "bad {tok} -> {s:?}"
            );
        }
    }
}
