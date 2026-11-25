//! Integration tests for `cs-amxx`: `.amxx` loading, VM execution, lifecycle forwards,
//! `set_task`, `register_clcmd`/`concmd`/`srvcmd`, CVars/PCVars, `cstrike`, `fun`, `engine`,
//! `fakemeta`, and `hamsandwich` natives inside a real `cs_engine::Server`.

use std::cell::RefCell;
use std::rc::Rc;

use cs_amxx::amxxfile::build_test_amxx_plugin;
use cs_amxx::natives::{
    AmxxCsHooks, HAM_IGNORED, HAM_SUPERCEDE, HamFunction, PLUGIN_CONTINUE, PLUGIN_HANDLED,
    PRINT_CENTER, PRINT_CHAT, PRINT_CONSOLE, ev_const, pev_const,
};
use cs_amxx::vm::op::*;
use cs_amxx::{AmxxGame, AmxxRuntime, NoCsHooks};
use cs_cvar::CVarRegistry;
use cs_engine::{ConsoleOutcome, EdictId, EngineApi, FullPackCtx, GameDll, Server, ServerConfig, ServerMap};
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{PacketTransport, PeerAddr, TransportError};
use glam::Vec3;

// --- Mini Pawn 3.2 bytecode assembler for tests ---

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

    /// Appends an unpacked NUL-terminated string and returns its AMX data address.
    fn add_str(&mut self, s: &str) -> u32 {
        self.align4();
        let addr = self.bytes.len() as u32;
        for &b in s.as_bytes() {
            self.bytes.extend_from_slice(&i32::from(b).to_le_bytes());
        }
        self.bytes.extend_from_slice(&0i32.to_le_bytes());
        addr
    }

    /// Appends a single cell and returns its AMX data address.
    fn add_cell(&mut self, val: i32) -> u32 {
        self.align4();
        let addr = self.bytes.len() as u32;
        self.bytes.extend_from_slice(&val.to_le_bytes());
        addr
    }

    /// Appends a cell array and returns its AMX data address.
    fn add_cells(&mut self, vals: &[i32]) -> u32 {
        self.align4();
        let addr = self.bytes.len() as u32;
        for &v in vals {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        addr
    }

    /// Reserves `num_cells` zeroed cells and returns the AMX data address.
    fn alloc_cells(&mut self, num_cells: usize) -> u32 {
        self.align4();
        let addr = self.bytes.len() as u32;
        self.bytes.resize(self.bytes.len() + num_cells * 4, 0);
        addr
    }
}

#[derive(Clone, Copy)]
enum Arg {
    Const(i32),
    Param(u32),
    Pri,
}

struct CodeBuilder {
    cells: Vec<u32>,
    publics: Vec<(String, u32)>,
    natives: Vec<String>,
}

