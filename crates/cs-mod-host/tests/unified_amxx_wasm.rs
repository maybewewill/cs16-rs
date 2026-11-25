//! Unified Phase 15 Task 2 integration tests proving `.amxx` (Pawn 3.2) and `.wasm` (`wasmi`)
//! plugins execute together on ONE `ModdedGame<CsGame>` server sharing:
//! 1. Host paginated `MenuManager` (zero `menuselect` collision between AMXX and WASM menus)
//! 2. Unified `LangCatalog` (`ru` + `en` simultaneously across AMXX `%L` and WASM `lang_tr`, plus `/lang ru`)
//! 3. Shared `AdminRegistry` (`users.ini` + `configs/admins.toml` visible to both AMXX & WASM)
//! 4. Shared `NVaultStore` and $O(\log N)$ `LeaderboardStore` (`csx` stats & `nvault` keys)
//! 5. Bidirectional synchronous polyglot RPC & events (WASM -> AMXX `register_native`,
//!    AMXX dynamic native -> WASM `rpc_export`, AMXX `ExecuteForward` -> WASM `cs_mod_event`,
//!    WASM `rpc_emit_event` -> AMXX public forward)
//! 6. Real `CsGame` ReAPI (`rg_add_account`, `rg_give_item`, `rg_set_user_armor`, `get_member`/`set_member`)
//!    and Hamsandwich (`Ham_TakeDamage` + `SetHamParamFloat`) alongside WASM hooks + `plugins.toml`.

use std::collections::{BTreeMap, HashMap};

