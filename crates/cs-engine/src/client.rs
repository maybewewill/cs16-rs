//! State of one client slot: where it is in the sign-on, its address and its netchan.

use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::netchan::{FRAGMENT_S2C_MAX_SIZE, NetChan};
use cs_transport::PeerAddr;

use crate::frame::{FrameRing, MAX_DATAGRAM};
use crate::lifecycle::{PingTracker, SETINFO_BURST, SETINFO_RATE_PER_SEC};
use crate::usercmd::{MsecBudget, TokenBucket};

/// Limit on the fragment bytes a client may have buffered for reassembly per incoming stream.
/// Clients only send small reliable messages, so this is far below the netchan default.
pub const MAX_CLIENT_INCOMING_STREAM: usize = 64 * 1024;

/// Sign-on stage: the string command the server waits for next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignonStage {
    AwaitNew,
    AwaitSendRes,
    AwaitSpawn,
    AwaitSendEnts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    Free,
    Signon(SignonStage),
    Active,
}

impl ClientState {
    /// Past `new`: the client has the server info and receives `reliable_all` and userinfo
    /// broadcasts.
    pub(crate) fn is_past_new(self) -> bool {
        matches!(
            self,
            ClientState::Signon(
                SignonStage::AwaitSendRes | SignonStage::AwaitSpawn | SignonStage::AwaitSendEnts
            ) | ClientState::Active
        )
    }
}

/// One client slot.
pub(crate) struct Client {
    pub(crate) state: ClientState,
    /// `None` exactly when the slot is free.
    pub(crate) addr: Option<PeerAddr>,
    pub(crate) chan: NetChan,
    pub(crate) userid: i32,
    /// Server time of the `connect` that took the slot.
    pub(crate) connected_at: f64,
    /// Server time the current sign-on started: the connect, or the last map change.
    pub(crate) signon_since: f64,
    /// `GameDll::client_put_in_server` was called for this connection.
    pub(crate) put_in_server: bool,
    /// Unreliable bytes for the next datagram.
    pub(crate) unreliable: Vec<u8>,
    /// Real time the client may still spend on user commands.
    pub(crate) budget: MsecBudget,
    /// Last user command selected to run; repeated for losses the backups do not cover.
    pub(crate) last_cmd: GoldSrcUserCmd,
    /// Unparsable payloads received in a row while `Active`.
    pub(crate) bad_moves: u32,
    /// Rate limit for string commands other than the sign-on ones.
    pub(crate) stringcmds: TokenBucket,
    /// Rate limit for `setinfo`, on top of `stringcmds`.
    pub(crate) setinfos: TokenBucket,
    /// Server time of the last accepted name change.
    pub(crate) last_name_change: Option<f64>,
    /// Server time of the last datagram the netchan accepted (of the connect, before any).
    pub(crate) last_heard: f64,
    /// Send times of recent datagrams and the round trips measured from them.
    pub(crate) pings: PingTracker,
    /// Updates sent, kept as delta bases.
    pub(crate) frames: FrameRing,
    /// Server time the next update is due.
    pub(crate) next_update_at: f64,
    /// Sequence of the update the client last asked deltas from (`clc_delta`).
    pub(crate) delta_seq: Option<u8>,
    /// Server time from which the client's `rate` lets it be sent its next update.
    pub(crate) clear_time: f64,
    /// Due updates held back by the rate since the last update sent.
    pub(crate) choke_count: u32,
}

impl Client {
    pub(crate) fn free() -> Self {
        Self {
            state: ClientState::Free,
            addr: None,
            chan: NetChan::new(FRAGMENT_S2C_MAX_SIZE),
            userid: 0,
            connected_at: 0.0,
            signon_since: 0.0,
            put_in_server: false,
            unreliable: Vec::new(),
            budget: MsecBudget::new(),
            last_cmd: GoldSrcUserCmd::default(),
            bad_moves: 0,
            stringcmds: TokenBucket::stringcmds(),
            setinfos: TokenBucket::new(SETINFO_RATE_PER_SEC, SETINFO_BURST),
            last_name_change: None,
            last_heard: 0.0,
            pings: PingTracker::default(),
            frames: FrameRing::new(),
            next_update_at: 0.0,
            delta_seq: None,
            clear_time: 0.0,
            choke_count: 0,
        }
    }

    /// Queues unreliable bytes for the next update, keeping the queue within [`MAX_DATAGRAM`]:
    /// when they would take it past that, the queue is cleared first, and bytes longer than
    /// that on their own are not queued. Returns whether anything was dropped.
    pub(crate) fn queue_unreliable(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let dropped = self.unreliable.len() + bytes.len() > MAX_DATAGRAM;
        if dropped {
            self.unreliable.clear();
        }
        if bytes.len() <= MAX_DATAGRAM {
            self.unreliable.extend_from_slice(bytes);
        }
        dropped
    }

