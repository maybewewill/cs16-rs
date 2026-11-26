//! Static GitHub Pages `/mods/` Plugin Hub generator for `cs16-rs`.
//!
//! Dynamically scans `mods/std/`, `mods/community/`, and `mods/gamemodes/` on disk so all
//! built-in Standard, Community, and Gamemode plugins (including future plugins merged into
//! those directories such as `retakes` and `multi_1v1.py`) are indexed automatically into:
//! - `<out_dir>/mods/index.html` — Searchable Steam-2003 VGUI2 showcase cards + source viewer
//! - `<out_dir>/mods/mods.css` & `<out_dir>/mods/mods.js`
//! - `<out_dir>/mods/plugins.json` — Cross-worker JSON catalog consumed by `/playground/?mod=<id>`
//! - `<out_dir>/mods/downloads/*` — Ready-to-download `.py`, `.csmod`, and source bundles

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use cs_mod_api::{CsModArchive, ModManifest};
use cs_mod_py::PySourceSyntaxCheck;

use crate::docs_gen::{
    PORTAL_CSS, html_escape, json_escape, render_top_navbar, slugify, workspace_root,
};

/// Summary metrics returned by [`generate_mods_hub`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModsHubStats {
    pub total_plugins: usize,
    pub standard: usize,
    pub community: usize,
    pub gamemodes: usize,
    pub python_plugins: usize,
}

#[derive(Debug, Clone)]
struct SourceFileEntry {
    path: String,
    content: String,
}

#[derive(Debug, Clone)]
struct DiscoveredPlugin {
    id: String,
    name: String,
    category: String,
    category_label: String,
    lang: String,
    lang_key: String,
    version: String,
    side: String,
    summary: String,
    permissions: Vec<String>,
    source_files: Vec<SourceFileEntry>,
    download_url: String,
    source_download_url: Option<String>,
    playground_compatible: bool,
    playground_code: String,
}

/// Generates the full `/mods/` Plugin Hub into `<out_dir>/mods/` using the auto-detected workspace root.
pub fn generate_mods_hub(out_dir: &Path) -> Result<ModsHubStats, String> {
    let root = workspace_root();
    generate_mods_hub_with_root(&root, out_dir)
}

/// Generates the full `/mods/` Plugin Hub into `<out_dir>/mods/` scanning `workspace_root/mods/`.
pub fn generate_mods_hub_with_root(
    workspace_root: &Path,
    out_dir: &Path,
) -> Result<ModsHubStats, String> {
    let mods_out = out_dir.join("mods");
    let downloads_out = mods_out.join("downloads");
    fs::create_dir_all(&downloads_out).map_err(|e| io_err(&downloads_out, &e))?;

    let categories = [
        ("std", "standard", "Standard"),
        ("community", "community", "Community"),
        ("gamemodes", "gamemodes", "Gamemodes"),
    ];

    let mut plugins = Vec::new();
    for (dir_name, cat_key, cat_label) in categories {
        let dir = workspace_root.join("mods").join(dir_name);
        if !dir.is_dir() {
            continue;
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| io_err(&dir, &e))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        entries.sort();

        for path in entries {
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("py") {
                plugins.push(load_python_file_plugin(
                    workspace_root,
                    &path,
                    cat_key,
                    cat_label,
                    &downloads_out,
                )?);
            } else if path.is_dir() && path.join("mod.toml").is_file() {
                plugins.push(load_directory_plugin(
                    workspace_root,
                    &path,
                    cat_key,
                    cat_label,
                    &downloads_out,
                )?);
            }
        }
    }

    let standard = plugins.iter().filter(|p| p.category == "standard").count();
    let community = plugins.iter().filter(|p| p.category == "community").count();
    let gamemodes = plugins.iter().filter(|p| p.category == "gamemodes").count();
    let python_plugins = plugins.iter().filter(|p| p.playground_compatible).count();

    let plugins_json = encode_plugins_json(&plugins);
    let mods_css = format!("{PORTAL_CSS}\n{MODS_EXTRA_CSS}");
    let index_html = render_mods_index_html(&plugins, &plugins_json);

    write_text(&mods_out.join("plugins.json"), &plugins_json)?;
    write_text(&mods_out.join("mods.css"), &mods_css)?;
    write_text(&mods_out.join("mods.js"), MODS_JS)?;
    write_text(&mods_out.join("index.html"), &index_html)?;

    Ok(ModsHubStats {
        total_plugins: plugins.len(),
        standard,
        community,
        gamemodes,
        python_plugins,
    })
}

