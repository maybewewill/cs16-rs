//! Text the server sends for the HUD: `TextMsg` and `SayText` templates with `%s1`..`%s4`
//! slots and `#Token` names, and the MOTD's HTML.

use cs_assets::scheme::LocalizationTable;

/// Fills a message `text` with its `params`: a `#Token` is looked up first, `%s1`..`%s4` take
/// the parameters in order, a bare `%s` takes the next one. Control bytes (`\x01` to `\x04`,
/// the colour codes of chat) are dropped.
pub fn format_message(text: &str, params: &[String], loc: Option<&LocalizationTable>) -> String {
    let template = match loc {
        Some(l) if text.starts_with('#') => l.resolve(text),
        _ => text,
    };
    let mut out = String::new();
    let mut next = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1}'..='\u{4}' => {}
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
                // A parameter may itself be a token (`#Cstrike_Name`).
                match loc {
                    Some(l) if p.starts_with('#') => out.push_str(l.resolve(p)),
                    _ => out.push_str(p),
                }
            }
            _ => out.push(c),
        }
    }
    // A template with no slot and parameters left: show them after it (a raw text message).
    if !template.contains('%') && !params.is_empty() && out == template {
        for p in params {
            out.push(' ');
            out.push_str(p);
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
                let name = tag.trim_start_matches('/').split_whitespace().next().unwrap_or("");
                let name = name.to_ascii_lowercase();
                if matches!(name.as_str(), "style" | "script") {
                    in_skipped = !tag.starts_with('/');
                }
                if matches!(name.as_str(), "br" | "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3") {
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
        assert_eq!(format_message("\u{2}%s1: %s2", &p(&["a", "b"]), None), "a: b");
        assert_eq!(format_message("%s killed %s", &p(&["a", "b"]), None), "a killed b");
    }

    #[test]
    fn a_plain_message_keeps_its_parameters() {
        assert_eq!(format_message("#Game_connected", &p(&["cs-probe"]), None), "#Game_connected cs-probe");
        assert_eq!(format_message("Hello", &[], None), "Hello");
    }

    #[test]
    fn html_becomes_text() {
        let page = "<html><head><style>p{color:red}</style></head><body><h1>Welcome</h1><p>Play &amp; enjoy</p><br><br><br>Bye</body></html>";
        assert_eq!(strip_html(page), "Welcome\n\nPlay & enjoy\n\nBye");
    }
}
