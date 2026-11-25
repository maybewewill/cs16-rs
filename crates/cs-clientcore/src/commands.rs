//! The client's console command buffer for text the server stuffs (`svc_stufftext`): line
//! splitting, tokenizing and the allow/deny rules for commands.
//!
//! Ported from Xash3D FWGS `engine/common/cmd.c` (read 2026-11-23): `Cbuf_ExecuteCommandsFromBuffer`
//! (line splitting), `Cmd_ShouldAllowCommand` (the filter) and `Cmd_ExecuteStringWithPrivilegeCheck`
//! (the order: alias, command, cvar, forward to the server). The registry flags mirror
//! `Cmd_AddRestrictedCommand` in `engine/client/cl_main.c`.
//!
//! Not ported: aliases, `cmd_scripting` `$cvar` substitution (privileged text only), conditional
//! `:` prefixes, `wait`. Stuffed text is never privileged, so none of them would apply.

use std::collections::BTreeMap;

use crate::cvars::has_filtered_prefix;

/// Only the local user may run it (`CMD_PRIVILEGED`, `Cmd_AddRestrictedCommand`).
pub const CMD_PRIVILEGED: u32 = 1 << 0;
/// Refused from a stufftext while `cl_filterstuffcmd` is on (`CMD_FILTERABLE`).
pub const CMD_FILTERABLE: u32 = 1 << 1;
/// The server may run it with a stufftext. A command registered without this flag is the
/// user's: a deliberate deviation from Xash (which lets any unrestricted command run), so a
/// shell that registers `exec`, `bind` or `screenshot` does not hand them to a server by
/// forgetting a flag. The core's own safe commands (`reconnect`, `echo`, ...) carry it.
pub const CMD_STUFFTEXT_OK: u32 = 1 << 2;

/// Longest command line the buffer accepts (`MAX_CMD_LINE` is 2048 in Xash).
pub const MAX_CMD_LINE: usize = 2048;

/// One parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleCommand {
    pub name: String,
    pub args: Vec<String>,
}

impl ConsoleCommand {
    /// The command line rebuilt the way `Cmd_ForwardToServer` sends it: the name, then the
    /// arguments (quoted when they hold a space), `cmd <text>` sends just the text.
    pub fn forward_text(&self) -> String {
        let quote = |a: &String| {
            if a.is_empty() || a.contains([' ', ';']) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        };
        let args: Vec<String> = self.args.iter().map(quote).collect();
        if self.name.eq_ignore_ascii_case("cmd") {
            if args.is_empty() {
                "\n".to_string()
            } else {
                args.join(" ")
            }
        } else if args.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.name, args.join(" "))
        }
    }
}

/// The commands the client knows by name and their flags.
#[derive(Debug, Clone)]
pub struct CommandRegistry {
    cmds: BTreeMap<String, u32>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    /// The engine commands of `cl_main.c` that matter to the connection, with Xash's flags.
    /// Commands whose handler lives in the shell (`exec`, `bind`, `play`, ...) are added by the
    /// shell with [`register`](Self::register); until then they are forwarded to the server like
    /// any unknown command.
    pub fn new() -> Self {
        let mut r = Self {
            cmds: BTreeMap::new(),
        };
        for name in [
            "reconnect",
            "disconnect",
            "echo",
            "fullserverinfo",
            "cmd",
            "upload",
        ] {
            r.register(name, CMD_STUFFTEXT_OK);
        }
        // `Cmd_AddRestrictedCommand` in `CL_InitLocal` (slowhack defense).
        for name in [
            "setinfo",
            "userinfo",
            "connect",
            "retry",
            "quit",
            "exit",
            "rcon",
            "rcon_password",
            "rcon_address",
            "bind",
            "unbind",
            "unbindall",
            "exec",
            "alias",
            "motd_write",
            "writecfg",
            "writeid",
            "writeip",
            "_restart",
            "condump",
            "screenshot",
            "record",
            "killdemo",
            "localservers",
            "internetservers",
            "ui_queryserver",
            "waveplaylen",
            "replaybufferdat",
        ] {
            r.register(name, CMD_PRIVILEGED);
        }
        r
    }

    pub fn register(&mut self, name: &str, flags: u32) {
        self.cmds.insert(name.to_ascii_lowercase(), flags);
    }

    pub fn flags(&self, name: &str) -> Option<u32> {
        self.cmds.get(&name.to_ascii_lowercase()).copied()
    }

