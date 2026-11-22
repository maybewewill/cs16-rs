//! The end of a client's life and the housekeeping around it: releasing a slot for any reason
//! (telling a kicked client why), timeouts, `shutdown`, map changes, userinfo broadcasts and
//! pings.
//!
//! The ping measure (average round trip of the last [`PING_SAMPLES`] acknowledged datagrams)
//! and the three `svc_disconnect` datagrams of a kick are working assumptions, unverified
//! against a real client.

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::messages::{
    write_svc_disconnect, write_svc_pings, write_svc_stufftext, write_svc_updateuserinfo,
};
use cs_transport::PacketTransport;

use crate::client::{Client, ClientState};
use crate::edict::MAX_EDICTS_WIRE;
use crate::game::{ClientView, GameDll};
use crate::map::ServerMap;
use crate::server::{SV_TIME_AT_MAP_START, Server, ServerConfig};
use crate::world::WorldLinks;

/// Seconds between the `svc_pings` messages sent to active clients.
pub const PINGS_INTERVAL_SECS: f64 = 2.0;
/// Shortest time between two accepted name changes of one client.
pub const NAME_CHANGE_MIN_INTERVAL_SECS: f64 = 1.0;
/// `setinfo` commands a client may send per second, sustained. Every accepted one is
/// broadcast to all players, so this is far below the general string command limit.
pub const SETINFO_RATE_PER_SEC: f64 = 2.0;
/// `setinfo` commands a client may send in one burst.
pub const SETINFO_BURST: f64 = 8.0;

/// Datagrams carrying `svc_disconnect` sent to a client that is kicked or shut down.
const DISCONNECT_DATAGRAMS: usize = 3;
/// Longest `svc_disconnect` reason sent, in bytes.
const MAX_DISCONNECT_REASON_BYTES: usize = 128;
/// Round-trip samples a client's ping is averaged over.
const PING_SAMPLES: usize = 16;
/// Sequences whose send times are kept per client.
const PING_SEND_HISTORY: usize = 64;
/// Most client slots a server may have.
pub(crate) const MAX_CLIENT_SLOTS: u8 = 32;
/// Lowest `min_rate` a server may be configured with.
const MIN_RATE_FLOOR: u32 = 1000;

/// Send times of a client's recent datagrams and the round trips measured from their
/// acknowledgements.
#[derive(Debug, Clone)]
pub(crate) struct PingTracker {
    /// `(sequence, server time sent)`, indexed by sequence modulo [`PING_SEND_HISTORY`].
    sent: [Option<(u32, f64)>; PING_SEND_HISTORY],
    /// Round trips in seconds, a ring of the last [`PING_SAMPLES`].
    samples: [f64; PING_SAMPLES],
    count: usize,
    next: usize,
}

impl Default for PingTracker {
    fn default() -> Self {
        Self {
            sent: [None; PING_SEND_HISTORY],
            samples: [0.0; PING_SAMPLES],
            count: 0,
            next: 0,
        }
    }
}

impl PingTracker {
    /// Remembers that datagram `seq` went out at `now`.
    pub(crate) fn record_sent(&mut self, seq: u32, now: f64) {
        self.sent[seq as usize % PING_SEND_HISTORY] = Some((seq, now));
    }

    /// A datagram accepted at `now` acknowledged sequence `seq_ack`: takes a sample when that
    /// sequence's send time is still kept. Each sequence gives at most one sample.
    pub(crate) fn acknowledged(&mut self, seq_ack: u32, now: f64) {
        let entry = &mut self.sent[seq_ack as usize % PING_SEND_HISTORY];
        let Some((seq, sent_at)) = *entry else {
            return;
        };
        if seq != seq_ack {
            return;
        }
        *entry = None;
        self.samples[self.next] = (now - sent_at).max(0.0);
        self.next = (self.next + 1) % PING_SAMPLES;
        self.count = (self.count + 1).min(PING_SAMPLES);
    }

