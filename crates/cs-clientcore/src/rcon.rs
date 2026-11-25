//! Client-side GoldSrc OOB RCON state machine (`challenge rcon` -> `rcon <ch> "<pw>" <cmd>`
//! -> `0xFFFF_FFFF 'l'` reply collector).

use cs_proto48::handshake::{OobPacket, encode_challenge_rcon, encode_rcon, parse_oob_packet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RconClientState {
    Idle,
    AwaitChallenge,
    Done,
}

/// Pure-state GoldSrc RCON client usable on both UDP sockets and loopback/tests.
#[derive(Debug, Clone)]
pub struct RconClient {
    state: RconClientState,
    password: String,
    command: String,
    replies: Vec<String>,
}

impl Default for RconClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RconClient {
    pub fn new() -> Self {
        Self {
            state: RconClientState::Idle,
            password: String::new(),
            command: String::new(),
            replies: Vec::new(),
        }
    }

    pub fn state(&self) -> &RconClientState {
        &self.state
    }

    /// Starts an RCON transaction and returns the `challenge rcon\n` datagram to send.
    pub fn start_command(&mut self, password: &str, command: &str) -> Vec<u8> {
        self.password = password.to_string();
        self.command = command.to_string();
        self.replies.clear();
        self.state = RconClientState::AwaitChallenge;
        encode_challenge_rcon()
    }

    /// Processes an incoming `0xFFFF_FFFF` OOB packet from the server.
    /// Returns the authenticated `rcon <challenge> "<pw>" <cmd>\n` datagram once the
    /// challenge arrives, or `None` after collecting an `'l'` print output packet.
    pub fn on_datagram(&mut self, raw: &[u8]) -> Option<Vec<u8>> {
        let Ok(pkt) = parse_oob_packet(raw) else {
            return None;
        };
        match pkt {
            OobPacket::S2cChallengeRcon(challenge)
                if self.state == RconClientState::AwaitChallenge =>
            {
                self.state = RconClientState::Done;
                Some(encode_rcon(challenge, &self.password, &self.command))
            }
            OobPacket::A2aPrint(text) => {
                self.replies.push(text);
                None
            }
            _ => None,
        }
    }

    /// Drains any accumulated RCON print text chunks.
    pub fn take_replies(&mut self) -> Vec<String> {
        std::mem::take(&mut self.replies)
    }
}

#[cfg(test)]
mod tests {
    use cs_proto48::handshake::{encode_a2a_print, encode_s2c_challenge_rcon};

    use super::*;

    #[test]
    fn rcon_client_roundtrips_challenge_command_and_replies() {
        let mut c = RconClient::new();
        let req = c.start_command("hunter2", "status");
        assert_eq!(parse_oob_packet(&req).unwrap(), OobPacket::ChallengeRcon);

        let send_cmd = c
            .on_datagram(&encode_s2c_challenge_rcon(424242))
            .expect("must emit rcon command after challenge");
        assert!(matches!(
            parse_oob_packet(&send_cmd).unwrap(),
            OobPacket::Rcon {
                challenge: 424242,
                ref password,
                ref command,
                ..
            } if password == "hunter2" && command == "status"
        ));

        assert!(c.on_datagram(&encode_a2a_print("hostname: Test\n")).is_none());
        assert_eq!(c.take_replies(), vec!["hostname: Test\n".to_string()]);
        assert!(c.take_replies().is_empty());
    }
}
