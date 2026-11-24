//! The client's connection to a server: the connectionless handshake, the netchan, the sign-on
//! sequence and the commands the client sends.
//!
//! The state machine is `cs-probe`'s `ProbeClient` (it reaches `InGame` on stock HLDS build
//! 10210 and on `cs-engine`), moved here and extended with what a real client does and the
//! probe skipped: resource tables, file consistency, downloads, stufftext execution with
//! Xash's filter, cvar-driven userinfo and rates, `svc_sendcvarvalue` answers. The sequence is
//! checked against Xash3D FWGS `engine/client/cl_main.c` and `cl_parse.c` (read 2026-11-23):
//!
//! ```text
//! getchallenge -> challenge -> connect -> connection
//!   -> "new" -> svc_serverinfo -> "sendres" -> svc_resourcerequest (clc_resourcelist)
//!   -> svc_resourcelist -> [dlfile ..., clc_fileconsistency] -> "spawn <count> <crc>"
//!   -> svc_signonnum 1 -> "sendents" -> first svc_packetentities (InGame)
//! ```
//!
//! The core never reads a clock: the caller passes `now` to [`Connection::pump`].

use std::collections::{BTreeMap, VecDeque};

use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::crypto::com_munge2;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::handshake::{
    OobPacket, default_cs16_protinfo, default_cs16_userinfo, encode_connect_48,
    encode_getchallenge, lan_steam_auth_ticket, parse_oob_packet,
};
use cs_proto48::info::InfoString;
use cs_proto48::messages::{
    MoveVarsMsg, PacketEntitiesMsg, ServerInfoMsg, SvcMessage, SvcParseContext, WeaponRecords, clc,
    clc_move_body_len, entity_schema_name, parse_svc_stream_partial, write_clc_cvarvalue2,
    write_clc_delta, write_clc_fileconsistency, write_clc_move, write_clc_resourcelist,
    write_clc_stringcmd,
};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};
use cs_transport::{PacketTransport, PeerAddr};

use crate::commands::{CommandRegistry, ConsoleCommand, split_lines, tokenize};
use crate::cvars::{CVAR_PRIVILEGED, CVAR_PROTECTED, CVAR_USERINFO, ClientCvars};
use crate::frames::{ClientFrames, FrameOutcome};
use crate::events::{FiredEvent, FxEnv};
use crate::hud::ClientEffects;
use crate::precache::PrecacheTables;
use crate::predict::{self, PredictInputs, Predictor};
use crate::resources::{AllFilesPresent, ResourceFiles, ResourceManager, parse_file_stream};
use crate::tempents::{FxWorld, NoWorld};

/// Reliable bytes the connection lets pile up unacknowledged before it gives the server up:
/// stuffed commands forwarded to a server that does not answer, mostly.
pub const MAX_RELIABLE_BACKLOG: usize = 64 * 1024;
/// Console commands waiting for the shell before the connection gives up on it.
pub const MAX_SHELL_COMMANDS: usize = 1024;
/// Opcodes [`Connection::opcode_log`] keeps at least (it drops the oldest half past twice this).
pub const OPCODE_LOG_CAPACITY: usize = 65536;
/// Seconds a sign-on may go without the server taking a step (server info, resource list,
/// sign-on number, a file) before the client leaves; any datagram counts while a download
/// is pending.
pub const SIGNON_TIMEOUT: f64 = 30.0;
/// Seconds the downloads of one sign-on may take in all, however many bytes keep coming.
pub const DOWNLOAD_TIMEOUT: f64 = 300.0;
/// A connectionless handshake packet is resent when no answer arrived within this time.
const HANDSHAKE_RESEND: f64 = 1.0;
/// Previously sent commands repeated in every `clc_move` as backups.
const MAX_BACKUP_CMDS: usize = 2;
/// Queued commands sent as new commands in one packet; the rest wait for the next packet, so
/// the `clc_move` body stays far below its one-byte length limit.
const MAX_NEW_CMDS: usize = 16;
/// `cl_cmdrate` bounds (`CL_WritePacket`).
pub const MIN_CMDRATE: f32 = 10.0;
pub const MAX_CMDRATE: f32 = 100.0;
/// Reason given when the server demands file consistency checks and the policy refuses them.
pub const CONSISTENCY_REASON: &str = "consistency required: set mp_consistency 0";
/// Messages [`Connection::messages`] keeps.
pub const MESSAGE_LOG_CAPACITY: usize = 1024;
/// Console lines [`Connection::console_log`] keeps.
pub const CONSOLE_LOG_CAPACITY: usize = 256;
/// Draining console buffer capacity ([`Connection::take_console_lines`]).
const PENDING_CONSOLE_CAPACITY: usize = 1024;
/// Reason given when `svc_resourcelist` arrives before `svc_serverinfo`.
pub const NO_SERVERINFO_REASON: &str = "resourcelist before serverinfo";

/// A view-angle change forced by the server (`svc_setangle` / `svc_addangle`) for the shell camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewAngleUpdate {
    Set(glam::Vec3),
    AddYaw(f32),
}

/// Identity and settings a connection starts with.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub name: String,
    pub steam_id: u64,
    pub cdkey_md5_hex: String,
    pub cvars: ClientCvars,
    pub consistency: ConsistencyPolicy,
    /// Send the server a stuffed command the client does not know (`Cmd_ForwardToServer`), as
    /// the original client does: a server that stuffs `kill` makes the player kill itself. A
    /// tool that only observes (the probe) turns it off.
    pub forward_stufftext: bool,
}

impl ConnectionConfig {
    pub fn new(name: &str, steam_id: u64, cdkey_md5_hex: &str) -> Self {
        Self {
            name: name.to_string(),
            steam_id,
            cdkey_md5_hex: cdkey_md5_hex.to_string(),
            cvars: ClientCvars::new(),
            consistency: ConsistencyPolicy::Answer,
            forward_stufftext: true,
        }
    }
}

/// What to do when the server asks for file consistency checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsistencyPolicy {
    /// Answer with `clc_fileconsistency`, as the original does.
    Answer,
    /// Leave the server ("consistency required"): for a client with no game files to hash.
    Refuse,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnState {
    Idle,
    AwaitChallenge,
    AwaitConnection,
    Signon,
    InGame,
    Rejected(String),
    Disconnected(String),
}

impl ConnState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, ConnState::Rejected(_) | ConnState::Disconnected(_))
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct ConnStats {
    pub datagrams_in: u64,
    pub bytes_in: u64,
    pub largest_datagram: usize,
    pub full_updates: u64,
    pub delta_updates: u64,
    /// Entity updates received during the last second of the caller's clock.
    pub frames_in_last_second: u32,
    /// Size in bytes of the most recent datagram received from the server.
    pub last_datagram: usize,
    /// Datagrams from the server that could not be parsed (or were not understood).
    pub parse_errors: u64,
}

/// Records every datagram of a connection (the probe's capture files).
pub trait DatagramTap: Send {
    /// `sent` is `true` for a datagram going to the server. An error ends the recording.
    fn record(&mut self, sent: bool, time: f64, data: &[u8]) -> std::io::Result<()>;
}

