//! Counter-Strike 1.6 CVar registry, command dispatcher, key bindings, and `config.cfg` persistence.

pub mod defaults;
pub mod parser;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ops::{BitAnd, BitOr};

pub const MAX_LOG_LINES: usize = 2000;
pub const MAX_HISTORY: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CVarFlags(u32);

impl CVarFlags {
    pub const NONE: Self = Self(0);
    pub const ARCHIVE: Self = Self(1 << 0);
    pub const SERVER: Self = Self(1 << 1);
    pub const CHEAT: Self = Self(1 << 2);
    pub const REPLICATED: Self = Self(1 << 3);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for CVarFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for CVarFlags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

#[derive(Debug, Clone)]
pub struct CVar {
    pub name: String,
    pub value: String,
    pub default_value: String,
    pub flags: CVarFlags,
    pub description: String,
}

impl CVar {
    pub fn as_f32(&self) -> f32 {
        self.value
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    }

    pub fn as_i32(&self) -> i32 {
        self.value
            .trim()
            .parse::<i32>()
            .or_else(|_| {
                self.value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map(|f| f as i32)
                    .ok_or(())
            })
            .unwrap_or(0)
    }

    pub fn as_bool(&self) -> bool {
        let v = self.value.trim();
        v == "1" || v.eq_ignore_ascii_case("true") || self.as_f32() != 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Command,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct ConsoleLine {
    pub level: LogLevel,
    pub text: String,
}

/// High-level actions emitted by console commands for the client/server runtime to execute.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineAction {
    LoadMap(String),
    Disconnect,
    Quit,
    ExecFile(String),
    WriteConfig,
    ToggleConsole,
    ToggleMenu,
    ToggleNoclip,
    SetPos([f32; 3]),
    GetPos,
    Status,
    Connect(String),
    Screenshot(Option<String>),
    OpenBuyMenu,
    OpenBuyEquipMenu,
    BuyItem(String),
    ChooseTeam,
    GiveWeapon(String),
    ToggleGod,
    Suicide,
    RestartRound,
    BotAdd(Option<String>),
    BotKick,
    Say { team_only: bool, text: String },
    OpenChat { team_only: bool },
    DropWeapon,
    LastInv,
    SelectSlot(u8),
    ToggleNightVision,
    OpenRadio(u8),
    /// `impulse <n>`: sent with the next usercmd (100 flashlight, 201 spray).
    Impulse(u8),
    /// `mp3 <play|loop|stop> [file]`.
    Mp3Command(Vec<String>),
    /// `cd <play|loop|stop|off> [track]`.
    CdCommand(Vec<String>),
    /// `play <file>` / `playvol <file> <vol>`.
    PlaySound { sample: String, volume: f32 },
    /// `spk <sentence>` / `speak <sentence>`.
    SpeakSentence(String),
    /// `stopsound`.
    StopSound,
    ActionInput { name: String, pressed: bool },
    /// A command the console does not know, handed on while the client is connected
    /// ([`CVarRegistry::set_forward_unknown`]): the shell sends the line to the server
    /// (`jointeam`, `menuselect`, `radio1` and the rest of the game's own commands).
    Forward(Vec<String>),
}

const KNOWN_COMMANDS: &[(&str, &str)] = &[
    ("echo", "Print text to the console"),
    ("exec", "Execute a .cfg script file"),
    ("bind", "Bind a key to a command"),
    ("unbind", "Remove a key binding"),
    ("unbindall", "Remove all key bindings"),
    ("alias", "Define a command alias"),
    ("toggle", "Toggle a CVar between 0 and 1 (or cycle values)"),
    ("cvarlist", "List registered CVars (optional prefix filter)"),
    ("cmdlist", "List available console commands"),
    ("find", "Search commands and CVars by substring"),
    ("clear", "Clear console output"),
    ("help", "Show brief console usage help"),
    ("map", "Load a BSP map (e.g. map de_dust2)"),
    ("changelevel", "Change current map (alias for map)"),
    ("disconnect", "Disconnect from current map/server to main menu"),
    ("quit", "Exit the game"),
    ("exit", "Exit the game"),
    ("status", "Show current engine / map status"),
    ("noclip", "Toggle free-camera flight mode"),
    ("setpos", "Teleport camera/player to X Y Z"),
    ("getpos", "Print current position and view angles"),
    ("writecfg", "Save archived CVars and binds to config.cfg"),
    ("connect", "Connect to a server ip:port"),
    ("screenshot", "Save a PNG screenshot of the current frame"),
    ("buy", "Open the weapon buy menu (or buy <item>)"),
    ("chooseteam", "Open team selection menu"),
    ("give", "Give a weapon directly (e.g. give ak47)"),
    ("god", "Toggle invulnerability"),
    ("kill", "Commit suicide"),
    ("sv_restart", "Restart the match and reset scores"),
    ("bot_add", "Add a bot to the smaller team"),
    ("bot_add_t", "Add a Terrorist bot"),
    ("bot_add_ct", "Add a Counter-Terrorist bot"),
    ("bot_kick", "Kick all bots from the server"),
    ("say", "Send a chat message"),
    ("say_team", "Send a team chat message"),
    ("drop", "Drop currently held weapon"),
    ("lastinv", "Switch to previously held weapon"),
    ("mp3", "Control MP3 background music playback (play/loop/stop)"),
    ("cd", "Control CD/soundtrack track playback (play/loop/stop/off)"),
    ("play", "Play a sound effect locally"),
    ("playvol", "Play a sound effect locally at specified volume"),
    ("spk", "Speak a sentences.txt / VOX phrase locally"),
    ("speak", "Speak a sentences.txt / VOX phrase locally"),
    ("stopsound", "Stop all active sounds"),
    ("+voicerecord", "Transmit microphone voice while held"),
    ("-voicerecord", "Stop transmitting microphone voice"),
    ("toggleconsole", "Open or close the developer console"),
    ("cancelselect", "Toggle pause menu"),
];

/// Commands that never go to a server, even when the console does not know them: the rcon and
/// server passwords (sent in clear text they would leak), the client's own configuration and
/// connection commands.
pub const NEVER_FORWARDED: &[&str] = &[
    "rcon",
    "rcon_password",
    "rcon_address",
    "password",
    "sv_password",
    "bind",
    "unbind",
    "unbindall",
    "alias",
    "exec",
    "writecfg",
    "config",
    "connect",
    "quit",
    "exit",
    "disconnect",
    "map",
    "changelevel",
];

/// Whether a command (lowercase) must stay local.
pub fn is_never_forwarded(cmd: &str) -> bool {
    NEVER_FORWARDED.contains(&cmd)
}

pub struct CVarRegistry {
    vars: BTreeMap<String, CVar>,
    binds: BTreeMap<String, String>,
    aliases: HashMap<String, String>,
    log: VecDeque<ConsoleLine>,
    history: VecDeque<String>,
    dirty: bool,
    forward_unknown: bool,
}

impl Default for CVarRegistry {
    fn default() -> Self {
        Self::new_with_defaults()
    }
}

impl CVarRegistry {
    pub fn new_empty() -> Self {
        Self {
            vars: BTreeMap::new(),
            binds: BTreeMap::new(),
            aliases: HashMap::new(),
            log: VecDeque::with_capacity(256),
            history: VecDeque::with_capacity(MAX_HISTORY),
            dirty: false,
            forward_unknown: false,
        }
    }

