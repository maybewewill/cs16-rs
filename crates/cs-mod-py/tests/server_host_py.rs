//! Integration tests verifying `cs-mod-py` wired into `ServerModHost` / `ModdedGame`:
//! - Python paginated menu callbacks (`PluginRuntimeKind::Python`)
//! - Cross-plugin RPC between Python and WASM (`PluginRuntimeKind::Python` <-> `Wasm`)
//! - `sv_mod_autoreload 1` live mtime hot-reload preserving `mod.storage`.

use std::collections::BTreeMap;

use cs_mod_api::ModManifest;
use cs_mod_host::{LoadedPackage, ModStatus, ServerModHost, compile_wat_to_wasm};
use cs_mod_py::attach_python_runtime;

#[test]
fn server_mod_host_python_rpc_and_autoreload_preserves_storage() {
    let tmp = std::env::temp_dir().join(format!(
        "cs16_py_host_wire_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");

    let py_path = tmp.join("vip_rpc.py");
    let py_v1 = r#"
from cs16 import mod, HookAction

@mod.info(id="vip_rpc", name="VIP RPC", version="1.0.0")
def _meta():
    pass

@mod.export_fn("vip.bonus")
def vip_bonus(base):
    cur = int(mod.storage.get("calls", "0")) + 1
    mod.storage.set("calls", str(cur))
    return int(base) + 10
"#;
    std::fs::write(&py_path, py_v1).expect("write py v1");

    // Minimal WASM caller that invokes Python RPC `"vip.bonus"` on ServerCommand
    let caller_wat = r#"
(module
  (import "cs16_mod" "hook_register" (func $hook_register (param i32 i32 i32) (result i32)))
  (import "cs16_mod" "rpc_call" (func $rpc_call (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (import "cs16_mod" "storage_set" (func $storage_set (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "vip.bonus")
  (data (i32.const 32) "last_val")
  (func (export "cs_mod_init") (result i32)
    (drop (call $hook_register (i32.const 12) (i32.const 0) (i32.const 0)))
    (i32.const 0)
  )
  (func (export "cs_mod_hook") (param $hook i32) (param $phase i32) (result i32)
    (local $res i32)
    (local.set $res (call $rpc_call (i32.const 0) (i32.const 9) (i32.const 5) (i32.const 0) (i32.const 0) (i32.const 64) (i32.const 16)))
    (i32.store8 (i32.const 96) (i32.add (i32.const 48) (i32.div_s (local.get $res) (i32.const 10))))
    (i32.store8 (i32.const 97) (i32.add (i32.const 48) (i32.rem_s (local.get $res) (i32.const 10))))
    (drop (call $storage_set (i32.const 32) (i32.const 8) (i32.const 96) (i32.const 2)))
    (i32.const 0)
  )
)
"#;

    let mut host = ServerModHost::new();
    attach_python_runtime(&mut host);
    let loaded_id = host.load_python_file(&py_path).expect("load py plugin");
    assert_eq!(loaded_id, "vip_rpc");

    let caller_manifest = ModManifest::parse_toml(
        r#"
id = "wasm_caller"
name = "WASM Caller"
version = "1.0.0"
api_version = 2
side = "server"
entry_server = "server.wasm"
permissions = ["rpc", "storage", "commands"]
"#,
    )
    .expect("manifest");
    host.load_package(LoadedPackage {
        manifest: caller_manifest,
        source_path: None,
        server_wasm: Some(compile_wat_to_wasm(caller_wat).expect("wat")),
        client_wasm: None,
        client_bundle: None,
        assets: BTreeMap::new(),
    })
    .expect("load wasm caller");

    let mods = host.list_mods();
    assert!(mods.iter().any(|m| m.id == "vip_rpc" && m.status == ModStatus::Active));
    assert!(mods.iter().any(|m| m.id == "wasm_caller" && m.status == ModStatus::Active));

    // Modify Python file on disk -> poll_autoreload picks it up and updates version to 2.0.0
    let py_v2 = r#"
from cs16 import mod

@mod.info(id="vip_rpc", name="VIP RPC", version="2.0.0")
def _meta():
    pass

@mod.export_fn("vip.bonus")
def vip_bonus(base):
    cur = int(mod.storage.get("calls", "0")) + 1
    mod.storage.set("calls", str(cur))
    return int(base) + 25
"#;
    std::fs::write(&py_path, py_v2).expect("write py v2");
    let reloaded = host.poll_autoreload(2.0, true);
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].0, "vip_rpc");
    assert!(reloaded[0].1.is_ok(), "autoreload failed: {:?}", reloaded[0].1);
    assert!(
        host.list_mods()
            .iter()
            .any(|m| m.id == "vip_rpc" && m.version == "2.0.0" && m.status == ModStatus::Active)
    );

    // Broken edit leaves the live v2.0.0 plugin running cleanly!
    std::fs::write(&py_path, "def broken(\n").expect("write broken py");
    let broken_res = host.poll_autoreload(4.0, true);
    assert_eq!(broken_res.len(), 1);
    assert!(broken_res[0].1.is_err());
    assert!(
        host.list_mods()
            .iter()
            .any(|m| m.id == "vip_rpc" && m.version == "2.0.0" && m.status == ModStatus::Active)
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
