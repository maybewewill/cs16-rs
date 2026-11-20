//! Quake / GoldSrc command-line and `config.cfg` tokenizer.
//!
//! Supports double-quoted arguments, `;` command separators outside quotes,
//! and `//` line comments.

/// Parse a script or single console input into a sequence of commands,
/// where each command is a non-empty list of tokens `[name, arg1, arg2, ...]`.
pub fn parse_commands(input: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut token = String::new();
    let mut has_token = false;
    let mut in_quotes = false;

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if in_quotes {
            if ch == '"' {
                in_quotes = false;
                current.push(std::mem::take(&mut token));
                has_token = false;
            } else {
                token.push(ch);
            }
            i += 1;
            continue;
        }

        // Line comment `//` outside quotes
        if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            if has_token {
                current.push(std::mem::take(&mut token));
                has_token = false;
            }
            if !current.is_empty() {
                commands.push(std::mem::take(&mut current));
            }
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        match ch {
            '"' => {
                if has_token {
                    current.push(std::mem::take(&mut token));
                    has_token = false;
                }
                in_quotes = true;
            }
            '\n' | '\r' | ';' => {
                if has_token {
                    current.push(std::mem::take(&mut token));
                    has_token = false;
                }
                if !current.is_empty() {
                    commands.push(std::mem::take(&mut current));
                }
            }
            c if c.is_whitespace() => {
                if has_token {
                    current.push(std::mem::take(&mut token));
                    has_token = false;
                }
            }
            other => {
                token.push(other);
                has_token = true;
            }
        }
        i += 1;
    }

    if has_token || in_quotes {
        current.push(token);
    }
    if !current.is_empty() {
        commands.push(current);
    }

    commands
}

/// Convert a key-down bind string (e.g. `"+forward; +speed"`) into its key-up counterpart
/// (`"-forward; -speed"`). Commands that do not begin with `+` are omitted on key-up,
/// matching GoldSrc's `Key_Event` behavior.
pub fn key_up_commands(bind_cmd: &str) -> Vec<Vec<String>> {
    parse_commands(bind_cmd)
        .into_iter()
        .filter_map(|mut tokens| {
            let first = tokens.first_mut()?;
            if let Some(rest) = first.strip_prefix('+') {
                *first = format!("-{rest}");
                Some(tokens)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quotes_semicolons_and_comments() {
        let cfg = r#"
            // Sample CS 1.6 config
            bind "w" "+forward"
            bind "SPACE" "+jump; +duck" // crouch-jump
            sensitivity "2.5"
            cl_crosshair_color "50 250 50"
            echo hello world; fps_max 100
        "#;
        let cmds = parse_commands(cfg);
        assert_eq!(cmds.len(), 6);
        assert_eq!(cmds[0], vec!["bind", "w", "+forward"]);
        assert_eq!(cmds[1], vec!["bind", "SPACE", "+jump; +duck"]);
        assert_eq!(cmds[2], vec!["sensitivity", "2.5"]);
        assert_eq!(cmds[3], vec!["cl_crosshair_color", "50 250 50"]);
        assert_eq!(cmds[4], vec!["echo", "hello", "world"]);
        assert_eq!(cmds[5], vec!["fps_max", "100"]);
    }

    #[test]
    fn generates_key_up_only_for_plus_actions() {
        let up = key_up_commands("+jump; echo hi; +duck");
        assert_eq!(up, vec![vec!["-jump"], vec!["-duck"]]);
    }
}