    /// The average of the kept samples in milliseconds, rounded; 0 without samples.
    pub(crate) fn ping_ms(&self) -> u16 {
        if self.count == 0 {
            return 0;
        }
        let average = self.samples[..self.count].iter().sum::<f64>() / self.count as f64;
        // `as` saturates: a huge average becomes `u16::MAX`, clamped again by the writer.
        (average * 1000.0).round() as u16
    }
}

impl ServerConfig {
    /// The configuration with unusable values replaced: `min_rate` at least 1000, `max_rate`
    /// at least `min_rate`, `min_update_rate` at least 1, `max_update_rate` at least
    /// `min_update_rate`, `max_clients` within 1..=32, `max_edicts` at most
    /// [`MAX_EDICTS_WIRE`], and timeouts that are not positive and finite set to the defaults.
    pub(crate) fn sanitized(mut self) -> Self {
        let defaults = ServerConfig::default();
        self.min_rate = self.min_rate.max(MIN_RATE_FLOOR);
        self.max_rate = self.max_rate.max(self.min_rate);
        self.min_update_rate = self.min_update_rate.max(1);
        self.max_update_rate = self.max_update_rate.max(self.min_update_rate);
        self.max_clients = self.max_clients.clamp(1, MAX_CLIENT_SLOTS);
        self.max_edicts = self.max_edicts.min(MAX_EDICTS_WIRE);
        let usable = |secs: f64| secs.is_finite() && secs > 0.0;
        if !usable(self.timeout_secs) {
            self.timeout_secs = defaults.timeout_secs;
        }
        if !usable(self.signon_timeout_secs) {
            self.signon_timeout_secs = defaults.signon_timeout_secs;
        }
        self
    }
}

impl<G: GameDll> Server<G> {
    /// Switches to `map` without dropping anyone: the game deactivates the old map and every
    /// client is taken out of the server, the spawn count goes up, the edicts (with serials no
    /// old id can match), the area tree and the precache tables are rebuilt for `map` and the
    /// game activates it; then every connected client restarts its sign-on, its old reliable
    /// data discarded and `reconnect` sent through the reliable stream. The init buffer and the light styles of
    /// the old map are cleared (the new map's activation refills them). Clients are not reported
    /// to the game as disconnected: its `server_deactivate` owns that state. Call it between
    /// [`frame`] calls.
    ///
    /// A map change the game asked for before the switch, or while it deactivated the old map or
    /// activated the new one, is forgotten when the switch is done: it belongs to the map that is
    /// gone and must not take the new one away (it is cleared after `activate_map`, so a request
    /// made by `server_activate` is dropped too; the game asks again from its first frame).
    ///
    /// [`frame`]: Server::frame
    pub fn change_map(&mut self, map: ServerMap) {
        self.with_game(|game, eng| game.server_deactivate(eng));
        for (client, view) in self.clients.iter_mut().zip(&mut self.views) {
            client.put_in_server = false;
            view.active = false;
        }
        self.spawn_count += 1;
        self.outbox.reset_map_state();
        self.edicts = self
            .edicts
            .successor(self.cfg.max_clients, self.cfg.max_edicts);
        self.sv_time = SV_TIME_AT_MAP_START;
        self.warned_movetypes.clear();
        let decals = self.precache.decals().to_vec();
        let loader = self.precache.model_loader();
        self.precache = Self::map_precache(&map);
        self.precache.set_decals(decals);
        self.precache.set_model_loader(loader);
        self.world = WorldLinks::new(map.world_bounds());
        self.map = map;
        self.activate_map();
        self.outbox.change_level = None;
        let mut reconnect = MsgWriter::new();
        write_svc_stufftext(&mut reconnect, "reconnect\n");
        let now = self.real_time;
        for client in self.clients.iter_mut().filter(|c| !c.is_free()) {
            client.restart_signon(now);
            client.chan.write_reliable(reconnect.as_slice());
        }
    }

