//! Counter-Strike 1.6 CVar registry, command dispatcher, key bindings, and `config.cfg` persistence.

pub mod defaults;
pub mod parser;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::{BitAnd, BitOr};

pub const MAX_LOG_LINES: usize = 2000;
pub const MAX_HISTORY: usize = 200;
pub const MAX_WAIT_QUEUE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CVarFlags(u32);

impl CVarFlags {
    pub const NONE: Self = Self(0);
    pub const ARCHIVE: Self = Self(1 << 0);
    pub const USERINFO: Self = Self(1 << 1);
    pub const SERVER: Self = Self(1 << 2);
    pub const EXTDLL: Self = Self(1 << 3);
    pub const CLIENTDLL: Self = Self(1 << 4);
    /// Password/secret server cvar: A2S_RULES reports "1" when non-empty/non-zero, else "0" (`FCVAR_PROTECTED`).
    pub const PROTECTED: Self = Self(1 << 5);
    /// Multiplayer-locked cvar (`FCVAR_SPONLY`).
    pub const SPONLY: Self = Self(1 << 6);
    /// Printable ASCII only (`FCVAR_PRINTABLEONLY`).
    pub const PRINTABLEONLY: Self = Self(1 << 7);
    /// Changes not written to server log (`FCVAR_UNLOGGED`).
    pub const UNLOGGED: Self = Self(1 << 8);
    /// Strip leading and trailing whitespace (`FCVAR_NOEXTRAWHITESPACE`).
    pub const NOEXTRAWHITESPACE: Self = Self(1 << 9);
    /// Require `sv_cheats 1` unless changed locally (`FCVAR_CHEAT` parity).
    pub const CHEAT: Self = Self(1 << 10);
    /// Replicated to clients.
    pub const REPLICATED: Self = Self(1 << 11);

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

    /// Value reported in public `A2S_RULES` replies (`FCVAR_PROTECTED` returns `"1"` if set
    /// and non-zero, `"0"` otherwise, `rehlds/common/cvardef.h:27`).
    pub fn public_rules_value(&self) -> String {
        if self.flags.contains(CVarFlags::PROTECTED) {
            let v = self.value.trim();
            if v.is_empty() || v == "0" {
                "0".to_string()
            } else {
                "1".to_string()
            }
        } else {
            self.value.clone()
        }
    }
}

