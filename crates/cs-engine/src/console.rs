//! The server console: the commands an operator types at the dedicated server (or an RCON-like
//! transport later) and what they do. [`Server::console_command`] is the single entry point; it
//! never panics, whatever the line holds.
//!
//! Dispatch order for the first token (matched case-insensitively, an assumption about HLDS):
//! 1. the engine commands `status`, `kick`, `say`, `changelevel`, `map`, `quit`, `exit`;
//! 2. a registered cvar of that name (shows or sets it);
//! 3. [`GameDll::server_command`];
//! 4. otherwise `Unknown command "<name>"`.
//!
//! Text that clients control (their names) or that the operator typed is echoed with every
//! control character (U+0000 to U+001F and U+007F) replaced by `?`, so a client name cannot
//! inject terminal escape sequences into the operator's console.

use cs_proto48::handshake::tokenize_goldsrc_line;

use crate::client::ClientState;
use crate::game::GameDll;
use crate::server::Server;

/// Longest console line processed, in bytes; the rest of a longer line is ignored.
pub const MAX_CONSOLE_LINE: usize = 1024;
/// Longest map name `changelevel` and `map` accept, in characters.
const MAX_MAP_NAME: usize = 64;
/// Reason a client is told when `kick` is given none.
const DEFAULT_KICK_REASON: &str = "Kicked by server";

/// What a console command asks of its caller.
#[derive(Debug, PartialEq)]
pub enum ConsoleOutcome {
    /// Text to print on the console, possibly empty; errors are reported this way too.
    Output(String),
    /// Switch to the named map. The name is validated (`[A-Za-z0-9_-]`, 1 to 64 characters)
    /// but the engine does not check that the map exists; the caller loads it and then calls
    /// [`Server::change_map`].
    ChangeLevel(String),
    /// Shut the server down.
    Quit,
}

/// Replaces control characters (U+0000 to U+001F, U+007F) with `?`.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c < ' ' || c == '\u{7F}' { '?' } else { c })
        .collect()
}

/// Like [`clean`] but keeps newlines, for text that is meant to span lines.
fn clean_multiline(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c != '\n' && (c < ' ' || c == '\u{7F}') {
                '?'
            } else {
                c
            }
        })
        .collect()
}

/// `line` up to its first NUL and at most [`MAX_CONSOLE_LINE`] bytes, cut on a character
/// boundary.
fn clip_line(line: &str) -> &str {
    let line = line.split('\0').next().unwrap_or("");
    let mut end = line.len().min(MAX_CONSOLE_LINE);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    &line[..end]
}

fn output(text: impl Into<String>) -> ConsoleOutcome {
    ConsoleOutcome::Output(text.into())
}

