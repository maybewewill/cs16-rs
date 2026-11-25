//! Phase 15 Task 2 integration tests for expanded `cs-amxx` modules:
//! - `newmenus.inc` (`menu_create`, `menu_additem`, `menu_addblank`, `menu_addtext`,
//!   `menu_setprop`, `menu_display`, pagination `9. More`/`8. Back`/`0. Exit`,
//!   `menu_item_getinfo`, `player_menu_info`, `menu_destroy`)
//! - `lang.inc` (`register_dictionary`, `LookupLangKey`, `%L` with `LANG_PLAYER`/`LANG_SERVER`
//!   serving `ru` and `en` simultaneously)
//! - `csx.inc` (`client_damage`, `client_death`, `bomb_planting`/`planted`/`explode`/
//!   `defusing`/`defused`, `grenade_throw`, `get_stats`/`get_user_stats`/`get_statsnum`)
//! - `nvault.inc` (`nvault_open/set/pset/get/lookup/remove/prune/close`)
//! - Expanded `hamsandwich` (`SetHamParamFloat`/`Integer`, `ExecuteHam`/`ExecuteHamB`,
//!   `Ham_Player_Jump`, `Ham_Item_Deploy`, `Ham_Weapon_PrimaryAttack`, `Ham_Touch`)
//! - `reapi` subset (`rg_give_item`, `rg_remove_all_items`, `rg_set_user_armor`,
//!   `rg_add_account`, `rg_round_respawn`, `get_member`, `set_member`)
//! - Dynamic inter-plugin natives (`register_native`, `get_param`, `get_string`, `set_string`)
//!   and multi-forwards (`CreateMultiForward`, `ExecuteForward`, `DestroyForward`).

use std::cell::RefCell;
use std::rc::Rc;

use cs_amxx::amxxfile::build_test_amxx_plugin;
use cs_amxx::natives::{
    AS_ADD, AS_SET, AmxxCsHooks, CS_ARMOR_VESTHELM, ET_CONTINUE, FP_CELL, FP_STRING, GT_REPLACE,
    HAM_HANDLED, HAM_SUPERCEDE, HamFunction, MENU_EXIT, PLUGIN_CONTINUE, reapi_member,
};
use cs_amxx::vm::op::*;
use cs_amxx::{AmxxGame, AmxxRuntime};
use cs_cvar::CVarRegistry;
use cs_engine::{EdictId, EngineApi, FullPackCtx, GameDll, Server, ServerConfig, ServerMap};
use cs_mod_api::services::{LANG_PLAYER, LANG_SERVER, mprop};
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{PacketTransport, PeerAddr, TransportError};

struct DataBuilder {
    bytes: Vec<u8>,
}

impl DataBuilder {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn align4(&mut self) {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
    }

    fn add_str(&mut self, s: &str) -> u32 {
        self.align4();
        let addr = self.bytes.len() as u32;
        for &b in s.as_bytes() {
            self.bytes.extend_from_slice(&i32::from(b).to_le_bytes());
        }
        self.bytes.extend_from_slice(&0i32.to_le_bytes());
        addr
    }

    fn add_cell(&mut self, val: i32) -> u32 {
        self.align4();
        let addr = self.bytes.len() as u32;
        self.bytes.extend_from_slice(&val.to_le_bytes());
        addr
    }

    fn alloc_cells(&mut self, count: usize) -> u32 {
        self.align4();
        let addr = self.bytes.len() as u32;
        self.bytes.resize(self.bytes.len() + count * 4, 0);
        addr
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum Arg {
    Const(i32),
    Param(u32),
    Pri,
    Mem(u32),
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

    fn native_idx(&self, name: &str) -> u32 {
        self.natives
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("undeclared native {name}")) as u32
    }

    fn begin_public(&mut self, name: &str) {
        let off = (self.cells.len() * 4) as u32;
        self.publics.push((name.to_string(), off));
        self.cells.push(OP_PROC);
    }

    fn call_native(&mut self, name: &str, args: &[Arg]) {
        let idx = self.native_idx(name);
        for &arg in args.iter().rev() {
            match arg {
                Arg::Const(v) => {
                    self.cells.push(OP_PUSH_C);
                    self.cells.push(v as u32);
                }
                Arg::Param(p) => {
                    self.cells.push(OP_LOAD_S_PRI);
                    self.cells.push(12 + p * 4);
                    self.cells.push(OP_PUSH_PRI);
                }
                Arg::Pri => {
                    self.cells.push(OP_PUSH_PRI);
                }
                Arg::Mem(addr) => {
                    self.cells.push(OP_LOAD_PRI);
                    self.cells.push(addr);
                    self.cells.push(OP_PUSH_PRI);
                }
            }
        }
        let param_bytes = (args.len() * 4) as u32;
        self.cells.push(OP_PUSH_C);
        self.cells.push(param_bytes);
        self.cells.push(OP_SYSREQ_C);
        self.cells.push(idx);
        self.cells.push(OP_STACK);
        self.cells.push(param_bytes + 4);
    }

    fn store_pri(&mut self, addr: u32) {
        self.cells.push(OP_STOR_PRI);
        self.cells.push(addr);
    }

    fn load_pri(&mut self, addr: u32) {
        self.cells.push(OP_LOAD_PRI);
        self.cells.push(addr);
    }

    fn add_const(&mut self, val: i32) {
        self.cells.push(OP_ADD_C);
        self.cells.push(val as u32);
    }

    fn ret_pri(&mut self) {
        self.cells.push(OP_RETN);
    }

    fn ret_const(&mut self, val: i32) {
        self.cells.push(OP_CONST_PRI);
        self.cells.push(val as u32);
        self.cells.push(OP_RETN);
    }

    fn build(&self, data: &DataBuilder) -> Vec<u8> {
        let pub_refs: Vec<(&str, u32)> = self
            .publics
            .iter()
            .map(|(n, o)| (n.as_str(), *o))
            .collect();
        let nat_refs: Vec<&str> = self.natives.iter().map(String::as_str).collect();
        build_test_amxx_plugin(&pub_refs, &nat_refs, &[], &self.cells, &data.bytes, 4096, 0)
    }
}

struct NullTransport;
impl PacketTransport for NullTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        None
    }
    fn send(&mut self, _to: PeerAddr, _data: &[u8]) -> Result<(), TransportError> {
        Ok(())
    }
}