/// Client connection over any [`PacketTransport`].
pub struct Connection<T: PacketTransport> {
    transport: T,
    server: PeerAddr,
    cfg: ConnectionConfig,
    cvars: ClientCvars,
    commands: CommandRegistry,
    userinfo: InfoString,
    state: ConnState,
    signon_num: u8,
    chan: Option<NetChan>,
    ctx: SvcParseContext,
    splits: SplitPacketAssembler,
    challenge: u32,
    /// When the current connectionless handshake packet was last sent; `None` sends it on the
    /// next pump.
    handshake_sent_at: Option<f64>,
    /// Earliest time the next netchan packet may go out.
    next_send_at: f64,
    server_info: Option<ServerInfoMsg>,
    entities: BTreeMap<u16, GoldSrcEntityState>,
    /// Task 4 hook: frame ring, entity store and interpolation (`frames.rs`).
    frames: ClientFrames,
    /// The movement prediction of the local player (`predict.rs`).
    predict: Predictor,
    client_data: Option<GoldSrcClientData>,
    /// The `weapon_data_t` list of the last `svc_clientdata` received.
    weapon_records: WeaponRecords,
    userinfos: BTreeMap<u8, String>,
    movevars: Option<MoveVarsMsg>,
    opcode_log: Vec<u8>,
    /// The last [`MESSAGE_LOG_CAPACITY`] messages received, without the per-update ones.
    messages: VecDeque<SvcMessage>,
    stats: ConnStats,
    /// Arrival times of entity updates within the last second.
    frame_times: VecDeque<f64>,
    cmd_queue: VecDeque<GoldSrcUserCmd>,
    /// Last sent commands, newest first, repeated as backups.
    sent_cmds: VecDeque<GoldSrcUserCmd>,
    /// Most recent command taken from the queue; repeated with `msec = 0` when the queue is empty.
    last_cmd: GoldSrcUserCmd,
    /// Low byte of the netchan sequence of the last entity update.
    last_entity_seq: Option<u8>,
    files: Box<dyn ResourceFiles>,
    resources: ResourceManager,
    precache: PrecacheTables,
    /// Events, temp entities, sounds and the HUD state the svc stream builds (task 6).
    fx: ClientEffects,
    /// What temp entities and events see of the world and the local weapon; the shell sets it.
    fx_local: FxLocal,
    fx_world: Box<dyn FxWorld>,
    /// The resource list arrived and a download or the `spawn` still waits.
    awaiting_resources: bool,
    console: VecDeque<String>,
    pending_console: VecDeque<String>,
    shell_commands: VecDeque<ConsoleCommand>,
    view_angle_updates: Vec<ViewAngleUpdate>,
    /// When the sign-on last advanced; `None` until the next pump stamps it.
    signon_progress: Option<f64>,
    /// When the downloads of this sign-on started waiting; `None` when none wait.
    download_started: Option<f64>,
    /// Bytes the netchan had buffered of unfinished fragment streams at the last packet.
    buffered_fragments: usize,
    full_server_info: String,
    tap: Option<Box<dyn DatagramTap>>,
    tap_error: Option<std::io::Error>,
    forced_delta_seq: Option<u8>,
    /// Test hook: `clc_delta` is left out of outgoing packets.
    delta_suppressed: bool,
    /// Test hook: the reliable acknowledgement bit outgoing packets carry instead of the
    /// channel's own.
    withheld_reliable_ack: Option<u32>,
    /// Outgoing `clc_move` messages still to be sent with a corrupted checksum (test hook).
    corrupt_moves: u32,
    /// Test hook: no `clc_move` in outgoing packets, not even the `msec` 0 placeholder.
    moves_suppressed: bool,
}

impl<T: PacketTransport> Connection<T> {
    pub fn new(transport: T, server: PeerAddr, mut cfg: ConnectionConfig) -> Self {
        cfg.cvars.set("name", &cfg.name.clone());
        let cvars = cfg.cvars.clone();
        let mut conn = Self {
            transport,
            server,
            cfg,
            cvars,
            commands: CommandRegistry::new(),
            userinfo: InfoString::new(),
            state: ConnState::Idle,
            signon_num: 0,
            chan: None,
            ctx: SvcParseContext::new(),
            splits: SplitPacketAssembler::default(),
            challenge: 0,
            handshake_sent_at: None,
            next_send_at: f64::NEG_INFINITY,
            server_info: None,
            entities: BTreeMap::new(),
            frames: ClientFrames::new(),
            predict: Predictor::new(),
            client_data: None,
            weapon_records: WeaponRecords::new(),
            userinfos: BTreeMap::new(),
            movevars: None,
            opcode_log: Vec::new(),
            messages: VecDeque::new(),
            stats: ConnStats::default(),
            frame_times: VecDeque::new(),
            cmd_queue: VecDeque::new(),
            sent_cmds: VecDeque::new(),
            last_cmd: GoldSrcUserCmd::default(),
            last_entity_seq: None,
            files: Box::new(AllFilesPresent),
            resources: ResourceManager::default(),
            precache: PrecacheTables::default(),
            fx: ClientEffects::default(),
            fx_local: FxLocal::default(),
            fx_world: Box::new(NoWorld),
            awaiting_resources: false,
            console: VecDeque::new(),
            pending_console: VecDeque::new(),
            shell_commands: VecDeque::new(),
            view_angle_updates: Vec::new(),
            signon_progress: None,
            download_started: None,
            buffered_fragments: 0,
            full_server_info: String::new(),
            tap: None,
            tap_error: None,
            forced_delta_seq: None,
            delta_suppressed: false,
            withheld_reliable_ack: None,
            corrupt_moves: 0,
            moves_suppressed: false,
        };
        conn.userinfo = conn.base_userinfo();
        conn
    }

    /// The `connect` userinfo: the CS 1.6 defaults, then every userinfo cvar.
    fn base_userinfo(&self) -> InfoString {
        let mut userinfo = default_cs16_userinfo(&self.cfg.name);
        for (key, value) in self.cvars.userinfo_pairs() {
            userinfo.set(key, value);
        }
        userinfo
    }

    /// Sets up a connectionless `getchallenge` on the next [`pump`](Self::pump); only resets
    /// the handshake state, not a previously active session.
    pub fn connect(&mut self) {
        self.state = ConnState::AwaitChallenge;
        self.handshake_sent_at = None;
    }

    /// Receives everything waiting, advances the state, then sends: a connectionless handshake
    /// packet when one is due, or one netchan packet when `now` is past the `cl_cmdrate`
    /// interval since the previous one.
    pub fn pump(&mut self, now: f64) {
        // The clock the prediction stamps new commands with.
        self.predict.set_now(now);
        while let Some((from, data)) = self.transport.recv() {
            if from != self.server {
                continue;
            }
            self.record(false, now, &data);
            self.stats.datagrams_in += 1;
            self.stats.bytes_in += data.len() as u64;
            self.stats.largest_datagram = self.stats.largest_datagram.max(data.len());
            self.stats.last_datagram = data.len();
            self.handle_datagram(&data, now);
        }

        while self.frame_times.front().is_some_and(|&t| t <= now - 1.0) {
            self.frame_times.pop_front();
        }
        self.stats.frames_in_last_second = self.frame_times.len() as u32;

        match self.state {
            ConnState::AwaitChallenge | ConnState::AwaitConnection => {
                if self
                    .handshake_sent_at
                    .is_none_or(|t| now - t >= HANDSHAKE_RESEND)
                {
                    let packet = if self.state == ConnState::AwaitChallenge {
                        encode_getchallenge(true)
                    } else {
                        encode_connect_48(
                            self.challenge,
                            &default_cs16_protinfo(&self.cfg.cdkey_md5_hex),
                            &self.userinfo,
                            &lan_steam_auth_ticket(self.cfg.steam_id),
                        )
                    };
                    self.send(&packet, now);
                    self.handshake_sent_at = Some(now);
                }
            }
            ConnState::Signon | ConnState::InGame => {
                if self.state == ConnState::Signon {
                    if self
                        .download_started
                        .is_some_and(|t| self.awaiting_resources && now - t > DOWNLOAD_TIMEOUT)
                    {
                        self.disconnect_with(format!(
                            "downloads did not finish within {DOWNLOAD_TIMEOUT} seconds"
                        ));
                        return;
                    }
                    let since = *self.signon_progress.get_or_insert(now);
                    if now - since > SIGNON_TIMEOUT {
                        self.disconnect_with(format!(
                            "signon timed out after {SIGNON_TIMEOUT} seconds without progress"
                        ));
                        return;
                    }
                }
                if now >= self.next_send_at {
                    self.send_netchan_packet(now);
                    self.next_send_at = now + self.command_interval();
                }
            }
            ConnState::Idle | ConnState::Rejected(_) | ConnState::Disconnected(_) => {}
        }
        // Replay the commands the server has not answered.
        self.run_prediction(now);
    }

