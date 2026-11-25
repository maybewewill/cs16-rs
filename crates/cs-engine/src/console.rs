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
    /// - `status`: lines `hostname: <the hostname cvar>` (the one source of the name, as in the
    ///   original: [`Server::hostname`], which `--hostname` and a console `hostname X` both
    ///   set; the same text is in `svc_serverinfo`, the A2S info reply and the game's MOTD
    ///   title) and `map: <name>`, then one line per connected client, columns separated by
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
    ///   clients already in the game are sent it again with their next command
    ///   (`SV_RunCmd` checks per command, as ReHLDS does).
    pub fn console_command(&mut self, line: &str) -> ConsoleOutcome {
        let argv = tokenize_goldsrc_line(clip_line(line));
        let Some(name) = argv.first() else {
            return output("");
        };
        if name.is_empty() {
            return output("");
        }
        match name.to_ascii_lowercase().as_str() {
            "status" => {
                if argv.get(1).is_some_and(|a| a.eq_ignore_ascii_case("full")) {
                    return self.console_status_full();
                }
                return self.console_status();
            }
            "users" => return self.console_users(),
            "kick" => return self.console_kick(&argv[1..]),
            "banid" => return self.console_banid(&argv[1..]),
            "removeid" => return self.console_removeid(&argv[1..]),
            "listid" => return output(self.bans.list_id(self.real_time)),
            "writeid" => return output(format!("Writing listid...\n{}", self.bans.write_id_cfg(self.real_time))),
            "addip" => return self.console_addip(&argv[1..]),
            "removeip" => return self.console_removeip(&argv[1..]),
            "listip" => return output(self.bans.list_ip(self.real_time)),
            "writeip" => return output(format!("Writing listip...\n{}", self.bans.write_ip_cfg(self.real_time))),
            "log" => return self.console_log(&argv[1..]),
            "logaddress" => return self.console_logaddress(&argv[1..], false),
            "logaddress_add" => return self.console_logaddress(&argv[1..], true),
            "logaddress_del" => return self.console_logaddress_del(&argv[1..]),
            "logaddress_delall" => {
                self.logger.clear_addresses();
                return output("Cleared all log addresses\n");
            }
            "pause" => return self.console_pause(),
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
            clean(&self.hostname()),
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
            if client.fake {
                // `Host_Status_f` (ReHLDS `host_cmd.cpp` 625-635, read 2026-11-23) prints `BOT`
                // where a client's unique id is and still prints the ping (0, `SV_CalcPing`);
                // here the address column, which a bot has none of, reads `BOT`.
                text.push_str(&format!(
                    "{} {} \"{}\" {} 0ms BOT\n",
                    index + 1,
                    client.userid,
                    clean(name),
                    state,
                ));
                continue;
            }
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
        let clean_msg = clean(&args.join(" "));
        self.logger.log(
            self.real_time,
            &format!("\"Console<0><Console><Console>\" say \"{clean_msg}\""),
        );
        let text = format!("Console: {clean_msg}\n");
        self.with_game(|_, eng| eng.print_all(&text));
        output("")
    }

    fn console_status_full(&self) -> ConsoleOutcome {
        let active_count = self.clients.iter().filter(|c| !c.is_free()).count();
        let bots = self.fake_client_count();
        let mut text = format!(
            "hostname:  {}\nversion :  48/1.1.2.7/Stdio {} secure\ntcp/ip  :  0.0.0.0:27015\nmap     :  {}\nplayers :  {} active ({} max, {} bots)\n\n#      name userid uniqueid frag time ping loss adr\n",
            clean(&self.hostname()),
            self.cfg.build,
            clean(&self.map.name),
            active_count,
            self.cfg.max_clients,
            bots,
        );
        for (index, client) in self.clients.iter().enumerate() {
            if client.is_free() {
                continue;
            }
            let name = clean(self.views[index].userinfo.get("name").unwrap_or(""));
            let uid = if client.fake {
                "BOT".to_string()
            } else {
                clean(&client.auth_id)
            };
            let adr = if client.fake {
                "BOT".to_string()
            } else {
                client.addr.map_or_else(String::new, |a| a.to_string())
            };
            let elapsed = (self.real_time - client.connected_at).max(0.0) as u64;
            let hh = elapsed / 3600;
            let mm = (elapsed / 60) % 60;
            let ss = elapsed % 60;
            text.push_str(&format!(
                "# {:>2} \"{}\" {} {} 0 {hh:02}:{mm:02}:{ss:02} {} 0 {}\n",
                index + 1,
                name,
                client.userid,
                uid,
                client.pings.ping_ms(),
                clean(&adr),
            ));
        }
        text.push_str(&format!("{active_count} users\n"));
        output(text)
    }

    fn console_users(&self) -> ConsoleOutcome {
        let mut text = String::from("<slot:userid:\"name\">\n");
        let mut count = 0usize;
        for (index, client) in self.clients.iter().enumerate() {
            if client.is_free() {
                continue;
            }
            count += 1;
            let name = clean(self.views[index].userinfo.get("name").unwrap_or(""));
            text.push_str(&format!("{}:{}: \"{}\"\n", index + 1, client.userid, name));
        }
        text.push_str(&format!("{count} users\n"));
        output(text)
    }

    fn console_banid(&mut self, args: &[String]) -> ConsoleOutcome {
        let Some(mins_str) = args.first() else {
            return output("Usage: banid <minutes> <#userid | uniqueid> [kick]\n");
        };
        let Ok(mins) = mins_str.parse::<f64>() else {
            return output("Usage: banid <minutes> <#userid | uniqueid> [kick]\n");
        };
        let Some(target) = args.get(1) else {
            return output("Usage: banid <minutes> <#userid | uniqueid> [kick]\n");
        };
        let kick = args.get(2).is_some_and(|a| a.eq_ignore_ascii_case("kick"));
        let mut kick_slot = None;
        let auth_id = if target.starts_with('#') || target.bytes().all(|b| b.is_ascii_digit()) {
            match self.find_kick_target(target) {
                Ok(slot) => {
                    kick_slot = Some(slot);
                    self.clients[usize::from(slot)].auth_id.clone()
                }
                Err(msg) => return output(msg),
            }
        } else {
            target.clone()
        };
        if auth_id.is_empty() {
            return output("Cannot ban empty uniqueid\n");
        }
        if let Err(e) = self.bans.add_id(mins, &auth_id, self.real_time) {
            return output(format!("{e}\n"));
        }
        self.logger.log(
            self.real_time,
            &format!("Banid: \"<{auth_id}>\" banned ({mins} min)"),
        );
        if kick && let Some(slot) = kick_slot {
            self.kick(slot, "Kicked and banned");
        }
        output(format!("banid: \"{}\" banned ({mins} min)\n", clean(&auth_id)))
    }

    fn console_removeid(&mut self, args: &[String]) -> ConsoleOutcome {
        let Some(id) = args.first() else {
            return output("Usage: removeid <uniqueid>\n");
        };
        if self.bans.remove_id(id) {
            output(format!("removeid: removed \"{}\"\n", clean(id)))
        } else {
            output(format!("removeid: couldn't find \"{}\"\n", clean(id)))
        }
    }

    fn console_addip(&mut self, args: &[String]) -> ConsoleOutcome {
        let Some(mins_str) = args.first() else {
            return output("Usage: addip <minutes> <ipaddress>\n");
        };
        let Ok(mins) = mins_str.parse::<f64>() else {
            return output("Usage: addip <minutes> <ipaddress>\n");
        };
        let Some(ip) = args.get(1) else {
            return output("Usage: addip <minutes> <ipaddress>\n");
        };
        match self.bans.add_ip(mins, ip, self.real_time) {
            Ok(()) => output(format!("addip: \"{}\" banned ({mins} min)\n", clean(ip))),
            Err(e) => output(format!("{e}\n")),
        }
    }

    fn console_removeip(&mut self, args: &[String]) -> ConsoleOutcome {
        let Some(ip) = args.first() else {
            return output("Usage: removeip <ipaddress>\n");
        };
        if self.bans.remove_ip(ip) {
            output(format!("removeip: removed \"{}\"\n", clean(ip)))
        } else {
            output(format!("removeip: couldn't find \"{}\"\n", clean(ip)))
        }
    }

    fn console_log(&mut self, args: &[String]) -> ConsoleOutcome {
        match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("on") => {
                self.logger.enabled = true;
                self.logger.log(self.real_time, "Log file started");
                output("Server logging enabled.\n")
            }
            Some("off") => {
                self.logger.log(self.real_time, "Log file closed");
                self.logger.enabled = false;
                output("Server logging disabled.\n")
            }
            _ => output(format!(
                "Server logging is currently {}\n",
                if self.logger.enabled { "enabled" } else { "disabled" }
            )),
        }
    }

    fn parse_log_peer(args: &[String]) -> Option<cs_transport::PeerAddr> {
        let addr_str = match args {
            [ip, port] => format!("{ip}:{port}"),
            [ip_port] => ip_port.clone(),
            _ => return None,
        };
        addr_str
            .parse::<std::net::SocketAddr>()
            .ok()
            .map(cs_transport::PeerAddr::Udp)
    }

    fn console_logaddress(&mut self, args: &[String], append: bool) -> ConsoleOutcome {
        let Some(peer) = Self::parse_log_peer(args) else {
            return output("Usage: logaddress <ip> <port>\n");
        };
        if append {
            let _ = self.logger.add_address(peer);
        } else {
            self.logger.set_single_address(peer);
        }
        output(format!("logaddress: {peer}\n"))
    }

    fn console_logaddress_del(&mut self, args: &[String]) -> ConsoleOutcome {
        let Some(peer) = Self::parse_log_peer(args) else {
            return output("Usage: logaddress_del <ip> <port>\n");
        };
        if self.logger.remove_address(&peer) {
            output(format!("Removed logaddress {peer}\n"))
        } else {
            output(format!("logaddress {peer} not found\n"))
        }
    }

    fn console_pause(&mut self) -> ConsoleOutcome {
        if self.cvars.get_f32("pausable") <= 0.0 {
            return output("Cannot pause\n");
        }
        self.paused = !self.paused;
        let mut w = cs_proto48::bitbuf::MsgWriter::new();
        cs_proto48::messages::write_svc_setpause(&mut w, self.paused);
        let bytes = w.into_bytes();
        for client in &mut self.clients {
            if client.state == ClientState::Active && !client.fake {
                client.chan.write_reliable(&bytes);
            }
        }
        if self.paused {
            output("Server paused\n")
        } else {
            output("Server unpaused\n")
        }
    }
}
