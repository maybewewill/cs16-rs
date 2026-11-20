//! The server: client slots, connectionless dispatch, one netchan per client, the sign-on
//! sequence and the per-frame datagram to every client.

use std::collections::BTreeMap;

use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{DeltaRecord, DeltaRegistry, DeltaSchema, GoldSrcEntityState};
use cs_proto48::handshake::{encode_s2c_connection, encode_s2c_reject, tokenize_goldsrc_line};
use cs_proto48::messages::{
    ClcMessage, ServerInfoMsg, entity_schema_name, parse_clc_stream, wire_entity_type,
    write_svc_clientdata, write_svc_packetentities_full, write_svc_stufftext, write_svc_time,
    write_svc_updateuserinfo,
};
use cs_proto48::query::{PlayerQueryEntry, cs16_server_query_info};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{MAX_UDP_PACKET, PacketTransport, PeerAddr};

use crate::client::{Client, ClientState, SignonStage};
use crate::edict::{DEFAULT_MAX_EDICTS, EdictPool};
use crate::game::{ClientView, EngineApi, GameDll, Outbox, entity_state_from_vars};
use crate::map::ServerMap;
use crate::oob::{ChallengeIssuer, ConnectRequest, OobContext, OobLimiter, OobOutcome, handle_oob};
use crate::precache::PrecacheTables;
use crate::signon::{
    UserInfoEntry, move_vars, munged_map_crc, write_new_stage, write_sendres_stage,
    write_spawn_baselines, write_spawn_stage,
};

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
    cfg: ServerConfig,
    map: ServerMap,
    game: G,
    deltas: DeltaRegistry,
    cvars: CVarRegistry,
    usermsgs: UserMsgRegistry,
    edicts: EdictPool,
    precache: PrecacheTables,
    outbox: Outbox,
    /// What the game sees of each slot, indexed like `clients`.
    views: Vec<ClientView>,
    clients: Vec<Client>,
    /// Entity states sent with `svc_spawnbaseline`, by entity number.
    baselines: Vec<(u16, GoldSrcEntityState)>,
    /// The same baselines as delta records, the base of full entity updates.
    baseline_records: BTreeMap<u16, (u8, DeltaRecord)>,
    usercmd_schema: Option<DeltaSchema>,
    challenges: ChallengeIssuer,
    limiter: OobLimiter,
    spawn_count: u32,
    next_userid: i32,
    time: f64,
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
        };
        server.with_game(|game, eng| game.server_activate(eng));
        server.precache.lock();
        server.compute_baselines();
        server
    }

    /// One server frame: handles every waiting datagram, frees clients whose sign-on took too
    /// long, queues the per-frame update and sends one datagram to every connected client.
    pub fn frame(&mut self, net: &mut dyn PacketTransport, now: f64) {
        self.time = now;
        while let Some((from, data)) = net.recv() {
            self.handle_datagram(net, from, &data);
        }
        self.drop_stalled_signons();
        self.queue_full_entity_updates();
        self.send_datagrams(net);
    }

    pub fn client_state(&self, slot: u8) -> ClientState {
        self.clients
            .get(usize::from(slot))
            .map_or(ClientState::Free, |c| c.state)
    }

    pub fn client_count(&self) -> usize {
        self.clients.iter().filter(|c| !c.is_free()).count()
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
    fn with_game<R>(&mut self, f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R) -> R {
        let result = self.call_game(f);
        self.drain_outbox();
        result
    }

    /// Calls into the game without draining the outbox.
    fn call_game<R>(&mut self, f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R) -> R {
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
    fn drain_outbox(&mut self) {
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
                client.unreliable.extend_from_slice(unreliable.as_slice());
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

            let kicks = std::mem::take(&mut self.outbox.kicks);
            if kicks.is_empty() {
                return;
            }
            // Telling the kicked client why is left to a later task.
            for (slot, _reason) in kicks {
                self.release_slot(slot);
            }
        }
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
    fn free_slot(&mut self, slot: u8) {
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

    fn slot_of(&self, addr: &PeerAddr) -> Option<u8> {
        self.clients
            .iter()
            .position(|c| c.addr.as_ref() == Some(addr))
            .map(|i| i as u8)
    }

    fn free_slot_index(&self) -> Option<u8> {
        self.clients
            .iter()
            .position(Client::is_free)
            .map(|i| i as u8)
    }

    fn handle_datagram(&mut self, net: &mut dyn PacketTransport, from: PeerAddr, data: &[u8]) {
        if data.starts_with(&[0xFF; 4]) {
            self.handle_connectionless(net, from, data);
        } else if let Some(slot) = self.slot_of(&from) {
            self.handle_netchan(slot, data);
        }
    }

    fn handle_connectionless(
        &mut self,
        net: &mut dyn PacketTransport,
        from: PeerAddr,
        data: &[u8],
    ) {
        if !self.limiter.allow(&from, self.time) {
            return;
        }
        let players: Vec<PlayerQueryEntry> = self
            .clients
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_free())
            .map(|(slot, c)| PlayerQueryEntry {
                index: slot as u8,
                name: self.views[slot]
                    .userinfo
                    .get("name")
                    .unwrap_or_default()
                    .to_string(),
                score: 0,
                duration_secs: (self.time - c.connected_at) as f32,
            })
            .collect();
        let query = cs16_server_query_info(
            &self.cfg.hostname,
            &self.map.name,
            players.len(),
            usize::from(self.cfg.max_clients),
            0,
        );
        let ctx = OobContext {
            challenges: &self.challenges,
            query: &query,
            players: &players,
            rules: &[],
            is_full: self.free_slot_index().is_none() && self.slot_of(&from).is_none(),
        };
        let reply = match handle_oob(data, &from, self.time, &ctx) {
            OobOutcome::Reply(reply) => reply,
            OobOutcome::Connect(request) => self.connect(from, request),
            OobOutcome::Ignore => return,
        };
        let _ = net.send(from, &reply);
    }

    /// Accepts a checked connect request and returns the reply for it.
    fn connect(&mut self, from: PeerAddr, request: ConnectRequest) -> Vec<u8> {
        let slot = match self.slot_of(&from) {
            Some(slot) => {
                self.free_slot(slot);
                slot
            }
            // `handle_oob` already rejected a full server.
            None => match self.free_slot_index() {
                Some(slot) => slot,
                None => return encode_s2c_reject("Server is full.\n"),
            },
        };
        let index = usize::from(slot);
        let mut userinfo = request.userinfo;
        userinfo.set("name", self.unique_name(&request.name));
        self.views[index] = ClientView {
            userinfo,
            ..ClientView::default()
        };
        self.clients[index] = Client::connected(from, self.next_userid, self.time);

        if let Err(reason) = self.with_game(|game, eng| game.client_connect(eng, slot)) {
            self.free_slot(slot);
            return encode_s2c_reject(&reason);
        }
        let userid = self.next_userid;
        self.next_userid += 1;
        encode_s2c_connection(userid, &from.to_string(), false, self.cfg.build)
    }

    /// `name`, or `(1)name`, `(2)name`… when a connected client already uses it.
    fn unique_name(&self, name: &str) -> String {
        let taken = |candidate: &str| {
            self.clients.iter().zip(&self.views).any(|(c, v)| {
                !c.is_free()
                    && v.userinfo
                        .get("name")
                        .is_some_and(|n| n.eq_ignore_ascii_case(candidate))
            })
        };
        let mut candidate = name.to_string();
        let mut n = 1;
        while taken(&candidate) {
            candidate = format!("({n}){name}");
            n += 1;
        }
        candidate
    }

    fn handle_netchan(&mut self, slot: u8, data: &[u8]) {
        let index = usize::from(slot);
        let Ok(Some(incoming)) = self.clients[index].chan.process_packet(data) else {
            return;
        };
        let Some(schema) = self.usercmd_schema.clone() else {
            return;
        };
        // A completed file stream (`completed_file_stream`) is discarded.
        let payloads = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for payload in payloads {
            let Ok(messages) = parse_clc_stream(&payload, &schema, incoming.sequence) else {
                continue;
            };
            for message in messages {
                if self.clients[index].is_free() {
                    return;
                }
                // User commands, deltas and resource lists are handled by later tasks.
                if let ClcMessage::StringCmd(line) = message {
                    self.handle_stringcmd(slot, &line);
                }
            }
        }
    }

    fn handle_stringcmd(&mut self, slot: u8, line: &str) {
        let argv = tokenize_goldsrc_line(line);
        let Some(cmd) = argv.first() else {
            return;
        };
        let state = self.clients[usize::from(slot)].state;
        match (cmd.as_str(), state) {
            ("new", ClientState::Signon(_)) => self.stage_new(slot),
            ("sendres", ClientState::Signon(SignonStage::AwaitSendRes)) => {
                self.stage_sendres(slot);
            }
            ("spawn", ClientState::Signon(SignonStage::AwaitSpawn)) => {
                self.stage_spawn(slot, &argv);
            }
            ("sendents", ClientState::Signon(SignonStage::AwaitSendEnts)) => {
                self.clients[usize::from(slot)].state = ClientState::Active;
            }
            ("new" | "sendres" | "spawn" | "sendents", _) => {}
            _ => self.with_game(|game, eng| game.client_command(eng, slot, &argv)),
        }
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

    /// Stand-in for delta frames: every `Active` client gets `svc_time`, its client data
    /// without a delta base and a full `svc_packetentities` of every live edict with a model.
    fn queue_full_entity_updates(&mut self) {
        let entities: Vec<GoldSrcEntityState> = self
            .edicts
            .iter_live()
            .filter(|(_, e)| e.vars.modelindex != 0)
            .map(|(id, e)| entity_state_from_vars(id.index, &e.vars))
            .collect();
        for slot in 0..self.cfg.max_clients {
            if self.clients[usize::from(slot)].state != ClientState::Active {
                continue;
            }
            let client_data = self.with_game(|game, eng| game.update_client_data(eng, slot));
            let mut w = MsgWriter::new();
            write_svc_time(&mut w, self.time as f32);
            if let Some(schema) = self.deltas.get("clientdata_t") {
                write_svc_clientdata(&mut w, schema, None, &client_data, self.time);
            }
            write_svc_packetentities_full(
                &mut w,
                &self.deltas,
                self.cfg.max_clients,
                &self.baseline_records,
                &entities,
                self.time,
            );
            let client = &mut self.clients[usize::from(slot)];
            if client.state == ClientState::Active {
                client.unreliable.extend_from_slice(w.as_slice());
            }
        }
    }

    /// One netchan datagram to every connected client. When the queued unreliable bytes would
    /// push the datagram past `MAX_UDP_PACKET`, they are dropped for this frame.
    fn send_datagrams(&mut self, net: &mut dyn PacketTransport) {
        for client in &mut self.clients {
            let Some(addr) = client.addr else {
                continue;
            };
            let unreliable = std::mem::take(&mut client.unreliable);
            let packet = if unreliable.is_empty() {
                client.chan.build_packet(&[])
            } else {
                let mut trial = client.chan.clone();
                let packet = trial.build_packet(&unreliable);
                if packet.len() <= MAX_UDP_PACKET {
                    client.chan = trial;
                    packet
                } else {
                    client.chan.build_packet(&[])
                }
            };
            if packet.len() > MAX_UDP_PACKET {
                tracing::error!(%addr, len = packet.len(), "netchan packet exceeds MAX_UDP_PACKET");
                continue;
            }
            let _ = net.send(addr, &packet);
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
