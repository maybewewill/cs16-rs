//! Server-side voice bridge routing contract (`sv_voice_transcode`).
//!
//! `cs-engine` stays pure Rust and `wasm32`-compatible; native server binaries
//! (such as `cs-server`) attach an Opus-capable [`VoiceBridge`] via
//! `Server::set_voice_bridge` so retail `hl.exe` / native `cs-client` players
//! receive `SteamOpusPlc` packets untouched while `cs-web` browser slots
//! (`PeerAddr::Web`) exchange transcoded `CSVO` IMA-ADPCM frames.

use cs_cvar::CVarRegistry;
use cs_transport::PeerAddr;

/// Listener or sender transport category for voice bridging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceSlotKind {
    /// Retail `hl.exe` or native `cs-client` connected via UDP or loopback.
    NativeOrRetail,
    /// Browser `cs-web` client connected via WebSocket/WebRTC (`PeerAddr::Web`).
    WebBrowser,
}

impl VoiceSlotKind {
    /// Classifies a slot from its transport peer address.
    pub fn from_peer_addr(addr: Option<&PeerAddr>) -> Self {
        match addr {
            Some(PeerAddr::Web(_)) => Self::WebBrowser,
            _ => Self::NativeOrRetail,
        }
    }
}

/// Outcome of routing one incoming `clc_voicedata` payload toward a target slot class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceBridgeAction {
    /// Forward the incoming packet bytes verbatim (zero generation loss).
    Passthrough,
    /// Forward these transcoded bytes instead.
    Transcoded(Vec<u8>),
    /// Suppress forwarding for this target class (e.g. control-only reset or buffering).
    Suppress,
}

/// Pluggable voice bridge invoked by `Server::handle_voice_data` when `sv_voice_transcode` is active.
pub trait VoiceBridge: Send {
    fn bridge_packet(
        &mut self,
        sender_slot: u8,
        sender_kind: VoiceSlotKind,
        target_kind: VoiceSlotKind,
        data: &[u8],
    ) -> VoiceBridgeAction;
}

