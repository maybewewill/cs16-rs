//! The client core against `cs-engine` over a loopback: the whole sign-on, no hand-played server.

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
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        // The tables the client must end up with.
        let _ = eng.precache_model("models/test.mdl");
        let _ = eng.precache_sound("weapons/test.wav");
        let _ = eng.precache_generic("sprites/test.txt");
    }
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

fn config() -> ConnectionConfig {
    ConnectionConfig::new("core", 76_561_197_960_265_730, &md5_hex(b"cs-clientcore loopback"))
}

#[test]
fn connects_to_our_server_over_loopback() {
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
    let mut conn = Connection::new(endpoint, PeerAddr::Loopback(0), config());
    assert_eq!(conn.cvars().string("cl_cmdrate"), "100");
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
    assert_eq!(conn.state(), &ConnState::InGame, "{:?}", conn.messages());
    assert_eq!(conn.parse_context().max_clients, 4);
    assert!(conn.server_info().is_some());
    assert!(conn.stats().full_updates >= 1);
    assert_eq!(conn.stats().parse_errors, 0);
    // The server's resource list landed in the tables the client keeps, at the server's indices.
    assert!(conn.resources().needed().is_empty());
    let tables = conn.precache();
    let named = |table: &std::collections::BTreeMap<u16, cs_clientcore::PrecacheEntry>, name: &str| {
        table.iter().find(|(_, e)| e.name == name).map(|(&i, _)| i)
    };
    assert!(named(&tables.models, "models/test.mdl").is_some(), "{:?}", tables.models);
    assert!(named(&tables.sounds, "weapons/test.wav").is_some(), "{:?}", tables.sounds);
    assert!(named(&tables.generic, "sprites/test.txt").is_some(), "{:?}", tables.generic);
    // The map is model 1.
    assert_eq!(tables.world_model(), conn.server_info().map(|i| i.map_path.as_str()));

    // A few more frames: the delta stream keeps flowing and no packet goes unparsed.
    for _ in 0..100 {
        now += FRAME;
        conn.pump(now);
        server.frame(&mut hub, now);
    }
    assert_eq!(conn.state(), &ConnState::InGame);
    assert_eq!(conn.stats().parse_errors, 0);
    assert!(conn.stats().delta_updates > 0);
}
