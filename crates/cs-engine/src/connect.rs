//! Connectionless packets and connect handling: accepting a connect, choosing its slot,
//! making its name unique, and the reject and connection replies.

use cs_proto48::handshake::{encode_s2c_connection, encode_s2c_reject};
use cs_proto48::query::{
    A2S_INFO, A2S_PLAYER, A2S_RULES, PlayerQueryEntry, ServerQueryInfo, cs16_server_query_info,
};
use cs_transport::{PacketTransport, PeerAddr};

use crate::client::Client;
use crate::game::{ClientView, GameDll};
use crate::oob::{ConnectRequest, MAX_NAME_BYTES, OobContext, OobOutcome, handle_oob};
use crate::server::Server;

impl<G: GameDll> Server<G> {
    /// Fragment bytes the client in `slot` has buffered for reassembly; 0 for a free slot.
    pub fn client_incoming_buffered_bytes(&self, slot: u8) -> usize {
        self.clients
            .get(usize::from(slot))
            .map_or(0, |c| c.chan.incoming_buffered_bytes())
    }

    pub(crate) fn free_slot_index(&self) -> Option<u8> {
        self.clients
            .iter()
            .position(Client::is_free)
            .map(|i| i as u8)
    }

    pub(crate) fn handle_connectionless(
        &mut self,
        net: &mut dyn PacketTransport,
        from: PeerAddr,
        data: &[u8],
    ) {
        // Every datagram is charged to its address.
        if !self.limiter.allow(&from, self.real_time) {
            self.net_stats.oob_denied += 1;
            return;
        }
        if let Some(challenge) = crate::master::MasterRegistration::parse_challenge_reply(data)
            && self.masters.is_registered(&from)
        {
            let sv_pw = self.cvars.get_str("sv_password").trim();
            let password = !sv_pw.is_empty() && !sv_pw.eq_ignore_ascii_case("none");
            let snap = crate::master::HeartbeatSnapshot {
                players: self.client_count().min(255) as u8,
                max_players: self.cfg.max_clients,
                bots: self.fake_client_count().min(255) as u8,
                gamedir: self.cfg.game_dir.clone(),
                map: self.map.name.clone(),
                password,
                build: self.cfg.build.max(0) as u32,
            };
            let pkt = crate::master::MasterRegistration::encode_heartbeat_update(challenge, &snap);
            self.net_stats.send(net, from, &pkt);
            return;
        }
        if let Some(dgram) = crate::rcon::parse_rcon_datagram(data) {
            let policy = crate::rcon::RconPolicy {
                max_failures: self.cvars.get_i32("sv_rcon_maxfailures").max(1) as u32,
                min_failures: self.cvars.get_i32("sv_rcon_minfailures").max(1) as u32,
                min_failure_time: f64::from(self.cvars.get_f32("sv_rcon_minfailuretime")),
                ban_penalty_mins: f64::from(self.cvars.get_f32("sv_rcon_banpenalty")),
            }
            .clamped();
            let pw = self.cvars.get_str("rcon_password").to_string();
            match self.rcon.evaluate(from, self.real_time, dgram, &pw, policy) {
                crate::rcon::RconDecision::Reply(pkts) => {
                    for pkt in pkts {
                        self.net_stats.send(net, from, &pkt);
                    }
                }
                crate::rcon::RconDecision::Execute(cmd) => {
                    self.logger
                        .log(self.real_time, &format!("Rcon: \"{cmd}\" from \"{from}\""));
                    let reply_text = match self.console_command(&cmd) {
                        crate::console::ConsoleOutcome::Output(text) => text,
                        crate::console::ConsoleOutcome::ChangeLevel(map) => {
                            self.outbox.change_level = Some(map.clone());
                            format!("changing level to {map}\n")
                        }
                        crate::console::ConsoleOutcome::Quit => {
                            self.shutdown(net, "Server shutting down");
                            "Server shutting down\n".to_string()
                        }
                    };
                    for pkt in crate::rcon::encode_rcon_reply(&reply_text) {
                        self.net_stats.send(net, from, &pkt);
                    }
                }
            }
            return;
        }
        // The player list and query info are only built for the queries that send them.
        let is_query = matches!(data.get(4), Some(&(A2S_INFO | A2S_PLAYER | A2S_RULES)));
        if is_query {
            let max_q_sec = self
                .cvars
                .get("max_queries_sec")
                .map_or(3.0, |c| f64::from(c.as_f32()));
            let max_q_global = self
                .cvars
                .get("max_queries_sec_global")
                .map_or(30.0, |c| f64::from(c.as_f32()));
            let max_q_win = self
                .cvars
                .get("max_queries_window")
                .map_or(60.0, |c| f64::from(c.as_f32()));
            if !self
                .query_limiter
                .allow(&from, self.real_time, max_q_sec, max_q_global, max_q_win)
            {
                self.net_stats.oob_denied += 1;
                return;
            }
        }
        let (players, query) = if is_query {
            self.query_answers()
        } else {
            (Vec::new(), cs16_server_query_info("", "", 0, 0, 0))
        };
        let rules_owned: Vec<(&str, String)> = if data.get(4) == Some(&A2S_RULES) {
            self.cvars
                .vars()
                .values()
                .filter(|c| c.flags.contains(cs_cvar::CVarFlags::SERVER))
                .map(|c| (c.name.as_str(), c.public_rules_value()))
                .collect()
        } else {
            Vec::new()
        };
        let rule_refs: Vec<(&str, &str)> = rules_owned
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect();
        let ctx = OobContext {
            challenges: &self.challenges,
            query: &query,
            players: &players,
            rules: &rule_refs,
            is_full: self.free_slot_index().is_none() && self.slot_of(&from).is_none(),
            sv_contact: self.cvars.get_str("sv_contact"),
        };
        let (outcome, challenged) = handle_oob(data, &from, self.real_time, &ctx);
        let reply = match outcome {
            // A reply to a datagram without a challenge issued to its sender is also charged to
            // the global budget; datagrams that get no reply cost nothing from it.
            OobOutcome::Reply(reply) => {
                if !challenged && !self.oob_budget.try_take() {
                    self.net_stats.oob_denied += 1;
                    return;
                }
                reply
            }
            OobOutcome::Connect(request) => self.connect(from, request),
            OobOutcome::Ignore => return,
        };
        self.net_stats.send(net, from, &reply);
    }

