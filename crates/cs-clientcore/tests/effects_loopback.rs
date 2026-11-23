//! The connection hands every parsed svc message to its effect modules: against `cs-engine`
//! over a loopback, the server's `svc_newusermsg` table lands in the HUD state, and the frame
//! update runs on the connection's own tables.

use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{EdictId, EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{LoopbackHub, PeerAddr};

const FRAME: f64 = 0.01;

/// A game that overrides nothing it need not.
struct Bare;

impl GameDll for Bare {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }
    fn server_activate(&mut self, _: &mut EngineApi<'_>) {}
    fn server_deactivate(&mut self, _: &mut EngineApi<'_>) {}
    fn spawn_entity(
        &mut self,
        _: &mut EngineApi<'_>,
        _: EdictId,
        _: &str,
        _: &[(String, String)],
    ) -> bool {
        true
    }
    fn client_connect(&mut self, _: &mut EngineApi<'_>, _: u8) -> Result<(), String> {
        Ok(())
    }
    fn client_put_in_server(&mut self, _: &mut EngineApi<'_>, _: u8) {}
    fn client_disconnect(&mut self, _: &mut EngineApi<'_>, _: u8) {}
    fn client_command(&mut self, _: &mut EngineApi<'_>, _: u8, _: &[String]) {}
    fn client_userinfo_changed(&mut self, _: &mut EngineApi<'_>, _: u8) {}
    fn run_usercmd(&mut self, _: &mut EngineApi<'_>, _: u8, _: &GoldSrcUserCmd, _: u32) {}
    fn start_frame(&mut self, _: &mut EngineApi<'_>, _: f32) {}
    fn update_client_data(&mut self, _: &mut EngineApi<'_>, _: u8) -> GoldSrcClientData {
        GoldSrcClientData::default()
    }
}

#[test]
fn the_connection_feeds_the_effect_modules() {
    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        ServerMap::stub("test"),
        Bare,
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let mut hub = LoopbackHub::new();
    let endpoint = hub.connect();
    let cfg = ConnectionConfig::new("core", 76_561_197_960_265_730, &md5_hex(b"effects loopback"));
    let mut conn = Connection::new(endpoint, PeerAddr::Loopback(0), cfg);
    conn.connect();

    let mut now = 0.0;
    for _ in 0..500 {
        now += FRAME;
        conn.pump(now);
        server.frame(&mut hub, now);
        if conn.state() == &ConnState::InGame {
            break;
        }
    }
    assert_eq!(conn.state(), &ConnState::InGame);

    // The server's message table reached the HUD, by number.
    let hud = &conn.fx().hud;
    assert!(hud.user_messages.values().any(|m| m.name == "Health"));
    assert!(hud.user_messages.values().any(|m| m.name == "CurWeapon"));
    assert_eq!(
        hud.user_messages.len(),
        conn.parse_context().usermsgs.iter().count()
    );

    // The frame update runs on the connection's tables; nothing fires without events.
    assert!(conn.update_effects(now).is_empty());
    assert!(conn.fx().tempents.active().is_empty());
}