#[derive(Default)]
struct MockHooksGame {
    money: [i32; 32],
    armor: [i32; 32],
    armor_type: [i32; 32],
    team: [i32; 32],
    deaths: [i32; 32],
    respawns: Vec<u8>,
    given: Vec<(u8, String)>,
    removed_all: Vec<(u8, bool)>,
    members: std::collections::HashMap<(u16, i32), i32>,
    ham_execs: Vec<(HamFunction, u16, Vec<i32>)>,
    bots: Vec<u8>,
}

impl AmxxCsHooks for MockHooksGame {
    fn get_user_money(&self, slot: u8) -> i32 {
        self.money[usize::from(slot)]
    }
    fn set_user_money(&mut self, slot: u8, amount: i32) {
        self.money[usize::from(slot)] = amount;
    }
    fn get_user_team(&self, slot: u8) -> i32 {
        self.team[usize::from(slot)]
    }
    fn set_user_team(&mut self, slot: u8, team: i32) {
        self.team[usize::from(slot)] = team;
    }
    fn get_user_deaths(&self, slot: u8) -> i32 {
        self.deaths[usize::from(slot)]
    }
    fn set_user_deaths(&mut self, slot: u8, deaths: i32) {
        self.deaths[usize::from(slot)] = deaths;
    }
    fn give_item(&mut self, slot: u8, _eng: &mut EngineApi<'_>, item: &str) -> bool {
        self.given.push((slot, item.to_string()));
        true
    }
    fn strip_user_weapons(&mut self, slot: u8, _eng: &mut EngineApi<'_>) {
        self.removed_all.push((slot, false));
    }
    fn get_user_armor(&self, slot: u8, _eng: &EngineApi<'_>) -> i32 {
        self.armor[usize::from(slot)]
    }
    fn set_user_armor(&mut self, slot: u8, _eng: &mut EngineApi<'_>, armor: i32, armor_type: i32) {
        self.armor[usize::from(slot)] = armor;
        self.armor_type[usize::from(slot)] = armor_type;
    }
    fn round_respawn(&mut self, slot: u8, _eng: &mut EngineApi<'_>) -> bool {
        self.respawns.push(slot);
        true
    }
    fn remove_all_items(&mut self, slot: u8, _eng: &mut EngineApi<'_>, remove_suit: bool) {
        self.removed_all.push((slot, remove_suit));
    }
    fn get_member(&self, ent_index: u16, _eng: &EngineApi<'_>, member: i32) -> i32 {
        self.members.get(&(ent_index, member)).copied().unwrap_or(0)
    }
    fn set_member(&mut self, ent_index: u16, _eng: &mut EngineApi<'_>, member: i32, val: i32) -> bool {
        self.members.insert((ent_index, member), val);
        true
    }
    fn execute_ham(
        &mut self,
        func: HamFunction,
        ent_index: u16,
        _eng: &mut EngineApi<'_>,
        args: &[i32],
    ) -> i32 {
        self.ham_execs.push((func, ent_index, args.to_vec()));
        1
    }
}

struct SharedMock(Rc<RefCell<MockHooksGame>>);

impl AmxxCsHooks for SharedMock {
    fn get_user_money(&self, slot: u8) -> i32 {
        self.0.borrow().get_user_money(slot)
    }
    fn set_user_money(&mut self, slot: u8, amount: i32) {
        self.0.borrow_mut().set_user_money(slot, amount);
    }
    fn get_user_team(&self, slot: u8) -> i32 {
        self.0.borrow().get_user_team(slot)
    }
    fn set_user_team(&mut self, slot: u8, team: i32) {
        self.0.borrow_mut().set_user_team(slot, team);
    }
    fn get_user_deaths(&self, slot: u8) -> i32 {
        self.0.borrow().get_user_deaths(slot)
    }
    fn set_user_deaths(&mut self, slot: u8, deaths: i32) {
        self.0.borrow_mut().set_user_deaths(slot, deaths);
    }
    fn give_item(&mut self, slot: u8, eng: &mut EngineApi<'_>, item: &str) -> bool {
        self.0.borrow_mut().give_item(slot, eng, item)
    }
    fn strip_user_weapons(&mut self, slot: u8, eng: &mut EngineApi<'_>) {
        self.0.borrow_mut().strip_user_weapons(slot, eng);
    }
    fn get_user_armor(&self, slot: u8, eng: &EngineApi<'_>) -> i32 {
        self.0.borrow().get_user_armor(slot, eng)
    }
    fn set_user_armor(&mut self, slot: u8, eng: &mut EngineApi<'_>, armor: i32, armor_type: i32) {
        self.0.borrow_mut().set_user_armor(slot, eng, armor, armor_type);
    }
    fn round_respawn(&mut self, slot: u8, eng: &mut EngineApi<'_>) -> bool {
        self.0.borrow_mut().round_respawn(slot, eng)
    }
    fn remove_all_items(&mut self, slot: u8, eng: &mut EngineApi<'_>, remove_suit: bool) {
        self.0.borrow_mut().remove_all_items(slot, eng, remove_suit);
    }
    fn get_member(&self, ent_index: u16, eng: &EngineApi<'_>, member: i32) -> i32 {
        self.0.borrow().get_member(ent_index, eng, member)
    }
    fn set_member(&mut self, ent_index: u16, eng: &mut EngineApi<'_>, member: i32, val: i32) -> bool {
        self.0.borrow_mut().set_member(ent_index, eng, member, val)
    }
    fn execute_ham(
        &mut self,
        func: HamFunction,
        ent_index: u16,
        eng: &mut EngineApi<'_>,
        args: &[i32],
    ) -> i32 {
        self.0.borrow_mut().execute_ham(func, ent_index, eng, args)
    }
}