/// Returns whether server-side voice transcoding is enabled (`sv_voice_transcode`, default `"1"`).
pub fn sv_voice_transcode_enabled(cvars: &CVarRegistry) -> bool {
    let raw = cvars.get_str("sv_voice_transcode").trim();
    if raw.is_empty() {
        return true;
    }
    raw != "0" && !raw.eq_ignore_ascii_case("false")
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    use cs_proto48::delta::DeltaRegistry;
    use cs_proto48::messages::{SvcMessage, SvcParseContext, parse_svc_stream, write_clc_voicedata};
    use cs_proto48::bitbuf::MsgWriter;

    use super::*;
    use crate::{Server, ServerConfig, ServerMap};

    struct MockBridge;

    impl VoiceBridge for MockBridge {
        fn bridge_packet(
            &mut self,
            _sender_slot: u8,
            _sender_kind: VoiceSlotKind,
            target_kind: VoiceSlotKind,
            data: &[u8],
        ) -> VoiceBridgeAction {
            match target_kind {
                VoiceSlotKind::NativeOrRetail => VoiceBridgeAction::Passthrough,
                VoiceSlotKind::WebBrowser => {
                    let mut out = b"WEB:".to_vec();
                    out.extend_from_slice(data);
                    VoiceBridgeAction::Transcoded(out)
                }
            }
        }
    }

    #[test]
    fn voice_slot_kind_and_cvar_default_to_enabled() {
        assert_eq!(VoiceSlotKind::from_peer_addr(None), VoiceSlotKind::NativeOrRetail);
        assert_eq!(
            VoiceSlotKind::from_peer_addr(Some(&PeerAddr::Loopback(1))),
            VoiceSlotKind::NativeOrRetail
        );
        assert_eq!(
            VoiceSlotKind::from_peer_addr(Some(&PeerAddr::Udp(SocketAddr::V4(
                SocketAddrV4::new(Ipv4Addr::LOCALHOST, 27015)
            )))),
            VoiceSlotKind::NativeOrRetail
        );
        assert_eq!(
            VoiceSlotKind::from_peer_addr(Some(&PeerAddr::Web(7))),
            VoiceSlotKind::WebBrowser
        );

        let mut cvars = CVarRegistry::new_with_defaults();
        assert!(sv_voice_transcode_enabled(&cvars));
        cvars.register("sv_voice_transcode", "1", cs_cvar::CVarFlags::NONE, "voice transcode");
        assert!(sv_voice_transcode_enabled(&cvars));
        cvars.set("sv_voice_transcode", "0");
        assert!(!sv_voice_transcode_enabled(&cvars));
    }

    #[test]
    fn voice_bridge_routes_passthrough_to_native_and_transcoded_to_web() {
        use crate::client::ClientState;
        use crate::edict::EdictId;
        use crate::game::{EngineApi, GameDll};
        use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
        use cs_proto48::usermsg::UserMsgRegistry;

        struct StubGame;
        impl GameDll for StubGame {
            fn usermsgs(&self) -> UserMsgRegistry {
                UserMsgRegistry::cs16_default()
            }
            fn server_activate(&mut self, _eng: &mut EngineApi<'_>) {}
            fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {}
            fn spawn_entity(
                &mut self,
                _eng: &mut EngineApi<'_>,
                _id: EdictId,
                _classname: &str,
                _keys: &[(String, String)],
            ) -> bool {
                false
            }
            fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
                Ok(())
            }
            fn client_put_in_server(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
            fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
            fn client_command(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _argv: &[String]) {}
            fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
            fn run_usercmd(
                &mut self,
                _eng: &mut EngineApi<'_>,
                _slot: u8,
                _cmd: &GoldSrcUserCmd,
                _random_seed: u32,
            ) {
            }
            fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {}
            fn update_client_data(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> GoldSrcClientData {
                GoldSrcClientData::default()
            }
        }

        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.register("sv_voice_transcode", "1", cs_cvar::CVarFlags::NONE, "voice transcode");
        let mut server = Server::new(
            ServerConfig::default(),
            ServerMap::stub("de_dust2"),
            StubGame,
            DeltaRegistry::cs16_default(),
            cvars,
        );
        server.set_voice_bridge(Box::new(MockBridge));

        server.clients[0].state = ClientState::Active;
        server.clients[0].addr = Some(PeerAddr::Loopback(10));
        server.clients[1].state = ClientState::Active;
        server.clients[1].addr = Some(PeerAddr::Loopback(11));
        server.clients[2].state = ClientState::Active;
        server.clients[2].addr = Some(PeerAddr::Web(22));

        let payload = b"OPUS_PKT";
        let mut clc = MsgWriter::new();
        write_clc_voicedata(&mut clc, payload);
        let _ = clc.into_bytes();
        server.handle_voice_data(0, payload);

        let mut ctx = SvcParseContext::new();
        let msgs_native = parse_svc_stream(&server.clients[1].unreliable, &mut ctx).unwrap();
        assert_eq!(
            msgs_native,
            vec![SvcMessage::VoiceData {
                player_slot: 0,
                data: payload.to_vec(),
            }]
        );

        let msgs_web = parse_svc_stream(&server.clients[2].unreliable, &mut ctx).unwrap();
        assert_eq!(
            msgs_web,
            vec![SvcMessage::VoiceData {
                player_slot: 0,
                data: b"WEB:OPUS_PKT".to_vec(),
            }]
        );

        // When `sv_voice_transcode` is 0, both slots get raw passthrough.
        server.clients[1].unreliable.clear();
        server.clients[2].unreliable.clear();
        server.set_cvar("sv_voice_transcode", "0");
        server.handle_voice_data(0, payload);

        let msgs_web_off = parse_svc_stream(&server.clients[2].unreliable, &mut ctx).unwrap();
        assert_eq!(
            msgs_web_off,
            vec![SvcMessage::VoiceData {
                player_slot: 0,
                data: payload.to_vec(),
            }]
        );
    }
}