    /// The map the game asked to change to with `EngineApi::change_level` since the last call,
    /// if any (the name is already validated). The caller loads it and calls
    /// [`change_map`](Self::change_map), as for the console's `changelevel`; the request is
    /// forgotten here whether or not the map loads.
    pub fn take_pending_change_level(&mut self) -> Option<String> {
        self.outbox.change_level.take()
    }

    /// Releases the client in `slot`, sending it `svc_disconnect(reason)` at the end of the
    /// next frame. A free slot is left alone.
    pub fn kick(&mut self, slot: u8, reason: &str) {
        self.release_slot(slot, Some(reason));
        self.drain_outbox();
    }

    /// Sends every connected client `svc_disconnect(reason)` and releases it (the game hears
    /// `client_disconnect` for those it had put in), then lets the game deactivate the map
    /// (`server_deactivate`, once: a repeated shutdown does not call it again); from then on
    /// [`frame`](Self::frame) does nothing, so no connect or query is read.
    pub fn shutdown(&mut self, net: &mut dyn PacketTransport, reason: &str) {
        let first = !self.shutting_down;
        self.shutting_down = true;
        for slot in 0..self.cfg.max_clients {
            self.release_slot(slot, Some(reason));
        }
        self.drain_outbox();
        if first {
            self.with_game(|game, eng| game.server_deactivate(eng));
        }
        self.send_farewells(net);
    }

    /// Unacknowledged reliable bytes queued for the client in `slot`; 0 for a free slot.
    pub fn client_reliable_backlog(&self, slot: u8) -> usize {
        self.clients
            .get(usize::from(slot))
            .filter(|c| !c.is_free())
            .map_or(0, |c| c.chan.pending_reliable_bytes())
    }

    /// The one way a client leaves, whatever the cause. With a `farewell` reason the client is
    /// first sent `svc_disconnect(reason)` (queued for the end of the frame). A client that was
    /// put in the server is reported to the game; then its edict is reset and the slot freed,
    /// and its now empty userinfo is marked for broadcast so everyone else learns it left. The
    /// caller drains the outbox.
    pub(crate) fn release_slot(&mut self, slot: u8, farewell: Option<&str>) {
        let index = usize::from(slot);
        if self.clients.get(index).is_none_or(Client::is_free) {
            return;
        }
        if let Some(reason) = farewell {
            self.queue_farewell(index, reason);
        }
        if self.clients[index].put_in_server {
            self.call_game(|game, eng| game.client_disconnect(eng, slot));
        }
        self.edicts.reset_client(slot);
        self.world.forget(u16::from(slot) + 1);
        // The userid stays for the removal broadcast.
        let userid = self.clients[index].userid;
        self.clients[index] = Client {
            userid,
            ..Client::free()
        };
        self.views[index] = ClientView {
            userinfo_dirty: true,
            ..ClientView::default()
        };
    }

    /// Releases `slot` without telling it, and passes on what the game queued meanwhile.
    pub(crate) fn free_slot(&mut self, slot: u8) {
        self.release_slot(slot, None);
        self.drain_outbox();
    }

    /// Builds the [`DISCONNECT_DATAGRAMS`] datagrams that tell the client in `index` why it is
    /// released, each a netchan packet with `svc_disconnect` in its unreliable part. The reason
    /// is cut to [`MAX_DISCONNECT_REASON_BYTES`]. The client's pending reliable data is
    /// discarded first, so no farewell carries it and each stays small.
    fn queue_farewell(&mut self, index: usize, reason: &str) {
        let client = &mut self.clients[index];
        let Some(addr) = client.addr else {
            return;
        };
        client.chan.clear();
        let mut w = MsgWriter::new();
        write_svc_disconnect(&mut w, truncate_reason(reason));
        for _ in 0..DISCONNECT_DATAGRAMS {
            let packet = client.chan.build_packet(w.as_slice());
            self.farewells.push((addr, packet));
        }
    }

    /// Sends the queued final datagrams. They are few and bounded per released slot, so they
    /// do not wait for the client's clear time.
    pub(crate) fn send_farewells(&mut self, net: &mut dyn PacketTransport) {
        for (addr, packet) in std::mem::take(&mut self.farewells) {
            self.net_stats.send(net, addr, &packet);
        }
    }