impl GameDll for SharedMock {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        for name in ["Alpha", "Bravo"] {
            if let Some(slot) = eng.create_fake_client(name) {
                let eid = eng.client_edict(slot);
                if let Some(v) = eng.ent_mut(eid) {
                    v.classname = "player".to_string();
                    v.health = 100.0;
                }
                self.0.borrow_mut().bots.push(slot);
            }
        }
    }
    fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {}
    fn spawn_entity(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _id: EdictId,
        _classname: &str,
        _keys: &[(String, String)],
    ) -> bool {
        true
    }
    fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
        Ok(())
    }
    fn client_put_in_server(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
    fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
    fn client_command(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _argv: &[String]) {}
    fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
    fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {}
    fn update_client_data(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> GoldSrcClientData {
        GoldSrcClientData::default()
    }
    fn add_to_full_pack(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _ctx: &FullPackCtx<'_>,
        _state: &mut GoldSrcEntityState,
    ) -> bool {
        true
    }
    fn run_usercmd(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _slot: u8,
        _cmd: &GoldSrcUserCmd,
        _seed: u32,
    ) {
    }
}

type FrameAction = Box<dyn FnOnce(&mut AmxxGame<SharedMock>, &mut EngineApi<'_>)>;

struct TestDriver {
    game: AmxxGame<SharedMock>,
    pending: Rc<RefCell<Option<FrameAction>>>,
}

impl GameDll for TestDriver {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.game.usermsgs()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_activate(eng);
    }
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_deactivate(eng);
    }
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.game.spawn_entity(eng, id, classname, keys)
    }
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.game.client_connect(eng, slot)
    }
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_put_in_server(eng, slot);
    }
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_disconnect(eng, slot);
    }
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.game.client_command(eng, slot, argv);
    }
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_userinfo_changed(eng, slot);
    }
    fn run_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd, seed: u32) {
        self.game.run_usercmd(eng, slot, cmd, seed);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        self.game.start_frame(eng, frametime);
        let action = self.pending.borrow_mut().take();
        if let Some(f) = action {
            f(&mut self.game, eng);
        }
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.game.update_client_data(eng, slot)
    }
    fn add_to_full_pack(
        &mut self,
        eng: &mut EngineApi<'_>,
        ctx: &FullPackCtx<'_>,
        state: &mut GoldSrcEntityState,
    ) -> bool {
        self.game.add_to_full_pack(eng, ctx, state)
    }
}

struct TestHarness {
    srv: Server<TestDriver>,
    pending: Rc<RefCell<Option<FrameAction>>>,
    mock: Rc<RefCell<MockHooksGame>>,
    now: f64,
}

