//! Keeps the project licence consistent: GPL-3.0-or-later in the workspace
//! manifest, inherited by every crate, with the GPL v3 text in `LICENSE`.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn manifest(path: &Path) -> toml::Table {
    read(path)
        .parse()
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

fn assert_inherits_licence(path: &Path) {
    let manifest = manifest(path);
    let inherited = manifest
        .get("package")
        .and_then(|p| p.get("license"))
        .and_then(|l| l.get("workspace"))
        .and_then(toml::Value::as_bool);
    assert_eq!(
        inherited,
        Some(true),
        "{} must set license.workspace = true",
        path.display()
    );
}

#[test]
fn workspace_is_gpl3_or_later() {
    let root = workspace_root();

    let root_manifest = manifest(&root.join("Cargo.toml"));
    let licence = root_manifest
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("license"))
        .and_then(toml::Value::as_str);
    assert_eq!(licence, Some("GPL-3.0-or-later"));

    let mut crate_manifests: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("cannot list crates/")
        .map(|entry| {
            entry
                .expect("cannot read crates/ entry")
                .path()
                .join("Cargo.toml")
        })
        .filter(|path| path.is_file())
        .collect();
    crate_manifests.sort();
    assert!(!crate_manifests.is_empty(), "no crates found under crates/");
    for path in &crate_manifests {
        assert_inherits_licence(path);
    }
    assert_inherits_licence(&root.join("xtask/Cargo.toml"));

    let text = read(&root.join("LICENSE"));
    assert!(
        text.trim_start().starts_with("GNU GENERAL PUBLIC LICENSE"),
        "LICENSE must start with the GPL title"
    );
    assert!(
        text.contains("Version 3, 29 June 2007"),
        "LICENSE must be GPL version 3"
    );

    let readme = read(&root.join("README.md"));
    assert!(
        !readme.contains("MIT-licensed"),
        "README.md must not call anything MIT-licensed"
    );
}