    /// `CL_PredictMovement`, once per pump, after the packets were read and the commands sent.
    fn run_prediction(&mut self, now: f64) {
        if self.state != ConnState::InGame {
            return;
        }
        let Some(chan) = self.chan.as_ref() else {
            return;
        };
        let (ack, outgoing) = (chan.incoming_acknowledged, chan.outgoing_sequence);
        self.predict.apply_cvars(&self.cvars);
        let poses = predict::player_poses(&mut self.frames);
        let movevars = self
            .movevars
            .as_ref()
            .map(predict::movevars_from_msg)
            .unwrap_or_default();
        let frames = &self.frames;
        let inputs = PredictInputs {
            now,
            incoming_acknowledged: ack,
            outgoing_sequence: outgoing,
            frame: if frames.valid_sequence != 0 {
                frames.current_frame()
            } else {
                None
            },
            playernum: frames.entities.playernum,
            max_clients: frames.entities.max_clients,
            precache: &self.precache,
            movevars,
            max_velocity: frames.entities.max_velocity,
            poses: &poses,
            entities: Some(&frames.entities),
            remote_is_local: frames.settings.remote_is_local,
            intermission: false,
        };
        self.predict.update(&inputs);
        if self.predict.cl_lw && self.predict.prediction().predicted {
            self.fx_local.weapon_state = self.predict.predicted_weapon_state();
            self.fx_local.holding_shield = self.predict.predicted_holding_shield();
        }
        let local_ent = self
            .server_info
            .as_ref()
            .map_or(0, |i| i32::from(i.player_slot) + 1);
        for mut ev in self.predict.take_weapon_events() {
            if let Some(idx) = self.precache.event_index(ev.event_name) {
                if ev.args.entindex == 0 {
                    ev.args.entindex = local_ent;
                }
                self.fx.events.push_local(idx, ev.args);
            }
        }
    }

    /// Seconds between two netchan packets: `1 / cl_cmdrate`, with the cvar clamped to 10..100
    /// and written back as `CL_WritePacket` does (Xash `cl_main.c`, read 2026-11-23).
    fn command_interval(&mut self) -> f64 {
        let rate = self.cvars.float("cl_cmdrate");
        if rate < MIN_CMDRATE {
            self.cvars.set("cl_cmdrate", "10");
        } else if rate > MAX_CMDRATE {
            self.cvars.set("cl_cmdrate", "100");
        }
        1.0 / f64::from(self.cvars.float("cl_cmdrate"))
    }

    /// Sets a userinfo key sent in the `connect` packet in place of the default (call before
    /// [`connect`](Self::connect)), e.g. `_vgui_menus` `0` for a client that wants text menus.
    pub fn set_userinfo(&mut self, key: &str, value: &str) {
        self.userinfo.set(key, value);
    }

    pub fn queue_cmd(&mut self, cmd: GoldSrcUserCmd) {
        // The prediction runs the command before it is sent.
        self.predict.push_cmd(cmd.clone());
        if self.cmd_queue.len() >= crate::predict::MAX_PENDING_CMDS {
            self.cmd_queue.pop_front();
        }
        self.cmd_queue.push_back(cmd);
    }

    /// Queues a reliable string command. Ignored before the server accepted the connection
    /// (there is no channel to carry it yet).
    pub fn send_stringcmd(&mut self, cmd: &str) {
        let mut w = MsgWriter::new();
        write_clc_stringcmd(&mut w, cmd);
        self.queue_reliable(&w.into_bytes());
    }

    fn queue_reliable(&mut self, bytes: &[u8]) {
        let Some(chan) = self.chan.as_mut() else {
            return;
        };
        if chan.pending_reliable_bytes() + bytes.len() > MAX_RELIABLE_BACKLOG {
            self.disconnect_with("reliable message queue overflow".to_string());
            return;
        }
        chan.write_reliable(bytes);
    }

    /// Ends the connection with `reason` unless it already ended.
    fn disconnect_with(&mut self, reason: String) {
        if !self.state.is_terminal() {
            self.state = ConnState::Disconnected(reason);
        }
    }

    // ---- cvars and console -------------------------------------------------------------

    pub fn cvars(&self) -> &ClientCvars {
        &self.cvars
    }

