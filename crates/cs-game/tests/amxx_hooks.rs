//! Integration test verifying `AmxxGame<CsGame>` (`AmxxCsHooks` impl on real `CsGame`).

use std::collections::HashMap;

use cs_assets::bsp::Entity;
use cs_amxx::AmxxGame;
use cs_amxx::amxxfile::build_test_amxx_plugin;
use cs_amxx::natives::PLUGIN_HANDLED;
use cs_amxx::vm::op::*;
use cs_cvar::CVarRegistry;
use cs_engine::{ConsoleOutcome, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::ArmorType;
use cs_game::weapons::WeaponId;
use cs_proto48::delta::DeltaRegistry;
use cs_transport::LoopbackHub;

struct DataBuilder(Vec<u8>);

impl DataBuilder {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn add_str(&mut self, s: &str) -> u32 {
        let addr = self.0.len() as u32;
        for &b in s.as_bytes() {
            self.0.extend_from_slice(&i32::from(b).to_le_bytes());
        }
        self.0.extend_from_slice(&0i32.to_le_bytes());
        addr
    }
}

struct CodeBuilder {
    cells: Vec<u32>,
    publics: Vec<(String, u32)>,
    natives: Vec<String>,
}

impl CodeBuilder {
    fn new(natives: &[&str]) -> Self {
        Self {
            cells: vec![OP_HALT, 0],
            publics: Vec::new(),
            natives: natives.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn begin_public(&mut self, name: &str) {
        let off = (self.cells.len() * 4) as u32;
        self.publics.push((name.to_string(), off));
        self.cells.push(OP_PROC);
    }

    fn call_native(&mut self, name: &str, args: &[i32]) {
        let idx = self
            .natives
            .iter()
            .position(|n| n == name)
            .expect("declared native") as u32;
        for &v in args.iter().rev() {
            self.cells.push(OP_PUSH_C);
            self.cells.push(v as u32);
        }
        let param_bytes = (args.len() * 4) as u32;
        self.cells.push(OP_PUSH_C);
        self.cells.push(param_bytes);
        self.cells.push(OP_SYSREQ_C);
        self.cells.push(idx);
        self.cells.push(OP_STACK);
        self.cells.push(param_bytes + 4);
    }

    fn ret_const(&mut self, val: i32) {
        self.cells.push(OP_CONST_PRI);
        self.cells.push(val as u32);
        self.cells.push(OP_RETN);
    }

    fn build(&self, data: &DataBuilder) -> Vec<u8> {
        let pubs: Vec<(&str, u32)> = self
            .publics
            .iter()
            .map(|(n, o)| (n.as_str(), *o))
            .collect();
        let nats: Vec<&str> = self.natives.iter().map(String::as_str).collect();
        build_test_amxx_plugin(&pubs, &nats, &[], &self.cells, &data.0, 4096, 0)
    }
}

fn spawn_map() -> ServerMap {
    ServerMap::stub_with_entities(
        "de_dust2",
        vec![
            Entity {
                properties: HashMap::from([("classname".to_string(), "worldspawn".to_string())]),
            },
            Entity {
                properties: HashMap::from([
                    ("classname".to_string(), "info_player_start".to_string()),
                    ("origin".to_string(), "128 0 0".to_string()),
                ]),
            },
            Entity {
                properties: HashMap::from([
                    ("classname".to_string(), "info_player_deathmatch".to_string()),
                    ("origin".to_string(), "-128 0 0".to_string()),
                ]),
            },
        ],
    )
}

#[test]
fn amxx_plugin_mutates_real_cs_game_player_via_hooks() {
    let mut data = DataBuilder::new();
    let s_grant = data.add_str("amx_grant") as i32;
    let s_on_grant = data.add_str("on_grant") as i32;
    let s_strip = data.add_str("amx_strip") as i32;
    let s_on_strip = data.add_str("on_strip") as i32;
    let s_ak47 = data.add_str("weapon_ak47") as i32;
    let s_suit = data.add_str("item_assaultsuit") as i32;

    let mut code = CodeBuilder::new(&[
        "register_srvcmd",
        "cs_set_user_money",
        "give_item",
        "strip_user_weapons",
    ]);

    code.begin_public("plugin_init");
    code.call_native("register_srvcmd", &[s_grant, s_on_grant, -1, 0]);
    code.call_native("register_srvcmd", &[s_strip, s_on_strip, -1, 0]);
    code.ret_const(0);

    code.begin_public("on_grant");
    code.call_native("cs_set_user_money", &[1, 5432, 0]);
    code.call_native("give_item", &[1, s_ak47]);
    code.call_native("give_item", &[1, s_suit]);
    code.ret_const(PLUGIN_HANDLED);

    code.begin_public("on_strip");
    code.call_native("strip_user_weapons", &[1]);
    code.ret_const(PLUGIN_HANDLED);

    let bytes = code.build(&data);
    let game = AmxxGame::new(CsGame::new(), vec![("hooks_test.amxx".to_string(), bytes)]);
    let mut server = Server::new(
        ServerConfig::default(),
        spawn_map(),
        game,
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );

    let hub = LoopbackHub::new();
    let mut transport = hub.connect();
    server.frame(&mut transport, 0.1);
    let _ = server.console_command("bot_add_ct TestBot");
    for i in 1..=5 {
        server.frame(&mut transport, 0.1 + f64::from(i) * 0.05);
    }

    assert!(matches!(
        server.console_command("amx_grant"),
        ConsoleOutcome::Output(_)
    ));
    let cs = server.game().inner();
    let p = cs.state().players[0]
        .as_ref()
        .expect("bot should occupy slot 0");
    assert_eq!(p.money, 5432);
    assert_eq!(p.armor, 100.0);
    assert_eq!(p.kevlar, ArmorType::VestHelm);
    assert_eq!(cs.player_weapon_state(0).unwrap().2, Some(WeaponId::Ak47));

    assert!(matches!(
        server.console_command("amx_strip"),
        ConsoleOutcome::Output(_)
    ));
    assert_eq!(server.game().inner().player_weapon_state(0).unwrap().2, None);
}