impl CodeBuilder {
    fn new(natives: &[&str]) -> Self {
        Self {
            cells: vec![OP_HALT, 0], // Offset 0 is always HALT 0.
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

    fn store_pri(&mut self, data_addr: u32) {
        self.cells.push(OP_STOR_PRI);
        self.cells.push(data_addr);
    }

    fn load_pri(&mut self, data_addr: u32) {
        self.cells.push(OP_LOAD_PRI);
        self.cells.push(data_addr);
    }

    fn inc_mem(&mut self, data_addr: u32) {
        self.cells.push(OP_INC);
        self.cells.push(data_addr);
    }

    fn ret_const(&mut self, val: i32) {
        self.cells.push(OP_CONST_PRI);
        self.cells.push(val as u32);
        self.cells.push(OP_RETN);
    }

    fn build(&self, data: &DataBuilder, pubvars: &[(&str, u32)]) -> Vec<u8> {
        let pub_refs: Vec<(&str, u32)> = self
            .publics
            .iter()
            .map(|(n, o)| (n.as_str(), *o))
            .collect();
        let nat_refs: Vec<&str> = self.natives.iter().map(String::as_str).collect();
        build_test_amxx_plugin(
            &pub_refs,
            &nat_refs,
            pubvars,
            &self.cells,
            &data.bytes,
            4096,
            0,
        )
    }
}

// --- Mock CS Game & Engine Test Harness ---

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
struct MockCsGame {
    money: [i32; 32],
    team: [i32; 32],
    deaths: [i32; 32],
    armor: [i32; 32],
    armor_type: [i32; 32],
    given_items: Vec<(u8, String)>,
    stripped_slots: Vec<u8>,
    forwarded_clcmds: Vec<(u8, Vec<String>)>,
    spawned_entities: Vec<String>,
    precache_ok: bool,
    fake_bot_slot: Option<u8>,
    extra_entity: Option<EdictId>,
}

impl AmxxCsHooks for MockCsGame {
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
        self.given_items.push((slot, item.to_string()));
        true
    }
    fn strip_user_weapons(&mut self, slot: u8, _eng: &mut EngineApi<'_>) {
        self.stripped_slots.push(slot);
    }
    fn get_user_armor(&self, slot: u8, _eng: &EngineApi<'_>) -> i32 {
        self.armor[usize::from(slot)]
    }
    fn set_user_armor(&mut self, slot: u8, _eng: &mut EngineApi<'_>, armor: i32, armor_type: i32) {
        self.armor[usize::from(slot)] = armor;
        self.armor_type[usize::from(slot)] = armor_type;
    }
}

impl GameDll for MockCsGame {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }

    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        // Verify plugin_precache ran BEFORE server_activate precache lock!
        self.precache_ok = eng.model_index("models/custom_amxx.mdl").is_some();
        if let Some(slot) = eng.create_fake_client("TestBot") {
            self.fake_bot_slot = Some(slot);
            let eid = eng.client_edict(slot);
            if let Some(v) = eng.ent_mut(eid) {
                v.classname = "player".to_string();
                v.health = 100.0;
                v.origin = Vec3::new(100.0, 200.0, 300.0);
                v.view_ofs = Vec3::new(0.0, 0.0, 17.0);
            }
            self.money[usize::from(slot)] = 800;
            self.team[usize::from(slot)] = 2; // CT
        }
        if let Some(ent) = eng.create_entity("info_target") {
            self.extra_entity = Some(ent);
        }
    }

    fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {}

    fn spawn_entity(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _id: EdictId,
        classname: &str,
        _keys: &[(String, String)],
    ) -> bool {
        self.spawned_entities.push(classname.to_string());
        true
    }

    fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
        Ok(())
    }

    fn client_put_in_server(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
    fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}

    fn client_command(&mut self, _eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.forwarded_clcmds.push((slot, argv.to_vec()));
    }

    fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
    fn run_usercmd(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _cmd: &GoldSrcUserCmd, _r: u32) {}
    fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {}
    fn update_client_data(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> GoldSrcClientData {
        GoldSrcClientData::default()
    }
    fn setup_visibility(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Option<Vec3> {
        None
    }
    fn add_to_full_pack(
        &mut self,
        _eng: &mut EngineApi<'_>,
        ctx: &FullPackCtx<'_>,
        _state: &mut GoldSrcEntityState,
    ) -> bool {
        ctx.in_pvs || ctx.ent_id == ctx.viewer_id
    }
}

type HookActionFn = Box<dyn FnMut(&mut AmxxGame<MockCsGame>, &mut EngineApi<'_>)>;

struct TestDriver {
    game: AmxxGame<MockCsGame>,
    pending: Rc<RefCell<Option<HookActionFn>>>,
}

impl TestDriver {
    fn new(plugins: Vec<(String, Vec<u8>)>) -> (Self, Rc<RefCell<Option<HookActionFn>>>) {
        let pending = Rc::new(RefCell::new(None));
        (
            Self {
                game: AmxxGame::new(MockCsGame::default(), plugins),
                pending: Rc::clone(&pending),
            },
            pending,
        )
    }
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
    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        self.game.run_usercmd(eng, slot, cmd, random_seed);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        self.game.start_frame(eng, frametime);
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.game.update_client_data(eng, slot)
    }
    fn server_command(&mut self, eng: &mut EngineApi<'_>, argv: &[String]) -> Option<String> {
        if argv.first().map(String::as_str) == Some("__run") {
            if let Some(mut cb) = self.pending.borrow_mut().take() {
                cb(&mut self.game, eng);
            }
            return Some("ok\n".to_string());
        }
        self.game.server_command(eng, argv)
    }
}

fn spawn_test_server(
    plugins: Vec<(String, Vec<u8>)>,
    map: ServerMap,
) -> (Server<TestDriver>, Rc<RefCell<Option<HookActionFn>>>) {
    let (driver, pending) = TestDriver::new(plugins);
    let cfg = ServerConfig {
        max_clients: 8,
        ..ServerConfig::default()
    };
    let cvars = CVarRegistry::new_with_defaults();
    let server = Server::new(cfg, map, driver, DeltaRegistry::default(), cvars);
    (server, pending)
}

fn run_with_eng(
    server: &mut Server<TestDriver>,
    pending: &Rc<RefCell<Option<HookActionFn>>>,
    f: impl FnOnce(&mut AmxxGame<MockCsGame>, &mut EngineApi<'_>) + 'static,
) {
    let mut opt = Some(f);
    *pending.borrow_mut() = Some(Box::new(move |g, eng| {
        if let Some(func) = opt.take() {
            func(g, eng);
        }
    }));
    let out = server.console_command("__run");
    assert_eq!(out, ConsoleOutcome::Output("ok\n".to_string()));
}

#[test]
fn lifecycle_forwards_cvars_and_server_cmd_draining() {
    let mut d = DataBuilder::new();
    let s_title = d.add_str("Lifecycle Test");
    let s_ver = d.add_str("1.2.3");
    let s_author = d.add_str("cs16-rs");
    let s_cvar = d.add_str("amx_custom_cvar");
    let s_cvar_def = d.add_str("42");
    let s_mdl = d.add_str("models/custom_amxx.mdl");
    let s_sv_cmd = d.add_str("sv_gravity 250");
    let s_srv_name = d.add_str("my_srvcmd");
    let s_srv_cb = d.add_str("on_srvcmd");
    let s_fmt = d.add_str("cfg_done");
    let s_end_msg = d.add_str("plugin_ended");
    let pcvar_slot = d.add_cell(0);
    let max_clients_var = d.add_cell(0);
    let map_name_buf = d.alloc_cells(32);

    let mut c = CodeBuilder::new(&[
        "precache_model",
        "register_plugin",
        "register_cvar",
        "set_pcvar_num",
        "get_pcvar_num",
        "server_cmd",
        "server_print",
        "register_srvcmd",
        "read_argv",
        "log_amx",
    ]);

    c.begin_public("plugin_precache");
    c.call_native("precache_model", &[Arg::Const(s_mdl as i32)]);
    c.call_native("log_amx", &[Arg::Const(s_title as i32)]);
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("plugin_init");
    c.call_native(
        "register_plugin",
        &[
            Arg::Const(s_title as i32),
            Arg::Const(s_ver as i32),
            Arg::Const(s_author as i32),
        ],
    );
    c.call_native(
        "register_cvar",
        &[
            Arg::Const(s_cvar as i32),
            Arg::Const(s_cvar_def as i32),
            Arg::Const(0),
        ],
    );
    c.store_pri(pcvar_slot);
    c.load_pri(pcvar_slot);
    c.call_native("set_pcvar_num", &[Arg::Pri, Arg::Const(99)]);
    c.call_native(
        "register_srvcmd",
        &[
            Arg::Const(s_srv_name as i32),
            Arg::Const(s_srv_cb as i32),
        ],
    );
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("plugin_cfg");
    c.call_native("server_cmd", &[Arg::Const(s_sv_cmd as i32)]);
    c.call_native("server_print", &[Arg::Const(s_fmt as i32)]);
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("plugin_end");
    c.call_native("log_amx", &[Arg::Const(s_end_msg as i32)]);
    c.ret_const(PLUGIN_CONTINUE);

    let arg1_buf = d.alloc_cells(32);
    c.begin_public("on_srvcmd");
    c.call_native(
        "read_argv",
        &[Arg::Const(1), Arg::Const(arg1_buf as i32), Arg::Const(31)],
    );
    c.call_native("server_print", &[Arg::Const(arg1_buf as i32)]);
    c.ret_const(PLUGIN_HANDLED);

    let amxx = c.build(&d, &[("MaxClients", max_clients_var), ("MapName", map_name_buf)]);
    let (mut server, pending) = spawn_test_server(
        vec![("lifecycle.amxx".to_string(), amxx)],
        ServerMap::stub("de_inferno"),
    );

    // `plugin_precache`, `plugin_init`, and `plugin_cfg` already ran on `Server::new`!
    assert!(server.game().game.inner().precache_ok);
    // Check `MaxClients` and `MapName` pubvars were populated.
    let vm = &server.game().game.runtime().plugins[0].vm;
    assert_eq!(vm.read_cell(max_clients_var as i32).unwrap(), 8);
    assert_eq!(vm.get_amx_string(map_name_buf as i32).unwrap(), "de_inferno");

    // Check `server_cmd("sv_gravity 250")` drained into engine CVarRegistry!
    assert_eq!(server.console_command("sv_gravity"), ConsoleOutcome::Output("\"sv_gravity\" is \"250\"\n".to_string()));

    // Check `amxx plugins` and `amxx cvars`
    let ConsoleOutcome::Output(plugins_out) = server.console_command("amxx plugins") else {
        panic!("expected output");
    };
    assert!(plugins_out.contains("Lifecycle Test"));
    assert!(plugins_out.contains("1.2.3"));
    let ConsoleOutcome::Output(cvars_out) = server.console_command("amxx cvars") else {
        panic!("expected output");
    };
    assert!(cvars_out.contains("amx_custom_cvar = \"99\""));

    // Check `register_srvcmd`
    assert_eq!(
        server.console_command("my_srvcmd hello_world"),
        ConsoleOutcome::Output("hello_world\n".to_string())
    );

    // Check `plugin_end`
    run_with_eng(&mut server, &pending, move |g, eng| {
        g.server_deactivate(eng);
    });
    assert!(
        server
            .game()
            .game
            .runtime()
            .state
            .amx_logs()
            .iter()
            .any(|l| l == "plugin_ended")
    );
}

#[test]
fn set_task_oneshot_repeat_and_remove_task() {
    let mut d = DataBuilder::new();
    let s_once = d.add_str("task_once");
    let s_rep = d.add_str("task_rep");
    let s_inf = d.add_str("task_inf");
    let s_flag_a = d.add_str("a");
    let s_flag_b = d.add_str("b");
    let param_arr = d.add_cells(&[77, 88]);
    let once_count = d.add_cell(0);
    let rep_count = d.add_cell(0);
    let inf_count = d.add_cell(0);
    let saved_param0 = d.add_cell(0);

    let mut c = CodeBuilder::new(&["set_task", "remove_task", "task_exists"]);
    c.begin_public("plugin_init");
    // One-shot at +0.6s with param_arr (2 cells), id=10
    c.call_native(
        "set_task",
        &[
            Arg::Const(0.6f32.to_bits() as i32),
            Arg::Const(s_once as i32),
            Arg::Const(10),
            Arg::Const(param_arr as i32),
            Arg::Const(2),
        ],
    );
    // Repeat 2 times ("a") every 0.2s, id=20
    c.call_native(
        "set_task",
        &[
            Arg::Const(0.2f32.to_bits() as i32),
            Arg::Const(s_rep as i32),
            Arg::Const(20),
            Arg::Const(0),
            Arg::Const(0),
            Arg::Const(s_flag_a as i32),
            Arg::Const(2),
        ],
    );
    // Infinite ("b") every 0.2s, id=30
    c.call_native(
        "set_task",
        &[
            Arg::Const(0.2f32.to_bits() as i32),
            Arg::Const(s_inf as i32),
            Arg::Const(30),
            Arg::Const(0),
            Arg::Const(0),
            Arg::Const(s_flag_b as i32),
        ],
    );
    c.ret_const(PLUGIN_CONTINUE);

    // task_once(const param[], id)
    c.begin_public("task_once");
    c.inc_mem(once_count);
    // param[0] -> load param ptr (arg 0), LOAD_I, store to saved_param0
    c.cells.push(OP_LOAD_S_PRI);
    c.cells.push(12);
    c.cells.push(OP_LOAD_I);
    c.store_pri(saved_param0);
    // Also remove task 30 inside task_once!
    c.call_native("remove_task", &[Arg::Const(30)]);
    c.ret_const(0);

    c.begin_public("task_rep");
    c.inc_mem(rep_count);
    c.ret_const(0);

    c.begin_public("task_inf");
    c.inc_mem(inf_count);
    c.ret_const(0);

    let amxx = c.build(&d, &[]);
    let (mut server, _pending) = spawn_test_server(
        vec![("tasks.amxx".to_string(), amxx)],
        ServerMap::stub("de_dust2"),
    );
    let mut net = NullTransport;

    // Server starts at sv_time = 1.0 (run_physics runs BEFORE sv_time += frametime).
    server.frame(&mut net, 0.25); // physics @ 1.00 -> sv_time = 1.25
    server.frame(&mut net, 0.50); // physics @ 1.25 (rep#1, inf#1 due at 1.2 -> 1.45) -> 1.50
    server.frame(&mut net, 0.75); // physics @ 1.50 (rep#2, inf#2 due at 1.45 -> 1.65) -> 1.75
    server.frame(&mut net, 1.00); // physics @ 1.75 (once due at 1.6 -> removes inf before inf#3) -> 2.00
    server.frame(&mut net, 1.25); // physics @ 2.00 (no more tasks should fire) -> 2.25

    let vm = &server.game().game.runtime().plugins[0].vm;
    assert_eq!(vm.read_cell(once_count as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(saved_param0 as i32).unwrap(), 77);
    assert_eq!(vm.read_cell(rep_count as i32).unwrap(), 2);
    assert_eq!(vm.read_cell(inf_count as i32).unwrap(), 2);
}

#[test]
fn clcmd_interception_and_client_lifecycle_forwards() {
    let mut d = DataBuilder::new();
    let s_cmd_block = d.add_str("say_secret");
    let s_cb_block = d.add_str("on_secret");
    let s_cmd_pass = d.add_str("ping_cmd");
    let s_cb_pass = d.add_str("on_ping");
    let s_match = d.add_str("block_me");
    let arg_buf = d.alloc_cells(32);
    let args_buf = d.alloc_cells(64);
    let connect_count = d.add_cell(0);
    let putin_count = d.add_cell(0);
    let disc_count = d.add_cell(0);
    let dmg_sum = d.add_cell(0);
    let death_count = d.add_cell(0);
    let last_argc = d.add_cell(0);

    let mut c = CodeBuilder::new(&[
        "register_clcmd",
        "read_argc",
        "read_argv",
        "read_args",
        "equal",
    ]);

    c.begin_public("plugin_init");
    c.call_native(
        "register_clcmd",
        &[Arg::Const(s_cmd_block as i32), Arg::Const(s_cb_block as i32)],
    );
    c.call_native(
        "register_clcmd",
        &[Arg::Const(s_cmd_pass as i32), Arg::Const(s_cb_pass as i32)],
    );
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("on_secret");
    c.call_native("read_argc", &[]);
    c.store_pri(last_argc);
    c.call_native(
        "read_argv",
        &[Arg::Const(1), Arg::Const(arg_buf as i32), Arg::Const(31)],
    );
    c.call_native(
        "read_args",
        &[Arg::Const(args_buf as i32), Arg::Const(63)],
    );
    c.ret_const(PLUGIN_HANDLED); // blocks forwarding to inner GameDll!

    c.begin_public("on_ping");
    c.ret_const(PLUGIN_CONTINUE); // allows forwarding to inner GameDll!

    c.begin_public("client_connect");
    c.inc_mem(connect_count);
    // Reject slot 2 (player_id == 3)
    c.cells.push(OP_LOAD_S_PRI);
    c.cells.push(12);
    c.cells.push(OP_EQ_C_PRI);
    c.cells.push(3);
    c.cells.push(OP_RETN);

    c.begin_public("client_putinserver");
    c.inc_mem(putin_count);
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("client_disconnect");
    c.inc_mem(disc_count);
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("client_damage");
    c.load_pri(dmg_sum);
    c.cells.push(OP_LOAD_S_ALT);
    c.cells.push(20); // arg 2 (0-based): damage
    c.cells.push(OP_ADD);
    c.store_pri(dmg_sum);
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("client_death");
    c.inc_mem(death_count);
    c.ret_const(PLUGIN_CONTINUE);

    let _ = s_match;
    let amxx = c.build(&d, &[]);
    let (mut server, pending) = spawn_test_server(
        vec![("clcmd.amxx".to_string(), amxx)],
        ServerMap::stub("de_dust2"),
    );

    run_with_eng(&mut server, &pending, move |g, eng| {
        assert!(g.client_connect(eng, 0).is_ok());
        assert!(g.client_connect(eng, 2).is_err()); // player_id 3 rejected by plugin!
        g.client_put_in_server(eng, 0);

        g.client_command(
            eng,
            0,
            &[
                "say_secret".to_string(),
                "alpha".to_string(),
                "beta".to_string(),
            ],
        );
        g.client_command(eng, 0, &["ping_cmd".to_string(), "1".to_string()]);
        g.notify_client_damage(eng, Some(0), 0, 35, 28, 1, false);
        g.notify_client_death(eng, Some(0), 0, 28, 1, false);
        g.client_disconnect(eng, 0);
    });

    let inner = server.game().game.inner();
    // `say_secret` was blocked by PLUGIN_HANDLED; `ping_cmd` was forwarded.
    assert_eq!(inner.forwarded_clcmds.len(), 1);
    assert_eq!(inner.forwarded_clcmds[0].1, vec!["ping_cmd", "1"]);

    let vm = &server.game().game.runtime().plugins[0].vm;
    assert_eq!(vm.read_cell(connect_count as i32).unwrap(), 2);
    assert_eq!(vm.read_cell(putin_count as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(disc_count as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(dmg_sum as i32).unwrap(), 35);
    assert_eq!(vm.read_cell(death_count as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(last_argc as i32).unwrap(), 3);
    assert_eq!(vm.get_amx_string(arg_buf as i32).unwrap(), "alpha");
    assert_eq!(vm.get_amx_string(args_buf as i32).unwrap(), "alpha beta");
}

#[test]
fn cstrike_fun_engine_fakemeta_and_player_query_natives() {
    let mut d = DataBuilder::new();
    let s_ak = d.add_str("weapon_ak47");
    let s_classname = d.add_str("custom_target");
    let s_flags_a = d.add_str("a");
    let s_team_ct = d.add_str("CT");
    let s_fmt = d.add_str("hp=%d str=%s");
    let s_hello = d.add_str("ok");
    let hp_cell = d.add_cell(150);
    let vec_in = d.add_cells(&[
        10.0f32.to_bits() as i32,
        20.0f32.to_bits() as i32,
        30.0f32.to_bits() as i32,
    ]);
    let vec_out = d.alloc_cells(3);
    let players_out = d.alloc_cells(32);
    let num_out = d.add_cell(0);
    let armor_type_out = d.add_cell(0);
    let name_out = d.alloc_cells(32);
    let auth_out = d.alloc_cells(32);
    let ip_out = d.alloc_cells(32);
    let fmt_out = d.alloc_cells(64);
    let origin_out = d.alloc_cells(3);
    let sz_out = d.alloc_cells(32);

    let got_money = d.add_cell(0);
    let got_team = d.add_cell(0);
    let got_armor = d.add_cell(0);
    let got_deaths = d.add_cell(0);
    let got_alive = d.add_cell(0);
    let got_bot = d.add_cell(0);
    let got_maxp = d.add_cell(0);
    let got_gt = d.add_cell(0);
    let got_pev_hp = d.add_cell(0);
    let got_ev_int = d.add_cell(0);
    let got_ev_fl = d.add_cell(0);

    let mut c = CodeBuilder::new(&[
        "cs_get_user_money",
        "cs_set_user_money",
        "cs_get_user_team",
        "cs_set_user_team",
        "cs_get_user_armor",
        "cs_set_user_armor",
        "cs_get_user_deaths",
        "cs_set_user_deaths",
        "set_user_health",
        "set_user_gravity",
        "set_user_maxspeed",
        "give_item",
        "strip_user_weapons",
        "set_user_godmode",
        "pev",
        "set_pev",
        "entity_get_float",
        "entity_get_int",
        "entity_get_vector",
        "entity_get_string",
        "entity_set_float",
        "entity_set_int",
        "entity_set_vector",
        "entity_set_string",
        "is_user_alive",
        "is_user_connected",
        "is_user_bot",
        "get_players",
        "get_maxplayers",
        "get_gametime",
        "get_user_name",
        "get_user_authid",
        "get_user_ip",
        "get_user_origin",
        "client_print",
        "console_print",
        "format",
    ]);

    c.begin_public("run_checks");
    // Player 1 (slot 0) checks:
    c.call_native("cs_get_user_money", &[Arg::Const(1)]);
    c.store_pri(got_money);
    c.call_native("cs_set_user_money", &[Arg::Const(1), Arg::Const(16000), Arg::Const(1)]);
    c.call_native("cs_get_user_team", &[Arg::Const(1)]);
    c.store_pri(got_team);
    c.call_native("cs_set_user_team", &[Arg::Const(1), Arg::Const(1)]);
    c.call_native("cs_set_user_armor", &[Arg::Const(1), Arg::Const(100), Arg::Const(2)]);
    c.call_native(
        "cs_get_user_armor",
        &[Arg::Const(1), Arg::Const(armor_type_out as i32)],
    );
    c.store_pri(got_armor);
    c.call_native("cs_set_user_deaths", &[Arg::Const(1), Arg::Const(7)]);
    c.call_native("cs_get_user_deaths", &[Arg::Const(1)]);
    c.store_pri(got_deaths);

    // Fun natives on player 1:
    c.call_native("set_user_health", &[Arg::Const(1), Arg::Const(150)]);
    c.call_native(
        "set_user_gravity",
        &[Arg::Const(1), Arg::Const(0.5f32.to_bits() as i32)],
    );
    c.call_native(
        "set_user_maxspeed",
        &[Arg::Const(1), Arg::Const(320.0f32.to_bits() as i32)],
    );
    c.call_native("strip_user_weapons", &[Arg::Const(1)]);
    c.call_native("give_item", &[Arg::Const(1), Arg::Const(s_ak as i32)]);
    c.call_native("set_user_godmode", &[Arg::Const(1), Arg::Const(1)]);

    // Fakemeta pev / set_pev on player 1 & entity 9 (arg 0 is entity index!):
    c.call_native(
        "pev",
        &[Arg::Const(1), Arg::Const(pev_const::PEV_HEALTH)],
    );
    c.store_pri(got_pev_hp);
    c.call_native(
        "set_pev",
        &[
            Arg::Param(0),
            Arg::Const(pev_const::PEV_CLASSNAME),
            Arg::Const(s_classname as i32),
        ],
    );
    c.call_native(
        "set_pev",
        &[
            Arg::Param(0),
            Arg::Const(pev_const::PEV_ORIGIN),
            Arg::Const(vec_in as i32),
        ],
    );

    // Engine entity_set_* / entity_get_* on entity arg 0:
    c.call_native(
        "entity_set_int",
        &[Arg::Param(0), Arg::Const(ev_const::EV_INT_IUSER2), Arg::Const(555)],
    );
    c.call_native(
        "entity_get_int",
        &[Arg::Param(0), Arg::Const(ev_const::EV_INT_IUSER2)],
    );
    c.store_pri(got_ev_int);
    c.call_native(
        "entity_set_float",
        &[
            Arg::Param(0),
            Arg::Const(ev_const::EV_FL_SCALE),
            Arg::Const(2.25f32.to_bits() as i32),
        ],
    );
    c.call_native(
        "entity_get_float",
        &[Arg::Param(0), Arg::Const(ev_const::EV_FL_SCALE)],
    );
    c.store_pri(got_ev_fl);
    c.call_native(
        "entity_get_vector",
        &[
            Arg::Param(0),
            Arg::Const(ev_const::EV_VEC_ORIGIN),
            Arg::Const(vec_out as i32),
        ],
    );
    c.call_native(
        "entity_get_string",
        &[
            Arg::Param(0),
            Arg::Const(ev_const::EV_SZ_CLASSNAME),
            Arg::Const(sz_out as i32),
            Arg::Const(31),
        ],
    );

    // Player queries & printing & formatting:
    c.call_native("is_user_alive", &[Arg::Const(1)]);
    c.store_pri(got_alive);
    c.call_native("is_user_bot", &[Arg::Const(1)]);
    c.store_pri(got_bot);
    c.call_native("get_maxplayers", &[]);
    c.store_pri(got_maxp);
    c.call_native("get_gametime", &[]);
    c.store_pri(got_gt);
    c.call_native(
        "get_players",
        &[
            Arg::Const(players_out as i32),
            Arg::Const(num_out as i32),
            Arg::Const(s_flags_a as i32),
        ],
    );
    c.call_native(
        "get_user_name",
        &[Arg::Const(1), Arg::Const(name_out as i32), Arg::Const(31)],
    );
    c.call_native(
        "get_user_authid",
        &[Arg::Const(1), Arg::Const(auth_out as i32), Arg::Const(31)],
    );
    c.call_native(
        "get_user_ip",
        &[
            Arg::Const(1),
            Arg::Const(ip_out as i32),
            Arg::Const(31),
            Arg::Const(1),
        ],
    );
    c.call_native(
        "get_user_origin",
        &[Arg::Const(1), Arg::Const(origin_out as i32), Arg::Const(1)],
    );
    c.call_native(
        "format",
        &[
            Arg::Const(fmt_out as i32),
            Arg::Const(63),
            Arg::Const(s_fmt as i32),
            Arg::Const(hp_cell as i32),
            Arg::Const(s_hello as i32),
        ],
    );
    c.call_native(
        "client_print",
        &[
            Arg::Const(1),
            Arg::Const(PRINT_CENTER),
            Arg::Const(fmt_out as i32),
        ],
    );
    c.call_native(
        "client_print",
        &[
            Arg::Const(0),
            Arg::Const(PRINT_CHAT),
            Arg::Const(fmt_out as i32),
        ],
    );
    c.call_native(
        "console_print",
        &[Arg::Const(0), Arg::Const(fmt_out as i32)],
    );
    c.ret_const(1);

    let _ = (s_team_ct, PRINT_CONSOLE);
    let amxx = c.build(&d, &[]);
    let (mut server, pending) = spawn_test_server(
        vec![("modules.amxx".to_string(), amxx)],
        ServerMap::stub("de_dust2"),
    );

    let mut net = NullTransport;
    server.frame(&mut net, 0.1); // adopts fake bot into server.clients!

    let extra_idx = server.game().game.inner().extra_entity.unwrap().index;
    run_with_eng(&mut server, &pending, move |g, eng| {
        let (rt, inner) = g.parts_mut();
        let res = rt
            .call_public(0, eng, inner, "run_checks", &[i32::from(extra_idx)])
            .unwrap();
        assert_eq!(res, Some(1));
    });

    let inner = server.game().game.inner();
    let bot_slot = inner.fake_bot_slot.unwrap();
    assert_eq!(inner.money[usize::from(bot_slot)], 16000);
    assert_eq!(inner.team[usize::from(bot_slot)], 1);
    assert_eq!(inner.armor[usize::from(bot_slot)], 100);
    assert_eq!(inner.armor_type[usize::from(bot_slot)], 2);
    assert_eq!(inner.deaths[usize::from(bot_slot)], 7);
    assert_eq!(inner.stripped_slots, vec![bot_slot]);
    assert_eq!(inner.given_items, vec![(bot_slot, "weapon_ak47".to_string())]);

    let bot_edict = server.edicts().by_index(u16::from(bot_slot) + 1).unwrap().1;
    assert_eq!(bot_edict.vars.health, 150.0);
    assert_eq!(bot_edict.vars.gravity, 0.5);
    assert_eq!(bot_edict.vars.max_speed, 320.0);
    assert_ne!(bot_edict.vars.flags & cs_engine::consts::FL_GODMODE, 0);

    let vm = &server.game().game.runtime().plugins[0].vm;
    assert_eq!(vm.read_cell(got_money as i32).unwrap(), 800);
    assert_eq!(vm.read_cell(got_team as i32).unwrap(), 2);
    assert_eq!(vm.read_cell(got_armor as i32).unwrap(), 100);
    assert_eq!(vm.read_cell(armor_type_out as i32).unwrap(), 2);
    assert_eq!(vm.read_cell(got_deaths as i32).unwrap(), 7);
    assert_eq!(
        f32::from_bits(vm.read_cell(got_pev_hp as i32).unwrap() as u32),
        150.0
    );
    assert_eq!(vm.read_cell(got_ev_int as i32).unwrap(), 555);
    assert_eq!(
        f32::from_bits(vm.read_cell(got_ev_fl as i32).unwrap() as u32),
        2.25
    );
    assert_eq!(
        f32::from_bits(vm.read_cell(vec_out as i32).unwrap() as u32),
        10.0
    );
    assert_eq!(
        vm.get_amx_string(sz_out as i32).unwrap(),
        "custom_target"
    );
    assert_eq!(vm.read_cell(got_alive as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(got_bot as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(got_maxp as i32).unwrap(), 8);
    assert!(f32::from_bits(vm.read_cell(got_gt as i32).unwrap() as u32) >= 1.0);
    assert_eq!(vm.read_cell(num_out as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(players_out as i32).unwrap(), 1);
    assert_eq!(vm.get_amx_string(name_out as i32).unwrap(), "TestBot");
    assert_eq!(vm.get_amx_string(auth_out as i32).unwrap(), "BOT");
    assert_eq!(vm.get_amx_string(ip_out as i32).unwrap(), "127.0.0.1");
    assert_eq!(vm.read_cell(origin_out as i32).unwrap(), 100);
    assert_eq!(vm.read_cell((origin_out + 8) as i32).unwrap(), 317); // z + view_ofs.z
    assert_eq!(vm.get_amx_string(fmt_out as i32).unwrap(), "hp=150 str=ok");
}

#[test]
fn hamsandwich_spawn_supercede_takedamage_and_killed() {
    let mut d = DataBuilder::new();
    let s_blocked_ent = d.add_str("func_breakable");
    let s_kept_ent = d.add_str("info_player_start");
    let s_player = d.add_str("player");
    let s_cb_pre_block = d.add_str("ham_pre_block");
    let s_cb_post_spawn = d.add_str("ham_post_spawn");
    let s_cb_dmg = d.add_str("ham_take_dmg");
    let s_cb_killed = d.add_str("ham_killed");
    let post_spawn_cnt = d.add_cell(0);
    let dmg_cnt = d.add_cell(0);
    let killed_cnt = d.add_cell(0);

    let mut c = CodeBuilder::new(&["RegisterHam"]);
    c.begin_public("plugin_init");
    c.call_native(
        "RegisterHam",
        &[
            Arg::Const(HamFunction::Spawn as i32),
            Arg::Const(s_blocked_ent as i32),
            Arg::Const(s_cb_pre_block as i32),
            Arg::Const(0),
        ],
    );
    c.call_native(
        "RegisterHam",
        &[
            Arg::Const(HamFunction::Spawn as i32),
            Arg::Const(s_kept_ent as i32),
            Arg::Const(s_cb_post_spawn as i32),
            Arg::Const(1),
        ],
    );
    c.call_native(
        "RegisterHam",
        &[
            Arg::Const(HamFunction::TakeDamage as i32),
            Arg::Const(s_player as i32),
            Arg::Const(s_cb_dmg as i32),
            Arg::Const(0),
        ],
    );
    c.call_native(
        "RegisterHam",
        &[
            Arg::Const(HamFunction::Killed as i32),
            Arg::Const(s_player as i32),
            Arg::Const(s_cb_killed as i32),
            Arg::Const(1),
        ],
    );
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("ham_pre_block");
    c.ret_const(HAM_SUPERCEDE);

    c.begin_public("ham_post_spawn");
    c.inc_mem(post_spawn_cnt);
    c.ret_const(HAM_IGNORED);

    c.begin_public("ham_take_dmg");
    c.inc_mem(dmg_cnt);
    c.ret_const(HAM_SUPERCEDE);

    c.begin_public("ham_killed");
    c.inc_mem(killed_cnt);
    c.ret_const(HAM_IGNORED);

    let amxx = c.build(&d, &[]);
    let (mut server, pending) =
        spawn_test_server(vec![("ham.amxx".to_string(), amxx)], ServerMap::stub("de_dust2"));

    run_with_eng(&mut server, &pending, move |g, eng| {
        let e1 = eng.create_entity("func_breakable").unwrap();
        assert!(!g.spawn_entity(eng, e1, "func_breakable", &[]));
        let e2 = eng.create_entity("info_player_start").unwrap();
        assert!(g.spawn_entity(eng, e2, "info_player_start", &[]));

        let dmg_res = g.notify_ham_take_damage(eng, 1, "player", 2, 2, 50.0, 2, false);
        assert_eq!(dmg_res, HAM_SUPERCEDE);
        let kill_res = g.notify_ham_killed(eng, 1, "player", 2, 0, true);
        assert_eq!(kill_res, HAM_IGNORED);
    });

    let spawned = &server.game().game.inner().spawned_entities;
    assert!(!spawned.iter().any(|c| c == "func_breakable"));
    assert!(spawned.iter().any(|c| c == "info_player_start"));

    let vm = &server.game().game.runtime().plugins[0].vm;
    assert_eq!(vm.read_cell(post_spawn_cnt as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(dmg_cnt as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(killed_cnt as i32).unwrap(), 1);
}

#[test]
fn string_float_and_event_message_natives() {
    let mut d = DataBuilder::new();
    let s_a = d.add_str("Counter");
    let s_b = d.add_str("COUNTER");
    let s_sub = d.add_str("unt");
    let s_num = d.add_str("-42");
    let s_ev = d.add_str("RoundTime");
    let s_ev_cb = d.add_str("on_roundtime");
    let s_ev_flags = d.add_str("bc");
    let s_msg_cb = d.add_str("on_msg");
    let s_msg_name = d.add_str("Health");
    let copy_buf = d.alloc_cells(16);
    let num_buf = d.alloc_cells(16);

    let out_eq = d.add_cell(0);
    let out_eqi = d.add_cell(0);
    let out_cont = d.add_cell(0);
    let out_len = d.add_cell(0);
    let out_s2n = d.add_cell(0);
    let out_flt = d.add_cell(0);
    let out_cmp = d.add_cell(0);
    let out_msgid = d.add_cell(0);
    let ev_fired = d.add_cell(0);

    let mut c = CodeBuilder::new(&[
        "copy",
        "equal",
        "equali",
        "contain",
        "containi",
        "strlen",
        "str_to_num",
        "num_to_str",
        "float",
        "floatadd",
        "floatsub",
        "floatmul",
        "floatdiv",
        "floatround",
        "floatcmp",
        "register_event",
        "get_user_msgid",
        "register_message",
    ]);

    c.begin_public("plugin_init");
    c.call_native(
        "register_event",
        &[
            Arg::Const(s_ev as i32),
            Arg::Const(s_ev_cb as i32),
            Arg::Const(s_ev_flags as i32),
        ],
    );
    c.call_native("get_user_msgid", &[Arg::Const(s_msg_name as i32)]);
    c.store_pri(out_msgid);
    c.load_pri(out_msgid);
    c.call_native("register_message", &[Arg::Pri, Arg::Const(s_msg_cb as i32)]);
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("run_math");
    c.call_native(
        "copy",
        &[Arg::Const(copy_buf as i32), Arg::Const(15), Arg::Const(s_a as i32)],
    );
    c.call_native("equal", &[Arg::Const(s_a as i32), Arg::Const(s_b as i32)]);
    c.store_pri(out_eq);
    c.call_native("equali", &[Arg::Const(s_a as i32), Arg::Const(s_b as i32)]);
    c.store_pri(out_eqi);
    c.call_native("containi", &[Arg::Const(s_b as i32), Arg::Const(s_sub as i32)]);
    c.store_pri(out_cont);
    c.call_native("strlen", &[Arg::Const(s_a as i32)]);
    c.store_pri(out_len);
    c.call_native("str_to_num", &[Arg::Const(s_num as i32)]);
    c.store_pri(out_s2n);
    c.call_native(
        "num_to_str",
        &[Arg::Const(-42), Arg::Const(num_buf as i32), Arg::Const(15)],
    );

    // ((float(3) + 2.0) * 4.0 - 2.0) / 3.0 = 18/3 = 6.0 -> floatround = 6
    c.call_native("float", &[Arg::Const(3)]);
    c.call_native(
        "floatadd",
        &[Arg::Pri, Arg::Const(2.0f32.to_bits() as i32)],
    );
    c.call_native(
        "floatmul",
        &[Arg::Pri, Arg::Const(4.0f32.to_bits() as i32)],
    );
    c.call_native(
        "floatsub",
        &[Arg::Pri, Arg::Const(2.0f32.to_bits() as i32)],
    );
    c.call_native(
        "floatdiv",
        &[Arg::Pri, Arg::Const(3.0f32.to_bits() as i32)],
    );
    c.call_native("floatround", &[Arg::Pri, Arg::Const(0)]);
    c.store_pri(out_flt);
    c.call_native(
        "floatcmp",
        &[
            Arg::Const(5.0f32.to_bits() as i32),
            Arg::Const(2.0f32.to_bits() as i32),
        ],
    );
    c.store_pri(out_cmp);
    c.ret_const(1);

    c.begin_public("on_roundtime");
    c.inc_mem(ev_fired);
    c.ret_const(PLUGIN_CONTINUE);

    c.begin_public("on_msg");
    c.ret_const(PLUGIN_HANDLED);

    let amxx = c.build(&d, &[]);
    let (mut server, pending) = spawn_test_server(
        vec![("strflt.amxx".to_string(), amxx)],
        ServerMap::stub("de_dust2"),
    );

    run_with_eng(&mut server, &pending, move |g, eng| {
        let mut rt = std::mem::replace(g.runtime_mut(), AmxxRuntime::new());
        let hooks = &mut NoCsHooks;
        rt.call_public(0, eng, hooks, "run_math", &[]).unwrap();
        rt.dispatch_event(eng, hooks, "RoundTime", Some(0));
        let msgid = rt.plugins[0].vm.read_cell(out_msgid as i32).unwrap();
        let mres = rt.dispatch_message(eng, hooks, msgid, 1, 1);
        assert_eq!(mres, PLUGIN_HANDLED);
        *g.runtime_mut() = rt;
    });

    let vm = &server.game().game.runtime().plugins[0].vm;
    assert_eq!(vm.get_amx_string(copy_buf as i32).unwrap(), "Counter");
    assert_eq!(vm.get_amx_string(num_buf as i32).unwrap(), "-42");
    assert_eq!(vm.read_cell(out_eq as i32).unwrap(), 0);
    assert_eq!(vm.read_cell(out_eqi as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(out_cont as i32).unwrap(), 2);
    assert_eq!(vm.read_cell(out_len as i32).unwrap(), 7);
    assert_eq!(vm.read_cell(out_s2n as i32).unwrap(), -42);
    assert_eq!(vm.read_cell(out_flt as i32).unwrap(), 6);
    assert_eq!(vm.read_cell(out_cmp as i32).unwrap(), 1);
    assert_eq!(vm.read_cell(ev_fired as i32).unwrap(), 1);
}