    /// Sets a cvar as the local user (`Cvar_DirectSet`). A change to a userinfo cvar updates the
    /// userinfo and, once connected, tells the server with `setinfo "<key>" "<value>"`
    /// (`CL_UpdateInfo`).
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        if self.cvars.set(name, value).is_none() {
            return;
        }
        // Task 4 hook: ex_interp / cl_nointerp / cl_updaterate may have changed.
        self.frames.apply_cvars(&self.cvars);
        let Some(var) = self.cvars.find(name) else {
            return;
        };
        if var.flags & CVAR_USERINFO != 0 {
            let key = var.name.clone();
            self.userinfo.set(key.as_str(), value);
            if self.chan.is_some() {
                self.send_stringcmd(&format!("setinfo \"{key}\" \"{value}\"\n"));
            }
        }
    }

    pub fn userinfo(&self) -> &InfoString {
        &self.userinfo
    }

    pub fn commands(&self) -> &CommandRegistry {
        &self.commands
    }

    /// Registers a command the shell handles; it runs as [`ConsoleCommand`] in
    /// [`take_shell_commands`](Self::take_shell_commands).
    pub fn register_command(&mut self, name: &str, flags: u32) {
        self.commands.register(name, flags);
    }

    /// Runs a console line typed by the local user: no filtering.
    pub fn execute_local(&mut self, line: &str) {
        for line in split_lines(line) {
            self.execute_line(&line, true);
        }
    }

    /// Commands the connection does not run itself, in arrival order.
    pub fn take_shell_commands(&mut self) -> Vec<ConsoleCommand> {
        self.shell_commands.drain(..).collect()
    }

    /// The console output (`Con_Printf`) of the connection: server prints, `echo`, refusals.
    pub fn console_log(&self) -> &VecDeque<String> {
        &self.console
    }

    /// Drains new console lines since the last call without losing prints once `console_log`
    /// reaches capacity.
    pub fn take_console_lines(&mut self) -> Vec<String> {
        self.pending_console.drain(..).collect()
    }

    /// Server-forced view angle updates (`svc_setangle`, `svc_addangle`) since the last call.
    pub fn take_view_angle_updates(&mut self) -> Vec<ViewAngleUpdate> {
        std::mem::take(&mut self.view_angle_updates)
    }

    fn print(&mut self, line: impl Into<String>) {
        let text = line.into();
        if self.console.len() >= CONSOLE_LOG_CAPACITY {
            self.console.pop_front();
        }
        self.console.push_back(text.clone());
        if self.pending_console.len() >= PENDING_CONSOLE_CAPACITY {
            self.pending_console.pop_front();
        }
        self.pending_console.push_back(text);
    }

    /// `svc_stufftext`: the text goes through the line splitter and runs unprivileged
    /// (`Cbuf_AddFilteredText`, executed straight away: Xash runs the buffer before the next
    /// message, `svc_serverdata` flushes it explicitly).
    fn stufftext(&mut self, text: &str) {
        if self.cvars.float("cl_trace_stufftext") > 0.0 {
            self.print(format!("Stufftext: {}", text.trim_end_matches('\n')));
        }
        for line in split_lines(text) {
            self.execute_line(&line, false);
        }
    }

    /// `Cmd_ExecuteStringWithPrivilegeCheck`: command, then cvar, then the server.
    fn execute_line(&mut self, line: &str, privileged: bool) {
        let Some(cmd) = tokenize(line) else {
            return;
        };
        let filter = self.cvars.float("cl_filterstuffcmd") > 0.0;
        if self.commands.flags(&cmd.name).is_some() {
            if privileged || self.commands.stufftext_may_run(&cmd.name, filter) {
                self.run_command(cmd);
            } else {
                self.print(format!(
                    "Could not execute privileged command {}",
                    cmd.name
                ));
            }
            return;
        }
        if let Some(cvar) = self.cvars.find(&cmd.name).cloned() {
            if cmd.args.is_empty() {
                self.print(format!("\"{}\" is \"{}\"", cvar.name, cvar.value));
            } else if privileged || self.cvars.stufftext_may_set(&cvar) {
                self.set_cvar(&cvar.name, &cmd.args[0]);
            } else {
                self.print(format!("{} is a privileged variable", cvar.name));
            }
            return;
        }
        // `Cmd_ForwardToServer`: the game DLL parses what the engine does not know.
        if self.chan.is_some()
            && !self.state.is_terminal()
            && (privileged || self.cfg.forward_stufftext)
        {
            self.send_stringcmd(&cmd.forward_text());
        } else {
            self.print(format!("Unknown command \"{}\"", cmd.name));
        }
    }

    fn run_command(&mut self, cmd: ConsoleCommand) {
        match cmd.name.to_ascii_lowercase().as_str() {
            "reconnect" => {
                if matches!(self.state, ConnState::Signon | ConnState::InGame) {
                    self.state = ConnState::Signon;
                    self.forget_map();
                    self.send_stringcmd("new");
                }
            }
            "disconnect" => {
                if !self.state.is_terminal() {
                    self.state = ConnState::Disconnected("disconnected".to_string());
                }
            }
            "echo" => self.print(cmd.args.join(" ")),
            "cmd" => {
                if self.chan.is_some() {
                    self.send_stringcmd(&cmd.forward_text());
                }
            }
            "fullserverinfo" => match cmd.args.as_slice() {
                [info] => self.full_server_info = info.clone(),
                _ => self.print("Usage: fullserverinfo <complete info string>"),
            },
            "setinfo" | "userinfo" => self.setinfo(&cmd),
            _ => {
                if self.shell_commands.len() >= MAX_SHELL_COMMANDS {
                    self.disconnect_with("console command flood: the shell does not take them".to_string());
                } else {
                    self.shell_commands.push_back(cmd);
                }
            }
        }
    }

    /// `CL_SetInfo_f`.
    fn setinfo(&mut self, cmd: &ConsoleCommand) {
        match cmd.args.as_slice() {
            [] => {
                let text = self.userinfo.to_wire_string();
                self.print(format!("User info settings: {text}"));
            }
            [key, value] => {
                // A key that is a userinfo cvar goes through the cvar.
                if let Some(var) = self.cvars.find(key)
                    && var.flags & CVAR_USERINFO != 0
                {
                    let name = var.name.clone();
                    self.set_cvar(&name, value);
                } else {
                    self.userinfo.set(key.as_str(), value.as_str());
                    if self.chan.is_some() {
                        self.send_stringcmd(&format!("setinfo \"{key}\" \"{value}\"\n"));
                    }
                }
            }
            _ => self.print("Usage: setinfo [ <key> <value> ]"),
        }
    }

    /// What `svc_sendcvarvalue` is answered with (`CL_ParseCvarValue`).
    fn cvar_report(&self, name: &str) -> String {
        if name.eq_ignore_ascii_case("sv_version") {
            return "1.1.2.2/Stdio,48,10211".to_string();
        }
        match self.cvars.find(name) {
            Some(c) if c.flags & CVAR_PRIVILEGED != 0 => "CVAR is privileged".to_string(),
            Some(c) if c.flags & CVAR_PROTECTED != 0 => "CVAR is protected".to_string(),
            Some(c) => c.value.clone(),
            None => "Bad CVAR request".to_string(),
        }
    }

    // ---- test hooks --------------------------------------------------------------------

    /// Overrides the sequence sent in `clc_delta`; `None` restores the last entity update's.
    pub fn force_delta_seq(&mut self, seq: Option<u8>) {
        self.forced_delta_seq = seq;
    }

    /// Test hook: while `true`, outgoing packets carry no `clc_delta`, which asks the server for
    /// full updates.
    pub fn suppress_delta(&mut self, suppress: bool) {
        self.delta_suppressed = suppress;
    }

    /// Test hook: while `true`, outgoing packets keep acknowledging the reliable message state
    /// of the moment the hook was set, so the server never sees later reliable messages
    /// acknowledged and keeps resending them. `None` before the server accepted the connection.
    pub fn withhold_reliable_ack(&mut self, withhold: bool) {
        self.withheld_reliable_ack = if withhold {
            self.chan.as_ref().map(|c| c.incoming_reliable_sequence)
        } else {
            None
        };
    }

    /// Test hook: flips the checksum byte of the next `n` outgoing `clc_move` messages, so the
    /// server cannot parse the packets that carry them.
    pub fn corrupt_next_moves(&mut self, n: u32) {
        self.corrupt_moves = n;
    }

    /// Test hook: while `suppressed`, outgoing packets carry no `clc_move` at all (by default
    /// every packet has one: the queued commands, or the last one again with `msec` 0), so the
    /// server runs no command of this client. Queued commands wait.
    pub fn suppress_moves(&mut self, suppressed: bool) {
        self.moves_suppressed = suppressed;
    }

    /// Test hook: sequence number the next netchan packet will carry; `None` before the server
    /// accepted the connection.
    pub fn outgoing_sequence(&self) -> Option<u32> {
        self.chan.as_ref().map(|c| c.outgoing_sequence)
    }

    /// Test hook: sequence number of the last netchan packet received from the server; `None`
    /// before the server accepted the connection.
    pub fn incoming_sequence(&self) -> Option<u32> {
        self.chan.as_ref().map(|c| c.incoming_sequence)
    }

    /// The message parser's state (baselines, frame history), for tests.
    pub fn parse_context(&self) -> &SvcParseContext {
        &self.ctx
    }

    /// Low byte of the netchan sequence of the last entity update, for tests.
    pub fn last_entity_seq(&self) -> Option<u8> {
        self.last_entity_seq
    }

    // ---- accessors ---------------------------------------------------------------------

    pub fn state(&self) -> &ConnState {
        &self.state
    }

    /// The last `svc_signonnum` the server sent (0 before any).
    pub fn signon_num(&self) -> u8 {
        self.signon_num
    }

    pub fn server_info(&self) -> Option<&ServerInfoMsg> {
        self.server_info.as_ref()
    }

    pub fn player_slot(&self) -> Option<u8> {
        self.server_info.as_ref().map(|info| info.player_slot)
    }

    pub fn entities(&self) -> &BTreeMap<u16, GoldSrcEntityState> {
        &self.entities
    }

    /// Task 4 hook: the frame ring, the entity store and the clock.
    pub fn frames(&self) -> &ClientFrames {
        &self.frames
    }

    /// Task 4 hook: advance the clock (`advance`) and list the visible entities (`emit`).
    pub fn frames_mut(&mut self) -> &mut ClientFrames {
        &mut self.frames
    }

    /// The movement prediction (`cl.simorg`, the error checks, the sounds).
    pub fn predictor(&self) -> &Predictor {
        &self.predict
    }

    pub fn predictor_mut(&mut self) -> &mut Predictor {
        &mut self.predict
    }

    /// The map's collision hulls, which the prediction walks in (the shell loads the BSP and
    /// calls this after each map change: `forget_map` clears it, and without it the player is
    /// not predicted).
    pub fn set_world_collision(
        &mut self,
        world: Option<std::sync::Arc<cs_pm::collision::WorldCollision>>,
    ) {
        self.predict.set_world(world);
    }

    /// Spawn baselines received for the current map, decoded, by entity number.
    pub fn baselines(&self) -> BTreeMap<u16, GoldSrcEntityState> {
        let deltas = &self.ctx.deltas;
        let max_clients = self.ctx.max_clients;
        self.ctx
            .baselines
            .iter()
            .filter_map(|(&number, (ent_type, rec))| {
                let schema = deltas.get(entity_schema_name(number, *ent_type, max_clients))?;
                Some((number, GoldSrcEntityState::from_delta(number, *ent_type, rec, schema)))
            })
            .collect()
    }

    pub fn client_data(&self) -> Option<&GoldSrcClientData> {
        self.client_data.as_ref()
    }

    /// The `weapon_data_t` list of the last `svc_clientdata` the server sent, as the client
    /// rebuilds it from the deltas, by weapon index; an index not listed is all zero.
    pub fn weapon_data(&self) -> BTreeMap<u8, GoldSrcWeaponData> {
        let Some(schema) = self.ctx.deltas.get("weapon_data_t") else {
            return BTreeMap::new();
        };
        self.weapon_records
            .iter()
            .map(|(&index, record)| (index, GoldSrcWeaponData::from_delta(record, schema)))
            .collect()
    }

    /// The server's view of every player's userinfo, by slot.
    pub fn userinfos(&self) -> &BTreeMap<u8, String> {
        &self.userinfos
    }

    /// The movement variables of the last `svc_newmovevars`.
    pub fn movevars(&self) -> Option<&MoveVarsMsg> {
        self.movevars.as_ref()
    }

    /// The server info string of the last `fullserverinfo`.
    pub fn full_server_info(&self) -> &str {
        &self.full_server_info
    }

    /// The events, temp entities, sounds and HUD state built from the svc stream.
    pub fn fx(&self) -> &ClientEffects {
        &self.fx
    }

    pub fn fx_mut(&mut self) -> &mut ClientEffects {
        &mut self.fx
    }

    /// Gives the effects the world they collide with and the models they animate with (the
    /// shell's map and model cache); the default is [`NoWorld`].
    pub fn set_fx_world(&mut self, world: Box<dyn FxWorld>) {
        self.fx_world = world;
    }

    /// The local player's current weapon state bits (`m_iWeaponState`), whether a shield is
    /// held, and `cl_righthand`: what the weapon events consult.
    pub fn set_fx_local(&mut self, weapon_state: i32, holding_shield: bool, right_hand: bool) {
        self.fx_local = FxLocal {
            weapon_state,
            holding_shield,
            right_hand,
        };
    }

    /// Runs what the engine runs right before a frame is drawn: fires the due events and steps
    /// the temp entities (`CL_FireEvents`, `CL_TempEntUpdate`), at client time `now`.
    /// The gravity is the server's (`movevars.gravity`), 800 before the movevars arrive.
    pub fn update_effects(&mut self, now: f64) -> Vec<FiredEvent> {
        let gravity = self.movevars.as_ref().map_or(800.0, |v| v.gravity);
        let env = fx_env(
            now,
            &self.precache,
            &self.entities,
            self.ctx.max_clients,
            self.server_info.as_ref().map(|i| i.player_slot),
            self.client_data.as_ref(),
            self.fx_world.as_ref(),
            self.fx_local,
        );
        self.fx.update(&env, gravity)
    }

    pub fn precache(&self) -> &PrecacheTables {
        &self.precache
    }

    pub fn resources(&self) -> &ResourceManager {
        &self.resources
    }

    /// Replaces the file view used for the missing-file check, the consistency answer and the
    /// downloads (default: every file exists).
    pub fn set_files(&mut self, files: Box<dyn ResourceFiles>) {
        self.files = files;
    }

    /// Forgets the logged messages, so a test can tell what arrives from now on.
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// The last 1024 messages received, oldest first, without `svc_nop`, `svc_time`,
    /// `svc_clientdata` and the entity updates that every frame carries. User messages are
    /// decoded. A map change does not clear it.
    pub fn messages(&self) -> &VecDeque<SvcMessage> {
        &self.messages
    }

    /// Opcode of every decoded server message in arrival order; a user message by its id.
    pub fn opcode_log(&self) -> &[u8] {
        &self.opcode_log
    }

    pub fn stats(&self) -> ConnStats {
        self.stats
    }

    /// The transport the connection talks through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Records every datagram exchanged with the server from now on.
    pub fn set_tap(&mut self, tap: Box<dyn DatagramTap>) {
        self.tap = Some(tap);
    }

    /// The error that stopped the recording, if writing failed.
    pub fn tap_error(&self) -> Option<&std::io::Error> {
        self.tap_error.as_ref()
    }

    fn record(&mut self, sent: bool, time: f64, data: &[u8]) {
        if let Some(tap) = self.tap.as_mut()
            && let Err(e) = tap.record(sent, time, data)
        {
            self.tap = None;
            self.tap_error = Some(e);
        }
    }

    /// Sends one datagram to the server. A send error is treated like a datagram lost on the
    /// network: handshake packets are resent and netchan reliable data is retransmitted.
    fn send(&mut self, data: &[u8], now: f64) {
        self.record(true, now, data);
        let _ = self.transport.send(self.server, data);
    }

    // ---- receiving ---------------------------------------------------------------------

    fn handle_datagram(&mut self, data: &[u8], now: f64) {
        let Some(datagram) = self.splits.push(data) else {
            return;
        };
        if datagram.starts_with(&CONNECTIONLESS_HEADER.to_le_bytes()) {
            self.handle_connectionless(&datagram);
        } else {
            self.handle_netchan(&datagram, now);
        }
    }

    fn handle_connectionless(&mut self, datagram: &[u8]) {
        match parse_oob_packet(datagram) {
            Err(_) | Ok(OobPacket::Unknown(_)) => self.stats.parse_errors += 1,
            Ok(OobPacket::S2cChallenge { challenge, .. })
                if self.state == ConnState::AwaitChallenge =>
            {
                self.challenge = challenge;
                self.state = ConnState::AwaitConnection;
                self.handshake_sent_at = None;
            }
            Ok(OobPacket::S2cConnection { .. }) if self.state == ConnState::AwaitConnection => {
                self.state = ConnState::Signon;
                self.signon_num = 0;
                self.chan = Some(NetChan::new(FRAGMENT_C2S_SIZE));
                self.next_send_at = f64::NEG_INFINITY;
                self.send_stringcmd("new");
            }
            Ok(OobPacket::S2cReject(reason))
                if matches!(
                    self.state,
                    ConnState::AwaitChallenge | ConnState::AwaitConnection
                ) =>
            {
                self.state = ConnState::Rejected(reason.trim_end_matches('\n').to_string());
            }
            // Late duplicates of handshake answers, prints and the like.
            Ok(_) => {}
        }
    }

    fn handle_netchan(&mut self, datagram: &[u8], now: f64) {
        let Some(chan) = self.chan.as_mut() else {
            self.stats.parse_errors += 1;
            return;
        };
        let incoming = match chan.process_packet(datagram) {
            Ok(Some(incoming)) => incoming,
            // Duplicate or out-of-order packet.
            Ok(None) => return,
            Err(_) => {
                self.stats.parse_errors += 1;
                return;
            }
        };
        self.ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;

        // While a download is pending, progress is fragment bytes arriving (or a finished
        // file), not just any datagram: a server that sends nothing useful times out.
        let buffered = self.chan.as_ref().map_or(0, |c| c.incoming_buffered_bytes());
        if self.resources.pending_downloads() > 0 && buffered > self.buffered_fragments {
            self.signon_progress = Some(now);
        }
        self.buffered_fragments = buffered;
        // A stream that does not parse to its end still yields the messages before the bad
        // one; the rest of that stream is lost (no message boundary), the connection goes on.
        let mut failed = false;
        let streams = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for stream in streams {
            let (messages, error) = parse_svc_stream_partial(&stream, &mut self.ctx);
            failed |= error.is_some();
            for message in messages {
                self.opcode_log.push(message.opcode());
                self.log_message(&message);
                self.handle_svc(message, now);
            }
        }
        if self.opcode_log.len() > 2 * OPCODE_LOG_CAPACITY {
            self.opcode_log.drain(..OPCODE_LOG_CAPACITY);
        }
        if failed {
            self.stats.parse_errors += 1;
        }
        if let Some(file) = incoming.completed_file_stream {
            self.on_file_stream(&file, now);
        }
    }

    /// Keeps `message` in the bounded message log unless it is one of the per-update ones.
    pub fn log_message(&mut self, message: &SvcMessage) {
        if matches!(
            message,
            SvcMessage::Nop
                | SvcMessage::Time(_)
                | SvcMessage::ClientData { .. }
                | SvcMessage::PacketEntities(_)
        ) {
            return;
        }
        if self.messages.len() >= MESSAGE_LOG_CAPACITY {
            self.messages.pop_front();
        }
        self.messages.push_back(message.clone());
    }

    fn handle_svc(&mut self, message: SvcMessage, now: f64) {
        // Task 6 hook: every parsed message also goes to the effect modules (events, temp
        // entities, sound, HUD). They run before the match below, so an event sees the entity
        // update of the message before it, which `apply_entities` stored.
        let env = fx_env(
            now,
            &self.precache,
            &self.entities,
            self.ctx.max_clients,
            self.server_info.as_ref().map(|i| i.player_slot),
            self.client_data.as_ref(),
            self.fx_world.as_ref(),
            self.fx_local,
        );
        self.fx.handle(&message, &env);

        let signon = self.state == ConnState::Signon;
        // Task 4 hook: every parsed message goes to the frame store first.
        if matches!(message, SvcMessage::PacketEntities(_)) {
            self.frames.apply_cvars(&self.cvars);
        }
        let sequence = self.chan.as_ref().map_or(0, |c| c.incoming_sequence);
        let frame_outcome =
            self.frames
                .handle(&message, &self.ctx, &self.precache, sequence, now);
        match message {
            SvcMessage::Disconnect(reason) if !self.state.is_terminal() => {
                self.state = ConnState::Disconnected(reason.trim_end_matches('\n').to_string());
            }
            SvcMessage::ServerInfo(info) => {
                self.signon_progress = Some(now);
                self.server_info = Some(*info);
                if signon {
                    self.send_stringcmd("sendres");
                }
            }
            // `CL_ParseResourceRequest`: the client's own resources. It has no customization
            // store yet, so the list is empty.
            SvcMessage::ResourceRequest { .. } if signon => {
                let mut w = MsgWriter::new();
                write_clc_resourcelist(&mut w, &[]);
                self.queue_reliable(&w.into_bytes());
            }
            SvcMessage::ResourceList {
                resources,
                consistency_indices,
            } if signon => {
                self.signon_progress = Some(now);
                self.on_resource_list(&resources, &consistency_indices, now);
            }
            SvcMessage::ResourceLocation(text) => {
                if let Err(reason) = self.resources.add_download_locations(&text) {
                    self.disconnect_with(reason);
                }
            }
            SvcMessage::SignonNum(n) => {
                self.signon_progress = Some(now);
                self.signon_num = n;
                if signon && n == 1 {
                    self.send_stringcmd("sendents");
                }
            }
            // Task 4 hook: an update the frame store refused is not acknowledged (the server
            // then sends a full one).
            SvcMessage::PacketEntities(_) if matches!(frame_outcome, FrameOutcome::Refused(_)) => {
                self.last_entity_seq = None;
            }
            SvcMessage::PacketEntities(update) => self.apply_entities(&update, now),
            SvcMessage::ClientData { parsed, .. } => {
                self.client_data = Some(parsed);
                self.weapon_records = self
                    .ctx
                    .weapondata_history
                    .get(&self.ctx.last_incoming_seq)
                    .cloned()
                    .unwrap_or_default();
            }
            // The client takes the view the server forces (`svc_setangle`), as the engine's
            // client does: the commands it sends from then on look that way.
            SvcMessage::SetAngle(angles) => {
                let clean = glam::Vec3::new(
                    if angles.x.is_finite() { angles.x } else { 0.0 },
                    if angles.y.is_finite() { angles.y } else { 0.0 },
                    if angles.z.is_finite() { angles.z } else { 0.0 },
                );
                self.last_cmd.viewangles = clean;
                self.view_angle_updates.push(ViewAngleUpdate::Set(clean));
            }
            // `svc_addangle` turns the view by a yaw.
            SvcMessage::AddAngle(yaw) => {
                if yaw.is_finite() {
                    self.last_cmd.viewangles.y += yaw;
                    self.view_angle_updates.push(ViewAngleUpdate::AddYaw(yaw));
                }
            }
            SvcMessage::UpdateUserInfo { slot, userinfo, .. } => {
                self.userinfos.insert(slot, userinfo);
            }
            SvcMessage::StuffText(text)
                if matches!(self.state, ConnState::Signon | ConnState::InGame) =>
            {
                self.stufftext(&text);
            }
            SvcMessage::Print(text) => self.print(text.trim_end_matches('\n').to_string()),
            SvcMessage::NewMoveVars(vars) => self.movevars = Some(sanitize_movevars(*vars)),
            SvcMessage::DecalName { index, name } => {
                self.precache.decals.insert(
                    u16::from(index),
                    crate::precache::PrecacheEntry {
                        name,
                        download_size: 0,
                        flags: 0,
                    },
                );
            }
            SvcMessage::SendCvarValue(name) => {
                let mut w = MsgWriter::new();
                w.write_u8(clc::CVARVALUE);
                w.write_str(&self.cvar_report(&name));
                self.queue_reliable(&w.into_bytes());
            }
            SvcMessage::SendCvarValue2 {
                request_id,
                cvar_name,
            } => {
                let mut w = MsgWriter::new();
                write_clc_cvarvalue2(&mut w, request_id, &cvar_name, &self.cvar_report(&cvar_name));
                self.queue_reliable(&w.into_bytes());
            }
            _ => {}
        }
    }

    /// `CL_ParseResourceList` and `CL_StartResourceDownloading`: fill the precache tables, work
    /// out what files are missing and either wait for their downloads or go on to `spawn`.
    fn on_resource_list(
        &mut self,
        resources: &[cs_proto48::messages::ResourceEntry],
        consistency_indices: &[u16],
        now: f64,
    ) {
        self.download_started = Some(now);
        if self.cfg.consistency == ConsistencyPolicy::Refuse && !consistency_indices.is_empty() {
            self.state = ConnState::Disconnected(CONSISTENCY_REASON.to_string());
            return;
        }
        if self.server_info.is_none() {
            self.state = ConnState::Disconnected(NO_SERVERINFO_REASON.to_string());
            return;
        }
        self.precache.add(resources);
        let spawn_count = self.server_info.as_ref().map_or(0, |i| i.spawn_count);
        if let Err(reason) = self.resources.begin(resources, consistency_indices, spawn_count) {
            self.state = ConnState::Disconnected(reason);
            return;
        }
        self.resources.estimate_missing(self.files.as_ref());
        self.awaiting_resources = true;
        self.request_missing();
    }

    /// `CL_BatchResourceRequest`: ask for the files still missing; when none is, finish.
    fn request_missing(&mut self) {
        let allow = self.cvars.float("cl_allowdownload") > 0.0;
        let outcome = self.resources.batch_request(allow);
        for line in outcome.skipped {
            self.print(line);
        }
        if let Some(path) = outcome.fatal {
            self.state = ConnState::Disconnected(format!(
                "missing required file {path} (the server does not allow it to be fetched, or downloads are off)"
            ));
            self.awaiting_resources = false;
            return;
        }
        for path in outcome.dlfiles {
            self.send_stringcmd(&format!("dlfile {path}"));
        }
        if outcome.done {
            self.finish_resources();
        }
    }

    /// `CL_RegisterResources`: the consistency answer when one is owed, then `spawn`.
    fn finish_resources(&mut self) {
        self.awaiting_resources = false;
        let Some(spawn_count) = self.server_info.as_ref().map(|i| i.spawn_count) else {
            self.state = ConnState::Disconnected(NO_SERVERINFO_REASON.to_string());
            return;
        };
        if let Some(payload) = self
            .resources
            .consistency_answer(self.files.as_ref(), spawn_count)
        {
            let mut w = MsgWriter::new();
            write_clc_fileconsistency(&mut w, &payload);
            self.queue_reliable(&w.into_bytes());
        }
        self.send_spawn();
    }

    /// A file arrived through the file stream (`CL_ProcessFile`). Only a file this client asked
    /// for with `dlfile` is kept: the name must match a pending request, which settles it
    /// before anything is stored. Anything else a server sends is dropped.
    fn on_file_stream(&mut self, stream: &[u8], now: f64) {
        let Some(file) = parse_file_stream(stream) else {
            self.print("file fragment received with a bad header, name or size, ignoring");
            return;
        };
        if !self.resources.file_received(&file.name) {
            self.print(format!("unrequested file {} ignored", file.name));
            return;
        }
        self.signon_progress = Some(now);
        if !self.files.store(&file.name, &file.data) && !self.files.exists(&file.name) {
            self.print(format!("could not keep downloaded file {}", file.name));
        }
        if self.awaiting_resources && self.state == ConnState::Signon {
            self.request_missing();
        }
    }

    /// `spawn <spawn_count> <crc>`: the map CRC munged with `COM_Munge2` keyed by
    /// `(-1 - spawn_count) & 0xFF`, printed as a signed 32-bit integer.
    /// Without `svc_serverinfo` there is no spawn count or map CRC to answer with, so the client
    /// stops instead of waiting in `Signon` forever.
    ///
    /// The map CRC is the server's word: the client does not check it against a loaded BSP yet
    /// (the shell loads the map; the check belongs with it).
    fn send_spawn(&mut self) {
        let Some(info) = self.server_info.as_ref() else {
            self.state = ConnState::Disconnected(NO_SERVERINFO_REASON.to_string());
            return;
        };
        let spawn_count = info.spawn_count;
        let mut crc = info.map_crc.to_le_bytes();
        com_munge2(&mut crc, (-1i32).wrapping_sub(spawn_count as i32) & 0xFF);
        let crc = i32::from_le_bytes(crc);
        self.send_stringcmd(&format!("spawn {spawn_count} {crc}"));
    }

    /// Drops everything that belongs to the previous map before a `reconnect` sign-on. The
    /// channel, user infos, stats, opcode log, cvars and the caller's `force_delta_seq` stay.
    fn forget_map(&mut self) {
        self.entities.clear();
        self.frames.reset();
        self.predict.reset();
        self.fx.clear_effects();
        self.ctx.baselines.clear();
        self.ctx.instance_baselines.clear();
        self.ctx.entity_frames.clear();
        self.ctx.clientdata_history.clear();
        self.ctx.weapondata_history.clear();
        self.weapon_records.clear();
        self.server_info = None;
        self.client_data = None;
        self.movevars = None;
        self.frame_times.clear();
        self.cmd_queue.clear();
        self.sent_cmds.clear();
        self.view_angle_updates.clear();
        self.last_entity_seq = None;
        self.resources.clear();
        self.precache.clear();
        self.awaiting_resources = false;
        self.signon_num = 0;
        self.signon_progress = None;
        self.download_started = None;
        self.buffered_fragments = 0;
    }

    /// Replaces the entity view with the complete frame the parser rebuilt from this update.
    fn apply_entities(&mut self, update: &PacketEntitiesMsg, now: f64) {
        let deltas = &self.ctx.deltas;
        let max_clients = self.ctx.max_clients;
        self.entities = update
            .entities
            .iter()
            .filter_map(|(&number, (ent_type, rec))| {
                let schema = deltas.get(entity_schema_name(number, *ent_type, max_clients))?;
                let state = GoldSrcEntityState::from_delta(number, *ent_type, rec, schema);
                Some((number, state))
            })
            .collect();

        if update.is_delta {
            self.stats.delta_updates += 1;
        } else {
            self.stats.full_updates += 1;
        }
        self.frame_times.push_back(now);
        self.last_entity_seq = Some(self.ctx.last_incoming_seq);
        if self.state == ConnState::Signon {
            self.state = ConnState::InGame;
        }
    }

    // ---- sending -----------------------------------------------------------------------

    /// One netchan packet: `clc_move` with the queued commands (or the last command repeated
    /// with `msec = 0`) plus up to two backups, then `clc_delta` when there is a delta base.
    fn send_netchan_packet(&mut self, now: f64) {
        let Some(chan) = self.chan.as_mut() else {
            return;
        };
        let mut unreliable = MsgWriter::new();

        if let Some(schema) = self
            .ctx
            .deltas
            .get("usercmd_t")
            .filter(|_| !self.moves_suppressed)
        {
            let max_take = self.cmd_queue.len().min(MAX_NEW_CMDS);
            let take = if max_take == 0 {
                0
            } else {
                let newest_first_all: Vec<GoldSrcUserCmd> = self
                    .cmd_queue
                    .iter()
                    .take(max_take)
                    .rev()
                    .chain(self.sent_cmds.iter())
                    .cloned()
                    .collect();
                let backups = self.sent_cmds.len() as u8;
                let mut fit = max_take;
                while fit > 1
                    && clc_move_body_len(schema, backups, &newest_first_all[max_take - fit..]) > 255
                {
                    fit -= 1;
                }
                fit
            };
            let mut new_cmds: Vec<GoldSrcUserCmd> = self.cmd_queue.drain(..take).collect();
            // The prediction learns which packet carries which commands (the
            // server acknowledges packets).
            let mut placeholder = None;
            match new_cmds.last() {
                Some(last) => self.last_cmd = last.clone(),
                None => {
                    let held = GoldSrcUserCmd {
                        msec: 0,
                        ..self.last_cmd.clone()
                    };
                    placeholder = Some(held.clone());
                    new_cmds.push(held);
                }
            }
            self.predict
                .packet_sent(chan.outgoing_sequence, take, placeholder);

            let cmds_newest_first: Vec<GoldSrcUserCmd> = new_cmds
                .iter()
                .rev()
                .chain(self.sent_cmds.iter())
                .cloned()
                .collect();
            let mut mv = MsgWriter::new();
            write_clc_move(
                &mut mv,
                schema,
                chan.outgoing_sequence,
                0,
                self.sent_cmds.len() as u8,
                &cmds_newest_first,
            );
            let mut mv = mv.into_bytes();
            if self.corrupt_moves > 0 {
                self.corrupt_moves -= 1;
                // Opcode, length, then the checksum byte.
                mv[2] ^= 0xFF;
            }
            unreliable.write_bytes(&mv);
            for cmd in new_cmds {
                self.sent_cmds.push_front(cmd);
            }
            self.sent_cmds.truncate(MAX_BACKUP_CMDS);
        }

        if let Some(seq) = self.forced_delta_seq.or(self.last_entity_seq)
            && !self.delta_suppressed
        {
            write_clc_delta(&mut unreliable, seq);
        }
        let packet = match self.withheld_reliable_ack {
            Some(ack) => {
                let own = std::mem::replace(&mut chan.incoming_reliable_sequence, ack);
                let packet = chan.build_packet(&unreliable.into_bytes());
                chan.incoming_reliable_sequence = own;
                packet
            }
            None => chan.build_packet(&unreliable.into_bytes()),
        };
        self.send(&packet, now);
    }
}

