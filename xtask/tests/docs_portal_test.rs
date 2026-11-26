//! Integration verification for the `/docs/` Developer Portal and `/mods/` Plugin Hub generators.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cs_mod_api::HookId;
use xtask::docs_gen;
use xtask::mods_hub_gen;
use xtask::parity;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits directly inside workspace root")
        .to_path_buf()
}

fn fresh_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "cs16_docs_portal_{label}_{}_{}",
        std::process::id(),
        nanos
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

/// Exhaustively enumerates every numeric `HookId` discriminant defined in `cs-mod-api`.
fn all_cs_mod_api_hooks() -> Vec<HookId> {
    (0u32..=256).filter_map(HookId::from_u32).collect()
}

fn collect_expected_mod_ids(root: &Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for sub in ["std", "community", "gamemodes"] {
        let dir = root.join("mods").join(sub);
        if !dir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&dir).expect("read mods subdir").flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("py") {
                let src = fs::read_to_string(&path).expect("read py plugin");
                let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("p.py");
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("p");
                let id = cs_mod_py::PySourceSyntaxCheck::check(fname, &src)
                    .ok()
                    .and_then(|r| r.meta.id)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| stem.to_string());
                ids.insert(id);
            } else if path.is_dir() && path.join("mod.toml").is_file() {
                let raw = fs::read_to_string(path.join("mod.toml")).expect("read mod.toml");
                let m = cs_mod_api::ModManifest::parse_toml(&raw).expect("valid mod.toml");
                ids.insert(m.id);
            }
        }
    }
    ids
}