impl TestHarness {
    fn new(runtime: AmxxRuntime) -> Self {
        let mock = Rc::new(RefCell::new(MockHooksGame::default()));
        let pending = Rc::new(RefCell::new(None));
        let driver = TestDriver {
            game: AmxxGame::with_runtime(SharedMock(Rc::clone(&mock)), runtime),
            pending: Rc::clone(&pending),
        };
        let srv = Server::new(
            ServerConfig {
                max_clients: 8,
                ..ServerConfig::default()
            },
            ServerMap::stub("de_dust2"),
            driver,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        Self {
            srv,
            pending,
            mock,
            now: 0.0,
        }
    }

    fn run(&mut self, f: impl FnOnce(&mut AmxxGame<SharedMock>, &mut EngineApi<'_>) + 'static) {
        *self.pending.borrow_mut() = Some(Box::new(f));
        let mut net = NullTransport;
        self.now += 0.01;
        self.srv.frame(&mut net, self.now);
    }

    fn prints(&self) -> &[String] {
        self.srv.game().game.runtime().state.server_prints()
    }
}

#[test]
fn newmenus_pagination_menuselect_item_getinfo_and_player_menu_info() {
    let mut data = DataBuilder::new();
    let s_title = data.add_str("\\yWeapons Menu");
    let s_handler = data.add_str("on_menu");
    let s_text = data.add_str("\\dSelect one:");
    let s_next = data.add_str("NextPage");
    let mut items = Vec::new();
    for i in 0..9 {
        items.push((
            data.add_str(&format!("Gun {i}")),
            data.add_str(&format!("info_{i}")),
        ));
    }
    let s_fmt = data.add_str("MENU_CB:%d:%s:%s");
    let s_fmt_info = data.add_str("PMINFO:%d:%d:%d");
    let g_menu = data.add_cell(0);
    let cell_old = data.add_cell(0);
    let cell_new = data.add_cell(0);
    let cell_page = data.add_cell(0);
    let cell_has = data.add_cell(0);
    let cell_acc = data.add_cell(0);
    let cell_cb = data.add_cell(0);
    let buf_info = data.alloc_cells(32);
    let buf_name = data.alloc_cells(32);
    let s_open = data.add_str("openmenu");
    let s_open_cb = data.add_str("cmd_open");
    let s_destroy = data.add_str("destroymenu");
    let s_destroy_cb = data.add_str("cmd_destroy");

    let mut code = CodeBuilder::new(&[
        "register_clcmd",
        "menu_create",
        "menu_additem",
        "menu_addblank",
        "menu_addtext",
        "menu_setprop",
        "menu_display",
        "menu_item_getinfo",
        "player_menu_info",
        "menu_destroy",
        "server_print",
    ]);

    code.begin_public("plugin_init");
    code.call_native(
        "register_clcmd",
        &[Arg::Const(s_open as i32), Arg::Const(s_open_cb as i32)],
    );
    code.call_native(
        "register_clcmd",
        &[Arg::Const(s_destroy as i32), Arg::Const(s_destroy_cb as i32)],
    );
    code.call_native(
        "menu_create",
        &[Arg::Const(s_title as i32), Arg::Const(s_handler as i32)],
    );
    code.store_pri(g_menu);
    code.call_native(
        "menu_addtext",
        &[Arg::Mem(g_menu), Arg::Const(s_text as i32)],
    );
    code.call_native("menu_addblank", &[Arg::Mem(g_menu)]);
    for (lbl, inf) in items {
        code.call_native(
            "menu_additem",
            &[Arg::Mem(g_menu), Arg::Const(lbl as i32), Arg::Const(inf as i32)],
        );
    }
    code.call_native(
        "menu_setprop",
        &[Arg::Mem(g_menu), Arg::Const(mprop::NEXTNAME), Arg::Const(s_next as i32)],
    );
    code.ret_const(0);

    code.begin_public("cmd_open");
    code.call_native(
        "menu_display",
        &[Arg::Param(0), Arg::Mem(g_menu), Arg::Const(0)],
    );
    code.call_native(
        "player_menu_info",
        &[
            Arg::Param(0),
            Arg::Const(cell_old as i32),
            Arg::Const(cell_new as i32),
            Arg::Const(cell_page as i32),
        ],
    );
    code.store_pri(cell_has);
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt_info as i32),
            Arg::Const(cell_has as i32),
            Arg::Const(cell_new as i32),
            Arg::Const(cell_page as i32),
        ],
    );
    code.ret_const(1);

    code.begin_public("on_menu");
    code.call_native(
        "menu_item_getinfo",
        &[
            Arg::Param(1),
            Arg::Param(2),
            Arg::Const(cell_acc as i32),
            Arg::Const(buf_info as i32),
            Arg::Const(31),
            Arg::Const(buf_name as i32),
            Arg::Const(31),
            Arg::Const(cell_cb as i32),
        ],
    );
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt as i32),
            Arg::Const(cell_cb as i32),
            Arg::Const(buf_info as i32),
            Arg::Const(buf_name as i32),
        ],
    );
    code.ret_const(0);

    code.begin_public("cmd_destroy");
    code.call_native("menu_destroy", &[Arg::Mem(g_menu)]);
    code.ret_const(1);

    let mut rt = AmxxRuntime::new();
    rt.load_plugin("menus.amxx", &code.build(&data)).unwrap();
    let mut h = TestHarness::new(rt);

    // Open menu on slot 0 (player_id = 1)
    h.run(|game, eng| {
        game.client_command(eng, 0, &["openmenu".into()]);
        // Page 0 -> Press 9 ("NextPage") to turn to page 1 (items 7..8)
        game.client_command(eng, 0, &["menuselect".into(), "9".into()]);
        // Verify player_menu_info now shows page 1
        let info = game.runtime().services().lock().menus.player_menu_info(0);
        assert_eq!(info, Some((1, 1)));
        // Press 2 on page 1 -> selects item index 8 ("Gun 8", "info_8")!
        game.client_command(eng, 0, &["menuselect".into(), "2".into()]);
        // Re-open and press 0 -> MENU_EXIT
        game.client_command(eng, 0, &["openmenu".into()]);
        game.client_command(eng, 0, &["menuselect".into(), "0".into()]);
        assert_eq!(game.runtime().services().lock().menus.player_menu_info(0), None);
        game.client_command(eng, 0, &["destroymenu".into()]);
    });

    let prints = h.prints();
    assert!(prints.iter().any(|p| p == "PMINFO:1:1:0"), "prints={prints:?}");
    assert!(
        prints.iter().any(|p| p == "MENU_CB:-1:info_8:Gun 8"),
        "page 1 option 2 should select item 8 (info_8 / Gun 8), prints={prints:?}"
    );
    let _ = MENU_EXIT;
}