    /// While set, a command nobody knows becomes [`EngineAction::Forward`] instead of an
    /// "Unknown command" warning: a connected client sends it to the server.
    pub fn set_forward_unknown(&mut self, forward: bool) {
        self.forward_unknown = forward;
    }

    pub fn new_with_defaults() -> Self {
        let mut reg = Self::new_empty();
        defaults::register_default_cvars(&mut reg);
        defaults::register_default_binds(&mut reg);
        reg.dirty = false;
        reg
    }

    pub fn register(
        &mut self,
        name: &str,
        default_value: &str,
        flags: CVarFlags,
        description: &str,
    ) {
        let key = name.to_ascii_lowercase();
        self.vars.insert(
            key.clone(),
            CVar {
                name: key,
                value: default_value.to_string(),
                default_value: default_value.to_string(),
                flags,
                description: description.to_string(),
            },
        );
    }

    pub fn get(&self, name: &str) -> Option<&CVar> {
        self.vars.get(&name.to_ascii_lowercase())
    }

    pub fn get_str(&self, name: &str) -> &str {
        self.get(name).map(|c| c.value.as_str()).unwrap_or("")
    }

    pub fn get_f32(&self, name: &str) -> f32 {
        self.get(name).map(CVar::as_f32).unwrap_or(0.0)
    }