/// The local weapon facts the shell supplies to the weapon events.
#[derive(Debug, Clone, Copy, Default)]
struct FxLocal {
    weapon_state: i32,
    holding_shield: bool,
    right_hand: bool,
}

/// The environment of the effect modules, from the connection's fields (a function, not a
/// method, so the caller can borrow `fx` mutably beside it).
#[allow(clippy::too_many_arguments)]
fn fx_env<'a>(
    now: f64,
    precache: &'a PrecacheTables,
    entities: &'a BTreeMap<u16, GoldSrcEntityState>,
    max_clients: u8,
    player_slot: Option<u8>,
    client_data: Option<&GoldSrcClientData>,
    world: &'a dyn FxWorld,
    local: FxLocal,
) -> FxEnv<'a> {
    let mut env = FxEnv::new(now, precache, entities, world);
    env.weapon_state = local.weapon_state;
    env.holding_shield = local.holding_shield;
    env.right_hand = local.right_hand;
    env.max_clients = u16::from(max_clients);
    env.local_player = player_slot.map_or(0, |slot| u16::from(slot) + 1);
    if let Some(cd) = client_data {
        env.local_origin = cd.origin;
        env.local_view_ofs = cd.view_ofs;
        env.local_velocity = cd.velocity;
    }
    env
}