#[test]
fn lang_register_dictionary_lookup_and_percent_l_en_ru() {
    let tmp = std::env::temp_dir().join(format!("cs16_rs_amxx_lang_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("demo.txt"),
        "\u{FEFF}[en]\nWELCOME = Hello %s (%d)\n[ru]\nWELCOME = Privet %s (%d)\n",
    )
    .unwrap();

    let mut data = DataBuilder::new();
    let s_dict = data.add_str("demo.txt");
    let s_key = data.add_str("WELCOME");
    let s_fmt = data.add_str("%L");
    let s_name = data.add_str("Sasha");
    let g_num = data.add_cell(42);
    let buf_out = data.alloc_cells(64);
    let buf_lkp = data.alloc_cells(64);
    let s_fmt_out = data.add_str("FMT:%s|LKP:%s");
    let g_p1 = data.add_cell(1);
    let g_p2 = data.add_cell(2);
    let g_srv = data.add_cell(LANG_SERVER);
    let g_plr = data.add_cell(LANG_PLAYER);

    let mut code = CodeBuilder::new(&[
        "register_dictionary",
        "LookupLangKey",
        "format",
        "server_print",
    ]);

    code.begin_public("plugin_init");
    code.call_native("register_dictionary", &[Arg::Const(s_dict as i32)]);
    code.ret_const(0);

    code.begin_public("test_lang");
    // Slot 0 (id=1, ru)
    code.call_native(
        "format",
        &[
            Arg::Const(buf_out as i32),
            Arg::Const(63),
            Arg::Const(s_fmt as i32),
            Arg::Const(g_p1 as i32),
            Arg::Const(s_key as i32),
            Arg::Const(s_name as i32),
            Arg::Const(g_num as i32),
        ],
    );
    code.call_native(
        "LookupLangKey",
        &[
            Arg::Const(buf_lkp as i32),
            Arg::Const(63),
            Arg::Const(s_key as i32),
            Arg::Const(1),
        ],
    );
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt_out as i32),
            Arg::Const(buf_out as i32),
            Arg::Const(buf_lkp as i32),
        ],
    );
    // Slot 1 (id=2, en) + LANG_SERVER fallback (en)
    code.call_native(
        "format",
        &[
            Arg::Const(buf_out as i32),
            Arg::Const(63),
            Arg::Const(s_fmt as i32),
            Arg::Const(g_p2 as i32),
            Arg::Const(s_key as i32),
            Arg::Const(s_name as i32),
            Arg::Const(g_num as i32),
        ],
    );
    code.call_native(
        "LookupLangKey",
        &[
            Arg::Const(buf_lkp as i32),
            Arg::Const(63),
            Arg::Const(s_key as i32),
            Arg::Const(LANG_SERVER),
        ],
    );
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt_out as i32),
            Arg::Const(buf_out as i32),
            Arg::Const(buf_lkp as i32),
        ],
    );
    let _ = (g_srv, g_plr);
    code.ret_const(0);

    let mut rt = AmxxRuntime::new();
    rt.add_lang_search_dir(&tmp);
    rt.load_plugin("lang_demo.amxx", &code.build(&data)).unwrap();
    {
        let svc = rt.services();
        let mut s = svc.lock();
        s.lang.set_player_lang(0, None, "ru");
        s.lang.set_player_lang(1, None, "en");
    }
    let mut h = TestHarness::new(rt);
    h.run(|game, eng| {
        let (rt, inner) = game.parts_mut();
        rt.call_public(0, eng, inner, "test_lang", &[]).unwrap();
    });
    let _ = std::fs::remove_dir_all(&tmp);

    let prints = h.prints();
    assert!(
        prints.iter().any(|p| p == "FMT:Privet Sasha (42)|LKP:Privet %s (%d)"),
        "slot 0 (ru) should render Russian %L + LookupLangKey, prints={prints:?}"
    );
    assert!(
        prints.iter().any(|p| p == "FMT:Hello Sasha (42)|LKP:Hello %s (%d)"),
        "slot 1 (en) & LANG_SERVER should render English %L + LookupLangKey, prints={prints:?}"
    );
}

