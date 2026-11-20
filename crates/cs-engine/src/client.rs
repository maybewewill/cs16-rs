//! State of one client slot: where it is in the sign-on, its address and its netchan.

use cs_proto48::netchan::{FRAGMENT_S2C_MAX_SIZE, NetChan};
use cs_transport::PeerAddr;

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
    /// `GameDll::client_put_in_server` was called for this connection.
    pub(crate) put_in_server: bool,
    /// Unreliable bytes for the next datagram.
    pub(crate) unreliable: Vec<u8>,
}

impl Client {
    pub(crate) fn free() -> Self {
        Self {
            state: ClientState::Free,
            addr: None,
            chan: NetChan::new(FRAGMENT_S2C_MAX_SIZE),
            userid: 0,
            connected_at: 0.0,
            put_in_server: false,
            unreliable: Vec::new(),
        }
    }

    /// A fresh connection from `addr` waiting for `new`.
    pub(crate) fn connected(addr: PeerAddr, userid: i32, now: f64) -> Self {
        Self {
            state: ClientState::Signon(SignonStage::AwaitNew),
            addr: Some(addr),
            userid,
            connected_at: now,
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
    fn connected_client_awaits_new() {
        let addr = PeerAddr::Loopback(3);
        let c = Client::connected(addr, 7, 2.5);
        assert_eq!(c.state, ClientState::Signon(SignonStage::AwaitNew));
        assert_eq!((c.addr, c.userid, c.connected_at), (Some(addr), 7, 2.5));
        assert!(!c.put_in_server);
        assert!(Client::free().is_free());
        assert!(Client::free().addr.is_none());
    }
}
