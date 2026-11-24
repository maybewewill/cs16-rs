//! Starter project generator for authoring WASM mods in Rust with `cs-mod-sdk`.

use std::collections::BTreeMap;

use cs_mod_api::{ABI_VERSION, ModSide};

/// Generates a starter Rust WASM mod repository layout (`Cargo.toml`, `mod.toml`, `src/lib.rs`, `README.md`).
pub fn scaffold_mod_project(id: &str, name: &str, side: ModSide) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();

    let side_str = side.as_str();
    let entry_server = if side.has_server() {
        "entry_server = \"server.wasm\"\n"
    } else {
        ""
    };
    let entry_client = if side.has_client() {
        "entry_client = \"client.wasm\"\n"
    } else {
        ""
    };

    files.insert(
        "mod.toml".to_string(),
        format!(
            "[mod]\nid = \"{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\napi_version = {ABI_VERSION}\nside = \"{side_str}\"\n{entry_server}{entry_client}permissions = [\"messages\", \"entities\"]\nfuel_per_frame = 500000\nmax_memory_bytes = 4194304\n"
        ),
    );

    files.insert(
        "Cargo.toml".to_string(),
        format!(
            "[package]\nname = \"{id}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\ncs-mod-sdk = \"0.1\"\n"
        ),
    );

    files.insert(
        "src/lib.rs".to_string(),
        r#"use cs_mod_sdk::{HookAction, cs_mod_hook, cs_mod_init};

#[cs_mod_init]
pub extern "C" fn cs_mod_init() -> i32 {
    0
}

#[cs_mod_hook(start_frame, pre)]
pub fn on_start_frame() -> HookAction {
    HookAction::Continue
}
"#
        .to_string(),
    );

    files.insert(
        "README.md".to_string(),
        format!(
            "# {name}\n\nBuild for `wasm32-unknown-unknown` and place `mod.toml` + `.wasm` into `mods/{id}/` or package as `{id}.csmod`.\n"
        ),
    );

    files
}
