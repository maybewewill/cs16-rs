//! The server: client slots, one netchan per client and the sign-on sequence. Connectionless
//! packets and connects are in `connect.rs`, per-client updates in `frame.rs`.

use std::collections::BTreeMap;

use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{DeltaRecord, DeltaRegistry, DeltaSchema, GoldSrcEntityState};
use cs_proto48::handshake::tokenize_goldsrc_line;
use cs_proto48::messages::{
    ClcMessage, ServerInfoMsg, entity_schema_name, parse_clc_stream, wire_entity_type,
    write_svc_stufftext, write_svc_updateuserinfo,
};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{PacketTransport, PeerAddr};

use crate::client::{Client, ClientState, SignonStage};
use crate::edict::{DEFAULT_MAX_EDICTS, EdictPool};
use crate::game::{ClientView, EngineApi, GameDll, Outbox, entity_state_from_vars};
use crate::map::ServerMap;
use crate::oob::{ChallengeIssuer, OobLimiter};
use crate::precache::PrecacheTables;
use crate::signon::{
    UserInfoEntry, move_vars, munged_map_crc, write_new_stage, write_sendres_stage,
    write_spawn_baselines, write_spawn_stage,
};

/// Unacknowledged reliable bytes a client may have queued; past this it is dropped.
pub const MAX_RELIABLE_BACKLOG: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub hostname: String,
    pub max_clients: u8,
    pub game_dir: String,
    pub build: i32,
    pub timeout_secs: f64,
    pub signon_timeout_secs: f64,
    pub min_update_rate: u32,
    pub max_update_rate: u32,
    pub min_rate: u32,
    pub max_rate: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            hostname: "Counter-Strike 1.6 Rust".to_string(),
            max_clients: 16,
            game_dir: "cstrike".to_string(),
            build: 8684,
            timeout_secs: 60.0,
            signon_timeout_secs: 120.0,
            min_update_rate: 10,
            max_update_rate: 102,
            min_rate: 1000,
            max_rate: 100_000,
        }
    }
}

pub struct Server<G: GameDll> {
    pub(crate) cfg: ServerConfig,
    pub(crate) map: ServerMap,
    pub(crate) game: G,
    pub(crate) deltas: DeltaRegistry,
    pub(crate) cvars: CVarRegistry,
    pub(crate) usermsgs: UserMsgRegistry,
    pub(crate) edicts: EdictPool,
    pub(crate) precache: PrecacheTables,
    pub(crate) outbox: Outbox,
    /// What the game sees of each slot, indexed like `clients`.
    pub(crate) views: Vec<ClientView>,
    pub(crate) clients: Vec<Client>,
    /// Entity states sent with `svc_spawnbaseline`, by entity number.
    pub(crate) baselines: Vec<(u16, GoldSrcEntityState)>,
    /// The same baselines as delta records, the base of full entity updates.
    pub(crate) baseline_records: BTreeMap<u16, (u8, DeltaRecord)>,
    pub(crate) usercmd_schema: Option<DeltaSchema>,
    pub(crate) challenges: ChallengeIssuer,
    pub(crate) limiter: OobLimiter,
    pub(crate) spawn_count: u32,
    pub(crate) next_userid: i32,
    pub(crate) time: f64,
    pub(crate) net_stats: NetStats,
}

/// Monotonic counts of what the server refused from known client addresses.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NetStats {
    /// Datagrams the client's netchan refused: malformed, stale or duplicate.
    pub datagrams_rejected: u64,
    /// Payloads that reached `parse_clc_stream` and failed to parse.
    pub payloads_rejected: u64,
    /// Times queued unreliable messages for a client were discarded: the queue outgrew
    /// `MAX_DATAGRAM`, or it did not fit in the update that was to carry it.
    pub unreliable_dropped: u64,
    /// Updates sent without their entity block because it did not fit in the datagram.
    pub entity_blocks_dropped: u64,
}