    pub fn get_i32(&self, name: &str) -> i32 {
        self.get(name).map(CVar::as_i32).unwrap_or(0)
    }

    pub fn get_bool(&self, name: &str) -> bool {
        self.get(name).map(CVar::as_bool).unwrap_or(false)
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) -> bool {
        let key = name.to_ascii_lowercase();
        let value = value.into();
        if let Some(cvar) = self.vars.get_mut(&key) {
            if cvar.value != value {
                cvar.value = value;
                if cvar.flags.contains(CVarFlags::ARCHIVE) {
                    self.dirty = true;
                }
            }
            true
        } else {
            false
        }
    }

    pub fn vars(&self) -> &BTreeMap<String, CVar> {
        &self.vars
    }

    pub fn binds(&self) -> &BTreeMap<String, String> {
        &self.binds
    }

    pub fn bind(&mut self, key: &str, command: &str) {
        self.binds
            .insert(normalize_key_name(key), command.to_string());
        self.dirty = true;
    }

    pub fn unbind(&mut self, key: &str) {
        if self.binds.remove(&normalize_key_name(key)).is_some() {
            self.dirty = true;
        }
    }

    pub fn unbind_all(&mut self) {
        self.binds.clear();
        self.dirty = true;
    }

    pub fn bind_for_key(&self, key: &str) -> Option<&str> {
        self.binds.get(&normalize_key_name(key)).map(String::as_str)
    }

    pub fn key_for_command(&self, command: &str) -> Option<&str> {
        self.binds
            .iter()
            .find(|(_, cmd)| cmd.eq_ignore_ascii_case(command))
            .map(|(k, _)| k.as_str())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub fn log_lines(&self) -> &VecDeque<ConsoleLine> {
        &self.log
    }

    pub fn history(&self) -> &VecDeque<String> {
        &self.history
    }

    pub fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.log.len() >= MAX_LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(ConsoleLine {
            level,
            text: text.into(),
        });
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push_log(LogLevel::Info, text);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.push_log(LogLevel::Warning, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push_log(LogLevel::Error, text);
    }

    /// Execute a line typed interactively in the console (records history and echoes `] cmd`).
    pub fn execute_console_line(&mut self, line: &str) -> Vec<EngineAction> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        self.push_log(LogLevel::Command, format!("] {trimmed}"));
        if self.history.back().map(String::as_str) != Some(trimmed) {
            if self.history.len() >= MAX_HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(trimmed.to_string());
        }
        self.execute_script(trimmed)
    }

    /// Execute a script (multi-line or semicolon-separated), returning actions for the engine.
    pub fn execute_script(&mut self, script: &str) -> Vec<EngineAction> {
        let mut actions = Vec::new();
        for tokens in parser::parse_commands(script) {
            self.execute_tokens(&tokens, 0, &mut actions);
        }
        actions
    }

    /// Trigger a key press or release via the bind table.
    pub fn handle_key_bind(&mut self, key: &str, pressed: bool) -> Vec<EngineAction> {
        let Some(cmd) = self.bind_for_key(key).map(str::to_string) else {
            return Vec::new();
        };
        if pressed {
            self.execute_script(&cmd)
        } else {
            let mut actions = Vec::new();
            for tokens in parser::key_up_commands(&cmd) {
                self.execute_tokens(&tokens, 0, &mut actions);
            }
            actions
        }
    }