fn extract_attr_values(html: &str, attr: &str) -> Vec<String> {
    let needle = format!("{attr}=\"");
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find(&needle) {
        let after = &rest[pos + needle.len()..];
        if let Some(end) = after.find('"') {
            out.push(after[..end].to_string());
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    out
}

#[test]
fn docs_portal_and_mods_hub_are_complete_valid_and_link_checked() {
    let root = workspace_root();
    let out = fresh_temp_dir("full_portal");

    let docs_stats = docs_gen::generate_docs_portal_with_root(&root, &out)
        .expect("generate_docs_portal_with_root succeeds");
    let mods_stats = mods_hub_gen::generate_mods_hub_with_root(&root, &out)
        .expect("generate_mods_hub_with_root succeeds");

    // 1. Every HookId variant in cs-mod-api is documented in hooks.html & indexed in search-index.json.
    let hooks_in_api = all_cs_mod_api_hooks();
    assert!(hooks_in_api.len() >= 45, "expected 45+ HookId variants");
    assert_eq!(docs_stats.hooks, hooks_in_api.len());

    let hooks_html =
        fs::read_to_string(out.join("docs/hooks.html")).expect("read generated docs/hooks.html");
    let search_json = fs::read_to_string(out.join("docs/search-index.json"))
        .expect("read generated docs/search-index.json");

    for hook in &hooks_in_api {
        let snake = hook.as_name();
        let anchor = format!("id=\"hook-{snake}\"");
        assert!(
            hooks_html.contains(&anchor),
            "docs/hooks.html missing hook anchor {anchor}"
        );
        let search_url = format!("./hooks.html#hook-{snake}");
        assert!(
            search_json.contains(&search_url),
            "docs/search-index.json missing hook entry {search_url}"
        );
    }

    // 2. All entries from docs/parity/cvars.toml (735+) are indexed in cvars-deployment.html, cvars-data.json, and search-index.json.
    let cvars_toml =
        fs::read_to_string(root.join("docs/parity/cvars.toml")).expect("read cvars.toml");
    let cvars_cat = parity::parse_category("cvars", &cvars_toml).expect("parse cvars.toml");
    assert!(
        cvars_cat.items.len() >= 735,
        "expected >= 735 CVar entries, got {}",
        cvars_cat.items.len()
    );
    assert_eq!(docs_stats.cvars, cvars_cat.items.len());

    let cvars_html = fs::read_to_string(out.join("docs/cvars-deployment.html"))
        .expect("read docs/cvars-deployment.html");
    let cvars_json =
        fs::read_to_string(out.join("docs/cvars-data.json")).expect("read docs/cvars-data.json");

    for item in &cvars_cat.items {
        let json_id = format!("\"id\":\"{}\"", item.id);
        assert!(
            cvars_json.contains(&json_id),
            "docs/cvars-data.json missing CVar/command {}",
            item.id
        );
        let html_code = format!("<code>{}</code>", item.id);
        assert!(
            cvars_html.contains(&html_code),
            "docs/cvars-deployment.html missing CVar/command {}",
            item.id
        );
        let search_title = format!("\"t\":\"{}\"", item.id);
        assert!(
            search_json.contains(&search_title),
            "docs/search-index.json missing CVar/command {}",
            item.id
        );
    }

    // 3. All plugins in mods/ are emitted into mods/index.html & mods/plugins.json with required contract fields.
    let expected_mods = collect_expected_mod_ids(&root);
    assert!(
        expected_mods.len() >= 13,
        "expected at least 13 workspace plugins, found {}",
        expected_mods.len()
    );
    assert_eq!(mods_stats.total_plugins, expected_mods.len());

    let mods_html =
        fs::read_to_string(out.join("mods/index.html")).expect("read generated mods/index.html");
    let plugins_json = fs::read_to_string(out.join("mods/plugins.json"))
        .expect("read generated mods/plugins.json");

    for field in [
        "\"id\":",
        "\"name\":",
        "\"category\":",
        "\"lang\":",
        "\"summary\":",
        "\"source_files\":",
        "\"download_url\":",
        "\"playground_compatible\":",
    ] {
        assert!(
            plugins_json.contains(field),
            "mods/plugins.json missing contract field {field}"
        );
    }
    for mod_id in &expected_mods {
        let json_marker = format!("\"id\":\"{mod_id}\"");
        assert!(
            plugins_json.contains(&json_marker),
            "mods/plugins.json missing plugin {mod_id}"
        );
        let pg_link = format!("../playground/?mod={mod_id}");
        assert!(
            mods_html.contains(&pg_link),
            "mods/index.html missing Playground deep link {pg_link}"
        );
    }

    // 4. Every generated JS file passes `node --check` AND valid JSON parses cleanly via Node.
    let js_files = [
        out.join("docs/docs.js"),
        out.join("docs/vfx-designer.js"),
        out.join("docs/amxx-converter.js"),
        out.join("mods/mods.js"),
    ];
    for js in &js_files {
        let status = Command::new("node")
            .arg("--check")
            .arg(js)
            .status()
            .expect("invoke `node --check`");
        assert!(
            status.success(),
            "`node --check` failed for {}",
            js.display()
        );
    }

    let json_files = [
        out.join("docs/search-index.json"),
        out.join("docs/cvars-data.json"),
        out.join("mods/plugins.json"),
    ];
    for jf in &json_files {
        let status = Command::new("node")
            .args([
                "-e",
                "JSON.parse(require('fs').readFileSync(process.argv[1],'utf8'))",
            ])
            .arg(jf)
            .status()
            .expect("invoke node JSON.parse");
        assert!(
            status.success(),
            "invalid JSON generated in {}",
            jf.display()
        );
    }

    // 5. All internal relative href/src links in every generated HTML file resolve cleanly.
    let html_pages = [
        out.join("docs/index.html"),
        out.join("docs/hooks.html"),
        out.join("docs/api.html"),
        out.join("docs/vfx-menus.html"),
        out.join("docs/amxx-migration.html"),
        out.join("docs/cvars-deployment.html"),
        out.join("mods/index.html"),
    ];
    for page in &html_pages {
        let content = fs::read_to_string(page).expect("read generated HTML page");
        let page_dir = page.parent().expect("page parent dir");
        for attr in ["href", "src"] {
            for raw_link in extract_attr_values(&content, attr) {
                if raw_link.is_empty()
                    || raw_link.starts_with('#')
                    || raw_link.starts_with("http://")
                    || raw_link.starts_with("https://")
                {
                    continue;
                }
                let path_only = raw_link.split(['#', '?']).next().unwrap_or("");
                if path_only.is_empty() {
                    continue;
                }
                // Sibling worker target `/playground/` is staged by `p18-s2-playground`.
                if path_only == "../playground/" {
                    continue;
                }
                let resolved = if let Some(rel_root) = path_only.strip_prefix("../") {
                    if rel_root.is_empty() {
                        root.join("web/index.html")
                    } else if rel_root.starts_with("docs/") || rel_root.starts_with("mods/") {
                        let candidate = out.join(rel_root);
                        if candidate.is_dir() {
                            candidate.join("index.html")
                        } else {
                            candidate
                        }
                    } else {
                        root.join("web").join(rel_root)
                    }
                } else {
                    let candidate = page_dir.join(path_only);
                    if candidate.is_dir() {
                        candidate.join("index.html")
                    } else {
                        candidate
                    }
                };
                assert!(
                    resolved.exists(),
                    "broken relative {attr}=\"{raw_link}\" in {} (resolved to {})",
                    page.display(),
                    resolved.display()
                );
            }
        }
    }

    let _ = fs::remove_dir_all(&out);
}

#[test]
fn copy_web_subdirs_stages_playground_and_mods_hub_discovers_future_gamemodes() {
    let temp = fresh_temp_dir("staging_and_dynamic");
    let fake_web = temp.join("web");
    let fake_out = temp.join("dist_web");
    fs::create_dir_all(fake_web.join("playground/assets")).unwrap();
    fs::create_dir_all(fake_web.join("icons")).unwrap();
    fs::write(
        fake_web.join("playground/index.html"),
        "<h1>Playground</h1>",
    )
    .unwrap();
    fs::write(
        fake_web.join("playground/playground.js"),
        "console.log('ok');",
    )
    .unwrap();
    fs::write(fake_web.join("playground/assets/sample.txt"), "demo").unwrap();
    fs::write(fake_web.join("icons/icon-192.png"), "png").unwrap();

    let copied =
        mods_hub_gen::copy_web_subdirs(&fake_web, &fake_out).expect("copy_web_subdirs succeeds");
    assert_eq!(copied, 3);
    assert!(fake_out.join("playground/index.html").is_file());
    assert!(fake_out.join("playground/playground.js").is_file());
    assert!(fake_out.join("playground/assets/sample.txt").is_file());
    assert!(!fake_out.join("icons/icon-192.png").exists());

    // Synthetic workspace proving Retakes + Multi-1v1 Arena auto-discovery works seamlessly.
    let fake_ws = temp.join("ws");
    fs::create_dir_all(fake_ws.join("mods/gamemodes/retakes/src")).unwrap();
    fs::write(
        fake_ws.join("mods/gamemodes/retakes/mod.toml"),
        "[mod]\nid = \"retakes\"\nname = \"Competitive Retakes\"\nversion = \"1.0.0\"\napi_version = 2\nside = \"server\"\nentry_server = \"server.wasm\"\npermissions = [\"entities\", \"messages\", \"weapons\"]\n",
    )
    .unwrap();
    fs::write(
        fake_ws.join("mods/gamemodes/retakes/src/lib.rs"),
        "//! Post-plant bombsite retakes.\npub fn dummy() {}\n",
    )
    .unwrap();
    fs::write(
        fake_ws.join("mods/gamemodes/multi_1v1.py"),
        "from cs16 import mod\n@mod.info(id=\"multi_1v1\", name=\"Multi-1v1 Arena\", version=\"1.0.0\", description=\"1v1 ladder duels\")\ndef _meta(): pass\n",
    )
    .unwrap();

    let stats = mods_hub_gen::generate_mods_hub_with_root(&fake_ws, &fake_out)
        .expect("dynamic mods discovery succeeds");
    assert_eq!(stats.total_plugins, 2);
    assert_eq!(stats.gamemodes, 2);
    assert_eq!(stats.python_plugins, 1);

    let json = fs::read_to_string(fake_out.join("mods/plugins.json")).unwrap();
    assert!(json.contains("\"id\":\"retakes\""));
    assert!(json.contains("\"id\":\"multi_1v1\""));
    assert!(fake_out.join("mods/downloads/multi_1v1.py").is_file());

    let _ = fs::remove_dir_all(&temp);
}