#[test]
fn csx_forwards_and_leaderboard_backed_stats_and_nvault_full_lifecycle() {
    let mut data = DataBuilder::new();
    let s_vault = data.add_str("xp_vault");
    let s_k1 = data.add_str("k_temp");
    let s_v1 = data.add_str("123");
    let s_k2 = data.add_str("k_perm");
    let s_v2 = data.add_str("perm_ok");
    let s_fmt_nv = data.add_str("NV:%d:%s:%d:%d");
    let s_fmt_csx = data.add_str("CSX:N=%d:RANK=%d:K=%d:D=%d:HS=%d:DMG=%d:NAME=%s");
    let s_fmt_ev = data.add_str("EV:%s:%d");
    let s_bp = data.add_str("BOMB_PLANTED");
    let s_bd = data.add_str("BOMB_DEFUSED");
    let s_gt = data.add_str("GRENADE");

    let g_vh = data.add_cell(0);
    let g_int = data.add_cell(0);
    let g_ts = data.add_cell(0);
    let g_pruned = data.add_cell(0);
    let g_perm_ok = data.add_cell(0);
    let g_num = data.add_cell(0);
    let g_rank = data.add_cell(0);
    let stats_arr = data.alloc_cells(8);
    let body_arr = data.alloc_cells(8);
    let buf_val = data.alloc_cells(32);
    let buf_name = data.alloc_cells(32);
    let buf_auth = data.alloc_cells(32);
    let g_ev_arg = data.add_cell(0);

    let mut code = CodeBuilder::new(&[
        "nvault_open",
        "nvault_set",
        "nvault_pset",
        "nvault_get",
        "nvault_lookup",
        "nvault_prune",
        "nvault_remove",
        "nvault_close",
        "get_statsnum",
        "get_user_stats",
        "get_stats",
        "server_print",
    ]);

    code.begin_public("test_nvault");
    code.call_native("nvault_open", &[Arg::Const(s_vault as i32)]);
    code.store_pri(g_vh);
    code.call_native(
        "nvault_set",
        &[Arg::Mem(g_vh), Arg::Const(s_k1 as i32), Arg::Const(s_v1 as i32)],
    );
    code.call_native(
        "nvault_pset",
        &[Arg::Mem(g_vh), Arg::Const(s_k2 as i32), Arg::Const(s_v2 as i32)],
    );
    code.call_native(
        "nvault_get",
        &[Arg::Mem(g_vh), Arg::Const(s_k1 as i32)],
    );
    code.store_pri(g_int);
    code.call_native(
        "nvault_lookup",
        &[
            Arg::Mem(g_vh),
            Arg::Const(s_k2 as i32),
            Arg::Const(buf_val as i32),
            Arg::Const(31),
            Arg::Const(g_ts as i32),
        ],
    );
    code.store_pri(g_perm_ok);
    // Prune timestamps 0..=100 (should prune k_temp, keeping permanent k_perm!)
    code.call_native(
        "nvault_prune",
        &[Arg::Mem(g_vh), Arg::Const(0), Arg::Const(100)],
    );
    code.store_pri(g_pruned);
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt_nv as i32),
            Arg::Const(g_int as i32),
            Arg::Const(buf_val as i32),
            Arg::Const(g_pruned as i32),
            Arg::Const(g_perm_ok as i32),
        ],
    );
    code.call_native(
        "nvault_remove",
        &[Arg::Mem(g_vh), Arg::Const(s_k2 as i32)],
    );
    code.call_native("nvault_close", &[Arg::Mem(g_vh)]);
    code.ret_const(0);

    code.begin_public("bomb_planted");
    code.cells.push(OP_LOAD_S_PRI);
    code.cells.push(12);
    code.store_pri(g_ev_arg);
    code.call_native(
        "server_print",
        &[Arg::Const(s_fmt_ev as i32), Arg::Const(s_bp as i32), Arg::Const(g_ev_arg as i32)],
    );
    code.ret_const(0);

    code.begin_public("bomb_defused");
    code.cells.push(OP_LOAD_S_PRI);
    code.cells.push(12);
    code.store_pri(g_ev_arg);
    code.call_native(
        "server_print",
        &[Arg::Const(s_fmt_ev as i32), Arg::Const(s_bd as i32), Arg::Const(g_ev_arg as i32)],
    );
    code.ret_const(0);

    code.begin_public("grenade_throw");
    code.cells.push(OP_LOAD_S_PRI);
    code.cells.push(12 + 2 * 4);
    code.store_pri(g_ev_arg);
    code.call_native(
        "server_print",
        &[Arg::Const(s_fmt_ev as i32), Arg::Const(s_gt as i32), Arg::Const(g_ev_arg as i32)],
    );
    code.ret_const(0);

    code.begin_public("dump_csx");
    code.call_native("get_statsnum", &[]);
    code.store_pri(g_num);
    code.call_native(
        "get_user_stats",
        &[Arg::Const(1), Arg::Const(stats_arr as i32), Arg::Const(body_arr as i32)],
    );
    code.store_pri(g_rank);
    code.call_native(
        "get_stats",
        &[
            Arg::Const(0),
            Arg::Const(stats_arr as i32),
            Arg::Const(body_arr as i32),
            Arg::Const(buf_name as i32),
            Arg::Const(31),
            Arg::Const(buf_auth as i32),
            Arg::Const(31),
        ],
    );
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt_csx as i32),
            Arg::Const(g_num as i32),
            Arg::Const(g_rank as i32),
            Arg::Const(stats_arr as i32),       // [0] kills
            Arg::Const((stats_arr + 4) as i32), // [1] deaths
            Arg::Const((stats_arr + 8) as i32), // [2] headshots
            Arg::Const((stats_arr + 24) as i32), // [6] damage
            Arg::Const(buf_name as i32),
        ],
    );
    code.ret_const(0);

    let mut rt = AmxxRuntime::new();
    rt.load_plugin("csx_nvault.amxx", &code.build(&data)).unwrap();
    let mut h = TestHarness::new(rt);
    h.run(|game, eng| {
        let (rt, inner) = game.parts_mut();
        rt.call_public(0, eng, inner, "test_nvault", &[]).unwrap();
        rt.dispatch_client_damage(eng, inner, Some(0), 1, 108, 28, 1, false);
        rt.dispatch_client_death(eng, inner, Some(0), 1, 28, 1, false);
        rt.dispatch_bomb_planting(eng, inner, 0);
        rt.dispatch_bomb_planted(eng, inner, 0);
        rt.dispatch_bomb_defusing(eng, inner, 1);
        rt.dispatch_bomb_defused(eng, inner, 1);
        rt.dispatch_bomb_explode(eng, inner, Some(0), None);
        rt.dispatch_grenade_throw(eng, inner, 0, 55, 4);
        rt.call_public(0, eng, inner, "dump_csx", &[]).unwrap();
    });

    let prints = h.prints();
    assert!(prints.iter().any(|p| p == "NV:123:perm_ok:1:1"), "prints={prints:?}");
    assert!(prints.iter().any(|p| p == "EV:BOMB_PLANTED:1"), "prints={prints:?}");
    assert!(prints.iter().any(|p| p == "EV:BOMB_DEFUSED:2"), "prints={prints:?}");
    assert!(prints.iter().any(|p| p == "EV:GRENADE:4"), "prints={prints:?}");
    assert!(
        prints.iter().any(|p| p == "CSX:N=2:RANK=1:K=1:D=0:HS=1:DMG=108:NAME=Alpha"),
        "prints={prints:?}"
    );
}