    /// Records a datagram from `slot` that its netchan accepted, acknowledging `sequence_ack`.
    pub(crate) fn note_accepted(&mut self, slot: u8, sequence_ack: u32) {
        let now = self.real_time;
        let interval = crate::frame::update_interval(
            self.views[usize::from(slot)].userinfo.get("cl_updaterate"),
            &self.cfg,
        );
        let samples = self.cvars.get_f32("sv_unlagsamples");
        let client = &mut self.clients[usize::from(slot)];
        client.last_heard = now;
        client.pings.acknowledged(sequence_ack, now);
        // `SV_ExecuteClientMessage`: the ping of the acknowledged frame, then the latency.
        let since_connect = now - client.connected_at;
        client
            .frames
            .note_ack(sequence_ack, now, interval, since_connect);
        // Deliberate deviation: when the acknowledged frame is no longer among the 64 kept (a
        // very high ping), the last latency measured stays. HLDS takes the ping of whatever
        // newer frame now sits in that slot, a wrong, small value; with the last one such a
        // client is still rewound, as far as `sv_maxunlag` allows.
        if client.frames.get(sequence_ack).is_some() {
            let samples = if samples.is_finite() { samples as i32 } else { 1 };
            client.latency = client.frames.client_time(sequence_ack, samples);
        }
    }

    /// Releases, without telling them, clients that did not reach `Active` within
    /// `signon_timeout_secs` of connecting or of the last map change, and clients from which
    /// no datagram was accepted for `timeout_secs`.
    pub(crate) fn drop_timed_out_clients(&mut self) {
        for slot in 0..self.cfg.max_clients {
            let client = &self.clients[usize::from(slot)];
            if client.is_free() {
                continue;
            }
            let stalled = matches!(client.state, ClientState::Signon(_))
                && self.real_time - client.signon_since > self.cfg.signon_timeout_secs;
            let silent = self.real_time - client.last_heard > self.cfg.timeout_secs;
            if stalled || silent {
                self.free_slot(slot);
            }
        }
    }

    /// Every [`PINGS_INTERVAL_SECS`], queues one `svc_pings` listing every `Active` client
    /// (loss reported as 0) into each `Active` client's unreliable queue.
    pub(crate) fn queue_pings_if_due(&mut self) {
        if self.real_time < self.next_pings_at {
            return;
        }
        self.next_pings_at = self.real_time + PINGS_INTERVAL_SECS;
        let pings: Vec<(u8, u16, u8)> = self
            .clients
            .iter()
            .enumerate()
            .filter(|(_, c)| c.state == ClientState::Active)
            .map(|(slot, c)| (slot as u8, c.pings.ping_ms(), 0))
            .collect();
        if pings.is_empty() {
            return;
        }
        let mut w = MsgWriter::new();
        write_svc_pings(&mut w, &pings);
        for client in self
            .clients
            .iter_mut()
            .filter(|c| c.state == ClientState::Active)
        {
            if client.queue_unreliable(w.as_slice()) {
                self.net_stats.unreliable_dropped += 1;
            }
        }
    }

    /// Sends one `svc_updateuserinfo` per slot marked since the last call, however often it
    /// was marked, to every client past `new` through the reliable stream. A free slot always
    /// goes out with an empty userinfo.
    pub(crate) fn broadcast_userinfo(&mut self) {
        let mut w = MsgWriter::new();
        for (slot, view) in self.views.iter_mut().enumerate() {
            if !std::mem::take(&mut view.userinfo_dirty) {
                continue;
            }
            let client = &self.clients[slot];
            let info = if client.is_free() {
                String::new()
            } else {
                crate::signon::public_userinfo(&view.userinfo)
            };
            write_svc_updateuserinfo(&mut w, slot as u8, client.userid, &info, &[0; 16]);
        }
        if w.is_empty() {
            return;
        }
        for client in self.clients.iter_mut().filter(|c| c.state.is_past_new()) {
            client.chan.write_reliable(w.as_slice());
        }
        // Enforces the reliable backlog cap on what was written.
        self.drain_outbox();
    }
}