/// Context for enforcing GoldSrc `FCVAR_SPONLY`, `FCVAR_CHEAT`, `FCVAR_PRINTABLEONLY`, and
/// `FCVAR_NOEXTRAWHITESPACE` rules when changing a CVar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CvarSetContext {
    pub multiplayer: bool,
    pub cheats_enabled: bool,
    pub trusted: bool,
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
    InvNext,
    InvPrev,
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
    /// `adjust_crosshair` [color_index].
    AdjustCrosshair(Option<u8>),
    /// `togglescores`.
    ToggleScores,
    /// `playerlist`: opens VGUI2 PlayerListDialog (mute players).
    OpenPlayerList,
    /// `viewdemo` / `playdemo` [path]: opens VGUI2 DemoPlayerDialog.
    OpenDemoPlayer(Option<String>),
    /// `spec_mode <mode>`: set spectator camera mode.
    SpecMode(i32),
    /// `spec_menu <0|1>`: open or close spectator bottom controls.
    SpecMenu(i32),
    /// `record <name>`: start recording a GoldSrc `.dem` demo file.
    RecordDemo(String),
    /// `stop` / `stopdemo`: finish recording/playing the current demo.
    StopDemo,
    /// `playdemo <name>`: start headless/client playback of a `.dem` demo.
    PlayDemo(String),
    /// `timedemo <name>`: benchmark playback of a `.dem` file.
    TimeDemo(String),
    /// `rcon <cmd>`: execute an RCON command against `rcon_address` / active server.
    RconCommand(String),
    /// `setmaster <add|remove|enable|disable|list> [ip:port]`.
    SetMaster(Vec<String>),
    /// `force_centerview` / `centerview`: reset view pitch to 0.
    ForceCenterView,
    /// `condump [filename]`: dump console log to file.
    ConDump(Option<String>),
    /// `retry`: reconnect to the last server address.
    Retry,
    /// `reconnect`: reconnect to the current or last server.
    Reconnect,
    /// `restart` / `_restart`: reload current local map cleanly.
    RestartMap,
    /// `shutdownserver`: stop local server and return to menu.
    ShutdownServer,
    /// `fullupdate`: reset client delta sequence and request uncompressed snapshot.
    FullUpdate,
    /// `dlfile <resource>`: request a file from the active server.
    DlFile(String),
    /// `showbriefing`: open map briefing / MOTD.
    ShowBriefing,
    /// `demos` / `startdemos` / `lastdemo`: demo playlist control.
    Demos { kind: String, args: Vec<String> },
    /// `displaysoundlist`: print active mixer sound channels.
    DisplaySoundList,
    /// `mcache`: print cached studio/BSP models.
    MCache,
    /// `delta_stats`: print delta encoder statistics.
    DeltaStats,
    /// `delta_clear`: clear delta frame history.
    DeltaClear,
    /// `demo_timescale <0.1..4.0>`: set demo playback speed.
    DemoTimescale(f32),
    /// `demo_pause`: pause active demo playback.
    DemoPause,
    /// `demo_resume`: resume active demo playback.
    DemoResume,
    /// `demo_togglepause`: toggle demo playback pause.
    DemoTogglePause,
    /// `demo_goto <seconds>`: seek active demo playback to timestamp.
    DemoGoto(f64),
    /// `voice_scale_player <slot> <0.0..2.0>`: set per-player voice gain multiplier.
    VoiceScalePlayer { slot: u8, scale: f32 },
    /// Server/admin console command (`maps`, `version`, `ping`, `stats`, `user`, etc.).
    ServerCmd(Vec<String>),
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
    ("bindtoggle", "Bind a key to toggle a CVar between 0 and 1"),
    ("unbind", "Remove a key binding"),
    ("unbindall", "Remove all key bindings"),
    ("alias", "Define a command alias"),
    ("wait", "Defer remaining commands in buffer to next frame"),
    ("toggle", "Toggle a CVar between 0 and 1 (or cycle values)"),
    ("incrementvar", "Add a delta to a CVar within [min, max]"),
    ("multvar", "Multiply a CVar by a factor within [min, max]"),
    ("condump", "Save console text buffer to condump000.txt"),
    ("cvarlist", "List registered CVars (optional prefix filter)"),
    ("cmdlist", "List available console commands"),
    ("find", "Search commands and CVars by substring"),
    ("clear", "Clear console output"),
    ("help", "Show brief console usage help"),
    ("map", "Load a BSP map (e.g. map de_dust2)"),
    ("changelevel", "Change current map (alias for map)"),
    ("changelevel2", "Change current map in multiplayer"),
    ("disconnect", "Disconnect from current map/server to main menu"),
    ("retry", "Reconnect to the last server address"),
    ("reconnect", "Reconnect to the current/last server"),
    ("restart", "Restart current local map"),
    ("_restart", "Restart current local map"),
    ("shutdownserver", "Stop local server and disconnect to menu"),
    ("fullupdate", "Force full uncompressed entity snapshot"),
    ("dlfile", "Download a resource file from the connected server"),
    ("showbriefing", "Display map briefing / MOTD window"),
    ("demos", "Cycle/play queued demo playlist"),
    ("startdemos", "Start playing a list of demos"),
    ("lastdemo", "Replay the most recently played demo"),
    ("displaysoundlist", "Print active audio mixer channels"),
    ("mcache", "Print precached/loaded models"),
    ("delta_stats", "Print delta compression statistics"),
    ("delta_clear", "Reset delta compression baseline cache"),
    ("quit", "Exit the game"),
    ("exit", "Exit the game"),
    ("status", "Show current engine / map status"),
    ("version", "Show engine build and Protocol 48 version"),
    ("maps", "List available BSP maps (optional substring filter)"),
    ("ping", "Show connected clients and ping"),
    ("stats", "Show server/engine CPU and tick stats"),
    ("stat", "Show server/engine CPU and tick stats"),
    ("users", "List connected users"),
    ("user", "Show sanitized userinfo for a player"),
    ("serverinfo", "Show server info string"),
    ("localinfo", "Show or set server localinfo key-value store"),
    ("showinfo", "Show serverinfo / localinfo entry"),
    ("fullinfo", "Send complete userinfo string to server"),
    ("setinfo", "Show or update a userinfo key"),
    ("dropclient", "Disconnect a client slot from server"),
    ("kick", "Kick a client from server"),
    ("banid", "Ban a Steam/Auth ID"),
    ("removeid", "Remove a Steam/Auth ID ban"),
    ("listid", "List banned Steam/Auth IDs"),
    ("writeid", "Write banned IDs to bannedcfgfile"),
    ("addip", "Ban an IP address"),
    ("removeip", "Remove an IP ban"),
    ("listip", "List banned IP addresses"),
    ("writeip", "Write banned IPs to listipcfgfile"),
    ("log", "Toggle server logging (on/off)"),
    ("logaddress", "Set remote UDP log target"),
    ("logaddress_add", "Add remote UDP log target"),
    ("logaddress_del", "Remove remote UDP log target"),
    ("pause", "Toggle server pause"),
    ("setpause", "Pause the server"),
    ("unpause", "Unpause the server"),
    ("resetrcon", "Reset RCON failure counters and IP bans"),
    ("motd", "Print message of the day"),
    ("motd_write", "Write message of the day to motdfile"),
    ("tell", "Send a private message to a player"),
    ("noclip", "Toggle free-camera flight mode"),
    ("notarget", "Toggle FL_NOTARGET (bots ignore player, requires sv_cheats 1)"),
    ("fly", "Toggle MOVETYPE_FLY flight mode (requires sv_cheats 1)"),
    ("setpos", "Teleport camera/player to X Y Z"),
    ("getpos", "Print current position and view angles"),
    ("writecfg", "Save archived CVars and binds to config.cfg"),
    ("connect", "Connect to a server ip:port"),
    ("screenshot", "Save a PNG screenshot of the current frame"),
    ("snapshot", "Save a PNG screenshot of the current frame"),
    ("buy", "Open the weapon buy menu (or buy <item>)"),
    ("buyequip", "Open the equipment buy menu"),
    ("buyammo1", "Buy one primary ammo clip"),
    ("buyammo2", "Buy one secondary ammo clip"),
    ("autobuy", "Execute autobuy equipment purchase"),
    ("rebuy", "Rebuy previous round equipment"),
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
    ("messagemode", "Open global chat input"),
    ("messagemode2", "Open team chat input"),
    ("drop", "Drop currently held weapon"),
    ("lastinv", "Switch to previously held weapon"),
    ("invnext", "Select next weapon in inventory"),
    ("invprev", "Select previous weapon in inventory"),
    ("slot1", "Select weapon slot 1"),
    ("slot2", "Select weapon slot 2"),
    ("slot3", "Select weapon slot 3"),
    ("slot4", "Select weapon slot 4"),
    ("slot5", "Select weapon slot 5"),
    ("slot6", "Select menu item 6"),
    ("slot7", "Select menu item 7"),
    ("slot8", "Select menu item 8"),
    ("slot9", "Select menu item 9"),
    ("slot10", "Select menu item 10 / close menu"),
    ("nightvision", "Toggle night vision goggles"),
    ("radio1", "Open standard radio menu"),
    ("radio2", "Open group radio menu"),
    ("radio3", "Open report radio menu"),
    ("impulse", "Send impulse command (100=flashlight, 201=spray)"),
    ("mp3", "Control MP3 background music playback (play/loop/stop)"),
    ("cd", "Control CD/soundtrack track playback (play/loop/stop/off)"),
    ("play", "Play a sound effect locally"),
    ("playvol", "Play a sound effect locally at specified volume"),
    ("spk", "Speak a sentences.txt / VOX phrase locally"),
    ("speak", "Speak a sentences.txt / VOX phrase locally"),
    ("stopsound", "Stop all active sounds"),
    ("soundfade", "Fade audio volume over time"),
    ("+attack", "Primary attack while held"),
    ("-attack", "Release primary attack"),
    ("+attack2", "Secondary attack while held"),
    ("-attack2", "Release secondary attack"),
    ("+forward", "Move forward while held"),
    ("-forward", "Stop moving forward"),
    ("+back", "Move backward while held"),
    ("-back", "Stop moving backward"),
    ("+moveleft", "Strafe left while held"),
    ("-moveleft", "Stop strafing left"),
    ("+moveright", "Strafe right while held"),
    ("-moveright", "Stop strafing right"),
    ("+moveup", "Swim/move upward while held"),
    ("-moveup", "Stop moving upward"),
    ("+movedown", "Swim/move downward while held"),
    ("-movedown", "Stop moving downward"),
    ("+left", "Turn view left while held"),
    ("-left", "Stop turning left"),
    ("+right", "Turn view right while held"),
    ("-right", "Stop turning right"),
    ("+lookup", "Pitch view up while held"),
    ("-lookup", "Stop pitching up"),
    ("+lookdown", "Pitch view down while held"),
    ("-lookdown", "Stop pitching down"),
    ("+strafe", "Mouse X strafes instead of turning while held"),
    ("-strafe", "Stop mouse strafe mode"),
    ("+klook", "Forward/back keys look up/down while held"),
    ("-klook", "Stop keyboard look mode"),
    ("+mlook", "Enable mouse look while held"),
    ("-mlook", "Disable mouse look"),
    ("+jlook", "Enable joystick look while held"),
    ("-jlook", "Disable joystick look"),
    ("+jump", "Jump while held"),
    ("-jump", "Release jump"),
    ("+duck", "Crouch while held"),
    ("-duck", "Release crouch"),
    ("+speed", "Walk slowly while held"),
    ("-speed", "Release walk"),
    ("+reload", "Reload weapon while held"),
    ("-reload", "Release reload"),
    ("+use", "Use entity/bomb/hostage while held"),
    ("-use", "Release use"),
    ("+showscores", "Show scoreboard while held"),
    ("-showscores", "Hide scoreboard"),
    ("+voicerecord", "Transmit microphone voice while held"),
    ("-voicerecord", "Stop transmitting microphone voice"),
    ("+commandmenu", "Show CS 1.6 VGUI2 command menu while held"),
    ("-commandmenu", "Hide CS 1.6 VGUI2 command menu"),
    ("force_centerview", "Center view pitch to horizon"),
    ("centerview", "Center view pitch to horizon"),
    ("adjust_crosshair", "Cycle or set CS 1.6 crosshair color"),
    ("togglescores", "Toggle the multiplayer scoreboard"),
    ("playerlist", "Open the mute player list dialog"),
    ("viewdemo", "Open the VGUI2 demo playback transport dialog"),
    ("playdemo", "Play a recorded demo file"),
    ("record", "Start recording a GoldSrc .dem demo file"),
    ("stop", "Stop recording the active demo file"),
    ("stopdemo", "Stop recording or playing the active demo file"),
    ("timedemo", "Benchmark playback of a .dem demo file"),
    ("rcon", "Send an RCON command to a remote server"),
    ("setmaster", "Configure master server endpoints"),
    ("spec_mode", "Switch spectator camera mode"),
    ("spec_menu", "Show or hide spectator menu bar"),
    ("toggleconsole", "Open or close the developer console"),
    ("cancelselect", "Toggle pause menu"),
    ("new", "Protocol 48 signon stage 1 command"),
    ("sendres", "Protocol 48 signon resource request command"),
    ("spawn", "Protocol 48 signon spawn command"),
    ("sendents", "Protocol 48 signon entity request command"),
    ("heartbeat", "Send master server heartbeat"),
    ("reload", "Reload current map / save"),
    ("hostage_stop", "Toggle hostage movement freeze"),
    ("key_rotate", "[compat] Legacy key rotation toggle"),
    ("viewframe", "[compat] Debug viewmodel frame"),
    ("viewmodel", "[compat] Debug viewmodel"),
    ("viewnext", "[compat] Cycle debug viewmodel"),
    ("viewprev", "[compat] Cycle debug viewmodel"),
    ("_set_vid_level", "[compat] Video level helper"),
    ("_setaddons_folder", "[compat] Addons folder helper"),
    ("_setgamedir", "[compat] Game directory helper"),
    ("_sethdmodels", "[compat] HD models helper"),
    ("_setrenderer", "[compat] Renderer helper"),
    ("_setvideomode", "[compat] Video mode helper"),
    ("rethrow", "Re-throw last thrown grenade (requires sv_cheats 1)"),
    ("sv_rethrow_last_grenade", "Re-throw last thrown grenade (requires sv_cheats 1)"),
    ("give_all", "Give full armor, defuser, grenades, and full ammo (requires sv_cheats 1)"),
    ("ammo_full", "Refill primary and secondary reserve ammo (requires sv_cheats 1)"),
    ("demo_timescale", "Set demo playback speed multiplier (0.1..4.0)"),
    ("demo_pause", "Pause active demo playback"),
    ("demo_resume", "Resume active demo playback"),
    ("demo_togglepause", "Toggle active demo playback pause"),
    ("demo_goto", "Seek active demo playback to timestamp in seconds"),
    ("voice_scale_player", "Set per-player incoming voice gain multiplier (<slot> <0.0..2.0>)"),
    ("cfg_diff", "Print only CVars whose value differs from the vanilla CS 1.6 default"),
    ("lo3", "Run competitive Live on Three (3-restart) scrim sequence"),
    ("mp_live_on_three", "Run competitive Live on Three (3-restart) scrim sequence"),
    ("cp", "Save a KZ checkpoint on solid ground (requires sv_kz_mode 1)"),
    ("tp", "Teleport to latest KZ checkpoint (requires sv_kz_mode 1)"),
    ("gocheck", "Teleport to latest KZ checkpoint (requires sv_kz_mode 1)"),
    ("prevcp", "Step back to previous KZ checkpoint (requires sv_kz_mode 1)"),
    ("stuck", "Step back to previous KZ checkpoint (requires sv_kz_mode 1)"),
    ("start", "Teleport to KZ run start and reset timer (requires sv_kz_mode 1)"),
    ("reset", "Reset KZ timer and checkpoints (requires sv_kz_mode 1)"),
    ("mod_list", "List loaded polyglot server plugins (WASM, Python, AMXX)"),
    ("amx_plugins", "List loaded polyglot server plugins (AMX Mod X alias)"),
    ("mod_pause", "Pause a loaded server plugin by ID"),
    ("mod_unpause", "Resume a paused server plugin by ID"),
    ("mod_reload", "Hot-reload one server plugin or 'all' from disk"),
    ("mod_unload", "Unload a server plugin by ID"),
    ("mod_stats", "Show live per-plugin CPU/memory resource monitor (resmon)"),
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
    userinfo_dirty: BTreeSet<String>,
    server_dirty: BTreeSet<String>,
    wait_frames: u32,
    wait_queue: VecDeque<(Vec<String>, usize)>,
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
            userinfo_dirty: BTreeSet::new(),
            server_dirty: BTreeSet::new(),
            wait_frames: 0,
            wait_queue: VecDeque::new(),
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
        let changed = if let Some(cvar) = self.vars.get_mut(&key) {
            if cvar.value != value {
                cvar.value = value.clone();
                if cvar.flags.contains(CVarFlags::ARCHIVE) {
                    self.dirty = true;
                }
                if cvar.flags.contains(CVarFlags::USERINFO) {
                    self.userinfo_dirty.insert(key.clone());
                }
                if cvar.flags.contains(CVarFlags::SERVER) {
                    self.server_dirty.insert(key.clone());
                }
            }
            true
        } else {
            false
        };
        if changed {
            const MIRRORS: &[(&str, &str)] = &[
                ("cl_autowepswitch", "_cl_autowepswitch"),
                ("_cl_autowepswitch", "cl_autowepswitch"),
                ("cl_allow_download", "cl_allowdownload"),
                ("cl_allowdownload", "cl_allow_download"),
                ("cl_allow_upload", "cl_allowupload"),
                ("cl_allowupload", "cl_allow_upload"),
            ];
            for &(src, dst) in MIRRORS {
                if key == src
                    && let Some(peer) = self.vars.get_mut(dst)
                    && peer.value != value
                {
                    peer.value = value.clone();
                    if peer.flags.contains(CVarFlags::ARCHIVE) {
                        self.dirty = true;
                    }
                    if peer.flags.contains(CVarFlags::USERINFO) {
                        self.userinfo_dirty.insert(dst.to_string());
                    }
                    if peer.flags.contains(CVarFlags::SERVER) {
                        self.server_dirty.insert(dst.to_string());
                    }
                }
            }
        }
        changed
    }

    /// Drains `FCVAR_USERINFO` CVars modified since the last call as `(name, value)` pairs.
    pub fn take_dirty_userinfo(&mut self) -> Vec<(String, String)> {
        let keys: Vec<String> = std::mem::take(&mut self.userinfo_dirty).into_iter().collect();
        keys.into_iter()
            .filter_map(|k| self.vars.get(&k).map(|c| (k, c.value.clone())))
            .collect()
    }

    /// Drains `FCVAR_SERVER` CVars modified since the last call as `(name, value)` pairs.
    pub fn take_dirty_server(&mut self) -> Vec<(String, String)> {
        let keys: Vec<String> = std::mem::take(&mut self.server_dirty).into_iter().collect();
        keys.into_iter()
            .filter_map(|k| self.vars.get(&k).map(|c| (k, c.value.clone())))
            .collect()
    }

    /// Returns `(name, current_value, default_value)` for every CVar whose value differs from its
    /// registered vanilla default (`cfg_diff`).
    pub fn diff_default_cvars(&self) -> Vec<(&str, &str, &str)> {
        self.vars
            .values()
            .filter(|c| c.value != c.default_value)
            .map(|c| (c.name.as_str(), c.value.as_str(), c.default_value.as_str()))
            .collect()
    }

    /// Returns all registered `FCVAR_USERINFO` CVars as `(name, value)` pairs.
    pub fn userinfo_vars(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .values()
            .filter(|c| c.flags.contains(CVarFlags::USERINFO))
            .map(|c| (c.name.as_str(), c.value.as_str()))
    }

    /// Sets a CVar while enforcing GoldSrc `SPONLY`, `CHEAT`, `PRINTABLEONLY`, and
    /// `NOEXTRAWHITESPACE` flags (`Cvar_Set2` / ReHLDS `cvar.cpp`).
    pub fn set_with_context(
        &mut self,
        name: &str,
        value: &str,
        ctx: CvarSetContext,
    ) -> Result<bool, String> {
        let key = name.to_ascii_lowercase();
        let Some(flags) = self.vars.get(&key).map(|c| c.flags) else {
            return Ok(false);
        };
        if !ctx.trusted {
            if flags.contains(CVarFlags::SPONLY) && ctx.multiplayer {
                return Err(format!("Can't set {key} in multiplayer"));
            }
            if flags.contains(CVarFlags::CHEAT) && !ctx.cheats_enabled {
                return Err(format!("Can't set {key} unless sv_cheats is 1"));
            }
        }
        let mut sanitized = if flags.contains(CVarFlags::PRINTABLEONLY) {
            value
                .chars()
                .filter(|&c| c >= ' ' && c != '\u{7F}')
                .collect::<String>()
        } else {
            value.to_string()
        };
        if flags.contains(CVarFlags::NOEXTRAWHITESPACE) {
            sanitized = sanitized.trim().to_string();
        }
        Ok(self.set(&key, sanitized))
    }

    pub fn vars(&self) -> &BTreeMap<String, CVar> {
        &self.vars
    }

    pub fn known_command_names() -> impl Iterator<Item = &'static str> {
        KNOWN_COMMANDS.iter().map(|(name, _)| *name)
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
    ///
    /// `wait` (`Cmd_Wait_f` / `Cbuf_ExecuteCommandsFromBuffer` in Xash3D FWGS
    /// `engine/common/cmd.c`, read 2026-11-25) defers remaining commands in the buffer to
    /// subsequent frames via [`Self::tick_wait_queue`].
    pub fn execute_script(&mut self, script: &str) -> Vec<EngineAction> {
        let mut actions = Vec::new();
        let parsed: VecDeque<(Vec<String>, usize)> = parser::parse_commands(script)
            .into_iter()
            .map(|t| (t, 0))
            .collect();
        if self.wait_frames > 0 || !self.wait_queue.is_empty() {
            for item in parsed {
                if self.wait_queue.len() >= MAX_WAIT_QUEUE {
                    self.warn("Command wait queue overflow; dropping trailing commands");
                    break;
                }
                self.wait_queue.push_back(item);
            }
            return actions;
        }
        self.drain_command_deque(parsed, &mut actions);
        actions
    }

    /// Runs one frame of commands deferred by `wait`.
    pub fn tick_wait_queue(&mut self) -> Vec<EngineAction> {
        let mut actions = Vec::new();
        if self.wait_frames > 0 {
            self.wait_frames -= 1;
            return actions;
        }
        if self.wait_queue.is_empty() {
            return actions;
        }
        let queued = std::mem::take(&mut self.wait_queue);
        self.drain_command_deque(queued, &mut actions);
        actions
    }

    /// Number of commands currently deferred in the `wait` queue.
    pub fn pending_wait_count(&self) -> usize {
        self.wait_queue.len()
    }

    fn drain_command_deque(
        &mut self,
        mut pending: VecDeque<(Vec<String>, usize)>,
        actions: &mut Vec<EngineAction>,
    ) {
        while let Some((tokens, depth)) = pending.pop_front() {
            let mut deferred = VecDeque::new();
            let wait_count = self.execute_tokens_deferrable(&tokens, depth, actions, &mut deferred);
            if let Some(frames) = wait_count {
                // Prepend any remaining alias commands followed by the rest of `pending`.
                let space = MAX_WAIT_QUEUE.saturating_sub(self.wait_queue.len());
                let mut combined = VecDeque::new();
                for item in deferred.into_iter().chain(pending).take(space) {
                    combined.push_back(item);
                }
                while let Some(item) = combined.pop_back() {
                    self.wait_queue.push_front(item);
                }
                self.wait_frames = frames.saturating_sub(1);
                break;
            }
            // Prepend any un-deferred alias sub-commands (should already be executed unless wait happened).
            while let Some(item) = deferred.pop_back() {
                pending.push_front(item);
            }
        }
    }

    /// Format console log lines for `condump`.
    pub fn format_console_dump(&self) -> String {
        let mut out = String::new();
        for line in &self.log {
            out.push_str(&line.text);
            out.push('\n');
        }
        out
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
                let mut deferred = VecDeque::new();
                let _ = self.execute_tokens_deferrable(&tokens, 0, &mut actions, &mut deferred);
            }
            actions
        }
    }

    fn execute_tokens_deferrable(
        &mut self,
        tokens: &[String],
        depth: usize,
        actions: &mut Vec<EngineAction>,
        deferred: &mut VecDeque<(Vec<String>, usize)>,
    ) -> Option<u32> {
        if tokens.is_empty() {
            return None;
        }
        if depth > 16 {
            self.warn("Alias recursion limit reached");
            return None;
        }

        let cmd = tokens[0].to_ascii_lowercase();
        let args = &tokens[1..];

        // Check aliases first
        if let Some(body) = self.aliases.get(&cmd).cloned() {
            let subs = parser::parse_commands(&body);
            for (idx, sub) in subs.iter().enumerate() {
                let mut sub_deferred = VecDeque::new();
                if let Some(wait_n) =
                    self.execute_tokens_deferrable(sub, depth + 1, actions, &mut sub_deferred)
                {
                    for item in sub_deferred {
                        deferred.push_back(item);
                    }
                    for rem in &subs[idx + 1..] {
                        deferred.push_back((rem.clone(), depth + 1));
                    }
                    return Some(wait_n);
                }
            }
            return None;
        }

        // `+action` / `-action` input commands
        if let Some(rest) = cmd.strip_prefix('+') {
            actions.push(EngineAction::ActionInput {
                name: rest.to_string(),
                pressed: true,
            });
            return None;
        }
        if let Some(rest) = cmd.strip_prefix('-') {
            actions.push(EngineAction::ActionInput {
                name: rest.to_string(),
                pressed: false,
            });
            return None;
        }

        if cmd == "wait" {
            let frames = args
                .first()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(1)
                .clamp(1, 600) as u32;
            return Some(frames);
        }

        self.execute_tokens(tokens, depth, actions);
        None
    }

    fn execute_tokens(&mut self, tokens: &[String], _depth: usize, actions: &mut Vec<EngineAction>) {
        if tokens.is_empty() {
            return;
        }
        let cmd = tokens[0].to_ascii_lowercase();
        let args = &tokens[1..];

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
            "bindtoggle" => {
                if args.len() >= 2 {
                    let key = &args[0];
                    let var = args[1].to_ascii_lowercase();
                    self.bind(key, &format!("toggle {var}"));
                } else {
                    self.info("Usage: bindtoggle <key> <cvar>");
                }
            }
            "incrementvar" => {
                if args.len() < 4 {
                    self.info("Usage: incrementvar <cvar> <minValue> <maxValue> <delta>");
                    return;
                }
                let key = args[0].to_ascii_lowercase();
                let (Ok(min_v), Ok(max_v), Ok(delta)) = (
                    args[1].parse::<f64>(),
                    args[2].parse::<f64>(),
                    args[3].parse::<f64>(),
                ) else {
                    self.warn("incrementvar: invalid numeric arguments");
                    return;
                };
                if !self.vars.contains_key(&key) {
                    self.warn(format!("Unknown cvar \"{key}\""));
                    return;
                }
                let cur = f64::from(self.get_f32(&key));
                let mut next = cur + delta;
                if next > max_v {
                    next = min_v;
                } else if next < min_v {
                    next = max_v;
                }
                let formatted = format_cvar_number(next);
                self.set(&key, formatted);
            }
            "multvar" => {
                if args.len() < 4 {
                    self.info("Usage: multvar <cvar> <minValue> <maxValue> <factor>");
                    return;
                }
                let key = args[0].to_ascii_lowercase();
                let (Ok(min_v), Ok(max_v), Ok(factor)) = (
                    args[1].parse::<f64>(),
                    args[2].parse::<f64>(),
                    args[3].parse::<f64>(),
                ) else {
                    self.warn("multvar: invalid numeric arguments");
                    return;
                };
                if !self.vars.contains_key(&key) {
                    self.warn(format!("Unknown cvar \"{key}\""));
                    return;
                }
                let cur = f64::from(self.get_f32(&key));
                let next = (cur * factor).clamp(min_v.min(max_v), min_v.max(max_v));
                let formatted = format_cvar_number(next);
                self.set(&key, formatted);
            }
            "condump" => {
                actions.push(EngineAction::ConDump(args.first().cloned()));
            }
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
            "retry" => actions.push(EngineAction::Retry),
            "reconnect" => actions.push(EngineAction::Reconnect),
            "restart" | "_restart" => actions.push(EngineAction::RestartMap),
            "shutdownserver" => actions.push(EngineAction::ShutdownServer),
            "fullupdate" => actions.push(EngineAction::FullUpdate),
            "dlfile" => {
                if let Some(res) = args.first() {
                    actions.push(EngineAction::DlFile(res.clone()));
                } else {
                    self.info("Usage: dlfile <resource>");
                }
            }
            "showbriefing" => actions.push(EngineAction::ShowBriefing),
            "demos" | "startdemos" | "lastdemo" => {
                actions.push(EngineAction::Demos {
                    kind: cmd,
                    args: args.to_vec(),
                });
            }
            "displaysoundlist" => actions.push(EngineAction::DisplaySoundList),
            "mcache" => actions.push(EngineAction::MCache),
            "delta_stats" => actions.push(EngineAction::DeltaStats),
            "delta_clear" => actions.push(EngineAction::DeltaClear),
            "quit" | "exit" => actions.push(EngineAction::Quit),
            "status" => actions.push(EngineAction::Status),
            "force_centerview" | "centerview" => actions.push(EngineAction::ForceCenterView),
            "noclip" => actions.push(EngineAction::ToggleNoclip),
            "notarget" | "fly" | "rethrow" | "sv_rethrow_last_grenade" | "give_all"
            | "ammo_full" | "cp" | "tp" | "gocheck" | "prevcp" | "stuck" | "start"
            | "reset" | "autobuy" | "rebuy" | "setinfo" | "fullinfo" | "new" | "sendres"
            | "spawn" | "sendents" => {
                if self.forward_unknown {
                    actions.push(EngineAction::Forward(tokens.to_vec()));
                } else if matches!(
                    cmd.as_str(),
                    "notarget"
                        | "fly"
                        | "rethrow"
                        | "sv_rethrow_last_grenade"
                        | "give_all"
                        | "ammo_full"
                        | "cp"
                        | "tp"
                        | "gocheck"
                        | "prevcp"
                        | "stuck"
                        | "start"
                        | "reset"
                ) {
                    self.warn(format!("\"{cmd}\" requires an active game session"));
                }
            }
            "maps" | "version" | "ping" | "stats" | "stat" | "users" | "user" | "serverinfo"
            | "localinfo" | "showinfo" | "dropclient" | "kick" | "banid" | "removeid"
            | "listid" | "writeid" | "addip" | "removeip" | "listip" | "writeip" | "log"
            | "logaddress" | "logaddress_add" | "logaddress_del" | "pause" | "setpause"
            | "unpause" | "resetrcon" | "changelevel2" | "motd" | "motd_write" | "tell"
            | "hostage_stop" | "lo3" | "mp_live_on_three" => {
                actions.push(EngineAction::ServerCmd(tokens.to_vec()));
            }
            "demo_timescale" => {
                if let Some(raw) = args.first()
                    && let Ok(val) = raw.parse::<f32>()
                    && val.is_finite()
                {
                    actions.push(EngineAction::DemoTimescale(val.clamp(0.1, 4.0)));
                } else {
                    self.info("Usage: demo_timescale <0.1..4.0>");
                }
            }
            "demo_pause" => actions.push(EngineAction::DemoPause),
            "demo_resume" => actions.push(EngineAction::DemoResume),
            "demo_togglepause" => actions.push(EngineAction::DemoTogglePause),
            "demo_goto" => {
                if let Some(raw) = args.first()
                    && let Ok(secs) = raw.parse::<f64>()
                    && secs.is_finite()
                {
                    actions.push(EngineAction::DemoGoto(secs.max(0.0)));
                } else {
                    self.info("Usage: demo_goto <seconds>");
                }
            }
            "voice_scale_player" => {
                if args.len() >= 2
                    && let (Ok(slot), Ok(scale)) =
                        (args[0].parse::<u8>(), args[1].parse::<f32>())
                    && slot < 32
                    && scale.is_finite()
                {
                    actions.push(EngineAction::VoiceScalePlayer {
                        slot,
                        scale: scale.clamp(0.0, 2.0),
                    });
                } else {
                    self.info("Usage: voice_scale_player <slot 0..31> <0.0..2.0>");
                }
            }
            "cfg_diff" => {
                let diffs: Vec<(String, String, String)> = self
                    .diff_default_cvars()
                    .into_iter()
                    .map(|(n, v, d)| (n.to_string(), v.to_string(), d.to_string()))
                    .collect();
                if diffs.is_empty() {
                    self.info("All CVars match vanilla CS 1.6 defaults.");
                } else {
                    let count = diffs.len();
                    for (name, value, def) in diffs {
                        self.info(format!("  {name} = \"{value}\" (default: \"{def}\")"));
                    }
                    self.info(format!("{count} modified CVars"));
                }
            }
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
            "screenshot" | "snapshot" => {
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
            "invnext" => actions.push(EngineAction::InvNext),
            "invprev" => actions.push(EngineAction::InvPrev),
            "slot1" => actions.push(EngineAction::SelectSlot(1)),
            "slot2" => actions.push(EngineAction::SelectSlot(2)),
            "slot3" => actions.push(EngineAction::SelectSlot(3)),
            "slot4" => actions.push(EngineAction::SelectSlot(4)),
            "slot5" => actions.push(EngineAction::SelectSlot(5)),
            "slot6" => actions.push(EngineAction::SelectSlot(6)),
            "slot7" => actions.push(EngineAction::SelectSlot(7)),
            "slot8" => actions.push(EngineAction::SelectSlot(8)),
            "slot9" => actions.push(EngineAction::SelectSlot(9)),
            "slot10" => actions.push(EngineAction::SelectSlot(10)),
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
            "adjust_crosshair" => {
                let idx = args.first().and_then(|s| s.parse::<u8>().ok());
                actions.push(EngineAction::AdjustCrosshair(idx));
            }
            "togglescores" => actions.push(EngineAction::ToggleScores),
            "playerlist" => actions.push(EngineAction::OpenPlayerList),
            "viewdemo" | "playdemo" => {
                actions.push(EngineAction::OpenDemoPlayer(args.first().cloned()));
                if cmd == "playdemo"
                    && let Some(path) = args.first()
                {
                    actions.push(EngineAction::PlayDemo(path.clone()));
                }
            }
            "record" => {
                if let Some(name) = args.first() {
                    actions.push(EngineAction::RecordDemo(name.clone()));
                } else {
                    self.info("Usage: record <demoname>");
                }
            }
            "stop" | "stopdemo" => actions.push(EngineAction::StopDemo),
            "timedemo" => {
                if let Some(name) = args.first() {
                    actions.push(EngineAction::TimeDemo(name.clone()));
                } else {
                    self.info("Usage: timedemo <demoname>");
                }
            }
            "setmaster" => {
                actions.push(EngineAction::SetMaster(args.to_vec()));
            }
            "spec_mode" => {
                let mode = args.first().and_then(|s| s.parse::<i32>().ok()).unwrap_or(-1);
                actions.push(EngineAction::SpecMode(mode));
            }
            "spec_menu" => {
                let vis = args.first().and_then(|s| s.parse::<i32>().ok()).unwrap_or(1);
                actions.push(EngineAction::SpecMenu(vis));
            }
            "toggleconsole" => actions.push(EngineAction::ToggleConsole),
            "cancelselect" => actions.push(EngineAction::ToggleMenu),
            "heartbeat" | "reload" | "soundfade" | "key_rotate" | "viewframe" | "viewmodel"
            | "viewnext" | "viewprev" | "_set_vid_level" | "_setaddons_folder"
            | "_setgamedir" | "_sethdmodels" | "_setrenderer" | "_setvideomode" => {
                // Handled/accepted legacy and compatibility commands (no unknown-command warning).
            }
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

fn format_cvar_number(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    if (v - v.round()).abs() < 1e-6 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
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

    #[test]
    fn phase11_cvar_flags_protection_and_actions() {
        let mut reg = CVarRegistry::new_with_defaults();
        reg.register("sv_test_sp", "0", CVarFlags::SPONLY, "");
        reg.register("sv_test_cheat", "0", CVarFlags::CHEAT, "");
        reg.register("sv_test_print", "a", CVarFlags::PRINTABLEONLY, "");
        reg.register("sv_test_trim", "a", CVarFlags::NOEXTRAWHITESPACE, "");
        reg.register("sv_test_secret", "hunter2", CVarFlags::PROTECTED, "");

        let mp_no_cheats = CvarSetContext {
            trusted: false,
            multiplayer: true,
            cheats_enabled: false,
        };
        assert!(reg.set_with_context("sv_test_sp", "1", mp_no_cheats).is_err());
        assert!(reg.set_with_context("sv_test_cheat", "1", mp_no_cheats).is_err());
        assert!(reg
            .set_with_context(
                "sv_test_cheat",
                "1",
                CvarSetContext {
                    trusted: false,
                    multiplayer: true,
                    cheats_enabled: true,
                },
            )
            .unwrap());
        assert_eq!(reg.get_str("sv_test_cheat"), "1");

        reg.set_with_context("sv_test_print", "hi\x07\x7F!", mp_no_cheats)
            .unwrap();
        assert_eq!(reg.get_str("sv_test_print"), "hi!");
        reg.set_with_context("sv_test_trim", "  padded  ", mp_no_cheats)
            .unwrap();
        assert_eq!(reg.get_str("sv_test_trim"), "padded");

        assert_eq!(
            reg.vars().get("sv_test_secret").unwrap().public_rules_value(),
            "1"
        );
        reg.set("sv_test_secret", "");
        assert_eq!(
            reg.vars().get("sv_test_secret").unwrap().public_rules_value(),
            "0"
        );

        assert_eq!(
            reg.execute_script("record mydemo"),
            vec![EngineAction::RecordDemo("mydemo".into())]
        );
        assert_eq!(reg.execute_script("stop"), vec![EngineAction::StopDemo]);
        assert_eq!(reg.execute_script("stopdemo"), vec![EngineAction::StopDemo]);
        assert_eq!(
            reg.execute_script("timedemo bench"),
            vec![EngineAction::TimeDemo("bench".into())]
        );
        assert_eq!(
            reg.execute_script("setmaster add 127.0.0.1:27010"),
            vec![EngineAction::SetMaster(vec![
                "add".into(),
                "127.0.0.1:27010".into(),
            ])]
        );
    }

    #[test]
    fn default_wheel_binds_and_inventory_commands_are_registered() {
        let mut reg = CVarRegistry::new_with_defaults();
        assert_eq!(reg.bind_for_key("mwheelup"), Some("invprev"));
        assert_eq!(reg.bind_for_key("mwheeldown"), Some("invnext"));
        assert_eq!(reg.bind_for_key("mouse1"), Some("+attack"));
        assert_eq!(reg.bind_for_key("mouse2"), Some("+attack2"));

        reg.set_forward_unknown(true);
        assert_eq!(
            reg.handle_key_bind("mwheelup", true),
            vec![EngineAction::InvPrev]
        );
        assert!(reg.handle_key_bind("mwheelup", false).is_empty());
        assert_eq!(
            reg.handle_key_bind("mwheeldown", true),
            vec![EngineAction::InvNext]
        );
        assert!(reg.handle_key_bind("mwheeldown", false).is_empty());
    }

    #[test]
    fn wait_defers_remaining_script_commands_and_guards_overflow() {
        let mut reg = CVarRegistry::new_with_defaults();
        let out = reg.execute_script("echo a; wait; echo b; wait 2; echo c");
        assert!(out.is_empty());
        assert_eq!(reg.pending_wait_count(), 3);
        assert!(reg.log_lines().iter().any(|l| l.text == "a"));
        assert!(!reg.log_lines().iter().any(|l| l.text == "b"));

        // Frame 1: `echo b` runs and stops at `wait 2`.
        assert!(reg.tick_wait_queue().is_empty());
        assert!(reg.log_lines().iter().any(|l| l.text == "b"));
        assert!(!reg.log_lines().iter().any(|l| l.text == "c"));

        // Frame 2: second wait decrements (still 1 frame left).
        assert!(reg.tick_wait_queue().is_empty());
        assert!(!reg.log_lines().iter().any(|l| l.text == "c"));

        // Frame 3: `echo c` executes.
        assert!(reg.tick_wait_queue().is_empty());
        assert!(reg.log_lines().iter().any(|l| l.text == "c"));
        assert_eq!(reg.pending_wait_count(), 0);

        // Alias containing `wait` defers remaining alias commands as well.
        reg.execute_script("alias step_test \"echo s1; wait; echo s2\"; step_test");
        assert!(reg.log_lines().iter().any(|l| l.text == "s1"));
        assert!(!reg.log_lines().iter().any(|l| l.text == "s2"));
        let _ = reg.tick_wait_queue();
        assert!(reg.log_lines().iter().any(|l| l.text == "s2"));

        // Bounded queue guard prevents runaway scripts from hanging or OOM.
        let flood = "wait; ".repeat(MAX_WAIT_QUEUE + 50);
        let _ = reg.execute_script(&flood);
        assert!(reg.pending_wait_count() <= MAX_WAIT_QUEUE);
    }

    #[test]
    fn bindtoggle_incrementvar_multvar_condump_userinfo_and_classic_defaults() {
        let mut reg = CVarRegistry::new_with_defaults();
        assert_eq!(reg.get_str("s_occfactor"), "0");
        assert_eq!(reg.get_str("snd_occlusion"), "0");
        assert_eq!(reg.get_str("cl_hd_models"), "0");
        assert_eq!(reg.get_str("room_type"), "0");
        assert_eq!(reg.get_str("room_off"), "0");
        assert_eq!(reg.get_str("s_lerping"), "1");
        assert_eq!(reg.get_str("hisound"), "1");
        assert_eq!(reg.get_str("cl_yawspeed"), "210");
        assert_eq!(reg.get_str("cl_pitchspeed"), "225");
        assert_eq!(reg.get_str("cl_anglespeedkey"), "0.67");

        reg.set("crosshair", "1");
        let _ = reg.execute_script("bindtoggle x crosshair");
        assert_eq!(reg.bind_for_key("x"), Some("toggle crosshair"));
        let _ = reg.execute_script("incrementvar crosshair 0 1 1");
        assert_eq!(reg.get_str("crosshair"), "0");
        let _ = reg.execute_script("incrementvar crosshair 0 1 1");
        assert_eq!(reg.get_str("crosshair"), "1");

        reg.set("sensitivity", "2");
        let _ = reg.execute_script("multvar sensitivity 1 5 1.5");
        assert_eq!(reg.get_str("sensitivity"), "3");
        let _ = reg.execute_script("multvar sensitivity 1 5 3");
        assert_eq!(reg.get_str("sensitivity"), "5");

        assert_eq!(
            reg.execute_script("condump mylog.txt"),
            vec![EngineAction::ConDump(Some("mylog.txt".into()))]
        );
        let _ = reg.execute_console_line("echo dump_probe");
        assert!(reg.format_console_dump().contains("dump_probe"));

        let _ = reg.take_dirty_userinfo();
        reg.set("cl_autowepswitch", "0");
        let dirty = reg.take_dirty_userinfo();
        assert!(dirty.iter().any(|(k, v)| k == "_cl_autowepswitch" && v == "0"));
        assert_eq!(reg.get_str("_cl_autowepswitch"), "0");
    }

    #[test]
    fn task9_cfg_diff_and_demo_voice_commands() {
        let mut reg = CVarRegistry::new_with_defaults();
        assert!(reg.diff_default_cvars().is_empty());

        for name in [
            "sv_showimpacts",
            "sv_grenade_trajectory",
            "sv_showhitboxes",
            "mp_buy_anywhere",
            "mp_warmup_mode",
            "cl_showspeed",
            "cl_showkeys",
            "cl_crosshair_dot",
            "cl_crosshair_outline",
            "cl_crosshair_t",
            "cl_crosshair_gap",
            "cl_mute_enemy_team",
            "sv_damage_print",
            "mp_knife_round",
        ] {
            assert_eq!(reg.get_str(name), "0", "{name} must default to 0");
        }
        assert_eq!(reg.get_str("sv_grenade_trajectory_time"), "8");

        reg.set("cl_showspeed", "1");
        reg.set("sensitivity", "2.5");
        let diffs = reg.diff_default_cvars();
        assert_eq!(diffs.len(), 2);
        assert!(diffs.iter().any(|(n, v, d)| *n == "cl_showspeed" && *v == "1" && *d == "0"));
        assert!(diffs.iter().any(|(n, v, d)| *n == "sensitivity" && *v == "2.5" && *d == "3.0"));
        let _ = reg.execute_script("cfg_diff");
        assert!(reg.log_lines().iter().any(|l| l.text.contains("2 modified CVars")));

        assert_eq!(
            reg.execute_script("demo_timescale 0.5; demo_pause; demo_resume; demo_togglepause; demo_goto 12.5"),
            vec![
                EngineAction::DemoTimescale(0.5),
                EngineAction::DemoPause,
                EngineAction::DemoResume,
                EngineAction::DemoTogglePause,
                EngineAction::DemoGoto(12.5),
            ]
        );
        assert_eq!(
            reg.execute_script("voice_scale_player 3 0.75"),
            vec![EngineAction::VoiceScalePlayer { slot: 3, scale: 0.75 }]
        );
    }
}