    /// Forgets the updates sent so far: the next one is sent in full.
    pub(crate) fn reset_frames(&mut self) {
        self.frames = FrameRing::new();
        self.delta_seq = None;
    }

    /// Forgets what belonged to the previous map and waits for `new` again, the sign-on
    /// starting at `now`. Reliable data still queued or in flight is discarded; the netchan
    /// session, the rate state, the limits, the pings and the last-heard time stay.
    pub(crate) fn restart_signon(&mut self, now: f64) {
        self.state = ClientState::Signon(SignonStage::AwaitNew);
        self.signon_since = now;
        self.put_in_server = false;
        self.unreliable.clear();
        self.budget = MsecBudget::new();
        self.last_cmd = GoldSrcUserCmd::default();
        self.bad_moves = 0;
        self.choke_count = 0;
        self.reset_frames();
        self.chan.clear();
    }

    /// A fresh connection from `addr` waiting for `new`.
    pub(crate) fn connected(addr: PeerAddr, userid: i32, now: f64) -> Self {
        let mut chan = NetChan::new(FRAGMENT_S2C_MAX_SIZE);
        chan.set_max_incoming_stream(MAX_CLIENT_INCOMING_STREAM);
        Self {
            state: ClientState::Signon(SignonStage::AwaitNew),
            addr: Some(addr),
            chan,
            userid,
            connected_at: now,
            signon_since: now,
            last_heard: now,
            ..Self::free()
        }
    }

    pub(crate) fn is_free(&self) -> bool {
        self.state == ClientState::Free
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_new_covers_later_stages_and_active() {
        assert!(!ClientState::Free.is_past_new());
        assert!(!ClientState::Signon(SignonStage::AwaitNew).is_past_new());
        assert!(ClientState::Signon(SignonStage::AwaitSendRes).is_past_new());
        assert!(ClientState::Signon(SignonStage::AwaitSpawn).is_past_new());
        assert!(ClientState::Signon(SignonStage::AwaitSendEnts).is_past_new());
        assert!(ClientState::Active.is_past_new());
    }

    #[test]
    fn unreliable_queue_is_bounded() {
        let mut c = Client::free();
        assert!(!c.queue_unreliable(&[]));
        assert!(!c.queue_unreliable(&[1; 3000]));
        assert!(!c.queue_unreliable(&[2; MAX_DATAGRAM - 3000]));
        assert_eq!(c.unreliable.len(), MAX_DATAGRAM);
        // Overflow: the old queue goes, the new bytes stay.
        assert!(c.queue_unreliable(&[3; 10]));
        assert_eq!(c.unreliable, vec![3; 10]);
        // Bytes too long on their own are dropped too.
        assert!(c.queue_unreliable(&[4; MAX_DATAGRAM + 1]));
        assert!(c.unreliable.is_empty());
    }

    #[test]
    fn connected_client_awaits_new() {
        let addr = PeerAddr::Loopback(3);
        let c = Client::connected(addr, 7, 2.5);
        assert_eq!(c.state, ClientState::Signon(SignonStage::AwaitNew));
        assert_eq!((c.addr, c.userid, c.connected_at), (Some(addr), 7, 2.5));
        assert!(!c.put_in_server);
        assert!(Client::free().is_free());
        assert!(Client::free().addr.is_none());
    }

    #[test]
    fn restart_signon_keeps_the_connection() {
        let addr = PeerAddr::Loopback(3);
        let mut c = Client::connected(addr, 7, 2.5);
        c.state = ClientState::Active;
        c.put_in_server = true;
        c.unreliable.push(1);
        c.choke_count = 4;
        c.bad_moves = 2;
        c.delta_seq = Some(9);
        c.clear_time = 11.0;
        c.last_heard = 9.5;
        // One reliable message in flight, one queued.
        c.chan.write_reliable(&[1, 2, 3]);
        c.chan.build_packet(&[]);
        c.chan.write_reliable(&[4]);
        assert!(c.chan.pending_reliable_bytes() > 0);
        let outgoing = c.chan.outgoing_sequence;

        c.restart_signon(10.0);
        assert_eq!(c.state, ClientState::Signon(SignonStage::AwaitNew));
        assert_eq!((c.signon_since, c.connected_at), (10.0, 2.5));
        assert!(!c.put_in_server);
        assert!(c.unreliable.is_empty());
        assert_eq!((c.choke_count, c.bad_moves, c.delta_seq), (0, 0, None));
        assert_eq!(c.chan.pending_reliable_bytes(), 0);
        assert_eq!(c.chan.outgoing_sequence, outgoing);
        assert_eq!((c.addr, c.userid), (Some(addr), 7));
        assert_eq!((c.clear_time, c.last_heard), (11.0, 9.5));
    }
}