#[test]
fn expanded_hamsandwich_overrides_execute_ham_and_reapi_natives() {
    let mut data = DataBuilder::new();
    let s_player = data.add_str("player");
    let s_ak = data.add_str("weapon_ak47");
    let s_cb_dmg = data.add_str("on_dmg");
    let s_cb_jump = data.add_str("on_jump");
    let s_cb_deploy = data.add_str("on_deploy");
    let s_cb_atk = data.add_str("on_atk");
    let s_cb_spawn = data.add_str("on_spawn_post");
    let s_fmt = data.add_str("MEM:%d|SPAWN_HOOK:%d");
    let g_mem = data.add_cell(0);
    let g_spawn_hits = data.add_cell(0);

    let mut code = CodeBuilder::new(&[
        "RegisterHam",
        "SetHamParamFloat",
        "SetHamParamInteger",
        "ExecuteHam",
        "ExecuteHamB",
        "rg_give_item",
        "rg_remove_all_items",
        "rg_set_user_armor",
        "rg_add_account",
        "rg_round_respawn",
        "set_member",
        "get_member",
        "server_print",
    ]);

    code.begin_public("plugin_init");
    code.call_native(
        "RegisterHam",
        &[Arg::Const(HamFunction::TakeDamage as i32), Arg::Const(s_player as i32), Arg::Const(s_cb_dmg as i32), Arg::Const(0)],
    );
    code.call_native(
        "RegisterHam",
        &[Arg::Const(HamFunction::PlayerJump as i32), Arg::Const(s_player as i32), Arg::Const(s_cb_jump as i32), Arg::Const(0)],
    );
    code.call_native(
        "RegisterHam",
        &[Arg::Const(HamFunction::ItemDeploy as i32), Arg::Const(s_ak as i32), Arg::Const(s_cb_deploy as i32), Arg::Const(0)],
    );
    code.call_native(
        "RegisterHam",
        &[Arg::Const(HamFunction::WeaponPrimaryAttack as i32), Arg::Const(s_ak as i32), Arg::Const(s_cb_atk as i32), Arg::Const(0)],
    );
    code.call_native(
        "RegisterHam",
        &[Arg::Const(HamFunction::Spawn as i32), Arg::Const(s_player as i32), Arg::Const(s_cb_spawn as i32), Arg::Const(1)],
    );
    code.ret_const(0);

    code.begin_public("on_dmg");
    code.call_native("SetHamParamFloat", &[Arg::Const(4), Arg::Const(25.0f32.to_bits() as i32)]);
    code.call_native("SetHamParamInteger", &[Arg::Const(5), Arg::Const(64)]);
    code.ret_const(HAM_HANDLED);

    code.begin_public("on_jump");
    code.ret_const(HAM_SUPERCEDE);

    code.begin_public("on_deploy");
    code.ret_const(HAM_HANDLED);

    code.begin_public("on_atk");
    code.ret_const(HAM_SUPERCEDE);

    code.begin_public("on_spawn_post");
    code.load_pri(g_spawn_hits);
    code.add_const(1);
    code.store_pri(g_spawn_hits);
    code.ret_const(HAM_HANDLED);

    code.begin_public("run_reapi_and_exec");
    code.call_native("rg_remove_all_items", &[Arg::Const(1), Arg::Const(1)]);
    code.call_native("rg_give_item", &[Arg::Const(1), Arg::Const(s_ak as i32), Arg::Const(GT_REPLACE)]);
    code.call_native("rg_set_user_armor", &[Arg::Const(1), Arg::Const(100), Arg::Const(CS_ARMOR_VESTHELM)]);
    code.call_native("rg_add_account", &[Arg::Const(1), Arg::Const(4000), Arg::Const(AS_SET), Arg::Const(1)]);
    code.call_native("rg_add_account", &[Arg::Const(1), Arg::Const(500), Arg::Const(AS_ADD), Arg::Const(1)]);
    code.call_native("rg_round_respawn", &[Arg::Const(1)]);
    code.call_native("set_member", &[Arg::Const(1), Arg::Const(reapi_member::M_I_CLIP), Arg::Const(45)]);
    code.call_native("get_member", &[Arg::Const(1), Arg::Const(reapi_member::M_I_CLIP)]);
    code.store_pri(g_mem);
    // ExecuteHam (no hooks) then ExecuteHamB (triggers on_spawn_post!)
    code.call_native("ExecuteHam", &[Arg::Const(HamFunction::Spawn as i32), Arg::Const(1)]);
    code.call_native("ExecuteHamB", &[Arg::Const(HamFunction::Spawn as i32), Arg::Const(1)]);
    code.call_native(
        "server_print",
        &[Arg::Const(s_fmt as i32), Arg::Const(g_mem as i32), Arg::Const(g_spawn_hits as i32)],
    );
    code.ret_const(0);

    let mut rt = AmxxRuntime::new();
    rt.load_plugin("ham_reapi.amxx", &code.build(&data)).unwrap();
    let mut h = TestHarness::new(rt);

    h.run(|game, eng| {
        let (rt, inner) = game.parts_mut();
        let (ret, dmg, bits) = rt.dispatch_ham_take_damage_mut(eng, inner, 1, "player", 2, 2, 99.0, 2, false);
        assert_eq!(ret, HAM_HANDLED);
        assert_eq!(dmg, Some(25.0));
        assert_eq!(bits, Some(64));
        assert_eq!(rt.dispatch_ham_player_jump(eng, inner, 1, false), HAM_SUPERCEDE);
        assert_eq!(rt.dispatch_ham_item_deploy(eng, inner, 1, "weapon_ak47", false), HAM_HANDLED);
        assert_eq!(rt.dispatch_ham_weapon_primary_attack(eng, inner, 1, "weapon_ak47", false), HAM_SUPERCEDE);
        rt.call_public(0, eng, inner, "run_reapi_and_exec", &[]).unwrap();
    });

    let m = h.mock.borrow();
    assert_eq!(m.money[0], 4500);
    assert_eq!(m.armor[0], 100);
    assert_eq!(m.armor_type[0], CS_ARMOR_VESTHELM);
    assert_eq!(m.respawns, vec![0]);
    assert!(m.given.contains(&(0, "weapon_ak47".to_string())));
    assert_eq!(m.removed_all, vec![(0, true), (0, false)]);
    assert_eq!(m.ham_execs.len(), 2);
    let prints = h.prints();
    assert!(
        prints.iter().any(|p| p == "MEM:45|SPAWN_HOOK:1"),
        "ExecuteHam skips hooks, ExecuteHamB triggers post hook once, prints={prints:?}"
    );
}