fn sanitize_movevars(mut mv: MoveVarsMsg) -> MoveVarsMsg {
    let def = MoveVarsMsg::default();
    for (v, d) in [
        (&mut mv.gravity, def.gravity),
        (&mut mv.stopspeed, def.stopspeed),
        (&mut mv.maxspeed, def.maxspeed),
        (&mut mv.spectatormaxspeed, def.spectatormaxspeed),
        (&mut mv.accelerate, def.accelerate),
        (&mut mv.airaccelerate, def.airaccelerate),
        (&mut mv.wateraccelerate, def.wateraccelerate),
        (&mut mv.friction, def.friction),
        (&mut mv.edgefriction, def.edgefriction),
        (&mut mv.waterfriction, def.waterfriction),
        (&mut mv.entgravity, def.entgravity),
        (&mut mv.bounce, def.bounce),
        (&mut mv.stepsize, def.stepsize),
        (&mut mv.maxvelocity, def.maxvelocity),
        (&mut mv.zmax, def.zmax),
        (&mut mv.wave_height, def.wave_height),
        (&mut mv.rollangle, def.rollangle),
        (&mut mv.rollspeed, def.rollspeed),
    ] {
        if !v.is_finite() {
            *v = d;
        }
    }
    for (c, d) in mv.skycolor.iter_mut().zip(def.skycolor) {
        if !c.is_finite() {
            *c = d;
        }
    }
    for (v, d) in mv.skyvec.iter_mut().zip(def.skyvec) {
        if !v.is_finite() {
            *v = d;
        }
    }
    mv
}