/// `reason` cut on a character boundary to at most [`MAX_DISCONNECT_REASON_BYTES`].
fn truncate_reason(reason: &str) -> &str {
    let mut end = reason.len().min(MAX_DISCONNECT_REASON_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    &reason[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_reason_is_cut_on_a_character_boundary() {
        assert_eq!(truncate_reason("bye"), "bye");
        let exact = "x".repeat(MAX_DISCONNECT_REASON_BYTES);
        assert_eq!(truncate_reason(&exact), exact);
        let long = "y".repeat(500);
        assert_eq!(truncate_reason(&long).len(), MAX_DISCONNECT_REASON_BYTES);
        // A two-byte character straddling byte 128 is left out whole.
        let straddling = format!("{}é", "z".repeat(MAX_DISCONNECT_REASON_BYTES - 1));
        let cut = truncate_reason(&straddling);
        assert_eq!(cut, "z".repeat(MAX_DISCONNECT_REASON_BYTES - 1));
    }

    #[test]
    fn ping_is_the_average_of_the_last_samples() {
        let mut p = PingTracker::default();
        assert_eq!(p.ping_ms(), 0);
        p.record_sent(1, 1.0);
        p.record_sent(2, 1.1);
        p.acknowledged(1, 1.05);
        assert_eq!(p.ping_ms(), 50);
        // A second acknowledgement of the same sequence takes no sample.
        p.acknowledged(1, 1.5);
        assert_eq!(p.ping_ms(), 50);
        p.acknowledged(2, 1.2);
        assert_eq!(p.ping_ms(), 75);
        // An unknown sequence takes no sample either.
        p.acknowledged(7, 2.0);
        assert_eq!(p.ping_ms(), 75);

        // Only the last 16 samples count.
        for seq in 10..10 + PING_SAMPLES as u32 {
            p.record_sent(seq, 3.0);
            p.acknowledged(seq, 3.02);
        }
        assert_eq!(p.ping_ms(), 20);
    }

    #[test]
    fn ping_forgets_sequences_older_than_the_history() {
        let mut p = PingTracker::default();
        p.record_sent(1, 1.0);
        // Sequence 65 takes the place of sequence 1.
        p.record_sent(1 + PING_SEND_HISTORY as u32, 2.0);
        p.acknowledged(1, 2.5);
        assert_eq!(p.ping_ms(), 0);
        p.acknowledged(1 + PING_SEND_HISTORY as u32, 2.1);
        assert_eq!(p.ping_ms(), 100);
        // A huge round trip saturates instead of wrapping.
        let mut slow = PingTracker::default();
        slow.record_sent(1, 0.0);
        slow.acknowledged(1, 1e9);
        assert_eq!(slow.ping_ms(), u16::MAX);
    }

    #[test]
    fn config_sanitizing_fixes_only_unusable_values() {
        let cfg = ServerConfig {
            max_clients: 40,
            min_rate: 500,
            max_rate: 0,
            min_update_rate: 0,
            max_update_rate: 0,
            timeout_secs: 0.0,
            signon_timeout_secs: f64::NEG_INFINITY,
            ..ServerConfig::default()
        }
        .sanitized();
        assert_eq!(cfg.max_clients, 32);
        assert_eq!((cfg.min_rate, cfg.max_rate), (1000, 1000));
        assert_eq!((cfg.min_update_rate, cfg.max_update_rate), (1, 1));
        assert_eq!((cfg.timeout_secs, cfg.signon_timeout_secs), (60.0, 120.0));

        let defaults = ServerConfig::default().sanitized();
        let expected = ServerConfig::default();
        assert_eq!(defaults.max_clients, expected.max_clients);
        assert_eq!((defaults.min_rate, defaults.max_rate), (1000, 100_000));
        assert_eq!(
            (defaults.min_update_rate, defaults.max_update_rate),
            (10, 102)
        );
    }
}
