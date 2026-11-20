//! Connectionless packets and connect handling: accepting a connect, choosing its slot,
//! making its name unique, and the reject and connection replies.

use cs_proto48::handshake::{encode_s2c_connection, encode_s2c_reject};
use cs_proto48::query::{PlayerQueryEntry, cs16_server_query_info};
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
        userinfo.set("name", self.unique_name(&request.name, None));
        self.views[index] = ClientView {
            userinfo,
            ..ClientView::default()
        };
        self.clients[index] = Client::connected(from, self.next_userid, self.time);

        let result = self.call_game(|game, eng| game.client_connect(eng, slot));
        let kick_reason = self
            .outbox
            .kicks
            .iter()
            .find(|(kicked, _)| *kicked == slot)
            .map(|(_, reason)| reason.clone());
        self.drain_outbox();
        if let Err(reason) = result {
            self.free_slot(slot);
            return encode_s2c_reject(&reason);
        }
        if self.clients[index].is_free() {
            // Kicked, or dropped by the reliable backlog cap, during `client_connect`: no
            // userid is used up.
            let reason = kick_reason.unwrap_or_else(|| "Reliable channel overflowed\n".to_string());
            return encode_s2c_reject(&reason);
        }
        let userid = self.next_userid;
        self.next_userid += 1;
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