#[cfg(test)]
mod fx_tests {
    use super::*;
    use crate::tempents::ModelRef;
    use cs_proto48::messages::ResourceEntry;
    use cs_transport::LoopbackHub;

    struct Frames(u32);

    impl FxWorld for Frames {
        fn frame_count(&self, _model: &ModelRef) -> u32 {
            self.0
        }
    }

    fn connection() -> Connection<cs_transport::LoopbackEndpoint> {
        let hub = LoopbackHub::new();
        let cfg = ConnectionConfig::new("t", 1, "0123456789abcdef0123456789abcdef");
        let mut conn = Connection::new(hub.connect(), PeerAddr::Loopback(0), cfg);
        conn.precache.add(&[ResourceEntry {
            res_type: crate::resources::res_type::MODEL,
            filename: "sprites/smoke.spr".into(),
            index: 4,
            download_size: 0,
            flags: 0,
            md5: None,
            reserved: None,
        }]);
        conn
    }

    fn smoke() -> SvcMessage {
        let te = cs_proto48::tempent::smoke(glam::Vec3::ZERO, 4, 20, 10);
        SvcMessage::TempEntity {
            te_type: te.te_type,
            raw: te.payload,
        }
    }

    #[test]
    fn the_shells_world_gives_sprites_their_frames() {
        // Without a world every sprite has one frame and its animation is over at once.
        let mut bare = connection();
        bare.handle_svc(smoke(), 0.0);
        assert_eq!(bare.fx().tempents.active().len(), 1);
        bare.update_effects(0.05);
        assert!(bare.fx().tempents.active().is_empty());

        // With a world that reports five frames the smoke lives its 0.4 seconds.
        let mut conn = connection();
        conn.set_fx_world(Box::new(Frames(5)));
        conn.handle_svc(smoke(), 0.0);
        conn.update_effects(0.05);
        assert_eq!(conn.fx().tempents.active().len(), 1);
        assert_eq!(conn.fx().tempents.active()[0].frame_max, 4);
        conn.update_effects(0.5);
        assert!(conn.fx().tempents.active().is_empty());
    }