impl<G: GameDll> Server<G> {
    pub fn new(
        cfg: ServerConfig,
        map: ServerMap,
        game: G,
        deltas: DeltaRegistry,
        cvars: CVarRegistry,
    ) -> Self {
        let usermsgs = game.usermsgs();
        let edicts = EdictPool::new(cfg.max_clients, DEFAULT_MAX_EDICTS);
        let precache = PrecacheTables::new(&map.path, map.submodel_count);
        let last_submodel = map.submodel_count.saturating_sub(1);
        if last_submodel > 0 && precache.model_index(&format!("*{last_submodel}")).is_none() {
            tracing::error!(
                map = %map.name,
                submodels = map.submodel_count,
                "brush models do not fit the model precache table"
            );
        }
        let usercmd_schema = deltas.get("usercmd_t").cloned();
        if usercmd_schema.is_none() {
            tracing::error!("delta registry has no usercmd_t; client messages cannot be parsed");
        }
        let slots = usize::from(cfg.max_clients);
        let mut server = Self {
            outbox: Outbox::new(cfg.max_clients),
            views: (0..slots).map(|_| ClientView::default()).collect(),
            clients: (0..slots).map(|_| Client::free()).collect(),
            cfg,
            map,
            game,
            deltas,
            cvars,
            usermsgs,
            edicts,
            precache,
            baselines: Vec::new(),
            baseline_records: BTreeMap::new(),
            usercmd_schema,
            challenges: ChallengeIssuer::new(),
            limiter: OobLimiter::new(),
            spawn_count: 1,
            next_userid: 1,
            time: 0.0,
            net_stats: NetStats::default(),
        };
        server.with_game(|game, eng| game.server_activate(eng));
        server.precache.lock();
        server.compute_baselines();
        server
    }

    /// One server frame: handles every waiting datagram, frees clients whose sign-on took too
    /// long, then sends each client what is due for it (see `frame.rs`).
    pub fn frame(&mut self, net: &mut dyn PacketTransport, now: f64) {
        let real_dt = now - self.time;
        self.time = now;
        self.accrue_client_time(real_dt);
        while let Some((from, data)) = net.recv() {
            self.handle_datagram(net, from, &data);
        }
        self.drop_stalled_signons();
        self.send_client_frames(net);
    }

    pub fn client_state(&self, slot: u8) -> ClientState {
        self.clients
            .get(usize::from(slot))
            .map_or(ClientState::Free, |c| c.state)
    }

    pub fn client_count(&self) -> usize {
        self.clients.iter().filter(|c| !c.is_free()).count()
    }

    /// Userid of the client in `slot`; `None` for a free slot.
    pub fn client_userid(&self, slot: u8) -> Option<i32> {
        self.clients
            .get(usize::from(slot))
            .filter(|c| !c.is_free())
            .map(|c| c.userid)
    }

    pub fn net_stats(&self) -> NetStats {
        self.net_stats
    }

    pub fn spawn_count(&self) -> u32 {
        self.spawn_count
    }

    pub fn game(&self) -> &G {
        &self.game
    }

    pub fn game_mut(&mut self) -> &mut G {
        &mut self.game
    }

    pub fn edicts(&self) -> &EdictPool {
        &self.edicts
    }

    /// Calls into the game, then drains what it queued.
    pub(crate) fn with_game<R>(&mut self, f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R) -> R {
        let result = self.call_game(f);
        self.drain_outbox();
        result
    }

    /// Calls into the game without draining the outbox.
    pub(crate) fn call_game<R>(&mut self, f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R) -> R {
        let mut eng = EngineApi::new(
            &mut self.edicts,
            &mut self.precache,
            &mut self.cvars,
            &self.map,
            self.time,
            &mut self.outbox,
            &mut self.views,
        );
        f(&mut self.game, &mut eng)
    }