#[test]
fn dynamic_inter_plugin_natives_and_multi_forwards() {
    // Plugin 0: exports `add_and_prefix(val, in_str, out_str, maxlen)` via `register_native`,
    // and receives multi-forward `public on_custom_fwd(val, const msg[])`.
    let mut d0 = DataBuilder::new();
    let s_nat = d0.add_str("add_and_prefix");
    let s_cb = d0.add_str("native_handler");
    let s_fmt_out = d0.add_str("PRE:%s");
    let s_fmt_fwd = d0.add_str("P0_FWD:%d:%s");
    let buf_in = d0.alloc_cells(32);
    let buf_out = d0.alloc_cells(32);
    let g_val = d0.add_cell(0);
    let g_fwd_val = d0.add_cell(0);

    let mut c0 = CodeBuilder::new(&[
        "register_native",
        "get_param",
        "get_string",
        "set_string",
        "format",
        "server_print",
    ]);
    c0.begin_public("plugin_init");
    c0.call_native("register_native", &[Arg::Const(s_nat as i32), Arg::Const(s_cb as i32)]);
    c0.ret_const(0);

    c0.begin_public("native_handler");
    c0.call_native("get_param", &[Arg::Const(1)]);
    c0.add_const(100);
    c0.store_pri(g_val);
    c0.call_native("get_string", &[Arg::Const(2), Arg::Const(buf_in as i32), Arg::Const(31)]);
    c0.call_native(
        "format",
        &[Arg::Const(buf_out as i32), Arg::Const(31), Arg::Const(s_fmt_out as i32), Arg::Const(buf_in as i32)],
    );
    c0.call_native("set_string", &[Arg::Const(3), Arg::Const(buf_out as i32), Arg::Const(31)]);
    c0.load_pri(g_val);
    c0.ret_pri();

    c0.begin_public("on_custom_fwd");
    c0.cells.push(OP_LOAD_S_PRI);
    c0.cells.push(12);
    c0.store_pri(g_fwd_val);
    c0.call_native("server_print", &[Arg::Const(s_fmt_fwd as i32), Arg::Const(g_fwd_val as i32), Arg::Param(1)]);
    c0.ret_const(7);

    // Plugin 1: calls `add_and_prefix(4, "world", out_buf, 31)` (note `4` is a multiple of 4 to
    // verify scalar `get_param(1)` isn't confused with string pointer!), then creates, executes,
    // and destroys multi-forward `"on_custom_fwd"`.
    let mut d1 = DataBuilder::new();
    let s_in = d1.add_str("world");
    let buf_res = d1.alloc_cells(32);
    let s_fwd_name = d1.add_str("on_custom_fwd");
    let s_fwd_msg = d1.add_str("hello_fwd");
    let g_ret = d1.add_cell(0);
    let g_fwd = d1.add_cell(0);
    let g_fwd_ret = d1.add_cell(0);
    let g_fwd_arg = d1.add_cell(55);
    let g_destroyed = d1.add_cell(0);
    let s_fmt = d1.add_str("NAT:%d:%s|FWD:%d:%d");

    let mut c1 = CodeBuilder::new(&[
        "add_and_prefix",
        "CreateMultiForward",
        "ExecuteForward",
        "DestroyForward",
        "server_print",
    ]);
    c1.begin_public("run_test");
    c1.call_native(
        "add_and_prefix",
        &[Arg::Const(4), Arg::Const(s_in as i32), Arg::Const(buf_res as i32), Arg::Const(31)],
    );
    c1.store_pri(g_ret);
    c1.call_native(
        "CreateMultiForward",
        &[Arg::Const(s_fwd_name as i32), Arg::Const(ET_CONTINUE), Arg::Const(FP_CELL), Arg::Const(FP_STRING)],
    );
    c1.store_pri(g_fwd);
    c1.call_native(
        "ExecuteForward",
        &[Arg::Mem(g_fwd), Arg::Const(g_fwd_ret as i32), Arg::Const(g_fwd_arg as i32), Arg::Const(s_fwd_msg as i32)],
    );
    c1.call_native("DestroyForward", &[Arg::Mem(g_fwd)]);
    c1.store_pri(g_destroyed);
    c1.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt as i32),
            Arg::Const(g_ret as i32),
            Arg::Const(buf_res as i32),
            Arg::Const(g_fwd_ret as i32),
            Arg::Const(g_destroyed as i32),
        ],
    );
    c1.ret_const(PLUGIN_CONTINUE);

    let mut rt = AmxxRuntime::new();
    rt.load_plugin("provider.amxx", &c0.build(&d0)).unwrap();
    rt.load_plugin("consumer.amxx", &c1.build(&d1)).unwrap();
    let mut h = TestHarness::new(rt);

    h.run(|game, eng| {
        let (rt, inner) = game.parts_mut();
        rt.call_public(1, eng, inner, "run_test", &[]).unwrap();
    });

    let prints = h.prints();
    assert!(prints.iter().any(|p| p == "P0_FWD:55:hello_fwd"), "prints={prints:?}");
    assert!(
        prints.iter().any(|p| p == "NAT:104:PRE:world|FWD:7:1"),
        "prints={prints:?}"
    );
}