    #[test]
    fn the_local_weapon_facts_reach_the_events() {
        let mut conn = connection();
        conn.set_fx_local(2, true, true);
        let env = fx_env(
            0.0,
            &conn.precache,
            &conn.entities,
            32,
            Some(0),
            None,
            conn.fx_world.as_ref(),
            conn.fx_local,
        );
        assert_eq!(
            (env.weapon_state, env.holding_shield, env.right_hand, env.local_player),
            (2, true, true, 1)
        );
    }

    #[test]
    fn forget_map_clears_map_state_while_keeping_hud_and_queue_cmd_is_capped() {
        let mut conn = connection();
        conn.fx.hud.health = 77;
        conn.handle_svc(smoke(), 0.0);
        conn.frame_times.push_back(1.0);
        conn.buffered_fragments = 42;
        conn.movevars = Some(sanitize_movevars(MoveVarsMsg {
            gravity: f32::NAN,
            stepsize: f32::INFINITY,
            ..MoveVarsMsg::default()
        }));
        assert_eq!(conn.movevars.as_ref().unwrap().gravity, 800.0);
        assert_eq!(conn.movevars.as_ref().unwrap().stepsize, 18.0);

        for i in 0..(crate::predict::MAX_PENDING_CMDS + 10) {
            conn.queue_cmd(GoldSrcUserCmd {
                msec: 10,
                impulse: (i & 0xFF) as u8,
                ..Default::default()
            });
        }
        assert_eq!(conn.cmd_queue.len(), crate::predict::MAX_PENDING_CMDS);
        assert_eq!(conn.fx.tempents.active().len(), 1);

        conn.forget_map();
        assert!(conn.cmd_queue.is_empty());
        assert!(conn.frame_times.is_empty());
        assert_eq!(conn.buffered_fragments, 0);
        assert!(conn.movevars.is_none());
        assert!(conn.fx.tempents.active().is_empty());
        assert_eq!(conn.fx.hud.health, 77);
    }

    #[test]
    fn send_netchan_packet_packs_cmds_within_255_byte_limit() {
        let mut conn = connection();
        conn.chan = Some(NetChan::new(FRAGMENT_C2S_SIZE));
        let ucmd_schema = cs_proto48::delta::DeltaRegistry::cs16_default()
            .get("usercmd_t")
            .unwrap()
            .clone();
        conn.ctx.deltas.insert(ucmd_schema);
        for i in 0..16u8 {
            conn.queue_cmd(GoldSrcUserCmd {
                msec: 10 + i,
                viewangles: glam::Vec3::new((i as f32) * 3.1, (i as f32) * 19.7, (i as f32) * 1.3),
                forwardmove: 250.0 - (i as f32) * 15.0,
                sidemove: if i % 2 == 0 { 250.0 } else { -250.0 },
                upmove: (i as f32) * 12.0,
                buttons: 1u16 << (i % 12),
                impulse: 100 + i,
                weaponselect: i + 1,
                impact_index: i32::from(i) * 7,
                impact_position: glam::Vec3::new(i as f32, -(i as f32), (i as f32) * 2.0),
                ..Default::default()
            });
        }
        conn.send_netchan_packet(1.0);
        assert!(!conn.cmd_queue.is_empty(), "overflow cmds kept for next packet");
        assert!(conn.cmd_queue.len() < 16, "at least some cmds were sent");
    }

    #[test]
    fn setangle_and_console_lines_drain_past_ring_capacity() {
        let mut conn = connection();
        conn.handle_svc(
            SvcMessage::SetAngle(glam::Vec3::new(12.5, 90.0, f32::NAN)),
            0.0,
        );
        conn.handle_svc(SvcMessage::AddAngle(15.0), 0.0);
        assert_eq!(
            conn.take_view_angle_updates(),
            vec![
                ViewAngleUpdate::Set(glam::Vec3::new(12.5, 90.0, 0.0)),
                ViewAngleUpdate::AddYaw(15.0),
            ]
        );
        assert!(conn.take_view_angle_updates().is_empty());

        for i in 0..(CONSOLE_LOG_CAPACITY + 20) {
            conn.handle_svc(SvcMessage::Print(format!("line {i}\n")), 0.0);
        }
        let drained = conn.take_console_lines();
        assert_eq!(drained.len(), CONSOLE_LOG_CAPACITY + 20);
        assert_eq!(drained.last().map(String::as_str), Some("line 275"));
        conn.handle_svc(SvcMessage::Print("after full\n".to_string()), 0.0);
        assert_eq!(conn.take_console_lines(), vec!["after full".to_string()]);
    }
}