    /// `Cmd_ShouldAllowCommand` for text that came from the server. `filter` is
    /// `cl_filterstuffcmd > 0`.
    pub fn stufftext_may_run(&self, name: &str, filter: bool) -> bool {
        let flags = self.flags(name).unwrap_or(0);
        if flags & CMD_PRIVILEGED != 0 || flags & CMD_STUFFTEXT_OK == 0 || name.starts_with('@') {
            return false;
        }
        if !filter {
            return true;
        }
        if flags & CMD_FILTERABLE != 0 {
            return false;
        }
        !has_filtered_prefix(name)
    }
}

/// Splits stuffed text into command lines the way `Cbuf_ExecuteCommandsFromBuffer` does: a line
/// ends at a newline, a carriage return or a `;` outside quotes; `\"` and `\\` inside quotes do
/// not end the quote; a `//` that follows whitespace starts a comment to the end of the line.
/// Empty lines are dropped; a line of `MAX_CMD_LINE` bytes or more is dropped with its tail
/// (the original prints an overflow error and skips it).
pub fn split_lines(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let mut quotes = false;
        let mut comment: Option<usize> = None;
        while i < bytes.len() {
            if comment.is_none() {
                if bytes[i] == b'"' {
                    quotes = !quotes;
                }
                if quotes {
                    if i + 1 < bytes.len()
                        && bytes[i] == b'\\'
                        && (bytes[i + 1] == b'"' || bytes[i + 1] == b'\\')
                    {
                        i += 1;
                    }
                } else {
                    if bytes[i] == b'/'
                        && bytes.get(i + 1) == Some(&b'/')
                        && (i == start || bytes[i - 1] <= b' ')
                    {
                        comment = Some(i);
                    }
                    if bytes[i] == b';' {
                        break;
                    }
                }
            }
            if bytes[i] == b'\n' || bytes[i] == b'\r' {
                break;
            }
            i += 1;
        }
        let end = comment.unwrap_or(i.min(bytes.len()));
        let line = &text[start..end];
        // Step over the separator.
        i += 1;
        if line.len() >= MAX_CMD_LINE - 1 {
            continue;
        }
        if !line.trim().is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// `Cmd_TokenizeString`: whitespace separated tokens, a quoted token may hold spaces.
/// An empty line gives `None`.
pub fn tokenize(line: &str) -> Option<ConsoleCommand> {
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut token = String::new();
        if c == '"' {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                        token.push(chars.next().unwrap_or('\\'));
                    }
                    c => token.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                token.push(c);
                chars.next();
            }
        }
        tokens.push(token);
    }
    if tokens.is_empty() {
        return None;
    }
    let name = tokens.remove(0);
    Some(ConsoleCommand { name, args: tokens })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_on_semicolons_newlines_and_respect_quotes() {
        assert_eq!(
            split_lines("a 1; b \"x;y\"\nc // note\n\n  \r d"),
            vec!["a 1", " b \"x;y\"", "c ", " d"]
        );
        assert_eq!(split_lines("echo \"a\\\"b;c\""), vec!["echo \"a\\\"b;c\""]);
        // `//` inside a word is not a comment.
        assert_eq!(split_lines("exec http://x"), vec!["exec http://x"]);
    }

    #[test]
    fn tokens_and_forwarding() {
        let c = tokenize("say \"hello there\" 3").unwrap();
        assert_eq!(c.name, "say");
        assert_eq!(c.args, vec!["hello there", "3"]);
        assert_eq!(c.forward_text(), "say \"hello there\" 3");
        assert_eq!(tokenize("cmd say hi").unwrap().forward_text(), "say hi");
        assert_eq!(tokenize("  ").map(|c| c.name), None);
    }

    #[test]
    fn stufftext_filter_denies_restricted_prefixed_and_at_commands() {
        let r = CommandRegistry::new();
        assert!(r.stufftext_may_run("reconnect", true));
        // A shell command is the user's until it is marked.
        let mut r = r;
        r.register("spk", 0);
        assert!(!r.stufftext_may_run("spk", false));
        r.register("spk", CMD_STUFFTEXT_OK);
        assert!(r.stufftext_may_run("spk", true));
        assert!(!r.stufftext_may_run("quit", true));
        assert!(!r.stufftext_may_run("quit", false));
        assert!(!r.stufftext_may_run("setinfo", false));
        r.register("cl_something", CMD_STUFFTEXT_OK);
        assert!(!r.stufftext_may_run("cl_something", true));
        assert!(r.stufftext_may_run("cl_something", false));
        assert!(!r.stufftext_may_run("@hidden", false));
    }
}