    /// Moves what the game queued to the clients and carries out its kicks.
    pub(crate) fn drain_outbox(&mut self) {
        loop {
            for (slot, client) in self.clients.iter_mut().enumerate() {
                let reliable = std::mem::take(&mut self.outbox.reliable[slot]);
                let unreliable = std::mem::take(&mut self.outbox.unreliable[slot]);
                if client.is_free() {
                    continue;
                }
                if !reliable.is_empty() {
                    client.chan.write_reliable(reliable.as_slice());
                }
                if client.queue_unreliable(unreliable.as_slice()) {
                    self.net_stats.unreliable_dropped += 1;
                }
            }

            let all = std::mem::take(&mut self.outbox.reliable_all);
            let mut broadcast = all.into_bytes();
            for slot in 0..self.clients.len() {
                if !std::mem::take(&mut self.views[slot].userinfo_dirty) {
                    continue;
                }
                let client = &self.clients[slot];
                if client.is_free() {
                    continue;
                }
                let mut w = MsgWriter::new();
                write_svc_updateuserinfo(
                    &mut w,
                    slot as u8,
                    client.userid,
                    &self.views[slot].userinfo.to_wire_string(),
                    &[0; 16],
                );
                broadcast.extend_from_slice(w.as_slice());
            }
            if !broadcast.is_empty() {
                for client in self.clients.iter_mut().filter(|c| c.state.is_past_new()) {
                    client.chan.write_reliable(&broadcast);
                }
            }

            // Telling a kicked or dropped client why is left to a later task.
            let mut drops: Vec<u8> = std::mem::take(&mut self.outbox.kicks)
                .into_iter()
                .map(|(slot, _reason)| slot)
                .collect();
            drops.extend(self.overloaded_slots());
            if drops.is_empty() {
                return;
            }
            for slot in drops {
                self.release_slot(slot);
            }
        }
    }