    /// The player list and server info that A2S queries are answered with.
    fn query_answers(&self) -> (Vec<PlayerQueryEntry>, ServerQueryInfo) {
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
                duration_secs: (self.real_time - c.connected_at) as f32,
            })
            .collect();
        let mut query = cs16_server_query_info(
            &self.hostname(),
            &self.map.name,
            players.len(),
            usize::from(self.cfg.max_clients),
            self.fake_client_count(),
        );
        let sv_pw = self.cvars.get_str("sv_password").trim();
        query.password_protected = !sv_pw.is_empty() && !sv_pw.eq_ignore_ascii_case("none");
        (players, query)
    }

    /// Accepts a checked connect request and returns the reply for it.
    fn connect(&mut self, from: PeerAddr, request: ConnectRequest) -> Vec<u8> {
        if self.bans.is_ip_banned(&from, self.real_time)
            || self.rcon.is_ip_banned(&from, self.real_time)
        {
            return encode_s2c_reject("You have been banned from this server.\n");
        }
        let sv_pw = self.cvars.get_str("sv_password").trim().to_string();
        if !sv_pw.is_empty()
            && !sv_pw.eq_ignore_ascii_case("none")
            && request.userinfo.get("password").unwrap_or("") != sv_pw
        {
            return encode_s2c_reject("Invalid server password.\n");
        }
        let auth_id = crate::bans::derive_auth_id(
            &request.protinfo.to_wire_string(),
            request.userinfo.get("*sid"),
            &from,
        );
        if self.bans.is_id_banned(&auth_id, self.real_time) {
            return encode_s2c_reject("STEAM UserID is banned.\n");
        }
        let slot = match self.slot_of(&from) {
            Some(slot) => {
                self.free_slot(slot);
                slot
            }
            // `handle_oob` already rejected a full server.
            None => {
                // Retail `swds.dll:FUN_10205640`, `FUN_10206040`: reject `> 5` simultaneous
                // pending connections from the same IP.
                let pending_same_ip = self
                    .clients
                    .iter()
                    .filter(|c| {
                        !c.is_free()
                            && c.state != crate::client::ClientState::Active
                            && c.addr.as_ref().is_some_and(|a| same_ip(a, &from))
                    })
                    .count();
                if pending_same_ip >= 5 {
                    return encode_s2c_reject("Invalid connection.\n");
                }
                match self.free_slot_index() {
                    Some(slot) => slot,
                    None => return encode_s2c_reject("Server is full.\n"),
                }
            }
        };
        let index = usize::from(slot);
        let mut userinfo = request.userinfo;
        userinfo.set("name", self.unique_name(&request.name, None));
        // Nobody is told yet: `sendinfo` is set by `SV_WriteSpawn` (`stage_spawn`), not at connect.
        self.views[index] = ClientView {
            userinfo,
            in_use: true,
            ..ClientView::default()
        };
        self.clients[index] = Client::connected(from, self.next_userid, self.real_time);
        self.clients[index].auth_id = auth_id;
        if let Some(entry) = self.last_userinfo_hash.get_mut(index) {
            entry.0.clear();
            entry.1 = -100.0;
        }
        self.logger.log(
            self.real_time,
            &format!(
                "\"{}<{}>\" connected, address \"{}\"",
                self.views[index].userinfo.get("name").unwrap_or(""),
                self.next_userid,
                from
            ),
        );

        let result = self.call_game(|game, eng| game.client_connect(eng, slot));
        // A kick of the connecting client is answered with the reject, not `svc_disconnect`.
        let kick_at = self.outbox.kicks.iter().position(|(kicked, _)| *kicked == slot);
        let kick_reason = kick_at.map(|at| self.outbox.kicks.remove(at).1);
        self.drain_outbox();
        // Refused, kicked, or dropped by the reliable backlog cap during `client_connect`: no
        // userid is used up.
        if let Err(reason) = result {
            self.free_slot(slot);
            return encode_s2c_reject(&reason);
        }
        if let Some(reason) = kick_reason {
            self.free_slot(slot);
            return encode_s2c_reject(&reason);
        }
        if self.clients[index].is_free() {
            return encode_s2c_reject("Reliable channel overflowed\n");
        }
        let userid = self.next_userid;
        self.next_userid += 1;
        // Farewells built from the address's previous netchan would reach the new one.
        self.farewells.retain(|(addr, _)| *addr != from);
        encode_s2c_connection(userid, &from.to_string(), false, self.cfg.build)
    }

    #[cfg(test)]
    pub(crate) fn handle_oob_connect_for_test(&mut self, from: PeerAddr, data: &[u8]) -> Vec<u8> {
        let ctx = OobContext {
            challenges: &self.challenges,
            query: &cs16_server_query_info("", "", 0, 0, 0),
            players: &[],
            rules: &[],
            is_full: self.free_slot_index().is_none() && self.slot_of(&from).is_none(),
            sv_contact: self.cvars.get_str("sv_contact"),
        };
        match handle_oob(data, &from, self.real_time, &ctx).0 {
            OobOutcome::Connect(req) => self.connect(from, req),
            OobOutcome::Reply(rep) => rep,
            OobOutcome::Ignore => Vec::new(),
        }
    }

    /// `name`, or `(1)name`, `(2)name`… when a connected client other than `except` already
    /// uses it; a prefixed name is cut on a character boundary to stay within
    /// [`MAX_NAME_BYTES`].
    pub(crate) fn unique_name(&self, name: &str, except: Option<u8>) -> String {
        unique_name_among(&self.views, name, except)
    }
}

fn same_ip(a: &PeerAddr, b: &PeerAddr) -> bool {
    match (a, b) {
        (PeerAddr::Udp(sa), PeerAddr::Udp(sb)) => sa.ip() == sb.ip(),
        _ => a == b,
    }
}

/// [`Server::unique_name`] over the slots the game sees; `create_fake_client` names a bot with it.
pub(crate) fn unique_name_among(views: &[ClientView], name: &str, except: Option<u8>) -> String {
    let taken = |candidate: &str| {
        views.iter().enumerate().any(|(slot, v)| {
            Some(slot as u8) != except
                && v.in_use
                && v.userinfo
                    .get("name")
                    .is_some_and(|n| n.eq_ignore_ascii_case(candidate))
        })
    };
    let mut candidate = name.to_string();
    let mut n = 1;
    while taken(&candidate) {
        let prefix = format!("({n})");
        let mut end = name.len().min(MAX_NAME_BYTES.saturating_sub(prefix.len()));
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        candidate = format!("{prefix}{}", &name[..end]);
        n += 1;
    }
    candidate
}