/// Whether `name` is a map name `changelevel`, `map` and [`EngineApi::change_level`] accept:
/// 1 to 64 characters, each a letter, digit, `_` or `-`.
///
/// [`EngineApi::change_level`]: crate::EngineApi::change_level
pub fn valid_map_name(name: &str) -> bool {
    (1..=MAX_MAP_NAME).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn console_changelevel(args: &[String]) -> ConsoleOutcome {
    match args {
        [map] if valid_map_name(map) => ConsoleOutcome::ChangeLevel(map.clone()),
        [_] => output("Invalid map name: use 1 to 64 letters, digits, '_' or '-'\n"),
        _ => output("Usage: changelevel <map>\n"),
    }
}

impl<G: GameDll> Server<G> {
    /// Runs one console line and says what to do next.
    ///
    /// The line is cut at its first NUL byte and to [`MAX_CONSOLE_LINE`] bytes, then tokenized
    /// like a GoldSrc command line (quotes group). An empty or blank line gives `Output("")`.
    /// Invalid input (missing or extra arguments, unknown targets, bad map names) never changes
    /// anything and gives an `Output` that says why. See the module documentation for the
    /// dispatch order and the character cleaning of echoed text.
    ///
    /// Commands:
    /// - `status`: lines `hostname: <ServerConfig::hostname>` (the one source of the name: the
    ///   same text is in `svc_serverinfo` and the A2S info reply; the `hostname` cvar is not
    ///   consulted) and `map: <name>`, then one line per connected client, columns separated by
    ///   one space: `<slot> <userid> "<name>" <state> <ping>ms <address>`. The slot is 1-based as in
    ///   HLDS, the state `connecting` (signing on) or `active`.
    /// - `kick <#userid | slot | name> [reason]`: `#N` is a userid, a bare number a `status`
    ///   slot, anything else a name that must match exactly one client, ignoring case. The
    ///   reason (the other arguments joined by spaces, default `Kicked by server`) reaches the
    ///   client.
    /// - `say <text>`: prints `Console: <text>` to every client.
    /// - `changelevel <map>` and `map <map>`: [`ConsoleOutcome::ChangeLevel`].
    /// - `quit`, `exit`: [`ConsoleOutcome::Quit`].
    /// - `<cvar>` shows a cvar, `<cvar> <value>` sets it (further arguments are ignored).
    ///   The registry has no read-only flag, so every registered cvar can be set. Setting an
    ///   movement cvar changes what clients that sign on later are sent in `svc_newmovevars`;
    ///   clients already in the game are sent it again on the next frame.
    pub fn console_command(&mut self, line: &str) -> ConsoleOutcome {
        let argv = tokenize_goldsrc_line(clip_line(line));
        let Some(name) = argv.first() else {
            return output("");
        };
        if name.is_empty() {
            return output("");
        }
        match name.to_ascii_lowercase().as_str() {
            "status" => return self.console_status(),
            "kick" => return self.console_kick(&argv[1..]),
            "say" => return self.console_say(&argv[1..]),
            "changelevel" | "map" => return console_changelevel(&argv[1..]),
            "quit" | "exit" => return ConsoleOutcome::Quit,
            _ => {}
        }
        if let Some(cvar) = self.cvars.get(name) {
            let cvar_name = cvar.name.clone();
            return match argv.get(1) {
                None => output(format!(
                    "\"{}\" is \"{}\"\n",
                    clean(&cvar_name),
                    clean(&cvar.value)
                )),
                Some(value) => {
                    self.cvars.set(&cvar_name, value.clone());
                    output("")
                }
            };
        }
        if let Some(text) = self.with_game(|game, eng| game.server_command(eng, &argv)) {
            return output(clean_multiline(&text));
        }
        output(format!("Unknown command \"{}\"\n", clean(name)))
    }

    fn console_status(&self) -> ConsoleOutcome {
        let mut text = format!(
            "hostname: {}\nmap: {}\n",
            clean(&self.cfg.hostname),
            clean(&self.map.name)
        );
        for (index, client) in self.clients.iter().enumerate() {
            if client.is_free() {
                continue;
            }
            let state = match client.state {
                ClientState::Active => "active",
                _ => "connecting",
            };
            let name = self.views[index].userinfo.get("name").unwrap_or("");
            let addr = client.addr.map_or_else(String::new, |a| a.to_string());
            text.push_str(&format!(
                "{} {} \"{}\" {} {}ms {}\n",
                index + 1,
                client.userid,
                clean(name),
                state,
                client.pings.ping_ms(),
                clean(&addr),
            ));
        }
        output(text)
    }

    /// Slot of the connected client a `kick` target names, or the error text.
    fn find_kick_target(&self, target: &str) -> Result<u8, String> {
        let connected = |index: usize| self.clients.get(index).is_some_and(|c| !c.is_free());
        let not_found = || {
            Err(format!(
                "Can't find a client matching \"{}\"\n",
                clean(target)
            ))
        };
        if let Some(id) = target.strip_prefix('#') {
            let Ok(userid) = id.parse::<i32>() else {
                return not_found();
            };
            return match self
                .clients
                .iter()
                .position(|c| !c.is_free() && c.userid == userid)
            {
                Some(index) => Ok(index as u8),
                None => not_found(),
            };
        }
        if !target.is_empty() && target.bytes().all(|b| b.is_ascii_digit()) {
            return match target.parse::<usize>() {
                Ok(n) if n >= 1 && connected(n - 1) => Ok((n - 1) as u8),
                _ => not_found(),
            };
        }
        let mut found = None;
        for index in (0..self.clients.len()).filter(|&i| connected(i)) {
            let name = self.views[index].userinfo.get("name").unwrap_or("");
            if name.eq_ignore_ascii_case(target) {
                if found.is_some() {
                    return Err(format!(
                        "More than one client matches \"{}\"; use #userid or the slot number\n",
                        clean(target)
                    ));
                }
                found = Some(index as u8);
            }
        }
        match found {
            Some(slot) => Ok(slot),
            None => not_found(),
        }
    }

    fn console_kick(&mut self, args: &[String]) -> ConsoleOutcome {
        let Some(target) = args.first() else {
            return output("Usage: kick <#userid | slot | name> [reason]\n");
        };
        let slot = match self.find_kick_target(target) {
            Ok(slot) => slot,
            Err(text) => return output(text),
        };
        let reason = if args.len() > 1 {
            clean(&args[1..].join(" "))
        } else {
            DEFAULT_KICK_REASON.to_string()
        };
        let index = usize::from(slot);
        let name = clean(self.views[index].userinfo.get("name").unwrap_or(""));
        let userid = self.clients[index].userid;
        self.kick(slot, &reason);
        output(format!("Kicked \"{name}\" (userid {userid}): {reason}\n"))
    }

    fn console_say(&mut self, args: &[String]) -> ConsoleOutcome {
        if args.is_empty() {
            return output("Usage: say <text>\n");
        }
        let text = format!("Console: {}\n", clean(&args.join(" ")));
        self.with_game(|_, eng| eng.print_all(&text));
        output("")
    }
}