    /// Connected clients whose unacknowledged reliable data exceeds [`MAX_RELIABLE_BACKLOG`].
    fn overloaded_slots(&self) -> Vec<u8> {
        self.clients
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_free() && c.chan.pending_reliable_bytes() > MAX_RELIABLE_BACKLOG)
            .map(|(slot, _)| slot as u8)
            .collect()
    }

    /// Frees `slot`; a client that was put in the server is reported to the game first. The
    /// caller drains the outbox.
    fn release_slot(&mut self, slot: u8) {
        let index = usize::from(slot);
        if self.clients.get(index).is_none_or(Client::is_free) {
            return;
        }
        if self.clients[index].put_in_server {
            self.call_game(|game, eng| game.client_disconnect(eng, slot));
        }
        self.edicts.reset_client(slot);
        self.clients[index] = Client::free();
        self.views[index] = ClientView::default();
    }

    /// Frees `slot` and passes on what the game queued meanwhile.
    pub(crate) fn free_slot(&mut self, slot: u8) {
        self.release_slot(slot);
        self.drain_outbox();
    }

    /// Spawn baselines of every live, visible edict that is not a client slot.
    fn compute_baselines(&mut self) {
        let max_clients = self.cfg.max_clients;
        self.baselines = self
            .edicts
            .iter_live()
            .filter(|(id, e)| {
                !(1..=u16::from(max_clients)).contains(&id.index) && e.vars.modelindex != 0
            })
            .map(|(id, e)| (id.index, entity_state_from_vars(id.index, &e.vars)))
            .collect();
        self.baseline_records = self
            .baselines
            .iter()
            .filter_map(|(number, state)| {
                let wire_type = wire_entity_type(state.entity_type);
                let schema =
                    self.deltas
                        .get(entity_schema_name(*number, wire_type, max_clients))?;
                Some((*number, (state.entity_type, state.to_delta(schema))))
            })
            .collect();
    }

    pub(crate) fn slot_of(&self, addr: &PeerAddr) -> Option<u8> {
        self.clients
            .iter()
            .position(|c| c.addr.as_ref() == Some(addr))
            .map(|i| i as u8)
    }

    fn handle_datagram(&mut self, net: &mut dyn PacketTransport, from: PeerAddr, data: &[u8]) {
        if data.starts_with(&[0xFF; 4]) {
            self.handle_connectionless(net, from, data);
        } else if let Some(slot) = self.slot_of(&from) {
            self.handle_netchan(slot, data);
        }
    }

    fn handle_netchan(&mut self, slot: u8, data: &[u8]) {
        let index = usize::from(slot);
        let Ok(Some(incoming)) = self.clients[index].chan.process_packet(data) else {
            self.net_stats.datagrams_rejected += 1;
            return;
        };
        // The delta request is per packet: one without `clc_delta` asks for a full update.
        self.clients[index].delta_seq = None;
        let Some(schema) = self.usercmd_schema.clone() else {
            return;
        };
        // Losses are replayed once per packet, by its first `clc_move`.
        let mut dropped = incoming.dropped;
        // A completed file stream (`completed_file_stream`) is discarded.
        let payloads = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for payload in payloads {
            // The parser does not tell a bad `clc_move` checksum from other errors, so every
            // unparsable payload counts as a bad move.
            let Ok(messages) = parse_clc_stream(&payload, &schema, incoming.sequence) else {
                self.net_stats.payloads_rejected += 1;
                self.count_bad_move(slot);
                if self.clients[index].is_free() {
                    return;
                }
                continue;
            };
            // At most one sign-on stage command is honoured per payload.
            let mut stage_done = false;
            for message in messages {
                if self.clients[index].is_free() {
                    return;
                }
                // Resource lists are handled by a later task.
                match message {
                    ClcMessage::StringCmd(line) => {
                        self.handle_stringcmd(slot, &line, &mut stage_done);
                    }
                    ClcMessage::Move {
                        num_backup, cmds, ..
                    } => {
                        let lost = std::mem::take(&mut dropped);
                        self.handle_move(slot, &cmds, num_backup, lost, incoming.sequence);
                    }
                    ClcMessage::Delta(seq) => self.clients[index].delta_seq = Some(seq),
                    _ => {}
                }
            }
        }
    }

    /// Runs one string command. A sign-on stage command is honoured only in the stage that
    /// expects it and only while `stage_done` is clear; honouring one sets `stage_done`. Any
    /// other command needs a token from the client's string command bucket.
    fn handle_stringcmd(&mut self, slot: u8, line: &str, stage_done: &mut bool) {
        let argv = tokenize_goldsrc_line(line);
        let Some(cmd) = argv.first() else {
            return;
        };
        let state = self.clients[usize::from(slot)].state;
        let is_stage_cmd = matches!(cmd.as_str(), "new" | "sendres" | "spawn" | "sendents");
        if is_stage_cmd && *stage_done {
            return;
        }
        if !is_stage_cmd && !self.clients[usize::from(slot)].stringcmds.try_take() {
            return;
        }
        match (cmd.as_str(), state) {
            // `new` while the `new` stage's output is the latest sent is a repeat: ignored.
            (
                "new",
                ClientState::Signon(
                    SignonStage::AwaitNew | SignonStage::AwaitSpawn | SignonStage::AwaitSendEnts,
                ),
            ) => self.stage_new(slot),
            ("sendres", ClientState::Signon(SignonStage::AwaitSendRes)) => {
                self.stage_sendres(slot);
            }
            ("spawn", ClientState::Signon(SignonStage::AwaitSpawn)) => {
                self.stage_spawn(slot, &argv);
            }
            ("sendents", ClientState::Signon(SignonStage::AwaitSendEnts)) => {
                self.clients[usize::from(slot)].state = ClientState::Active;
            }
            _ if is_stage_cmd => return,
            ("setinfo", _) => self.client_setinfo(slot, &argv),
            ("disconnect" | "dropclient", _) => self.release_slot(slot),
            _ => self.with_game(|game, eng| game.client_command(eng, slot, &argv)),
        }
        *stage_done |= is_stage_cmd;
        // Enforces the reliable backlog cap on what the stage wrote.
        self.drain_outbox();
    }

    /// Userinfo of the connected slots that `include` accepts.
    fn userinfo_entries(&self, include: impl Fn(&Client) -> bool) -> Vec<UserInfoEntry> {
        self.clients
            .iter()
            .zip(&self.views)
            .enumerate()
            .filter(|(_, (c, _))| !c.is_free() && include(c))
            .map(|(slot, (c, v))| UserInfoEntry {
                slot: slot as u8,
                userid: c.userid,
                info: v.userinfo.to_wire_string(),
            })
            .collect()
    }

    fn stage_new(&mut self, slot: u8) {
        let index = usize::from(slot);
        if self.clients[index].put_in_server {
            // Restarting after `spawn`: end the old session the way a reconnect does.
            self.with_game(|game, eng| game.client_disconnect(eng, slot));
            if self.clients[index].is_free() {
                return;
            }
            self.edicts.reset_client(slot);
            self.clients[index].put_in_server = false;
            self.views[index].active = false;
        }
        self.clients[index].reset_frames();
        let info = ServerInfoMsg {
            protocol: 48,
            spawn_count: self.spawn_count,
            map_crc: self.map.crc,
            client_dll_md5: [0; 16],
            max_clients: self.cfg.max_clients,
            player_slot: slot,
            is_deathmatch: true,
            game_dir: self.cfg.game_dir.clone(),
            hostname: self.cfg.hostname.clone(),
            map_path: self.map.path.clone(),
            map_cycle: String::new(),
            vac_secure: false,
        };
        let userinfos = self.userinfo_entries(|c| c.state == ClientState::Active);
        let mut w = MsgWriter::new();
        write_new_stage(
            &mut w,
            &info,
            self.cfg.build,
            &self.deltas,
            &move_vars(&self.cvars, &self.map.entities),
            &self.usermsgs,
            &userinfos,
        );
        let client = &mut self.clients[usize::from(slot)];
        client.chan.write_reliable(w.as_slice());
        client.state = ClientState::Signon(SignonStage::AwaitSendRes);
    }

    fn stage_sendres(&mut self, slot: u8) {
        // File sizes arrive with the file-transfer task.
        let resources = self.precache.resource_list(&|_| 0);
        let mut w = MsgWriter::new();
        write_sendres_stage(&mut w, self.spawn_count, &resources);
        let client = &mut self.clients[usize::from(slot)];
        client.chan.write_reliable(w.as_slice());
        client.state = ClientState::Signon(SignonStage::AwaitSpawn);
    }

    /// `spawn <spawn_count> <crc>`.
    fn stage_spawn(&mut self, slot: u8, argv: &[String]) {
        let index = usize::from(slot);
        let count = argv.get(1).and_then(|a| a.parse::<u32>().ok());
        if count != Some(self.spawn_count) {
            let mut w = MsgWriter::new();
            write_svc_stufftext(&mut w, "reconnect\n");
            let client = &mut self.clients[index];
            client.chan.write_reliable(w.as_slice());
            client.state = ClientState::Signon(SignonStage::AwaitNew);
            return;
        }
        let expected = munged_map_crc(self.map.crc, self.spawn_count);
        let crc = argv.get(2).and_then(|a| a.parse::<i32>().ok());
        if crc != Some(expected) {
            tracing::warn!(slot, ?crc, expected, "map CRC mismatch in spawn");
        }

        let mut w = MsgWriter::new();
        write_spawn_baselines(
            &mut w,
            &self.deltas,
            self.cfg.max_clients,
            &self.baselines,
            self.time,
        );
        self.clients[index].chan.write_reliable(w.as_slice());

        self.with_game(|game, eng| game.client_put_in_server(eng, slot));
        if self.clients[index].is_free() {
            // Kicked while being put in.
            return;
        }
        self.clients[index].put_in_server = true;
        self.views[index].active = true;

        let view_angles = self
            .edicts
            .get(self.edicts.client(slot))
            .map(|e| e.vars.v_angle)
            .unwrap_or_default();
        let userinfos = self.userinfo_entries(|_| true);
        let mut w = MsgWriter::new();
        write_spawn_stage(&mut w, view_angles, self.time, &userinfos);
        let client = &mut self.clients[index];
        client.chan.write_reliable(w.as_slice());
        client.state = ClientState::Signon(SignonStage::AwaitSendEnts);
    }

    /// Frees clients that did not reach `Active` within `signon_timeout_secs` of connecting.
    fn drop_stalled_signons(&mut self) {
        for slot in 0..self.cfg.max_clients {
            let client = &self.clients[usize::from(slot)];
            if matches!(client.state, ClientState::Signon(_))
                && self.time - client.connected_at > self.cfg.signon_timeout_secs
            {
                self.free_slot(slot);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_brief() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.hostname, "Counter-Strike 1.6 Rust");
        assert_eq!(cfg.max_clients, 16);
        assert_eq!(cfg.game_dir, "cstrike");
        assert_eq!(cfg.build, 8684);
        assert_eq!((cfg.timeout_secs, cfg.signon_timeout_secs), (60.0, 120.0));
        assert_eq!((cfg.min_update_rate, cfg.max_update_rate), (10, 102));
        assert_eq!((cfg.min_rate, cfg.max_rate), (1000, 100_000));
    }
}