/// Recursively copies every non-`icons` subdirectory under `web_src` (such as `web/playground/**`)
/// into `out_dir/<subdir>/**` so sibling features deploy automatically with `cargo xtask web-build`.
pub fn copy_web_subdirs(web_src: &Path, out_dir: &Path) -> Result<usize, String> {
    if !web_src.is_dir() {
        return Ok(0);
    }
    let mut copied = 0usize;
    let mut dirs: Vec<PathBuf> = fs::read_dir(web_src)
        .map_err(|e| io_err(web_src, &e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name == "icons" {
            continue;
        }
        let dst = out_dir.join(name);
        copied += copy_dir_recursive(&dir, &dst)?;
    }
    Ok(copied)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<usize, String> {
    fs::create_dir_all(dst).map_err(|e| io_err(dst, &e))?;
    let mut copied = 0usize;
    let mut entries: Vec<PathBuf> = fs::read_dir(src)
        .map_err(|e| io_err(src, &e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();

    for path in entries {
        let Some(name) = path.file_name() else {
            continue;
        };
        let target = dst.join(name);
        if path.is_dir() {
            copied += copy_dir_recursive(&path, &target)?;
        } else if path.is_file() {
            fs::copy(&path, &target).map_err(|e| io_err(&path, &e))?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn load_python_file_plugin(
    _workspace_root: &Path,
    py_path: &Path,
    cat_key: &str,
    cat_label: &str,
    downloads_out: &Path,
) -> Result<DiscoveredPlugin, String> {
    let file_name = py_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin.py");
    let stem = py_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin");
    let content = fs::read_to_string(py_path).map_err(|e| io_err(py_path, &e))?;

    let (id, name, version, summary, permissions) =
        match PySourceSyntaxCheck::check(file_name, &content) {
            Ok(report) => {
                let id = report
                    .meta
                    .id
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| stem.to_string());
                let name = report
                    .meta
                    .name
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| title_case_slug(&id));
                let version = report
                    .meta
                    .version
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "1.0.0".to_string());
                let summary = report
                    .meta
                    .description
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| infer_summary_from_source(&id, &name, &content));
                let perms = if report.meta.permissions.is_empty() {
                    vec!["entities".to_string(), "messages".to_string()]
                } else {
                    report.meta.permissions
                };
                (id, name, version, summary, perms)
            }
            Err(_) => (
                stem.to_string(),
                title_case_slug(stem),
                "1.0.0".to_string(),
                infer_summary_from_source(stem, &title_case_slug(stem), &content),
                vec!["entities".to_string(), "messages".to_string()],
            ),
        };

    let dl_name = format!("{}.py", slugify_underscore(&id));
    fs::write(downloads_out.join(&dl_name), &content)
        .map_err(|e| io_err(&downloads_out.join(&dl_name), &e))?;

    Ok(DiscoveredPlugin {
        id,
        name,
        category: cat_key.to_string(),
        category_label: cat_label.to_string(),
        lang: "Python 3".to_string(),
        lang_key: "py".to_string(),
        version,
        side: "server".to_string(),
        summary,
        permissions,
        source_files: vec![SourceFileEntry {
            path: file_name.to_string(),
            content: content.clone(),
        }],
        download_url: format!("./downloads/{dl_name}"),
        source_download_url: None,
        playground_compatible: true,
        playground_code: content,
    })
}

fn load_directory_plugin(
    workspace_root: &Path,
    dir: &Path,
    cat_key: &str,
    cat_label: &str,
    downloads_out: &Path,
) -> Result<DiscoveredPlugin, String> {
    let manifest_path = dir.join("mod.toml");
    let manifest_raw =
        fs::read_to_string(&manifest_path).map_err(|e| io_err(&manifest_path, &e))?;
    let manifest = ModManifest::parse_toml(&manifest_raw)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    let (lang, lang_key, primary_rel) = detect_dir_language(dir);
    let mut source_files = Vec::new();
    let mut primary_content = String::new();

    let candidate_files = [
        primary_rel.as_str(),
        "client/src/lib.rs",
        "mod.toml",
        "lang.toml",
        "Cargo.toml",
        "go.mod",
    ];
    for rel in candidate_files {
        if rel.is_empty() {
            continue;
        }
        let full = dir.join(rel);
        if full.is_file()
            && !source_files.iter().any(|s: &SourceFileEntry| s.path == rel)
            && let Ok(text) = fs::read_to_string(&full)
        {
            if rel == primary_rel {
                primary_content = text.clone();
            }
            source_files.push(SourceFileEntry {
                path: rel.to_string(),
                content: text,
            });
        }
    }

    let safe_id = slugify_underscore(&manifest.id);
    let primary_ext = Path::new(&primary_rel)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt");
    let src_dl_name = format!("{safe_id}_source.{primary_ext}");
    let src_to_write = if primary_content.is_empty() {
        manifest_raw.clone()
    } else {
        primary_content.clone()
    };
    fs::write(downloads_out.join(&src_dl_name), &src_to_write)
        .map_err(|e| io_err(&downloads_out.join(&src_dl_name), &e))?;

    let playground_compatible = lang_key == "py";
    let (download_url, source_download_url) = if playground_compatible {
        let py_dl = format!("{safe_id}.py");
        fs::write(downloads_out.join(&py_dl), &src_to_write)
            .map_err(|e| io_err(&downloads_out.join(&py_dl), &e))?;
        (format!("./downloads/{py_dl}"), None)
    } else {
        let csmod_name = format!("{safe_id}.csmod");
        let csmod_dst = downloads_out.join(&csmod_name);
        if stage_or_pack_csmod(workspace_root, dir, &manifest, &csmod_dst)? {
            (
                format!("./downloads/{csmod_name}"),
                Some(format!("./downloads/{src_dl_name}")),
            )
        } else {
            (format!("./downloads/{src_dl_name}"), None)
        }
    };

    let mut permissions: Vec<String> = manifest
        .permissions
        .names()
        .into_iter()
        .map(str::to_string)
        .collect();
    for host in &manifest.http_hosts {
        permissions.push(format!("http:{host}"));
    }

    let summary = curated_or_inferred_summary(&manifest.id, &manifest.name, &primary_content);
    let playground_code = if playground_compatible {
        src_to_write
    } else {
        starter_python_equivalent(&manifest.id, &manifest.name)
    };

    Ok(DiscoveredPlugin {
        id: manifest.id,
        name: manifest.name,
        category: cat_key.to_string(),
        category_label: cat_label.to_string(),
        lang,
        lang_key,
        version: manifest.version,
        side: manifest.side.as_str().to_string(),
        summary,
        permissions,
        source_files,
        download_url,
        source_download_url,
        playground_compatible,
        playground_code,
    })
}

fn detect_dir_language(dir: &Path) -> (String, String, String) {
    if dir.join("src/lib.rs").is_file() {
        ("Rust".into(), "rust".into(), "src/lib.rs".into())
    } else if dir.join("main.cpp").is_file() {
        ("C++20".into(), "cpp".into(), "main.cpp".into())
    } else if dir.join("main.go").is_file() {
        ("Go".into(), "go".into(), "main.go".into())
    } else if dir.join("main.py").is_file() {
        ("Python 3".into(), "py".into(), "main.py".into())
    } else {
        ("Rust".into(), "rust".into(), "mod.toml".into())
    }
}

fn stage_or_pack_csmod(
    workspace_root: &Path,
    mod_dir: &Path,
    manifest: &ModManifest,
    dst: &Path,
) -> Result<bool, String> {
    let prebuilt = workspace_root
        .join("dist/mods")
        .join(format!("{}.csmod", manifest.id));
    if prebuilt.is_file() {
        fs::copy(&prebuilt, dst).map_err(|e| io_err(&prebuilt, &e))?;
        return Ok(true);
    }

    let server_wasm = manifest
        .entry_server
        .as_deref()
        .map(|rel| mod_dir.join(rel))
        .filter(|p| p.is_file());
    let client_wasm = manifest
        .entry_client
        .as_deref()
        .map(|rel| mod_dir.join(rel))
        .filter(|p| p.is_file());
    if server_wasm.is_none() && client_wasm.is_none() {
        return Ok(false);
    }

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert(
        "mod.toml".to_string(),
        fs::read(mod_dir.join("mod.toml")).map_err(|e| io_err(&mod_dir.join("mod.toml"), &e))?,
    );
    if let (Some(rel), Some(path)) = (manifest.entry_server.as_deref(), server_wasm) {
        files.insert(
            rel.to_string(),
            fs::read(&path).map_err(|e| io_err(&path, &e))?,
        );
    }
    if let (Some(rel), Some(path)) = (manifest.entry_client.as_deref(), client_wasm) {
        files.insert(
            rel.to_string(),
            fs::read(&path).map_err(|e| io_err(&path, &e))?,
        );
    }
    let lang_path = mod_dir.join("lang.toml");
    if lang_path.is_file() {
        files.insert(
            "lang.toml".to_string(),
            fs::read(&lang_path).map_err(|e| io_err(&lang_path, &e))?,
        );
    }
    for asset_rel in &manifest.assets {
        let candidate = mod_dir.join("assets").join(asset_rel);
        if candidate.is_file() {
            files.insert(
                format!("assets/{asset_rel}"),
                fs::read(&candidate).map_err(|e| io_err(&candidate, &e))?,
            );
        }
    }

    let archive_bytes =
        CsModArchive::write_zip(files.iter().map(|(k, v)| (k.as_str(), v.as_slice())))
            .map_err(|e| format!("failed to pack {}.csmod: {e}", manifest.id))?;
    fs::write(dst, archive_bytes).map_err(|e| io_err(dst, &e))?;
    Ok(true)
}

fn curated_or_inferred_summary(id: &str, name: &str, src: &str) -> String {
    match id {
        "std_admin" => "Complete AMXX+ administration suite: amxmodmenu, kick/ban/slap/slay, admin chat, map/cvar controls, EN/RU localization, and unified RBAC.".to_string(),
        "std_mapmanager" => "Go/WASI map cycle manager with Rock-the-Vote (/rtv), /nominate, nextmap/timeleft, end-of-map paginated vote menus, and RPC exports.".to_string(),
        "std_restmenu" => "Freestanding C++20 weapon restriction menu (/amx_restmenu) hooking CanBuyItem & CanHavePlayerItem with persistent NVault storage.".to_string(),
        "std_statsx" => "Real-time /rank, /top15, /me, /hp, /statsme & MiscStats multi-kill announcements powered by O(log N) LeaderboardStore.".to_string(),
        "csdm" => "Counter-Strike Deathmatch: instant respawn with spawn protection, paginated primary/secondary weapon menus, and client VGUI2 HUD.".to_string(),
        "gungame" => "26-level GunGame weapon escalation arena with knife-steal demotions, turbo refill, level-up shockwave VFX, and victory celebration.".to_string(),
        "semiclip" => "Zero-jitter C++20 teammate semiclip hooking PlayersShouldCollide with proximity-based TransAlpha fade.".to_string(),
        "zombie_plague" => "Full Biohazard infection mode: first-zombie countdown, knockback/HP scaling, glowing auras, laser/shockwave VFX, and client biohazard HUD.".to_string(),
        "retakes" => "Competitive post-plant Bombsite A/B 3T vs 4CT retakes with pistol/force/full-buy weapon allocator menus, site DHUD callouts, and team rotation.".to_string(),
        _ => infer_summary_from_source(id, name, src),
    }
}

fn infer_summary_from_source(id: &str, name: &str, src: &str) -> String {
    for line in src.lines().take(12) {
        let t = line.trim();
        let cleaned = t
            .strip_prefix("//!")
            .or_else(|| t.strip_prefix("//"))
            .or_else(|| t.strip_prefix('#'))
            .map(str::trim);
        if let Some(comment) = cleaned
            && !comment.is_empty()
            && !comment.contains("SPDX")
            && !comment.starts_with("go:build")
            && !comment.starts_with("Copyright")
        {
            return comment.to_string();
        }
    }
    format!("Built-in cs16-rs ABI v2 plugin `{name}` (`{id}`).")
}

fn starter_python_equivalent(id: &str, name: &str) -> String {
    match id {
        "csdm" => r#"# Live Playground Starter Equivalent: Counter-Strike Deathmatch (CSDM)
from cs16 import mod, vfx, Color, HookAction, Armor, Menu

@mod.info(id="csdm_playground", name="CSDM Quick Respawn", version="1.0.0")
def _meta():
    pass

def on_gun_pick(player, info):
    player.strip_weapons(keep_knife=True)
    player.give_item(info)
    player.give_item("weapon_deagle")
    player.set_armor(100, Armor.VESTHELM)
    player.chat(f"{Color.GREEN}[CSDM]{Color.DEFAULT} Equipped {info.replace('weapon_', '').upper()}!")

@mod.chat_command("/guns", "!guns")
def cmd_guns(player, args):
    menu = Menu("\\yCSDM Weapon Menu", callback=on_gun_pick)
    menu.add_item("AK-47 + Deagle", "weapon_ak47")
    menu.add_item("M4A1 + Deagle", "weapon_m4a1")
    menu.add_item("AWP + Deagle", "weapon_awp")
    menu.show(player)
    return HookAction.SUPERCEDE

@mod.hook("player_killed", phase="post")
def on_killed(ev):
    victim = ev.victim
    mod.set_timer(1.5, lambda _arg=0: victim.respawn())
    if ev.killer and not ev.team_kill:
        ev.killer.set_health(min(ev.killer.health + 20, 100.0))
        vfx.shockwave_ring(ev.victim.origin, 150.0, (255, 190, 40, 220))
"#
        .to_string(),
        "gungame" => r#"# Live Playground Starter Equivalent: GunGame Weapon Ladder
from cs16 import mod, vfx, Color, HookAction, Armor

LADDER = ["weapon_glock18", "weapon_deagle", "weapon_mp5navy", "weapon_ak47", "weapon_m4a1", "weapon_awp", "weapon_knife"]
levels = {}

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    lvl = levels.get(ev.player.slot, 0)
    ev.player.strip_weapons(keep_knife=True)
    ev.player.give_item(LADDER[min(lvl, len(LADDER) - 1)])
    ev.player.set_armor(100, Armor.VESTHELM)

@mod.hook("player_killed", phase="post")
def on_kill(ev):
    if ev.killer and not ev.team_kill:
        lvl = min(levels.get(ev.killer.slot, 0) + 1, len(LADDER) - 1)
        levels[ev.killer.slot] = lvl
        ev.killer.strip_weapons(keep_knife=True)
        ev.killer.give_item(LADDER[lvl])
        vfx.shockwave_ring(ev.killer.origin, 180.0, (0, 220, 255, 230))
        ev.killer.dhud(f"GUNGAME LEVEL {lvl + 1}/{len(LADDER)}", y=0.22, rgb=(255, 210, 60))
"#
        .to_string(),
        "zombie_plague" => r#"# Live Playground Starter Equivalent: Zombie Plague Biohazard
from cs16 import mod, vfx, Color, HookAction, RenderFx, RenderMode

zombies = set()

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    if ev.player.slot in zombies:
        ev.player.set_max_health(600.0)
        ev.player.set_health(600.0)
        ev.player.set_maxspeed(290.0)
        ev.player.set_rendering(RenderFx.GLOW_SHELL, (60, 255, 80), RenderMode.NORMAL, 20)

@mod.chat_command("/infect")
def cmd_infect(player, args):
    zombies.add(player.slot)
    player.set_max_health(600.0)
    player.set_health(600.0)
    player.set_rendering(RenderFx.GLOW_SHELL, (60, 255, 80), RenderMode.NORMAL, 20)
    vfx.shockwave_ring(player.origin, 220.0, (60, 255, 80, 240))
    mod.dhud(f"{player.name} IS A BIOHAZARD ZOMBIE!", y=0.22, rgb=(80, 255, 90))
    return HookAction.SUPERCEDE
"#
        .to_string(),
        "semiclip" => r#"# Live Playground Starter Equivalent: Teammate Semiclip
from cs16 import mod, HookAction

@mod.hook("players_should_collide", phase="pre")
def on_collide(ev):
    if ev.player_a.team == ev.player_b.team:
        ev.should_collide = False
        return HookAction.OVERRIDE
    return HookAction.CONTINUE
"#
        .to_string(),
        "retakes" => r#"# Live Playground Starter Equivalent: Competitive Bombsite Retakes
from cs16 import mod, vfx, Color, HookAction, Armor, Team

@mod.hook("round_start", phase="post")
def on_round(ev):
    site = "A" if (ev.round_num % 2 == 1) else "B"
    mod.dhud(f"RETAKE BOMBSITE {site} — ROUND {ev.round_num}", y=0.2, rgb=(255, 176, 0))

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    p.set_armor(100, Armor.VESTHELM)
    p.give_item("weapon_ak47" if p.team == Team.T else "weapon_m4a1")
    p.give_item("weapon_hegrenade")
    if p.team == Team.CT:
        p.set_defuser(True)
"#
        .to_string(),
        _ => format!(
            r#"# Live Playground Starter Equivalent for {name} (`{id}`)
from cs16 import mod, vfx, Color, HookAction, Armor

@mod.info(id="{id}_py", name="{name} (Playground)", version="1.0.0")
def _meta():
    pass

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    ev.player.set_armor(100, Armor.VESTHELM)
    ev.player.chat(f"{{Color.GREEN}}[{name}]{{Color.DEFAULT}} Ready!")

@mod.hook("player_killed", phase="post")
def on_kill(ev):
    if ev.killer and not ev.team_kill:
        ev.killer.add_money(300, flash=True)
        vfx.aura_light(ev.killer.origin, radius=18, rgb=(255, 200, 50))
"#
        ),
    }
}

fn title_case_slug(raw: &str) -> String {
    raw.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut chs = part.chars();
            match chs.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chs.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn slugify_underscore(raw: &str) -> String {
    slugify(raw).replace('-', "_")
}

fn encode_plugins_json(plugins: &[DiscoveredPlugin]) -> String {
    let mut out = String::with_capacity(plugins.len() * 2048);
    out.push('[');
    for (idx, p) in plugins.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let mut src_json = String::from("[");
        for (si, sf) in p.source_files.iter().enumerate() {
            if si > 0 {
                src_json.push(',');
            }
            let _ = write!(
                src_json,
                "{{\"path\":\"{}\",\"content\":\"{}\"}}",
                json_escape(&sf.path),
                json_escape(&sf.content),
            );
        }
        src_json.push(']');

        let mut perms_json = String::from("[");
        for (pi, perm) in p.permissions.iter().enumerate() {
            if pi > 0 {
                perms_json.push(',');
            }
            let _ = write!(perms_json, "\"{}\"", json_escape(perm));
        }
        perms_json.push(']');

        let src_dl = match &p.source_download_url {
            Some(u) => format!("\"{}\"", json_escape(u)),
            None => "null".to_string(),
        };

        let _ = write!(
            out,
            "{{\"id\":\"{}\",\"name\":\"{}\",\"category\":\"{}\",\"category_label\":\"{}\",\"lang\":\"{}\",\"lang_key\":\"{}\",\"version\":\"{}\",\"side\":\"{}\",\"summary\":\"{}\",\"permissions\":{},\"source_files\":{},\"download_url\":\"{}\",\"source_download_url\":{},\"playground_compatible\":{},\"playground_code\":\"{}\"}}",
            json_escape(&p.id),
            json_escape(&p.name),
            json_escape(&p.category),
            json_escape(&p.category_label),
            json_escape(&p.lang),
            json_escape(&p.lang_key),
            json_escape(&p.version),
            json_escape(&p.side),
            json_escape(&p.summary),
            perms_json,
            src_json,
            json_escape(&p.download_url),
            src_dl,
            if p.playground_compatible {
                "true"
            } else {
                "false"
            },
            json_escape(&p.playground_code),
        );
    }
    out.push(']');
    out
}

fn render_mods_index_html(plugins: &[DiscoveredPlugin], plugins_json: &str) -> String {
    let total = plugins.len();
    let std_count = plugins.iter().filter(|p| p.category == "standard").count();
    let comm_count = plugins.iter().filter(|p| p.category == "community").count();
    let gm_count = plugins.iter().filter(|p| p.category == "gamemodes").count();
    let py_count = plugins.iter().filter(|p| p.lang_key == "py").count();
    let rust_count = plugins.iter().filter(|p| p.lang_key == "rust").count();
    let cpp_count = plugins.iter().filter(|p| p.lang_key == "cpp").count();
    let go_count = plugins.iter().filter(|p| p.lang_key == "go").count();

    let mut cards = String::with_capacity(plugins.len() * 900);
    for p in plugins {
        let perms_html: String = p
            .permissions
            .iter()
            .take(6)
            .map(|perm| {
                format!(
                    r#"<span class="pill pill-cap">{}</span>"#,
                    html_escape(perm)
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let extra_perms = if p.permissions.len() > 6 {
            format!(r#" <span class="pill">+{}</span>"#, p.permissions.len() - 6)
        } else {
            String::new()
        };
        let dl_label = if p.download_url.ends_with(".csmod") {
            "Download .csmod"
        } else if p.download_url.ends_with(".py") {
            "Download .py"
        } else {
            "Download Source"
        };
        let src_dl_btn = match &p.source_download_url {
            Some(url) => format!(
                r#"<a class="vgui-btn" href="{}" download>Source</a>"#,
                html_escape(url)
            ),
            None => String::new(),
        };
        let pg_badge = if p.playground_compatible {
            r#"<span class="pill pill-amber">Native Python Playground</span>"#
        } else {
            r#"<span class="pill">WASM + Python Starter</span>"#
        };
        let filter_txt = format!(
            "{} {} {} {} {} {}",
            p.id,
            p.name,
            p.category,
            p.lang,
            p.summary,
            p.permissions.join(" ")
        )
        .to_ascii_lowercase();

        let _ = write!(
            cards,
            r#"<article class="mod-card" data-mod-id="{id}" data-cat="{cat}" data-lang="{lang_key}" data-filter-text="{ft}">
  <div class="mod-card-head">
    <div>
      <h3>{name}</h3>
      <div class="mod-id-row"><code>{id}</code> &middot; v{ver} &middot; {side}</div>
    </div>
    <div class="mod-badges">
      <span class="pill pill-id">{lang}</span>
      <span class="pill">{cat_label}</span>
    </div>
  </div>
  <p class="mod-summary">{summary}</p>
  <div class="mod-perms">{perms_html}{extra_perms} {pg_badge}</div>
  <div class="mod-actions">
    <button type="button" class="vgui-btn" data-view-source="{id}">View Source ({files_cnt})</button>
    <a class="vgui-btn" href="{dl_url}" download>{dl_label}</a>
    {src_dl_btn}
    <a class="vgui-btn accent" href="../playground/?mod={id}" data-open-playground="{id}">&#9654; Open in Playground</a>
  </div>
</article>"#,
            id = html_escape(&p.id),
            cat = html_escape(&p.category),
            lang_key = html_escape(&p.lang_key),
            ft = html_escape(&filter_txt),
            name = html_escape(&p.name),
            ver = html_escape(&p.version),
            side = html_escape(&p.side),
            lang = html_escape(&p.lang),
            cat_label = html_escape(&p.category_label),
            summary = html_escape(&p.summary),
            files_cnt = p.source_files.len(),
            dl_url = html_escape(&p.download_url),
        );
    }

    let navbar = render_top_navbar("mods", "Plugin Hub · Built-in Showcase", false);
    // Safe inline JSON script tag (json_escape already escapes `<` and `>` as `\u003c`/`\u003e`).
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Plugin Hub ({total} Built-in Mods) — cs16-rs</title>
  <meta name="description" content="Browse, inspect source, download .py/.csmod bundles, and launch built-in cs16-rs plugins in the live browser Playground.">
  <link rel="icon" href="../favicon.ico" sizes="any">
  <link rel="icon" type="image/png" sizes="32x32" href="../icons/favicon-32.png">
  <link rel="apple-touch-icon" href="../icons/apple-touch-icon.png">
  <link rel="stylesheet" href="./mods.css">
</head>
<body>
  {navbar}
  <main class="mods-container">
    <section class="hero-panel">
      <div class="hero-kicker">POLYGLOT MODDING SHOWCASE &middot; PYTHON 3 · RUST · C++20 · GO</div>
      <h1>cs16-rs Built-in Plugin Hub</h1>
      <p class="hero-lead">Every Standard AMXX+ replacement, Community script, and competitive Gamemode plugin shipped in the repository — inspect full syntax-highlighted source, download ready-to-run <code>.py</code> / <code>.csmod</code> packages, or launch straight into the live browser Playground in 1 click.</p>
      <div class="stat-strip">
        <div class="stat-tile"><span class="stat-num">{total}</span><span class="stat-label">Total Plugins</span></div>
        <div class="stat-tile"><span class="stat-num">{std_count}</span><span class="stat-label">Standard Suite</span></div>
        <div class="stat-tile"><span class="stat-num">{comm_count}</span><span class="stat-label">Community (.py)</span></div>
        <div class="stat-tile"><span class="stat-num">{gm_count}</span><span class="stat-label">Gamemodes</span></div>
      </div>
    </section>

    <section class="doc-section">
      <div class="filter-bar">
        <input type="search" id="mods-search" class="vgui-input" placeholder="Search plugins by name, ID, permission, language, or feature (e.g. vip, csdm, semiclip, retakes, vfx)...">
        <div class="filter-chips-row">
          <div class="filter-chips" id="mods-cat-chips">
            <button type="button" class="cat-chip active" data-cat-filter="all">All ({total})</button>
            <button type="button" class="cat-chip" data-cat-filter="standard">Standard ({std_count})</button>
            <button type="button" class="cat-chip" data-cat-filter="community">Community ({comm_count})</button>
            <button type="button" class="cat-chip" data-cat-filter="gamemodes">Gamemodes ({gm_count})</button>
          </div>
          <div class="filter-chips" id="mods-lang-chips">
            <button type="button" class="cat-chip active" data-lang-filter="all">All Languages</button>
            <button type="button" class="cat-chip" data-lang-filter="py">Python 3 ({py_count})</button>
            <button type="button" class="cat-chip" data-lang-filter="rust">Rust ({rust_count})</button>
            <button type="button" class="cat-chip" data-lang-filter="cpp">C++20 ({cpp_count})</button>
            <button type="button" class="cat-chip" data-lang-filter="go">Go ({go_count})</button>
          </div>
        </div>
        <span class="filter-count" id="mods-count">Showing {total} of {total} plugins</span>
      </div>
    </section>

    <section class="mods-grid" id="mods-grid">
      {cards}
    </section>

    <footer class="portal-footer">
      <div>Catalog JSON: <a href="./plugins.json"><code>./plugins.json</code></a> &middot; Dynamically generated from <code>mods/</code></div>
      <div class="footer-nav">
        <a href="../">Play Web Client</a> &middot;
        <a href="../docs/">Docs &amp; API</a> &middot;
        <a href="../playground/">Live Playground</a> &middot;
        <a href="../admin.html">Admin</a>
      </div>
    </footer>
  </main>

  <div class="source-modal-backdrop hidden" id="source-modal" role="dialog" aria-modal="true" aria-label="Plugin Source Viewer">
    <div class="source-modal-dialog">
      <div class="source-modal-head">
        <div>
          <strong id="modal-mod-title" style="color:var(--amber)">Plugin Source</strong>
          <span id="modal-mod-meta" class="pill" style="margin-left:8px"></span>
        </div>
        <div class="pane-actions">
          <button type="button" class="vgui-btn" id="btn-modal-copy">Copy File</button>
          <a class="vgui-btn" id="btn-modal-download" href="#" download>Download</a>
          <a class="vgui-btn accent" id="btn-modal-playground" href="../playground/">&#9654; Open in Playground</a>
          <button type="button" class="vgui-btn" id="btn-modal-close">Close (Esc)</button>
        </div>
      </div>
      <div class="code-tab-bar" id="modal-file-tabs"></div>
      <pre class="source-modal-code"><code id="modal-code-view"></code></pre>
    </div>
  </div>

  <script id="plugins-bootstrap" type="application/json">{plugins_json}</script>
  <script src="./mods.js"></script>
</body>
</html>"##
    )
}

fn write_text(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|e| io_err(path, &e))
}

fn io_err(path: &Path, e: &std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

const MODS_EXTRA_CSS: &str = r##"
.mods-container {
  max-width: 1360px;
  margin: 0 auto;
  padding: 18px 20px 36px 20px;
}

.filter-chips-row {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 8px;
}

.mods-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(380px, 1fr));
  gap: 14px;
}

.mod-card {
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  gap: 10px;
  background: var(--panel);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 14px;
}

.mod-card-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 8px;
  border-bottom: 1px solid var(--border-dark);
  padding-bottom: 8px;
}

.mod-card-head h3 { margin: 0 0 3px 0; color: var(--amber); font-size: 15px; }
.mod-id-row { font-size: 11px; color: var(--muted); }
.mod-badges { display: flex; flex-wrap: wrap; gap: 4px; justify-content: flex-end; }
.mod-summary { font-size: 12.5px; color: var(--text-bright); margin: 0; }
.mod-perms { display: flex; flex-wrap: wrap; gap: 4px; }
.mod-actions { display: flex; flex-wrap: wrap; gap: 6px; padding-top: 6px; border-top: 1px solid var(--border-dark); }

/* Tabbed Source Viewer Modal */
.source-modal-backdrop {
  position: fixed;
  inset: 0;
  z-index: 220;
  background: rgba(10, 13, 9, 0.84);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
}
.source-modal-backdrop.hidden { display: none; }

.source-modal-dialog {
  width: 980px;
  max-width: calc(100vw - 24px);
  max-height: 88vh;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border-top: 2px solid var(--border-light);
  border-left: 2px solid var(--border-light);
  border-right: 2px solid var(--border-dark);
  border-bottom: 2px solid var(--border-dark);
}

.source-modal-head {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  background: var(--panel-alt);
  border-bottom: 1px solid var(--border-dark);
  padding: 8px 12px;
}

.source-modal-code {
  flex: 1;
  overflow: auto;
  background: var(--terminal);
  color: #e4ecd9;
  padding: 14px 16px;
  font-size: 12px;
  line-height: 1.5;
  margin: 0;
}

.tok-kw { color: #e2c060; font-weight: bold; }
.tok-str { color: #96d682; }
.tok-cm { color: #7e8d77; font-style: italic; }
.tok-num { color: #84bfff; }

@media (max-width: 520px) {
  .mods-grid { grid-template-columns: minmax(0, 1fr); }
}
"##;

const MODS_JS: &str = r##"(function () {
  "use strict";

  let plugins = [];
  const byId = new Map();

  function loadBootstrap() {
    const el = document.getElementById("plugins-bootstrap");
    if (el && el.textContent) {
      try {
        const parsed = JSON.parse(el.textContent);
        if (Array.isArray(parsed)) setPlugins(parsed);
      } catch (_) {}
    }
    if (!plugins.length) {
      fetch("./plugins.json")
        .then((r) => r.json())
        .then((data) => { if (Array.isArray(data)) setPlugins(data); })
        .catch(() => {});
    }
  }

  function setPlugins(list) {
    plugins = list;
    byId.clear();
    for (const p of list) byId.set(p.id, p);
  }

  function stagePlaygroundImport(plugin) {
    if (!plugin) return;
    const code =
      plugin.playground_compatible && plugin.source_files && plugin.source_files.length
        ? plugin.source_files[0].content
        : plugin.playground_code || "";
    try {
      localStorage.setItem(
        "cs16_playground_import",
        JSON.stringify({ id: plugin.id, name: plugin.name, code })
      );
    } catch (_) {}
  }

  function escapeHtml(raw) {
    return raw
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function highlightSource(raw, filePath) {
    const ext = (filePath || "").split(".").pop().toLowerCase();
    const kwSets = {
      py: /\b(def|class|from|import|return|if|elif|else|for|in|while|not|and|or|None|True|False|global|pass|lambda)\b/g,
      rs: /\b(pub|fn|let|mut|if|else|match|for|in|return|use|struct|enum|impl|const|static|true|false|Some|None)\b/g,
      cpp: /\b(int32_t|uint8_t|float|bool|void|const|constexpr|static|auto|return|if|else|for|class|struct|namespace|true|false)\b/g,
      go: /\b(package|import|func|var|const|type|struct|return|if|else|for|range|true|false|nil)\b/g,
      toml: /\b(true|false)\b/g,
    };
    const kwRe = kwSets[ext] || kwSets.py;
    const isHashComment = ext === "py" || ext === "toml";

    return raw
      .split(/\r?\n/)
      .map((line) => {
        const trimmed = line.trim();
        if ((isHashComment && trimmed.startsWith("#")) || (!isHashComment && trimmed.startsWith("//"))) {
          return `<span class="tok-cm">${escapeHtml(line)}</span>`;
        }
        let html = escapeHtml(line);
        html = html.replace(/(&quot;|")([^"\\]*(\\.[^"\\]*)*)(")/g, '<span class="tok-str">$1$2$4</span>');
        html = html.replace(kwRe, '<span class="tok-kw">$1</span>');
        html = html.replace(/\b(\d+(?:\.\d+)?)\b/g, '<span class="tok-num">$1</span>');
        return html;
      })
      .join("\n");
  }

  function initFilters() {
    const searchEl = document.getElementById("mods-search");
    const countEl = document.getElementById("mods-count");
    const cards = Array.from(document.querySelectorAll(".mod-card"));
    let activeCat = "all";
    let activeLang = "all";

    function apply() {
      const q = (searchEl && searchEl.value ? searchEl.value : "").trim().toLowerCase();
      let shown = 0;
      for (const card of cards) {
        const catOk = activeCat === "all" || card.dataset.cat === activeCat;
        const langOk = activeLang === "all" || card.dataset.lang === activeLang;
        let qOk = !q || (card.dataset.filterText || "").includes(q);
        if (!qOk && q) {
          const p = byId.get(card.dataset.modId);
          if (p && p.source_files) {
            qOk = p.source_files.some((f) => (f.content || "").toLowerCase().includes(q));
          }
        }
        const vis = catOk && langOk && qOk;
        card.style.display = vis ? "" : "none";
        if (vis) shown++;
      }
      if (countEl) countEl.textContent = `Showing ${shown} of ${cards.length} plugins`;
    }

    if (searchEl) searchEl.addEventListener("input", apply);
    document.querySelectorAll("[data-cat-filter]").forEach((btn) => {
      btn.addEventListener("click", () => {
        activeCat = btn.dataset.catFilter || "all";
        document.querySelectorAll("[data-cat-filter]").forEach((b) => b.classList.toggle("active", b === btn));
        apply();
      });
    });
    document.querySelectorAll("[data-lang-filter]").forEach((btn) => {
      btn.addEventListener("click", () => {
        activeLang = btn.dataset.langFilter || "all";
        document.querySelectorAll("[data-lang-filter]").forEach((b) => b.classList.toggle("active", b === btn));
        apply();
      });
    });
  }

  function initSourceModalAndPlayground() {
    const modal = document.getElementById("source-modal");
    const titleEl = document.getElementById("modal-mod-title");
    const metaEl = document.getElementById("modal-mod-meta");
    const tabsEl = document.getElementById("modal-file-tabs");
    const codeEl = document.getElementById("modal-code-view");
    const copyBtn = document.getElementById("btn-modal-copy");
    const dlBtn = document.getElementById("btn-modal-download");
    const pgBtn = document.getElementById("btn-modal-playground");
    const closeBtn = document.getElementById("btn-modal-close");

    let currentPlugin = null;
    let currentContent = "";

    function selectFile(idx) {
      if (!currentPlugin || !currentPlugin.source_files || !currentPlugin.source_files[idx]) return;
      const sf = currentPlugin.source_files[idx];
      currentContent = sf.content || "";
      codeEl.innerHTML = highlightSource(currentContent, sf.path);
      tabsEl.querySelectorAll(".code-tab-btn").forEach((b, i) => b.classList.toggle("active", i === idx));
    }

    function openSourceModal(id) {
      const p = byId.get(id);
      if (!p || !modal) return;
      currentPlugin = p;
      titleEl.textContent = `${p.name} (${p.id})`;
      metaEl.textContent = `${p.lang} · ${p.category_label} · v${p.version}`;
      dlBtn.href = p.download_url;
      pgBtn.href = "../playground/?mod=" + encodeURIComponent(p.id);

      tabsEl.textContent = "";
      (p.source_files || []).forEach((sf, idx) => {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "code-tab-btn" + (idx === 0 ? " active" : "");
        btn.textContent = sf.path;
        btn.addEventListener("click", () => selectFile(idx));
        tabsEl.appendChild(btn);
      });
      selectFile(0);
      modal.classList.remove("hidden");
    }

    function closeModal() {
      if (modal) modal.classList.add("hidden");
    }

    document.addEventListener("click", (ev) => {
      const viewBtn = ev.target.closest("[data-view-source]");
      if (viewBtn) {
        openSourceModal(viewBtn.dataset.viewSource);
        return;
      }
      const pgLink = ev.target.closest("[data-open-playground]");
      if (pgLink) {
        stagePlaygroundImport(byId.get(pgLink.dataset.openPlayground));
      }
    });

    if (pgBtn) {
      pgBtn.addEventListener("click", () => stagePlaygroundImport(currentPlugin));
    }
    if (copyBtn) {
      copyBtn.addEventListener("click", () => {
        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(currentContent).then(() => {
            copyBtn.textContent = "Copied!";
            setTimeout(() => { copyBtn.textContent = "Copy File"; }, 1400);
          }).catch(() => {});
        }
      });
    }
    if (closeBtn) closeBtn.addEventListener("click", closeModal);
    if (modal) {
      modal.addEventListener("click", (ev) => {
        if (ev.target === modal) closeModal();
      });
    }
    document.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape" && modal && !modal.classList.contains("hidden")) closeModal();
    });
  }

  document.addEventListener("DOMContentLoaded", () => {
    loadBootstrap();
    initFilters();
    initSourceModalAndPlayground();
  });
})();
"##;