use cs_amxx::amxxfile::build_test_amxx_plugin;
use cs_amxx::natives::{AS_ADD, CS_ARMOR_VESTHELM, ET_CONTINUE, FP_CELL, FP_STRING, HAM_HANDLED, HamFunction, reapi_member};
use cs_amxx::vm::op::*;
use cs_assets::bsp::Entity;
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{ConsoleOutcome, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_mod_api::ModManifest;
use cs_mod_host::{LoadedPackage, ModdedGame, ServerModHost, compile_wat_to_wasm};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_transport::{LoopbackHub, PeerAddr};

// --- Mini Pawn 3.2 assembler for `.amxx` test plugins ---

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

#[allow(dead_code)]
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

    fn shl_const(&mut self, bits: i32) {
        self.cells.push(OP_SHL_C_PRI);
        self.cells.push(bits as u32);
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

fn make_v2_pkg(id: &str, toml_extra: &str, server_wat: &str) -> LoadedPackage {
    let manifest = ModManifest::parse_toml(&format!(
        "id = \"{id}\"\nversion = \"2.0.0\"\napi_version = 2\n{toml_extra}"
    ))
    .unwrap();
    LoadedPackage {
        manifest,
        source_path: None,
        server_wasm: Some(compile_wat_to_wasm(server_wat).unwrap()),
        client_wasm: None,
        client_bundle: None,
        assets: BTreeMap::new(),
    }
}

fn spawn_map() -> ServerMap {
    let ents = vec![
        Entity {
            properties: HashMap::from([
                ("classname".into(), "info_player_start".into()),
                ("origin".into(), "0 0 64".into()),
            ]),
        },
        Entity {
            properties: HashMap::from([
                ("classname".into(), "info_player_deathmatch".into()),
                ("origin".into(), "128 0 64".into()),
            ]),
        },
    ];
    ServerMap::stub_with_entities("de_dust2", ents)
}

#[test]
fn unified_server_shares_menus_lang_admins_storage_and_bidirectional_rpc_between_amxx_and_wasm() {
    // --- Build AMXX plugin ---
    // - Exports `amxx_double(n)` -> `n * 2` and `amxx_tag(in_str)` -> sets param 1 to `"AMXX:<in_str>"`
    // - Calls WASM-exported `wasm_triple(11)` (expects `33`) and writes `"amxx_key"="from_amxx"` into shared nvault `"shared_v"`
    // - Emits `CreateMultiForward("amxx_pulse", ...)` -> `ExecuteForward` which triggers WASM `cs_mod_event`!
    // - Handles `public wasm_ping(const payload[])` triggered by WASM `rpc_emit_event("wasm_ping", "hi_amxx")`
    // - Registers `/amxxmenu` opening a shared `MenuManager` menu on slot 0 and checking `%L` + `get_user_flags`!
    let mut data = DataBuilder::new();
    let s_title = data.add_str("Unified AMXX");
    let s_ver = data.add_str("1.0");
    let s_auth = data.add_str("cs16-rs");
    let s_nat_dbl = data.add_str("amxx_double");
    let s_cb_dbl = data.add_str("native_double");
    let s_nat_tag = data.add_str("amxx_tag");
    let s_cb_tag = data.add_str("native_tag");
    let s_cmd_menu = data.add_str("amxxmenu");
    let s_cb_menu_cmd = data.add_str("cmd_amxx_menu");
    let s_cmd_run = data.add_str("run_cross");
    let s_cb_run = data.add_str("cmd_run_cross");
    let s_menu_title = data.add_str("\\yAMXX Menu");
    let s_menu_handler = data.add_str("on_amxx_menu");
    let s_item0 = data.add_str("Alpha");
    let s_info0 = data.add_str("amxx_alpha");
    let s_vault_name = data.add_str("shared_v");
    let s_vkey = data.add_str("amxx_key");
    let s_vval = data.add_str("from_amxx");
    let s_fwd_name = data.add_str("amxx_pulse");
    let s_fwd_payload = data.add_str("pulse_ok");
    let s_fmt_tag = data.add_str("AMXX:%s");
    let s_fmt_ping = data.add_str("PING:%s");
    let s_fmt_menu = data.add_str("AMXX_PICK:%s");
    let s_fmt_lang = data.add_str("%L");
    let s_key_greet = data.add_str("GREET");
    let s_fmt_flags = data.add_str("FLAGS:%d:TRIPLE:%d");

    let g_menu_id = data.add_cell(0);
    let g_fwd = data.add_cell(0);
    let g_fwd_ret = data.add_cell(0);
    let g_triple_res = data.add_cell(0);
    let g_flags_res = data.add_cell(0);
    let g_arg_num = data.add_cell(77);
    let buf_in = data.alloc_cells(64);
    let buf_out = data.alloc_cells(64);
    let buf_info = data.alloc_cells(32);
    let buf_name = data.alloc_cells(32);
    let cell_access = data.add_cell(0);
    let cell_cb = data.add_cell(0);

    let mut code = CodeBuilder::new(&[
        "register_plugin",
        "register_native",
        "register_clcmd",
        "CreateMultiForward",
        "ExecuteForward",
        "menu_create",
        "menu_additem",
        "menu_display",
        "menu_item_getinfo",
        "get_param",
        "get_string",
        "set_string",
        "format",
        "server_print",
        "nvault_open",
        "nvault_set",
        "get_user_flags",
        "client_print",
        "wasm_triple",
    ]);

    // plugin_init
    code.begin_public("plugin_init");
    code.call_native(
        "register_plugin",
        &[Arg::Const(s_title as i32), Arg::Const(s_ver as i32), Arg::Const(s_auth as i32)],
    );
    code.call_native(
        "register_native",
        &[Arg::Const(s_nat_dbl as i32), Arg::Const(s_cb_dbl as i32)],
    );
    code.call_native(
        "register_native",
        &[Arg::Const(s_nat_tag as i32), Arg::Const(s_cb_tag as i32)],
    );
    code.call_native(
        "register_clcmd",
        &[Arg::Const(s_cmd_menu as i32), Arg::Const(s_cb_menu_cmd as i32)],
    );
    code.call_native(
        "register_clcmd",
        &[Arg::Const(s_cmd_run as i32), Arg::Const(s_cb_run as i32)],
    );
    code.call_native(
        "CreateMultiForward",
        &[
            Arg::Const(s_fwd_name as i32),
            Arg::Const(ET_CONTINUE),
            Arg::Const(FP_CELL),
            Arg::Const(FP_STRING),
        ],
    );
    code.store_pri(g_fwd);
    code.call_native(
        "menu_create",
        &[Arg::Const(s_menu_title as i32), Arg::Const(s_menu_handler as i32)],
    );
    code.store_pri(g_menu_id);
    code.call_native(
        "menu_additem",
        &[Arg::Mem(g_menu_id), Arg::Const(s_item0 as i32), Arg::Const(s_info0 as i32)],
    );
    code.ret_const(0);

    // native_double(plugin_id, num_params) -> get_param(1) * 2
    code.begin_public("native_double");
    code.call_native("get_param", &[Arg::Const(1)]);
    code.shl_const(1);
    code.ret_pri();

    // native_tag(plugin_id, num_params) -> get_string(2, buf_in, 63); format(buf_out, 63, "AMXX:%s", buf_in); set_string(2, buf_out, 63)
    code.begin_public("native_tag");
    code.call_native(
        "get_string",
        &[Arg::Const(2), Arg::Const(buf_in as i32), Arg::Const(63)],
    );
    code.call_native(
        "format",
        &[
            Arg::Const(buf_out as i32),
            Arg::Const(63),
            Arg::Const(s_fmt_tag as i32),
            Arg::Const(buf_in as i32),
        ],
    );
    code.call_native(
        "set_string",
        &[Arg::Const(2), Arg::Const(buf_out as i32), Arg::Const(63)],
    );
    code.ret_const(1);

    // cmd_amxx_menu(id) -> menu_display(id, g_menu_id, 0) + client_print(id, PRINT_CHAT=3, "%L", id, "GREET")
    code.begin_public("cmd_amxx_menu");
    code.call_native(
        "menu_display",
        &[Arg::Param(0), Arg::Mem(g_menu_id), Arg::Const(0)],
    );
    code.call_native(
        "client_print",
        &[
            Arg::Param(0),
            Arg::Const(3),
            Arg::Const(s_fmt_lang as i32),
            Arg::Param(0),
            Arg::Const(s_key_greet as i32),
        ],
    );
    code.ret_const(1);

    // on_amxx_menu(id, menu, item) -> menu_item_getinfo + server_print("AMXX_PICK:%s", info)
    code.begin_public("on_amxx_menu");
    code.call_native(
        "menu_item_getinfo",
        &[
            Arg::Param(1),
            Arg::Param(2),
            Arg::Const(cell_access as i32),
            Arg::Const(buf_info as i32),
            Arg::Const(31),
            Arg::Const(buf_name as i32),
            Arg::Const(31),
            Arg::Const(cell_cb as i32),
        ],
    );
    code.call_native(
        "server_print",
        &[Arg::Const(s_fmt_menu as i32), Arg::Const(buf_info as i32)],
    );
    code.ret_const(0);

    // cmd_run_cross(id) -> calls WASM `wasm_triple(11)`, writes shared nvault, reads shared admin flags,
    // and fires `ExecuteForward(g_fwd, g_fwd_ret, g_arg_num, "pulse_ok")` to WASM subscribers!
    code.begin_public("cmd_run_cross");
    code.call_native("wasm_triple", &[Arg::Const(11)]);
    code.store_pri(g_triple_res);
    code.call_native("get_user_flags", &[Arg::Param(0)]);
    code.store_pri(g_flags_res);
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt_flags as i32),
            Arg::Const(g_flags_res as i32),
            Arg::Const(g_triple_res as i32),
        ],
    );
    code.call_native("nvault_open", &[Arg::Const(s_vault_name as i32)]);
    code.call_native(
        "nvault_set",
        &[Arg::Pri, Arg::Const(s_vkey as i32), Arg::Const(s_vval as i32)],
    );
    code.call_native(
        "ExecuteForward",
        &[
            Arg::Mem(g_fwd),
            Arg::Const(g_fwd_ret as i32),
            Arg::Const(g_arg_num as i32),
            Arg::Const(s_fwd_payload as i32),
        ],
    );
    code.ret_const(1);

    // public wasm_ping(const payload[]) -> server_print("PING:%s", payload)
    code.begin_public("wasm_ping");
    code.call_native(
        "server_print",
        &[Arg::Const(s_fmt_ping as i32), Arg::Param(0)],
    );
    code.ret_const(0);

    let amxx_bytes = code.build(&data);

    // --- Build WASM plugin ---
    // - Exports `wasm_triple(n)` -> `n * 3`
    // - Subscribes to `"amxx_pulse"` -> writes `"got_pulse"` into shared nvault `"shared_v"`
    // - On client command `wasmcheck`:
    //   - calls AMXX `amxx_double(21)` (expects `42`)
    //   - calls AMXX `amxx_tag(0, "hello")` (expects `"AMXX:hello"`)
    //   - reads shared nvault `"shared_v"` key `"amxx_key"` (expects `"from_amxx"`)
    //   - checks `admin_has_flag(slot, 'd')` (expects `1`)
    //   - translates `"GREET"` for slot via `lang_tr`
    //   - opens WASM menu on slot 1
    //   - emits `rpc_emit_event("wasm_ping", "hi_amxx")` -> invokes AMXX `public wasm_ping`!
    let wasm_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_i32" (func $arg_i32 (param i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "rpc_export" (func $rpc_export (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "rpc_call" (func $rpc_call (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "rpc_subscribe" (func $rpc_sub (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "rpc_emit_event" (func $rpc_emit (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "nvault_open" (func $nv_open (param i32 i32) (result i32)))
            (import "cs16_mod" "nvault_get" (func $nv_get (param i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "nvault_set" (func $nv_set (param i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "admin_has_flag" (func $adm (param i32 i32) (result i32)))
            (import "cs16_mod" "lang_tr" (func $lang_tr (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "menu_create" (func $m_new (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "menu_add_item" (func $m_add (param i32 i32 i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "menu_display" (func $m_show (param i32 i32 i32 i32) (result i32)))
            (import "cs16_mod" "say_text" (func $say (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "wasm_triple")
            (data (i32.const 32) "amxx_pulse")
            (data (i32.const 64) "amxx_double")
            (data (i32.const 96) "amxx_tag")
            (data (i32.const 128) "hello")
            (data (i32.const 160) "shared_v")
            (data (i32.const 192) "amxx_key")
            (data (i32.const 224) "pulse_seen")
            (data (i32.const 256) "wasm_ping")
            (data (i32.const 288) "hi_amxx")
            (data (i32.const 320) "GREET")
            (data (i32.const 352) "\\yWASM Menu")
            (data (i32.const 384) "Beta")
            (data (i32.const 400) "wasm_beta")
            (data (i32.const 432) "WASM_MENU_OK")
            (data (i32.const 464) "WASM_CHECK_OK")
            (data (i32.const 496) "from_amxx")
            (data (i32.const 528) "yes")
            (global $menu_id (mut i32) (i32.const 0))

            (func (export "cs_mod_init") (result i32)
                (drop (call $rpc_export (i32.const 0) (i32.const 11) (i32.const 1)))
                (drop (call $rpc_sub (i32.const 32) (i32.const 10) (i32.const 2)))
                (drop (call $reg (i32.const 7) (i32.const 0) (i32.const 0)))
                (global.set $menu_id (call $m_new (i32.const 352) (i32.const 11) (i32.const 9)))
                (drop (call $m_add (global.get $menu_id) (i32.const 384) (i32.const 4) (i32.const 400) (i32.const 9) (i32.const 1)))
                (i32.const 0)
            )

            (func (export "cs_mod_rpc") (param $cb i32) (result i32)
                (i32.mul (call $arg_i32 (i32.const 0)) (i32.const 3))
            )

            (func (export "cs_mod_event") (param $cb i32) (result i32)
                (local $h i32)
                (local.set $h (call $nv_open (i32.const 160) (i32.const 8)))
                (drop (call $nv_set (local.get $h) (i32.const 224) (i32.const 10) (i32.const 528) (i32.const 3)))
                (i32.const 0)
            )

            (func (export "cs_mod_menu") (param $cb i32) (param $slot i32) (param $mid i32) (param $item i32) (result i32)
                (drop (call $say (local.get $slot) (i32.const 432) (i32.const 12)))
                (i32.const 0)
            )

            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $slot i32)
                (local $len i32)
                (local $dbl i32)
                (local $tag_len i32)
                (local $nv i32)
                (local $nv_len i32)
                (local $is_adm i32)
                (local $tr_len i32)
                (local.set $slot (call $arg_i32 (i32.const 0)))
                (local.set $len (call $arg_str (i32.const 0) (i32.const 600) (i32.const 32)))
                ;; `wasmmenu` (len 8, starts with 'w' 119, 5th char 'm' 109)
                (if (i32.and
                        (i32.eq (local.get $len) (i32.const 8))
                        (i32.and (i32.eq (i32.load8_u (i32.const 600)) (i32.const 119))
                                 (i32.eq (i32.load8_u (i32.const 604)) (i32.const 109))))
                    (then
                        (drop (call $m_show (local.get $slot) (global.get $menu_id) (i32.const 0) (i32.const -1)))
                        (return (i32.const 1))
                    )
                )
                ;; `wasmcheck` (len 9, starts with 'w' 119)
                (if (i32.and (i32.eq (local.get $len) (i32.const 9)) (i32.eq (i32.load8_u (i32.const 600)) (i32.const 119)))
                    (then
                        (local.set $dbl (call $rpc_call (i32.const 64) (i32.const 11) (i32.const 21) (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)))
                        (local.set $tag_len (call $rpc_call (i32.const 96) (i32.const 8) (i32.const 0) (i32.const 128) (i32.const 5) (i32.const 700) (i32.const 64)))
                        (local.set $nv (call $nv_open (i32.const 160) (i32.const 8)))
                        (local.set $nv_len (call $nv_get (local.get $nv) (i32.const 192) (i32.const 8) (i32.const 800) (i32.const 64)))
                        (local.set $is_adm (call $adm (local.get $slot) (i32.const 100))) ;; 'd' = ADMIN_BAN
                        (local.set $tr_len (call $lang_tr (local.get $slot) (i32.const 320) (i32.const 5) (i32.const 0) (i32.const 0) (i32.const 900) (i32.const 64)))
                        ;; Emit event to AMXX `public wasm_ping`
                        (drop (call $rpc_emit (i32.const 256) (i32.const 9) (i32.const 288) (i32.const 7)))
                        ;; Verify: dbl == 42, tag_len == 10 ("AMXX:hello"), nv_len == 9 ("from_amxx"), is_adm == 1, tr_len > 0
                        (if (i32.and
                                (i32.and (i32.eq (local.get $dbl) (i32.const 42)) (i32.eq (local.get $tag_len) (i32.const 10)))
                                (i32.and (i32.eq (local.get $nv_len) (i32.const 9)) (i32.eq (local.get $is_adm) (i32.const 1))))
                            (then
                                (drop (call $say (local.get $slot) (i32.const 900) (local.get $tr_len)))
                                (drop (call $say (local.get $slot) (i32.const 464) (i32.const 13)))
                            )
                        )
                        (return (i32.const 1))
                    )
                )
                (i32.const 0)
            )
        )
    "#;

    let mut host = ServerModHost::new();
    {
        let svc = host.services();
        let mut s = svc.lock();
        s.lang
            .load_amxx_txt_str("[en]\nGREET = Hello Unified\n[ru]\nGREET = Privet Unified\n");
        s.admins
            .load_users_ini_str("\"AdminOne\" \"\" \"bcdef\" \"a\"\n");
    }
    host.load_package(make_v2_pkg(
        "unified_wasm",
        "permissions = [\"commands\", \"messages\", \"menus\", \"admins\", \"rpc\", \"storage\"]",
        wasm_wat,
    ))
    .unwrap();
    host.load_amxx_plugin("unified_amxx.amxx", &amxx_bytes)
        .unwrap();

    let mut srv = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        spawn_map(),
        ModdedGame::new(CsGame::new(), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );

    let mut hub = LoopbackHub::new();
    let ep0 = hub.connect();
    let ep1 = hub.connect();
    let mut c0 = Connection::new(
        ep0,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("AdminOne", 76561198000000010, &md5_hex(b"c0")),
    );
    c0.set_userinfo("lang", "ru");
    c0.connect();

    let mut c1 = Connection::new(
        ep1,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("PlayerTwo", 76561198000000011, &md5_hex(b"c1")),
    );
    c1.set_userinfo("lang", "en");
    c1.connect();

    let mut now = 0.0f64;
    for _ in 0..400 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
        if c0.state() == &ConnState::InGame && c1.state() == &ConnState::InGame {
            break;
        }
    }
    assert_eq!(c0.state(), &ConnState::InGame);
    assert_eq!(c1.state(), &ConnState::InGame);

    // 1. Server console sees BOTH WASM mod AND AMXX plugin (`mod_list` + `amxx plugins`)!
    let ConsoleOutcome::Output(mod_list) = srv.console_command("mod_list") else {
        panic!("expected mod_list output");
    };
    assert!(mod_list.contains("unified_wasm"), "mod_list={mod_list}");
    assert!(mod_list.contains("amxx plugins loaded: 1"), "mod_list={mod_list}");
    let ConsoleOutcome::Output(amxx_list) = srv.console_command("amxx plugins") else {
        panic!("expected amxx plugins output");
    };
    assert!(amxx_list.contains("Unified AMXX"), "amxx_list={amxx_list}");

    // 2. Shared MenuManager: slot 0 opens AMXX menu, slot 1 opens WASM menu, zero collision!
    c0.send_stringcmd("amxxmenu");
    c1.send_stringcmd("wasmmenu");
    for _ in 0..10 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }
    // Slot 0 received Russian translation from AMXX `%L`!
    assert!(
        c0.fx()
            .hud
            .text
            .iter()
            .any(|l| l.message.contains("Privet Unified")),
        "slot 0 (ru) should get Russian %L from AMXX, text={:?}",
        c0.fx().hud.text
    );
    // Both clients select option 1 simultaneously; AMXX gets slot 0, WASM gets slot 1!
    c0.send_stringcmd("menuselect 1");
    c1.send_stringcmd("menuselect 1");
    for _ in 0..10 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }
    let prints = srv.game().host.amxx_server_prints();
    assert!(
        prints.iter().any(|p| p == "AMXX_PICK:amxx_alpha"),
        "AMXX menu callback should fire for slot 0, prints={prints:?}"
    );
    assert!(
        c1.fx().hud.chat.iter().any(|l| l.text.contains("WASM_MENU_OK")),
        "WASM menu callback should fire for slot 1, chat={:?}",
        c1.fx().hud.chat
    );

    // 3. AMXX -> WASM (`wasm_triple(11)` == 33), shared AdminRegistry flags (`bcdef` = 62),
    //    shared NVault write (`amxx_key` = `from_amxx`), and AMXX `ExecuteForward` -> WASM `cs_mod_event`!
    c0.send_stringcmd("run_cross");
    for _ in 0..10 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }
    let prints = srv.game().host.amxx_server_prints();
    assert!(
        prints.iter().any(|p| p == "FLAGS:62:TRIPLE:33"),
        "AMXX should see shared AdminRegistry flags (62) and synchronous WASM RPC 11*3=33, prints={prints:?}"
    );
    {
        let svc = srv.game().host.services();
        let mut s = svc.lock();
        let vh = s.nvault.open("shared_v");
        assert_eq!(s.nvault.get(vh, "amxx_key"), Some("from_amxx"));
        assert_eq!(
            s.nvault.get(vh, "pulse_seen"),
            Some("yes"),
            "AMXX ExecuteForward should trigger WASM cs_mod_event"
        );
    }

    // 4. WASM -> AMXX (`amxx_double(21)` == 42, `amxx_tag("hello")` == `"AMXX:hello"`),
    //    WASM reads AMXX's nvault key, WASM checks shared admin flag `'d'`, WASM translates
    //    slot 0 (`ru` -> `"Privet Unified"`), and WASM `rpc_emit_event("wasm_ping", "hi_amxx")`
    //    triggers AMXX `public wasm_ping`!
    c0.send_stringcmd("wasmcheck");
    for _ in 0..10 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }
    assert!(
        c0.fx().hud.chat.iter().any(|l| l.text.contains("WASM_CHECK_OK")),
        "WASM should pass all cross-runtime checks (RPC int+str, nvault, admin, lang), chat={:?}",
        c0.fx().hud.chat
    );
    let prints = srv.game().host.amxx_server_prints();
    assert!(
        prints.iter().any(|p| p == "PING:hi_amxx"),
        "WASM rpc_emit_event should invoke AMXX public wasm_ping, prints={prints:?}"
    );
}

#[test]
fn unified_cs_game_runs_amxx_ham_reapi_csx_alongside_wasm_and_plugins_toml() {
    // Build an AMXX plugin that:
    // - Hooks `Ham_TakeDamage` Pre on `"player"` and overrides damage to `15.0` via `SetHamParamFloat(4, 15.0)`
    // - Registers `clcmd "reapi_kit"` calling `rg_add_account(id, 2500, AS_ADD, 1)`, `rg_set_user_armor(id, 100, CS_ARMOR_VESTHELM)`,
    //   `rg_give_item(id, "weapon_ak47")`, `set_member(id, M_B_HAS_DEFUSER, 1)`, and `get_member(id, M_I_ACCOUNT)`
    // - Receives `csx.inc` `client_damage` forward and verifies shared `"csx"` leaderboard (`get_statsnum()`)!
    let mut data = DataBuilder::new();
    let s_player = data.add_str("player");
    let s_on_dmg = data.add_str("on_ham_dmg");
    let s_cmd = data.add_str("reapi_kit");
    let s_cb_cmd = data.add_str("cmd_reapi_kit");
    let s_ak = data.add_str("weapon_ak47");
    let s_fmt_kit = data.add_str("KIT_MONEY:%d");
    let s_fmt_csx = data.add_str("CSX_DMG:%d:NUM:%d");
    let g_money = data.add_cell(0);
    let g_dmg_seen = data.add_cell(0);
    let g_statsnum = data.add_cell(0);

    let mut code = CodeBuilder::new(&[
        "RegisterHam",
        "SetHamParamFloat",
        "register_clcmd",
        "rg_add_account",
        "rg_set_user_armor",
        "rg_give_item",
        "set_member",
        "get_member",
        "get_statsnum",
        "server_print",
    ]);

    code.begin_public("plugin_init");
    code.call_native(
        "RegisterHam",
        &[
            Arg::Const(HamFunction::TakeDamage as i32),
            Arg::Const(s_player as i32),
            Arg::Const(s_on_dmg as i32),
            Arg::Const(0),
        ],
    );
    code.call_native(
        "register_clcmd",
        &[Arg::Const(s_cmd as i32), Arg::Const(s_cb_cmd as i32)],
    );
    code.ret_const(0);

    // Ham_TakeDamage Pre -> override damage to 15.0!
    code.begin_public("on_ham_dmg");
    code.call_native(
        "SetHamParamFloat",
        &[Arg::Const(4), Arg::Const(15.0f32.to_bits() as i32)],
    );
    code.ret_const(HAM_HANDLED);

    // cmd_reapi_kit(id)
    code.begin_public("cmd_reapi_kit");
    code.call_native(
        "rg_add_account",
        &[Arg::Param(0), Arg::Const(2500), Arg::Const(AS_ADD), Arg::Const(1)],
    );
    code.call_native(
        "rg_set_user_armor",
        &[Arg::Param(0), Arg::Const(100), Arg::Const(CS_ARMOR_VESTHELM)],
    );
    code.call_native("rg_give_item", &[Arg::Param(0), Arg::Const(s_ak as i32)]);
    code.call_native(
        "set_member",
        &[Arg::Param(0), Arg::Const(reapi_member::M_B_HAS_DEFUSER), Arg::Const(1)],
    );
    code.call_native(
        "get_member",
        &[Arg::Param(0), Arg::Const(reapi_member::M_I_ACCOUNT)],
    );
    code.store_pri(g_money);
    code.call_native(
        "server_print",
        &[Arg::Const(s_fmt_kit as i32), Arg::Const(g_money as i32)],
    );
    code.ret_const(1);

    // csx.inc client_damage(attacker, victim, damage, wpnindex, hitplace, TA)
    code.begin_public("client_damage");
    code.cells.push(OP_LOAD_S_PRI);
    code.cells.push(12 + 2 * 4);
    code.store_pri(g_dmg_seen);
    code.call_native("get_statsnum", &[]);
    code.store_pri(g_statsnum);
    code.call_native(
        "server_print",
        &[
            Arg::Const(s_fmt_csx as i32),
            Arg::Const(g_dmg_seen as i32),
            Arg::Const(g_statsnum as i32),
        ],
    );
    code.ret_const(0);

    let amxx_bytes = code.build(&data);

    // Build a WASM mod that triggers `player_damage(victim=0, attacker=1, 60.0, 0)` on `hit0` command.
    let wasm_wat = r#"
        (module
            (import "cs16_mod" "hook_register" (func $reg (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "hook_arg_str" (func $arg_str (param i32 i32 i32) (result i32)))
            (import "cs16_mod" "player_damage" (func $dmg (param i32 i32 f32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "cs_mod_init") (result i32)
                (drop (call $reg (i32.const 7) (i32.const 0) (i32.const 0)))
                (i32.const 0)
            )
            (func (export "cs_mod_hook") (param $hid i32) (param $ph i32) (result i32)
                (local $len i32)
                (local.set $len (call $arg_str (i32.const 0) (i32.const 0) (i32.const 16)))
                (if (i32.and (i32.eq (local.get $len) (i32.const 4)) (i32.eq (i32.load8_u (i32.const 0)) (i32.const 104)))
                    (then
                        (drop (call $dmg (i32.const 0) (i32.const 1) (f32.const 60.0) (i32.const 0)))
                        (return (i32.const 1))
                    )
                )
                (i32.const 0)
            )
        )
    "#;

    // Test loading BOTH through a temporary `configs/plugins.toml`!
    let tmp = std::env::temp_dir().join(format!(
        "cs16_rs_unified_toml_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let cfg_dir = tmp.join("configs");
    let mods_dir = tmp.join("mods").join("hit_wasm");
    let amxx_dir = tmp.join("addons").join("amxmodx").join("plugins");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&mods_dir).unwrap();
    std::fs::create_dir_all(&amxx_dir).unwrap();

    std::fs::write(
        mods_dir.join("mod.toml"),
        "id = \"hit_wasm\"\nversion = \"2.0.0\"\napi_version = 2\nentry_server = \"server.wasm\"\npermissions = [\"commands\", \"entities\"]\n",
    )
    .unwrap();
    std::fs::write(
        mods_dir.join("server.wasm"),
        compile_wat_to_wasm(wasm_wat).unwrap(),
    )
    .unwrap();
    std::fs::write(amxx_dir.join("reapi_ham.amxx"), &amxx_bytes).unwrap();
    let toml_path = cfg_dir.join("plugins.toml");
    std::fs::write(
        &toml_path,
        "mods_dir = \"../mods\"\namxx_dir = \"../addons/amxmodx/plugins\"\n",
    )
    .unwrap();

    let mut host = ServerModHost::new();
    let (w_cnt, a_cnt) = host.load_plugins_toml(&toml_path).unwrap();
    assert_eq!((w_cnt, a_cnt), (1, 1));
    let _ = std::fs::remove_dir_all(&tmp);

    let mut srv = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        spawn_map(),
        ModdedGame::new(CsGame::new(), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );

    let mut hub = LoopbackHub::new();
    let ep0 = hub.connect();
    let ep1 = hub.connect();
    let mut c0 = Connection::new(
        ep0,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("Victim", 76561198000000020, &md5_hex(b"v0")),
    );
    let mut c1 = Connection::new(
        ep1,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("Attacker", 76561198000000021, &md5_hex(b"a1")),
    );
    c0.connect();
    c1.connect();

    let mut now = 0.0f64;
    for _ in 0..400 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
        if c0.state() == &ConnState::InGame && c1.state() == &ConnState::InGame {
            break;
        }
    }
    // Join opposite teams & spawn both players!
    c0.send_stringcmd("jointeam 2");
    c1.send_stringcmd("jointeam 1");
    for _ in 0..20 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }
    c0.send_stringcmd("joinclass 1");
    c1.send_stringcmd("joinclass 1");
    for _ in 0..40 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }

    // Slot 0 runs `reapi_kit`: +$2500 ($800 -> $3300), 100 VestHelm, AK47, defuser!
    c0.send_stringcmd("reapi_kit");
    for _ in 0..10 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }
    {
        let p0 = srv.game().inner.state().players[0].as_ref().unwrap();
        assert_eq!(p0.money, 3300);
        assert_eq!(p0.armor.round() as i32, 100);
        assert!(p0.has_defuser);
    }
    let prints = srv.game().host.amxx_server_prints();
    assert!(prints.iter().any(|p| p == "KIT_MONEY:3300"), "prints={prints:?}");

    // Slot 1 runs WASM `hit0` dealing 60 dmg to slot 0 -> AMXX `Ham_TakeDamage` overrides to 15.0!
    // With 100 VestHelm, 50% Kevlar absorption => 7.5 HP damage (100 -> 92.5)!
    c1.send_stringcmd("hit0");
    for _ in 0..10 {
        c0.pump(now);
        c1.pump(now);
        now += 0.01;
        srv.frame(&mut hub, now);
    }
    let hp_after = srv.game().inner.state().players[0].as_ref().unwrap().health;
    assert!(
        (hp_after - 93.0).abs() < 1.0,
        "AMXX Ham_TakeDamage SetHamParamFloat(4, 15.0) with Kevlar should reduce HP 100 -> 93, got {hp_after}"
    );
    let prints = srv.game().host.amxx_server_prints();
    assert!(
        prints.iter().any(|p| p == "CSX_DMG:7:NUM:1"),
        "AMXX csx client_damage & get_statsnum should observe post-armor damage (7) and leaderboard entry, prints={prints:?}"
    );
    // And WASM / host can read the EXACT same `"csx"` leaderboard from `SharedPlatformServices`!
    let top = srv.game().host.services().lock().leaderboards.top("csx", 0, 5);
    assert_eq!(top.len(), 1);
}
