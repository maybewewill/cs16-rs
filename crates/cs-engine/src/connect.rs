//! Connectionless packets and connect handling: accepting a connect, choosing its slot,
//! making its name unique, and the reject and connection replies.

use cs_proto48::handshake::{encode_s2c_connection, encode_s2c_reject};
use cs_proto48::query::{
    A2S_INFO, A2S_PLAYER, A2S_RULES, PlayerQueryEntry, ServerQueryInfo, cs16_server_query_info,
};
use cs_transport::{PacketTransport, PeerAddr};

use crate::client::Client;
use crate::game::{ClientView, GameDll};
use crate::oob::{
    ConnectRequest, MAX_NAME_BYTES, OobContext, OobOutcome, carries_valid_challenge, handle_oob,
};
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
        if !self.limiter.allow(&from, self.time) {
            self.net_stats.oob_denied += 1;
            return;
        }
        // The player list and query info are only built for the queries that send them.
        let is_query = matches!(data.get(4), Some(&(A2S_INFO | A2S_PLAYER | A2S_RULES)));
        let (players, query) = if is_query {
            self.query_answers()
        } else {
            (Vec::new(), cs16_server_query_info("", "", 0, 0, 0))
        };
        let ctx = OobContext {
            challenges: &self.challenges,
            query: &query,
            players: &players,
            rules: &[],
            is_full: self.free_slot_index().is_none() && self.slot_of(&from).is_none(),
        };
        let reply = match handle_oob(data, &from, self.time, &ctx) {
            // A reply to a datagram without a challenge issued to its sender is also charged to
            // the global budget; datagrams that get no reply cost nothing from it.
            OobOutcome::Reply(reply) => {
                if !carries_valid_challenge(data, &from, self.time, &self.challenges)
                    && !self.oob_budget.try_take()
                {
                    self.net_stats.oob_denied += 1;
                    return;
                }
                reply
            }
            OobOutcome::Connect(_) if self.shutting_down => {
                encode_s2c_reject("Server is shutting down.\n")
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
        (players, query)
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
        userinfo.set("name", self.unique_name(&request.name, None));
        // Everyone past `new` is told about the newcomer (replacing whoever had the slot).
        self.views[index] = ClientView {
            userinfo,
            in_use: true,
            userinfo_dirty: true,
            ..ClientView::default()
        };
        self.clients[index] = Client::connected(from, self.next_userid, self.time);

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

    /// `name`, or `(1)name`, `(2)name`… when a connected client other than `except` already
    /// uses it; a prefixed name is cut on a character boundary to stay within
    /// [`MAX_NAME_BYTES`].
    pub(crate) fn unique_name(&self, name: &str, except: Option<u8>) -> String {
        let taken = |candidate: &str| {
            self.clients
                .iter()
                .zip(&self.views)
                .enumerate()
                .any(|(slot, (c, v))| {
                    Some(slot as u8) != except
                        && !c.is_free()
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
}