    fn execute_tokens(&mut self, tokens: &[String], depth: usize, actions: &mut Vec<EngineAction>) {
        if tokens.is_empty() {
            return;
        }
        if depth > 16 {
            self.warn("Alias recursion limit reached");
            return;
        }

        let cmd = tokens[0].to_ascii_lowercase();
        let args = &tokens[1..];

        // Check aliases first
        if let Some(body) = self.aliases.get(&cmd).cloned() {
            for sub in parser::parse_commands(&body) {
                self.execute_tokens(&sub, depth + 1, actions);
            }
            return;
        }

        // `+action` / `-action` input commands
        if let Some(rest) = cmd.strip_prefix('+') {
            actions.push(EngineAction::ActionInput {
                name: rest.to_string(),
                pressed: true,
            });
            return;
        }
        if let Some(rest) = cmd.strip_prefix('-') {
            actions.push(EngineAction::ActionInput {
                name: rest.to_string(),
                pressed: false,
            });
            return;
        }

        match cmd.as_str() {
            "echo" => {
                self.info(args.join(" "));
            }
            "clear" => {
                self.log.clear();
            }
            "help" => {
                self.info("Useful commands: map <name>, cvarlist, cmdlist, find <text>, bind <key> <cmd>, noclip, net_graph 1, quit");
            }
            "exec" => {
                if let Some(file) = args.first() {
                    actions.push(EngineAction::ExecFile(file.clone()));
                } else {
                    self.info("Usage: exec <filename.cfg>");
                }
            }
            "bind" => match args.len() {
                0 => self.info("Usage: bind <key> [command]"),
                1 => {
                    let k = normalize_key_name(&args[0]);
                    if let Some(bound) = self.binds.get(&k).cloned() {
                        self.info(format!("\"{k}\" = \"{bound}\""));
                    } else {
                        self.info(format!("\"{k}\" is not bound"));
                    }
                }
                _ => {
                    let key = &args[0];
                    let body = args[1..].join(" ");
                    self.bind(key, &body);
                }
            },
            "unbind" => {
                if let Some(key) = args.first() {
                    self.unbind(key);
                } else {
                    self.info("Usage: unbind <key>");
                }
            }
            "unbindall" => {
                self.unbind_all();
            }
            "alias" => match args.len() {
                0 => {
                    let mut list: Vec<_> = self
                        .aliases
                        .iter()
                        .map(|(k, v)| format!("  {k} : {v}"))
                        .collect();
                    list.sort();
                    if list.is_empty() {
                        self.info("No aliases defined.");
                    } else {
                        for line in list {
                            self.info(line);
                        }
                    }
                }
                1 => {
                    let name = args[0].to_ascii_lowercase();
                    if let Some(val) = self.aliases.get(&name).cloned() {
                        self.info(format!("{name} : {val}"));
                    } else {
                        self.info(format!("Alias \"{name}\" not found"));
                    }
                }
                _ => {
                    let name = args[0].to_ascii_lowercase();
                    let body = args[1..].join(" ");
                    self.aliases.insert(name, body);
                }
            },
            "toggle" => {
                let Some(var_name) = args.first() else {
                    self.info("Usage: toggle <cvar> [val1 val2 ...]");
                    return;
                };
                let key = var_name.to_ascii_lowercase();
                let Some(current) = self.vars.get(&key).map(|c| c.value.clone()) else {
                    self.warn(format!("Unknown cvar \"{var_name}\""));
                    return;
                };
                let next = if args.len() >= 3 {
                    let choices = &args[1..];
                    let pos = choices.iter().position(|c| c == &current).unwrap_or(choices.len() - 1);
                    choices[(pos + 1) % choices.len()].clone()
                } else if current == "0" || current.eq_ignore_ascii_case("false") {
                    "1".to_string()
                } else {
                    "0".to_string()
                };
                self.set(&key, next.clone());
                self.info(format!("\"{key}\" is \"{next}\""));
            }
            "cvarlist" => {
                let prefix = args.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
                let lines: Vec<String> = self
                    .vars
                    .values()
                    .filter(|c| c.name.starts_with(&prefix))
                    .map(|c| format!("  {:<26} = {:<24} // {}", c.name, c.value, c.description))
                    .collect();
                let count = lines.len();
                for l in lines {
                    self.info(l);
                }
                self.info(format!("{count} CVars total"));
            }
            "cmdlist" => {
                let prefix = args.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
                let mut count = 0;
                for (name, desc) in KNOWN_COMMANDS {
                    if name.starts_with(&prefix) {
                        self.info(format!("  {name:<18} // {desc}"));
                        count += 1;
                    }
                }
                self.info(format!("{count} commands total"));
            }
            "find" => {
                let Some(needle) = args.first() else {
                    self.info("Usage: find <substring>");
                    return;
                };
                let needle_l = needle.to_ascii_lowercase();
                let mut found = 0;
                for (name, desc) in KNOWN_COMMANDS {
                    if name.contains(&needle_l) || desc.to_ascii_lowercase().contains(&needle_l) {
                        self.info(format!("  [cmd]  {name:<22} - {desc}"));
                        found += 1;
                    }
                }
                let matched_vars: Vec<String> = self
                    .vars
                    .values()
                    .filter(|c| {
                        c.name.contains(&needle_l)
                            || c.description.to_ascii_lowercase().contains(&needle_l)
                    })
                    .map(|c| format!("  [cvar] {:<22} = \"{}\" - {}", c.name, c.value, c.description))
                    .collect();
                found += matched_vars.len();
                for line in matched_vars {
                    self.info(line);
                }
                if found == 0 {
                    self.info(format!("No matches for \"{needle}\""));
                }
            }
            "map" | "changelevel" => {
                if let Some(map_name) = args.first() {
                    let clean = map_name
                        .trim()
                        .strip_suffix(".bsp")
                        .unwrap_or(map_name.trim())
                        .to_string();
                    actions.push(EngineAction::LoadMap(clean));
                } else {
                    self.info("Usage: map <mapname> (e.g. map de_dust2)");
                }
            }
            "disconnect" => actions.push(EngineAction::Disconnect),
            "quit" | "exit" => actions.push(EngineAction::Quit),
            "status" => actions.push(EngineAction::Status),
            "noclip" => actions.push(EngineAction::ToggleNoclip),
            "getpos" => actions.push(EngineAction::GetPos),
            "setpos" => {
                if args.len() >= 3
                    && let (Ok(x), Ok(y), Ok(z)) = (
                        args[0].parse::<f32>(),
                        args[1].parse::<f32>(),
                        args[2].parse::<f32>(),
                    )
                {
                    actions.push(EngineAction::SetPos([x, y, z]));
                } else {
                    self.info("Usage: setpos <x> <y> <z>");
                }
            }
            "writecfg" => actions.push(EngineAction::WriteConfig),
            "connect" => {
                if let Some(addr) = args.first() {
                    actions.push(EngineAction::Connect(addr.clone()));
                } else {
                    self.info("Usage: connect <ip:port>");
                }
            }
            "screenshot" => {
                actions.push(EngineAction::Screenshot(args.first().cloned()));
            }
            "buy" => {
                if let Some(item) = args.first() {
                    actions.push(EngineAction::BuyItem(item.clone()));
                } else {
                    actions.push(EngineAction::OpenBuyMenu);
                }
            }
            "buyequip" => {
                if let Some(item) = args.first() {
                    actions.push(EngineAction::BuyItem(item.clone()));
                } else {
                    actions.push(EngineAction::OpenBuyEquipMenu);
                }
            }
            "buyammo1" | "buyammo2" | "primammo" | "secammo" => {
                actions.push(EngineAction::BuyItem(cmd));
            }
            "ak47" | "m4a1" | "awp" | "deagle" | "usp" | "glock" | "p228" | "fiveseven"
            | "elites" | "elite" | "m3" | "xm1014" | "mp5" | "mac10" | "tmp" | "ump45"
            | "p90" | "famas" | "galil" | "aug" | "sg552" | "scout" | "g3sg1" | "sg550"
            | "m249" | "vest" | "vesthelm" | "defuser" | "hegren" | "flash" | "sgren"
            | "nvgs" => actions.push(EngineAction::BuyItem(cmd)),
            "nightvision" => actions.push(EngineAction::ToggleNightVision),
            "radio1" => actions.push(EngineAction::OpenRadio(1)),
            "radio2" => actions.push(EngineAction::OpenRadio(2)),
            "radio3" => actions.push(EngineAction::OpenRadio(3)),
            "impulse" => {
                match args.first().and_then(|a| a.parse::<u8>().ok()) {
                    Some(n) => actions.push(EngineAction::Impulse(n)),
                    None => self.info("Usage: impulse <0-255>"),
                }
            }
            "chooseteam" => actions.push(EngineAction::ChooseTeam),
            "give" => {
                if let Some(w) = args.first() {
                    actions.push(EngineAction::GiveWeapon(w.clone()));
                } else {
                    self.info("Usage: give <weapon> (e.g. give ak47, give awp, give deagle)");
                }
            }
            "god" => actions.push(EngineAction::ToggleGod),
            "kill" => actions.push(EngineAction::Suicide),
            "sv_restart" | "sv_restartround" => actions.push(EngineAction::RestartRound),
            "bot_add" => actions.push(EngineAction::BotAdd(args.first().cloned())),
            "bot_add_t" => actions.push(EngineAction::BotAdd(Some("t".into()))),
            "bot_add_ct" => actions.push(EngineAction::BotAdd(Some("ct".into()))),
            "bot_kick" => actions.push(EngineAction::BotKick),
            "say" => {
                if !args.is_empty() {
                    actions.push(EngineAction::Say {
                        team_only: false,
                        text: args.join(" "),
                    });
                }
            }
            "say_team" => {
                if !args.is_empty() {
                    actions.push(EngineAction::Say {
                        team_only: true,
                        text: args.join(" "),
                    });
                }
            }
            "messagemode" => actions.push(EngineAction::OpenChat { team_only: false }),
            "messagemode2" => actions.push(EngineAction::OpenChat { team_only: true }),
            "drop" => actions.push(EngineAction::DropWeapon),
            "lastinv" => actions.push(EngineAction::LastInv),
            "slot1" => actions.push(EngineAction::SelectSlot(1)),
            "slot2" => actions.push(EngineAction::SelectSlot(2)),
            "slot3" => actions.push(EngineAction::SelectSlot(3)),
            "slot4" => actions.push(EngineAction::SelectSlot(4)),
            "slot5" => actions.push(EngineAction::SelectSlot(5)),
            "mp3" => actions.push(EngineAction::Mp3Command(args.to_vec())),
            "cd" => actions.push(EngineAction::CdCommand(args.to_vec())),
            "play" => {
                if let Some(sample) = args.first() {
                    actions.push(EngineAction::PlaySound {
                        sample: sample.clone(),
                        volume: 1.0,
                    });
                } else {
                    self.info("Usage: play <filename.wav>");
                }
            }
            "playvol" => {
                if let Some(sample) = args.first() {
                    let volume = args
                        .get(1)
                        .and_then(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                        .unwrap_or(1.0)
                        .clamp(0.0, 2.0);
                    actions.push(EngineAction::PlaySound {
                        sample: sample.clone(),
                        volume,
                    });
                } else {
                    self.info("Usage: playvol <filename.wav> <volume>");
                }
            }
            "spk" | "speak" => {
                if !args.is_empty() {
                    actions.push(EngineAction::SpeakSentence(args.join(" ")));
                } else {
                    self.info("Usage: spk <sentence or !name>");
                }
            }
            "stopsound" => actions.push(EngineAction::StopSound),
            "toggleconsole" => actions.push(EngineAction::ToggleConsole),
            "cancelselect" => actions.push(EngineAction::ToggleMenu),
            _ => {
                // Check if it is a registered CVar
                if let Some(existing) = self.vars.get(&cmd).cloned() {
                    if args.is_empty() {
                        self.info(format!(
                            "\"{}\" is \"{}\" (default: \"{}\") - {}",
                            existing.name,
                            existing.value,
                            existing.default_value,
                            existing.description
                        ));
                    } else {
                        let new_val = args.join(" ");
                        self.set(&cmd, new_val);
                    }
                } else if is_never_forwarded(&cmd) {
                    // The user's own: passwords, config and key commands stay on this machine.
                    self.warn(format!("\"{cmd}\" is a local command and is not sent to the server"));
                } else if self.forward_unknown {
                    actions.push(EngineAction::Forward(tokens.to_vec()));
                } else {
                    // Print classic GoldSrc message so interactive console feedback is clear.
                    self.warn(format!("Unknown command: {}", tokens[0]));
                }
            }
        }
    }

    /// Tab-complete the current console input string.
    /// Returns `(completed_text, matching_candidates)`.
    pub fn complete(&self, input: &str, available_maps: &[String]) -> (Option<String>, Vec<String>) {
        let trimmed_start = input.trim_start();
        if trimmed_start.is_empty() {
            return (None, Vec::new());
        }

        // Complete map name after `map ` or `changelevel `
        for prefix_cmd in ["map ", "changelevel "] {
            if let Some(rest) = trimmed_start
                .to_ascii_lowercase()
                .strip_prefix(prefix_cmd)
                .map(|r| r.trim_start().to_string())
            {
                let matches: Vec<String> = available_maps
                    .iter()
                    .filter(|m| m.to_ascii_lowercase().starts_with(&rest))
                    .cloned()
                    .collect();
                let completed = common_prefix(&matches).map(|p| format!("{prefix_cmd}{p}"));
                return (completed, matches);
            }
        }

        // Only complete the first word when no whitespace yet
        if trimmed_start.contains(char::is_whitespace) {
            return (None, Vec::new());
        }

        let needle = trimmed_start.to_ascii_lowercase();
        let mut matches = Vec::new();
        for (cmd, _) in KNOWN_COMMANDS {
            if cmd.starts_with(&needle) {
                matches.push((*cmd).to_string());
            }
        }
        for name in self.vars.keys() {
            if name.starts_with(&needle) {
                matches.push(name.clone());
            }
        }
        matches.sort();
        matches.dedup();
        let completed = common_prefix(&matches);
        (completed, matches)
    }

    /// Serialize archived CVars and key bindings to authentic CS 1.6 `config.cfg` format.
    pub fn serialize_config(&self) -> String {
        let mut out = String::from("// Generated by cs16-rs\nunbindall\n");
        for (key, cmd) in &self.binds {
            out.push_str(&format!("bind \"{key}\" \"{cmd}\"\n"));
        }
        out.push('\n');
        for cvar in self.vars.values() {
            if cvar.flags.contains(CVarFlags::ARCHIVE) {
                out.push_str(&format!("{} \"{}\"\n", cvar.name, cvar.value));
            }
        }
        out
    }
}

pub fn normalize_key_name(key: &str) -> String {
    let k = key.trim().to_ascii_lowercase();
    match k.as_str() {
        "~" | "tilde" | "grave" | "backquote" => "`".to_string(),
        "esc" => "escape".to_string(),
        "control" | "lctrl" | "rctrl" => "ctrl".to_string(),
        "lshift" | "rshift" => "shift".to_string(),
        "lalt" | "ralt" => "alt".to_string(),
        "return" => "enter".to_string(),
        other => other.to_string(),
    }
}

fn common_prefix(items: &[String]) -> Option<String> {
    let first = items.first()?;
    let mut prefix = first.clone();
    for item in &items[1..] {
        let len = prefix
            .bytes()
            .zip(item.bytes())
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .count();
        prefix.truncate(len);
        if prefix.is_empty() {
            return None;
        }
    }
    Some(prefix)
}

#[cfg(test)]
mod tests {
    #[test]
    fn unknown_commands_are_forwarded_only_while_connected() {
        let mut reg = CVarRegistry::new_with_defaults();
        let none = reg.execute_script("jointeam 2");
        assert!(none.is_empty(), "not connected: nothing is sent");
        reg.set_forward_unknown(true);
        let sent = reg.execute_script("jointeam 2; menuselect \"1 2\"");
        assert_eq!(
            sent,
            vec![
                EngineAction::Forward(vec!["jointeam".into(), "2".into()]),
                EngineAction::Forward(vec!["menuselect".into(), "1 2".into()]),
            ]
        );
    }

    #[test]
    fn known_commands_and_cvars_are_not_forwarded() {
        let mut reg = CVarRegistry::new_with_defaults();
        reg.set_forward_unknown(true);
        for line in ["sensitivity 4", "echo hi", "bind x +use", "map de_dust2", "say hello"] {
            let actions = reg.execute_script(line);
            assert!(
                !actions.iter().any(|a| matches!(a, EngineAction::Forward(_))),
                "{line}: {actions:?}"
            );
        }
    }

    #[test]
    fn passwords_and_config_never_leave_the_machine() {
        let mut reg = CVarRegistry::new_with_defaults();
        reg.set_forward_unknown(true);
        for line in [
            "rcon_password hunter2",
            "rcon status",
            "password secret",
            "sv_password x",
            "RCON_PASSWORD hunter2",
        ] {
            let actions = reg.execute_script(line);
            assert!(actions.is_empty(), "{line}: {actions:?}");
        }
        assert!(is_never_forwarded("rcon_password"));
        assert!(!is_never_forwarded("jointeam"));
    }

    use super::*;

    #[test]
    fn executes_cvars_binds_aliases_and_roundtrips_config() {
        let mut reg = CVarRegistry::new_with_defaults();
        assert_eq!(reg.get_f32("sensitivity"), 3.0);

        let actions = reg.execute_console_line("sensitivity 1.85; map de_dust2");
        assert!((reg.get_f32("sensitivity") - 1.85).abs() < 1e-6);
        assert_eq!(actions, vec![EngineAction::LoadMap("de_dust2".into())]);

        reg.execute_console_line(r#"alias +cjump "+jump; +duck""#);
        let jump_actions = reg.execute_script("+cjump");
        assert_eq!(
            jump_actions,
            vec![
                EngineAction::ActionInput {
                    name: "jump".into(),
                    pressed: true
                },
                EngineAction::ActionInput {
                    name: "duck".into(),
                    pressed: true
                }
            ]
        );

        let saved = reg.serialize_config();
        let mut restored = CVarRegistry::new_with_defaults();
        restored.execute_script(&saved);
        assert!((restored.get_f32("sensitivity") - 1.85).abs() < 1e-6);
        assert_eq!(restored.bind_for_key("w"), Some("+forward"));
    }

    #[test]
    fn autocompletes_commands_cvars_and_maps() {
        let reg = CVarRegistry::new_with_defaults();
        let maps = vec![
            "de_aztec".to_string(),
            "de_dust".to_string(),
            "de_dust2".to_string(),
            "de_inferno".to_string(),
        ];

        let (done, list) = reg.complete("map de_d", &maps);
        assert_eq!(done.as_deref(), Some("map de_dust"));
        assert_eq!(list, vec!["de_dust", "de_dust2"]);

        let (done_cvar, _) = reg.complete("cl_crosshair_co", &maps);
        assert_eq!(done_cvar.as_deref(), Some("cl_crosshair_color"));
    }

    #[test]
    fn parses_real_steam_config_if_present() {
        let cfg_path = r"D:\SteamLibrary\steamapps\common\Half-Life\cstrike\config.cfg";
        let Ok(text) = std::fs::read_to_string(cfg_path) else {
            return;
        };
        let mut reg = CVarRegistry::new_with_defaults();
        reg.execute_script(&text);
        assert!(reg.get_f32("sensitivity") > 0.0);
        assert!(reg.binds().len() >= 5);
    }

    #[test]
    fn rejects_non_finite_cvars_and_preserves_buy_commands() {
        let mut reg = CVarRegistry::new_with_defaults();
        reg.set("sensitivity", "nan");
        assert_eq!(reg.get_f32("sensitivity"), 0.0);
        reg.set("sensitivity", "inf");
        assert_eq!(reg.get_f32("sensitivity"), 0.0);
        reg.set("fps_max", "-inf");
        assert_eq!(reg.get_i32("fps_max"), 0);

        assert_eq!(reg.execute_script("buy"), vec![EngineAction::OpenBuyMenu]);
        assert_eq!(
            reg.execute_script("buyequip"),
            vec![EngineAction::OpenBuyEquipMenu]
        );
        assert_eq!(
            reg.execute_script("buyammo1"),
            vec![EngineAction::BuyItem("buyammo1".into())]
        );
        assert_eq!(
            reg.execute_script("buyammo2"),
            vec![EngineAction::BuyItem("buyammo2".into())]
        );
        assert_eq!(
            reg.execute_script("primammo"),
            vec![EngineAction::BuyItem("primammo".into())]
        );
        assert_eq!(
            reg.execute_script("secammo"),
            vec![EngineAction::BuyItem("secammo".into())]
        );
    }
}
