//! Static GitHub Pages `/docs/` Modding & Engine Developer Portal generator for `cs16-rs`.
//!
//! Invoked automatically by `cargo xtask web-build` (and standalone `cargo xtask docs-build`)
//! so `dist/web/docs/` deploys directly to GitHub Pages AND serves cleanly from
//! `cs-server --web-dir dist/web`.
//!
//! All reference content is introspected directly from live workspace sources:
//! - Every [`cs_mod_api::HookId`] variant (52 hooks across GameDll, ReAPI Gameplay, EngineApi)
//! - Every [`cs_mod_api::Permission`] capability and [`cs_mod_api::AdminFlags`] RBAC flag
//! - Python SDK classes, methods, and signatures from `sdk/python/cs16.pyi`
//! - AMX Mod X native dispatches from `crates/cs-amxx/src/natives.rs` and
//!   [`cs_mod_host::amxx_import::STOCK_AMXX_PLUGINS`]
//! - Every CVar and console command from `docs/parity/cvars.toml`, cross-referenced with
//!   [`cs_cvar::CVarRegistry::new_with_defaults`] and [`cs_cvar::CVarRegistry::known_command_names`]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use cs_cvar::{CVarFlags, CVarRegistry};
use cs_mod_api::{
    ABI_VERSION, AdminFlags, DEFAULT_FUEL_PER_FRAME, DEFAULT_MEMORY_BYTES, HookId,
    MAX_FUEL_PER_FRAME, MAX_MEMORY_CAP, Permission,
};
use cs_mod_host::amxx_import::STOCK_AMXX_PLUGINS;

use crate::parity;

/// Summary metrics returned by [`generate_docs_portal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocsPortalStats {
    pub pages: usize,
    pub hooks: usize,
    pub api_methods: usize,
    pub cvars: usize,
    pub amxx_natives: usize,
    pub search_entries: usize,
}

/// Resolves the repository workspace root whether invoked from the repo root or `xtask/`.
pub(crate) fn workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join("docs/parity/cvars.toml").exists() && cwd.join("crates/cs-mod-api").exists() {
        return cwd;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(cwd)
}

/// Generates the full `/docs/` portal into `<out_dir>/docs/` using the auto-detected workspace root.
pub fn generate_docs_portal(out_dir: &Path) -> Result<DocsPortalStats, String> {
    let root = workspace_root();
    generate_docs_portal_with_root(&root, out_dir)
}

/// Generates the full `/docs/` portal into `<out_dir>/docs/` reading sources from `workspace_root`.
pub fn generate_docs_portal_with_root(
    workspace_root: &Path,
    out_dir: &Path,
) -> Result<DocsPortalStats, String> {
    let docs_dir = out_dir.join("docs");
    fs::create_dir_all(&docs_dir).map_err(|e| io_err(&docs_dir, &e))?;

    let hooks = collect_all_hooks();
    let py_api = introspect_python_stub(workspace_root)?;
    let api_groups = build_api_groups(&py_api);
    let api_method_count: usize = api_groups.iter().map(|g| g.entries.len()).sum();
    let amxx_natives = introspect_amxx_natives(workspace_root)?;
    let cvars = introspect_cvars(workspace_root)?;

    let mut search_entries = Vec::new();
    collect_guide_search_entries(&mut search_entries);
    for h in &hooks {
        search_entries.push(SearchEntry {
            title: format!("{} ({})", h.pascal, h.snake),
            category: "hook",
            url: format!("./hooks.html#hook-{}", h.snake),
            summary: format!(
                "[ID {} · {} · {}] {}",
                h.id, h.category, h.phases, h.summary
            ),
            keywords: format!("{} {} {} {}", h.snake, h.pascal, h.amxx_equiv, h.ctx_struct),
        });
    }
    for group in &api_groups {
        for m in &group.entries {
            search_entries.push(SearchEntry {
                title: format!("{}.{}", group.title, m.name),
                category: "api",
                url: format!("./api.html#api-{}", m.anchor),
                summary: format!("[cap: {}] {} — {}", m.capability, m.py_sig, m.summary),
                keywords: format!(
                    "{} {} {} {} {}",
                    m.name, m.py_sig, m.rust_sig, m.cpp_sig, m.go_sig
                ),
            });
        }
    }
    for n in &amxx_natives {
        search_entries.push(SearchEntry {
            title: n.native.clone(),
            category: "amxx",
            url: format!("./amxx-migration.html#amxx-{}", slugify(&n.native)),
            summary: format!("[{}] -> {} / {}", n.module, n.py_equiv, n.rust_equiv),
            keywords: format!("{} {} {} {}", n.native, n.module, n.py_equiv, n.rust_equiv),
        });
    }
    for c in &cvars {
        search_entries.push(SearchEntry {
            title: c.id.clone(),
            category: if c.kind == "command" {
                "command"
            } else {
                "cvar"
            },
            url: format!("./cvars-deployment.html#cvar-{}", slugify(&c.id)),
            summary: if c.default_value.is_empty() {
                format!("[{} · {}] {}", c.kind, c.category, c.description)
            } else {
                format!(
                    "[default: {} · {}] {}",
                    c.default_value, c.category, c.description
                )
            },
            keywords: format!("{} {} {}", c.id, c.category, c.flags.join(" ")),
        });
    }

    let index_html = render_index_page(
        hooks.len(),
        api_method_count,
        cvars.len(),
        amxx_natives.len(),
    );
    let hooks_html = render_hooks_page(&hooks);
    let api_html = render_api_page(&api_groups, &py_api);
    let vfx_html = render_vfx_menus_page();
    let amxx_html = render_amxx_page(&amxx_natives);
    let cvars_html = render_cvars_deployment_page(&cvars);
    let search_json = encode_search_index_json(&search_entries);
    let cvars_json = encode_cvars_json(&cvars);

    write_text(&docs_dir.join("docs.css"), PORTAL_CSS)?;
    write_text(&docs_dir.join("docs.js"), DOCS_JS)?;
    write_text(&docs_dir.join("vfx-designer.js"), VFX_DESIGNER_JS)?;
    write_text(&docs_dir.join("amxx-converter.js"), AMXX_CONVERTER_JS)?;
    write_text(&docs_dir.join("search-index.json"), &search_json)?;
    write_text(&docs_dir.join("cvars-data.json"), &cvars_json)?;
    write_text(&docs_dir.join("index.html"), &index_html)?;
    write_text(&docs_dir.join("hooks.html"), &hooks_html)?;
    write_text(&docs_dir.join("api.html"), &api_html)?;
    write_text(&docs_dir.join("vfx-menus.html"), &vfx_html)?;
    write_text(&docs_dir.join("amxx-migration.html"), &amxx_html)?;
    write_text(&docs_dir.join("cvars-deployment.html"), &cvars_html)?;

    Ok(DocsPortalStats {
        pages: 6,
        hooks: hooks.len(),
        api_methods: api_method_count,
        cvars: cvars.len(),
        amxx_natives: amxx_natives.len(),
        search_entries: search_entries.len(),
    })
}

fn write_text(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|e| io_err(path, &e))
}

fn io_err(path: &Path, e: &std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

/// Escapes special HTML characters for safe embedding in text nodes and double-quoted attributes.
pub(crate) fn html_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes a string as a JSON string literal (without outer quotes).
pub(crate) fn json_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 8);
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "item".to_string()
    } else {
        out
    }
}

/// Renders the shared Steam-2003 VGUI2 top navigation bar across `/docs/*` and `/mods/*`.
pub(crate) fn render_top_navbar(active: &str, subtitle: &str, has_search: bool) -> String {
    let links = [
        ("play", "../", "Play"),
        ("docs", "../docs/", "Docs &amp; API"),
        ("playground", "../playground/", "Live Playground"),
        ("mods", "../mods/", "Plugin Hub"),
        ("admin", "../admin.html", "Server Admin"),
    ];
    let mut nav_links = String::new();
    for (key, href, label) in links {
        let cls = if key == active {
            "vgui-nav-link active"
        } else {
            "vgui-nav-link"
        };
        let _ = write!(nav_links, r#"<a class="{cls}" href="{href}">{label}</a>"#);
    }
    let search_btn = if has_search {
        r#"<button type="button" class="vgui-btn search-trigger" id="btn-open-search" title="Search docs, hooks, API, 736 CVars &amp; AMXX natives (Ctrl+K or /)">Search <kbd>Ctrl+K</kbd></button>"#
    } else {
        r#"<a class="vgui-btn search-trigger" href="../docs/#search" title="Open full Docs &amp; API search">Search Docs</a>"#
    };
    let drawer_btn = if active == "docs" {
        r#"<button type="button" class="vgui-btn mobile-drawer-btn" id="btn-toggle-drawer" aria-label="Toggle docs navigation">&#9776; Menu</button>"#
    } else {
        ""
    };
    format!(
        r#"<header class="portal-navbar">
  <div class="navbar-left">
    {drawer_btn}
    <a class="brand-home" href="../">
      <img class="brand-logo" src="../icons/icon-192.png" alt="cs16-rs" width="24" height="24">
      <span class="brand-name">cs16-rs</span>
      <span class="brand-sub">{sub}</span>
    </a>
  </div>
  <nav class="navbar-links" aria-label="Primary portal navigation">{nav_links}</nav>
  <div class="navbar-right">
    {search_btn}
    <a class="vgui-btn" href="https://github.com/maybewewill/cs16-rs" target="_blank" rel="noopener">GitHub</a>
  </div>
</header>"#,
        sub = html_escape(subtitle),
    )
}

fn render_docs_shell(
    page_slug: &str,
    title: &str,
    description: &str,
    toc: &[(&str, &str)],
    body_html: &str,
    extra_scripts: &[&str],
) -> String {
    let nav_items = [
        (
            "index",
            "./index.html",
            "Overview & Quickstart",
            "60s setup, security, RPC, replay",
        ),
        (
            "hooks",
            "./hooks.html",
            "Hooks Encyclopedia (52)",
            "Pre/Post chains & overrides",
        ),
        (
            "api",
            "./api.html",
            "Host & SDK API Reference",
            "Player, World, VFX, KV, RBAC",
        ),
        (
            "vfx-menus",
            "./vfx-menus.html",
            "VFX, HUD & Menu Designer",
            "Interactive GoldSrc visual studio",
        ),
        (
            "amxx-migration",
            "./amxx-migration.html",
            "AMX Mod X Migration & .sma",
            "Natives table & live converter",
        ),
        (
            "cvars-deployment",
            "./cvars-deployment.html",
            "736 CVars & Deployment",
            "Console index, Docker, HTTP/3",
        ),
    ];
    let mut sidebar = String::new();
    for (slug, href, label, hint) in nav_items {
        let active_cls = if slug == page_slug { " active" } else { "" };
        let _ = write!(
            sidebar,
            r#"<a class="side-link{active_cls}" href="{href}"><span class="side-title">{label}</span><span class="side-hint">{hint}</span></a>"#
        );
    }

    let mut toc_html = String::new();
    for (id, label) in toc {
        let _ = write!(
            toc_html,
            r##"<a class="toc-link" href="#{id}">{label}</a>"##,
            id = html_escape(id),
            label = html_escape(label),
        );
    }

    let mut scripts_html = String::from(r#"<script src="./docs.js"></script>"#);
    for s in extra_scripts {
        let _ = write!(scripts_html, "\n  <script src=\"{s}\"></script>");
    }

    let navbar = render_top_navbar("docs", "Developer Portal · ABI v2", true);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{title} — cs16-rs Developer Portal</title>
  <meta name="description" content="{desc}">
  <link rel="icon" href="../favicon.ico" sizes="any">
  <link rel="icon" type="image/png" sizes="32x32" href="../icons/favicon-32.png">
  <link rel="apple-touch-icon" href="../icons/apple-touch-icon.png">
  <link rel="stylesheet" href="./docs.css">
</head>
<body data-page="{slug}">
  {navbar}
  <div class="portal-layout">
    <aside class="portal-sidebar" id="portal-sidebar" aria-label="Documentation sections">
      <div class="sidebar-group-title">Documentation</div>
      <nav class="sidebar-nav">{sidebar}</nav>
      <div class="sidebar-group-title">Language Preference</div>
      <div class="lang-switcher-box">
        <div class="lang-hint">Synced across all code examples:</div>
        <div class="global-lang-pills" role="group" aria-label="Global code language">
          <button type="button" class="lang-pill" data-set-lang="py">Python 3</button>
          <button type="button" class="lang-pill" data-set-lang="rust">Rust</button>
          <button type="button" class="lang-pill" data-set-lang="cpp">C++20</button>
          <button type="button" class="lang-pill" data-set-lang="go">Go</button>
          <button type="button" class="lang-pill" data-set-lang="amxx">AMXX</button>
        </div>
      </div>
      <div class="sidebar-group-title">Interactive Tools</div>
      <div class="sidebar-quick-links">
        <a class="vgui-btn full-btn accent" href="../playground/">&#9654; Live Browser Playground</a>
        <a class="vgui-btn full-btn" href="../mods/">Browse Built-in Plugin Hub</a>
      </div>
    </aside>

    <main class="portal-main">
      {body_html}
      <footer class="portal-footer">
        <div>cs16-rs ABI v{abi} &middot; 100% <code>#![forbid(unsafe_code)]</code> &middot; Auto-verified from workspace sources</div>
        <div class="footer-nav">
          <a href="../">Play Web Client</a> &middot;
          <a href="../playground/">Playground</a> &middot;
          <a href="../mods/">Plugin Hub</a> &middot;
          <a href="../admin.html">Admin</a>
        </div>
      </footer>
    </main>

    <aside class="portal-toc" aria-label="On this page">
      <div class="toc-title">On This Page</div>
      <nav class="toc-nav" id="toc-nav">{toc_html}</nav>
    </aside>
  </div>

  <div class="search-backdrop hidden" id="search-modal" role="dialog" aria-modal="true" aria-label="Search documentation">
    <div class="search-dialog">
      <div class="search-header">
        <input type="search" id="search-input" placeholder="Search guides, 52 hooks, SDK API, 736 CVars, AMXX natives..." autocomplete="off">
        <button type="button" class="vgui-btn" id="btn-close-search">Esc</button>
      </div>
      <div class="search-cats" id="search-cats">
        <button type="button" class="cat-chip active" data-search-cat="all">All</button>
        <button type="button" class="cat-chip" data-search-cat="guide">Guides</button>
        <button type="button" class="cat-chip" data-search-cat="hook">Hooks</button>
        <button type="button" class="cat-chip" data-search-cat="api">API</button>
        <button type="button" class="cat-chip" data-search-cat="cvar">CVars</button>
        <button type="button" class="cat-chip" data-search-cat="command">Commands</button>
        <button type="button" class="cat-chip" data-search-cat="amxx">AMXX</button>
      </div>
      <div class="search-meta" id="search-meta">Type to search across the entire portal...</div>
      <div class="search-results" id="search-results"></div>
    </div>
  </div>

  {scripts_html}
</body>
</html>"#,
        title = html_escape(title),
        desc = html_escape(description),
        slug = html_escape(page_slug),
        abi = ABI_VERSION,
    )
}

fn render_code_tabs(tabs: &[(&str, &str, &str)]) -> String {
    let mut buttons = String::new();
    let mut panels = String::new();
    for (idx, (lang_key, label, code)) in tabs.iter().enumerate() {
        let active = if idx == 0 { " active" } else { "" };
        let _ = write!(
            buttons,
            r#"<button type="button" class="code-tab-btn{active}" data-lang="{lang_key}">{label}</button>"#
        );
        let _ = write!(
            panels,
            r#"<div class="code-panel{active}" data-lang="{lang_key}"><button type="button" class="copy-btn" title="Copy code">Copy</button><pre><code>{code}</code></pre></div>"#,
            code = html_escape(code),
        );
    }
    format!(r#"<div class="code-tabs"><div class="code-tab-bar">{buttons}</div>{panels}</div>"#)
}

fn render_single_code(label: &str, code: &str) -> String {
    format!(
        r#"<div class="code-box"><div class="code-box-head"><span>{label}</span><button type="button" class="copy-btn">Copy</button></div><pre><code>{code}</code></pre></div>"#,
        label = html_escape(label),
        code = html_escape(code),
    )
}

// ===========================================================================
// 1. Exhaustive Hook Introspection (`cs_mod_api::HookId` — all 52 variants)
// ===========================================================================

struct HookDoc {
    id: u32,
    pascal: &'static str,
    snake: &'static str,
    category: &'static str,
    cat_key: &'static str,
    phases: &'static str,
    mutable_pre: bool,
    ctx_struct: &'static str,
    amxx_equiv: &'static str,
    summary: &'static str,
    args: Vec<(&'static str, &'static str, &'static str)>,
    override_note: &'static str,
    py_code: String,
    rust_code: String,
    cpp_code: String,
    go_code: String,
    amxx_code: String,
}

const ALL_HOOK_IDS: [HookId; 52] = [
    // GameDll (18)
    HookId::ServerActivate,
    HookId::ServerDeactivate,
    HookId::SpawnEntity,
    HookId::ClientConnect,
    HookId::ClientPutInServer,
    HookId::ClientDisconnect,
    HookId::ClientCommand,
    HookId::ClientUserinfoChanged,
    HookId::RunUsercmd,
    HookId::PostUsercmd,
    HookId::StartFrame,
    HookId::Think,
    HookId::Touch,
    HookId::Blocked,
    HookId::UpdateClientData,
    HookId::GetWeaponData,
    HookId::ServerCommand,
    HookId::AddToFullPack,
    // Gameplay / ReAPI (26)
    HookId::PlayerDeath,
    HookId::RoundStart,
    HookId::PlayerSpawn,
    HookId::PlayerTakeDamage,
    HookId::PlayerKilled,
    HookId::PlayerPreThink,
    HookId::PlayerPostThink,
    HookId::PlayerJump,
    HookId::PlayersShouldCollide,
    HookId::CanBuyItem,
    HookId::CanHavePlayerItem,
    HookId::WeaponDeploy,
    HookId::WeaponPrimaryAttack,
    HookId::WeaponReload,
    HookId::PlayerSay,
    HookId::MenuSelect,
    HookId::FreezeTimeEnd,
    HookId::RoundEnd,
    HookId::BombPlanting,
    HookId::BombPlanted,
    HookId::BombDefusing,
    HookId::BombDefused,
    HookId::BombExploded,
    HookId::HostageTouched,
    HookId::HostageRescued,
    HookId::GrenadeThrow,
    // EngineApi (8)
    HookId::EngEmitSound,
    HookId::EngSendUserMsg,
    HookId::EngClientPrint,
    HookId::EngCenterPrint,
    HookId::EngCreateEntity,
    HookId::EngRemoveEntity,
    HookId::EngKick,
    HookId::EngChangeLevel,
];

fn collect_all_hooks() -> Vec<HookDoc> {
    ALL_HOOK_IDS.iter().copied().map(describe_hook).collect()
}

fn describe_hook(hook: HookId) -> HookDoc {
    let id = hook as u32;
    let snake = hook.as_name();
    // Exhaustive match guarantees compile-time failure if any new HookId variant is added.
    let (
        pascal,
        category,
        cat_key,
        phases,
        mutable_pre,
        ctx_struct,
        amxx_equiv,
        summary,
        args,
        override_note,
        py_body,
        rust_body,
        cpp_body,
        go_body,
        amxx_body,
    ) = match hook {
        HookId::ServerActivate => (
            "ServerActivate",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            false,
            "ServerActivateArgs",
            "plugin_cfg / FM_ServerActivate",
            "Fired once when a map finishes loading and edicts are ready.",
            vec![
                (
                    "arg_i32(0) / ev.max_clients",
                    "i32",
                    "Server max player slots (1..=32)",
                ),
                (
                    "arg_str(0) / ev.map_name",
                    "str",
                    "Loaded BSP map name (e.g. de_dust2)",
                ),
            ],
            "Observation hook; use to precache sounds/models or load map-specific configs.",
            "mod.log(f'Map {ev.map_name} active for {ev.max_clients} slots')",
            "let map = host.hook_arg_str(0).unwrap_or_default();\n    host.print_all(&format!(\"Loaded {map}\"));\n    HookAction::Continue",
            "cs16::server::print_all(\"Map activated\");\n    return cs16::HookAction::Continue;",
            "cs16mod.PrintAll(\"Map activated\")\n    return cs16mod.Continue",
            "public plugin_cfg() {\n    server_print(\"Map activated\");\n}",
        ),
        HookId::ServerDeactivate => (
            "ServerDeactivate",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            false,
            "EmptyCtx",
            "plugin_end / FM_ServerDeactivate",
            "Fired right before map change or server shutdown to flush persistent state.",
            vec![("—", "none", "No hook arguments; flush storage/leaderboards")],
            "Non-cancellable shutdown lifecycle notification.",
            "mod.storage.set('last_map', mod.map_name)",
            "host.storage_set(\"last_map\", \"saved\");\n    HookAction::Continue",
            "cs16::storage::set(\"last_map\", \"saved\");\n    return cs16::HookAction::Continue;",
            "cs16mod.StorageSet(\"last_map\", \"saved\")\n    return cs16mod.Continue",
            "public plugin_end() {\n    nvault_close(g_vault);\n}",
        ),
        HookId::SpawnEntity => (
            "SpawnEntity",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "EntitySpawnArgs",
            "FM_Spawn / Ham_Spawn",
            "Fired when any world or dynamic entity spawns during map load or runtime.",
            vec![
                (
                    "arg_i32(0) / ev.ent_index",
                    "u16",
                    "1-based entity edict index",
                ),
                (
                    "arg_str(0) / ev.classname",
                    "str",
                    "Entity classname (e.g. armoury_entity)",
                ),
            ],
            "Returning Supercede in Pre prevents the entity from spawning.",
            "if ev.classname == 'armoury_entity':\n        return HookAction.SUPERCEDE",
            "if host.hook_arg_str(0).as_deref() == Some(\"armoury_entity\") {\n        return HookAction::Supercede;\n    }\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_Spawn(ent) {\n    return FMRES_IGNORED;\n}",
        ),
        HookId::ClientConnect => (
            "ClientConnect",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "ClientSlotArgs",
            "client_connect",
            "Fired when a client initiates connection; Pre can reject with Supercede.",
            vec![
                ("arg_i32(0) / ev.slot", "u8", "0-based player slot (0..31)"),
                ("arg_str(0)", "str", "Client name or IP address"),
            ],
            "Return Supercede in Pre to reject the connection (e.g. reserved slot / ban check).",
            "mod.log(f'Client connecting on slot {ev.slot}')",
            "let _slot = host.hook_arg_i32(0) as u8;\n    HookAction::Continue",
            "auto slot = cs16_hook_arg_i32(0);\n    return cs16::HookAction::Continue;",
            "_ = cs16mod.HookArgI32(0)\n    return cs16mod.Continue",
            "public client_connect(id) {\n    return PLUGIN_CONTINUE;\n}",
        ),
        HookId::ClientPutInServer => (
            "ClientPutInServer",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            false,
            "ClientSlotArgs",
            "client_putinserver",
            "Fired once a client finishes sign-on and enters the server.",
            vec![(
                "arg_i32(0) / ev.player",
                "u8 / Player",
                "Connected player slot",
            )],
            "Ideal for greeting chat/MOTD messages and loading player stats.",
            "ev.player.chat(f'{Color.GREEN}[Welcome]{Color.DEFAULT} Hello {ev.player.name}!')",
            "let p = Player::new(host.hook_arg_i32(0) as u8);\n    p.say_text(host, \"\\x04[Welcome]\\x01 GL HF!\");\n    HookAction::Continue",
            "cs16::Player(cs16_hook_arg_i32(0)).say_text(\"\\x04[Welcome]\\x01 GL HF!\");\n    return cs16::HookAction::Continue;",
            "cs16mod.NewPlayer(cs16mod.HookArgI32(0)).SayText(\"\\x04[Welcome]\\x01 GL HF!\")\n    return cs16mod.Continue",
            "public client_putinserver(id) {\n    client_print(id, print_chat, \"Welcome!\");\n}",
        ),
        HookId::ClientDisconnect => (
            "ClientDisconnect",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            false,
            "ClientSlotArgs",
            "client_disconnected",
            "Fired when a player disconnects or is kicked; clean up per-slot state here.",
            vec![(
                "arg_i32(0) / ev.slot",
                "u8",
                "Disconnecting player slot (0..31)",
            )],
            "Always runs; clear per-slot timers, flags, and cached references.",
            "mod.storage.set(f'seen_{ev.slot}', '1')",
            "let _slot = host.hook_arg_i32(0) as u8;\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public client_disconnected(id) {\n    g_score[id] = 0;\n}",
        ),
        HookId::ClientCommand => (
            "ClientCommand",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "ClientCommandArgs",
            "client_command / register_clcmd",
            "Fired on any client console command (e.g. drop, radio1, amx_kick, custom binds).",
            vec![
                (
                    "arg_i32(0) / ev.player",
                    "u8 / Player",
                    "Issuing player slot",
                ),
                ("arg_str(0) / ev.command", "str", "Command verb"),
                ("arg_str(1) / ev.args", "str", "Trailing command arguments"),
            ],
            "Return Supercede in Pre to intercept and suppress vanilla command execution.",
            "if ev.command == 'drop':\n        return HookAction.SUPERCEDE",
            "if host.hook_arg_str(0).as_deref() == Some(\"drop\") {\n        return HookAction::Supercede;\n    }\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public client_command(id) {\n    return PLUGIN_CONTINUE;\n}",
        ),
        HookId::ClientUserinfoChanged => (
            "ClientUserinfoChanged",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "ClientSlotArgs",
            "client_infochanged",
            "Fired when a client updates userinfo keys (name, model, rate, _pw, _lang).",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Player slot"),
                ("arg_str(0)", "str", "Updated userinfo infostring"),
            ],
            "Return Supercede in Pre to reject disallowed name or userinfo changes.",
            "mod.log(f'Userinfo changed for {ev.player.name}')",
            "let _slot = host.hook_arg_i32(0) as u8;\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public client_infochanged(id) {\n    return PLUGIN_CONTINUE;\n}",
        ),
        HookId::RunUsercmd => (
            "RunUsercmd",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "UsercmdArgs",
            "FM_CmdStart",
            "Fired before a player's movement/button usercmd runs on a server tick.",
            vec![
                ("arg_i32(0) / ev.slot", "u8", "Player slot"),
                ("arg_i32(1) / ev.buttons", "u16", "IN_* button bitmask"),
            ],
            "Return Supercede in Pre to block the usercmd.",
            "if ev.buttons & Button.USE:\n        pass",
            "let _buttons = host.hook_arg_i32(1);\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_CmdStart(id, uc) {\n    return FMRES_IGNORED;\n}",
        ),
        HookId::PostUsercmd => (
            "PostUsercmd",
            "GameDll (v1)",
            "gamedll",
            "Post",
            false,
            "UsercmdArgs",
            "FM_CmdEnd",
            "Fired immediately after a player's usercmd simulation completes.",
            vec![("arg_i32(0) / ev.slot", "u8", "Player slot")],
            "Post-movement inspection hook.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_CmdEnd(id) { }",
        ),
        HookId::StartFrame => (
            "StartFrame",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            false,
            "FrameArgs",
            "server_frame / FM_StartFrame",
            "Fired at the start of every 100 Hz server tick (use timers for periodic work to save fuel!).",
            vec![(
                "arg_f32(0) / ev.frametime",
                "f32",
                "Server tick delta in seconds (~0.01)",
            )],
            "Keep ultra-lightweight; prefer @mod.timer() for work slower than 100 Hz.",
            "dt = ev.frametime",
            "let _dt = host.hook_arg_f32(0);\n    HookAction::Continue",
            "float dt = cs16_hook_arg_f32(0);\n    (void)dt;\n    return cs16::HookAction::Continue;",
            "_ = cs16mod.HookArgF32(0)\n    return cs16mod.Continue",
            "public server_frame() { }",
        ),
        HookId::Think => (
            "Think",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "EntityIndexArgs",
            "FM_Think / Ham_Think",
            "Fired when a non-player entity executes its scheduled Think callback.",
            vec![(
                "arg_i32(0) / ev.ent_index",
                "u16",
                "Thinking entity edict index",
            )],
            "Return Supercede in Pre to skip entity think logic.",
            "ent = mod.entity(ev.ent_index)",
            "let _ent = host.hook_arg_i32(0) as u16;\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_Think(ent) { return FMRES_IGNORED; }",
        ),
        HookId::Touch => (
            "Touch",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "TouchArgs",
            "FM_Touch / register_touch",
            "Fired when two entities collide/touch (e.g. player touching weaponbox or trigger).",
            vec![
                ("arg_i32(0) / ev.ent_a", "u16", "Touched entity index"),
                ("arg_i32(1) / ev.ent_b", "u16", "Toucher entity index"),
            ],
            "Return Supercede in Pre to block pickup or trigger activation.",
            "if ev.ent_a > 0 and ev.ent_b > 0:\n        pass",
            "let (_a, _b) = (host.hook_arg_i32(0), host.hook_arg_i32(1));\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_Touch(touched, toucher) { return FMRES_IGNORED; }",
        ),
        HookId::Blocked => (
            "Blocked",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "TouchArgs",
            "FM_Blocked",
            "Fired when a moving brush entity (door/train/platform) is blocked by another entity.",
            vec![
                ("arg_i32(0) / ev.ent_a", "u16", "Moving entity index"),
                ("arg_i32(1) / ev.ent_b", "u16", "Blocking entity index"),
            ],
            "Return Supercede in Pre to ignore the block.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_Blocked(ent, other) { return FMRES_IGNORED; }",
        ),
        HookId::UpdateClientData => (
            "UpdateClientData",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            false,
            "ClientSlotArgs",
            "FM_UpdateClientData",
            "Fired when assembling per-client HUD/viewmodel clientdata snapshot.",
            vec![("arg_i32(0) / ev.slot", "u8", "Target client slot")],
            "Snapshot assembly hook for custom clientdata synchronization.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_UpdateClientData(id, sw, cd) { return FMRES_IGNORED; }",
        ),
        HookId::GetWeaponData => (
            "GetWeaponData",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            false,
            "ClientSlotArgs",
            "GetWeaponData",
            "Fired when packing weapon_data_t delta state for client prediction.",
            vec![("arg_i32(0) / ev.slot", "u8", "Target client slot")],
            "Weapon prediction data inspection hook.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "// Built-in prediction sync in GoldSrc",
        ),
        HookId::ServerCommand => (
            "ServerCommand",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "ServerCommandArgs",
            "register_srvcmd",
            "Fired when an operator or RCON executes a server console command.",
            vec![
                ("arg_str(0) / ev.command", "str", "Console command name"),
                ("arg_str(1) / ev.args", "str", "Full argument string"),
            ],
            "Return Supercede in Pre to handle custom server/RCON commands.",
            "if ev.command == 'my_status':\n        mod.log('OK')\n        return HookAction.SUPERCEDE",
            "if host.hook_arg_str(0).as_deref() == Some(\"my_status\") {\n        return HookAction::Supercede;\n    }\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "register_srvcmd(\"my_status\", \"cmd_status\");",
        ),
        HookId::AddToFullPack => (
            "AddToFullPack",
            "GameDll (v1)",
            "gamedll",
            "Pre / Post",
            true,
            "AddToFullPackArgs",
            "FM_AddToFullPack",
            "Fired per entity while packing client visibility frames; ideal for per-viewer transparency/semiclip.",
            vec![
                ("arg_i32(0) / ev.slot", "u8", "Receiving viewer slot"),
                ("arg_i32(1) / ev.ent_index", "u16", "Entity being packed"),
            ],
            "Return Supercede in Pre to hide an entity from that viewer.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_AddToFullPack(es, e, ent, host) { return FMRES_IGNORED; }",
        ),
        HookId::PlayerDeath => (
            "PlayerDeath",
            "Gameplay / ReAPI (v1)",
            "gameplay",
            "Post",
            false,
            "PlayerKilledCtx",
            "register_event(\"DeathMsg\", ...)",
            "Legacy v1 alias fired on player death (prefer PlayerKilled in ABI v2 for full hitgroup/headshot/weapon fields).",
            vec![
                ("arg_i32(0) / ev.victim", "u8 / Player", "Victim slot"),
                (
                    "arg_i32(1) / ev.killer",
                    "i32 / Option<Player>",
                    "Killer slot (-1 if world)",
                ),
            ],
            "Observation post-hook; see PlayerKilled for full ReAPI metadata.",
            "if ev.killer:\n        ev.killer.add_money(300)",
            "let killer = host.hook_arg_i32(1);\n    if killer >= 0 { Player::new(killer as u8).add_money(host, 300, true); }\n    HookAction::Continue",
            "int32_t k = cs16_hook_arg_i32(1);\n    if (k >= 0) cs16::Player(k).add_money(300);\n    return cs16::HookAction::Continue;",
            "if k := cs16mod.HookArgI32(1); k >= 0 { cs16mod.NewPlayer(k).AddMoney(300, true) }\n    return cs16mod.Continue",
            "register_event(\"DeathMsg\", \"ev_death\", \"a\");",
        ),
        HookId::RoundStart => (
            "RoundStart",
            "Gameplay / ReAPI (v1+v2)",
            "gameplay",
            "Pre / Post",
            false,
            "RoundStartCtx",
            "register_event(\"HLTV\", ...) / RG_CSGameRules_RestartRound",
            "Fired at the beginning of every Counter-Strike round (start of freeze time).",
            vec![(
                "arg_i32(0) / ev.round_num",
                "u32",
                "1-based current match round number",
            )],
            "Great for resetting per-round VIP menus, buy limits, and announcing round banners.",
            "mod.dhud(f'ROUND {ev.round_num} — LIVE!', y=0.2, rgb=(255, 180, 0))",
            "let round = host.hook_arg_i32(0);\n    host.print_all(&format!(\"Round {round} started!\"));\n    HookAction::Continue",
            "cs16::server::say_all(\"\\x04[Match]\\x01 New round started!\");\n    return cs16::HookAction::Continue;",
            "cs16mod.SayAll(\"\\x04[Match]\\x01 New round started!\")\n    return cs16mod.Continue",
            "register_logevent(\"ev_round_start\", 2, \"1=Round_Start\");",
        ),
        HookId::PlayerSpawn => (
            "PlayerSpawn",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "PlayerSpawnCtx",
            "RegisterHam(Ham_Spawn, \"player\", ...) / RG_CBasePlayer_Spawn",
            "Fired whenever a player spawns into the world. Post is the standard place to grant VIP kits, weapons, HP, and armor.",
            vec![(
                "arg_i32(0) / ev.player",
                "u8 / Player",
                "Spawning player slot (0..31)",
            )],
            "Pre can cancel spawn with Supercede; Post grants items safely after vanilla loadout.",
            "ev.player.set_armor(100, Armor.VESTHELM)\n    ev.player.give_item('weapon_hegrenade')",
            "let p = Player::new(host.hook_arg_i32(0) as u8);\n    p.set_armor(host, 100, ArmorKind::VestHelm);\n    HookAction::Continue",
            "cs16::Player(cs16_hook_arg_i32(0)).set_armor(100, cs16::ArmorKind::VestHelm);\n    return cs16::HookAction::Continue;",
            "cs16mod.NewPlayer(cs16mod.HookArgI32(0)).SetArmor(100, cs16mod.ArmorVestHelm)\n    return cs16mod.Continue",
            "RegisterHam(Ham_Spawn, \"player\", \"fw_PlayerSpawn\", 1);",
        ),
        HookId::PlayerTakeDamage => (
            "PlayerTakeDamage",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "PlayerTakeDamageCtx",
            "RegisterHam(Ham_TakeDamage, \"player\", ...) / RG_CBasePlayer_TakeDamage",
            "Fired whenever a player receives damage. Pre supports scaling damage, remapping hitgroup, or blocking damage entirely!",
            vec![
                (
                    "arg_i32(0) / ev.victim",
                    "u8 / Player",
                    "Damaged player slot",
                ),
                (
                    "arg_i32(1) / ev.attacker",
                    "i32 / Option<Player>",
                    "Attacker slot (-1 = world/fall)",
                ),
                (
                    "arg_i32(2) / ev.hitgroup",
                    "u8",
                    "Hitgroup (1=Head, 2=Chest, 3=Stomach, 4..7=Limbs)",
                ),
                (
                    "arg_i32(3) / ev.weapon_id",
                    "u8",
                    "WeaponId (e.g. 28=AK47, 18=AWP)",
                ),
                (
                    "arg_i32(4) / ev.damage_bits",
                    "u32",
                    "GoldSrc DMG_* bitmask",
                ),
                (
                    "arg_i32(5) / ev.team_attack",
                    "bool",
                    "1 if attacker and victim share team",
                ),
                (
                    "arg_f32(0) / ev.damage",
                    "f32",
                    "Incoming damage amount (mutable in Pre!)",
                ),
            ],
            "In Pre: mutate ev.damage / hook_set_override_f32(dmg) and return Override, or return Supercede to block all damage.",
            "if ev.attacker and ev.attacker.has_flag('t'):\n        ev.damage = ev.damage * 1.25\n        return HookAction.OVERRIDE",
            "let dmg = host.hook_arg_f32(0);\n    host.hook_set_override_f32(dmg * 1.25);\n    HookAction::Override",
            "float dmg = cs16_hook_arg_f32(0);\n    cs16_hook_set_override_f32(dmg * 1.25f);\n    return cs16::HookAction::Override;",
            "ev.SetDamage(ev.Damage * 1.25)\n    return cs16mod.Override",
            "public fw_TakeDamage(vic, inf, atk, Float:dmg, bits) {\n    SetHamParamFloat(4, dmg * 1.25);\n    return HAM_OVERRIDE;\n}",
        ),
        HookId::PlayerKilled => (
            "PlayerKilled",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "PlayerKilledCtx",
            "RegisterHam(Ham_Killed, \"player\", ...) / RG_CBasePlayer_Killed",
            "Fired when a player is fatally killed; exposes killer, weapon name/ID, hitgroup, headshot, and team_kill flags.",
            vec![
                (
                    "arg_i32(0) / ev.victim",
                    "u8 / Player",
                    "Killed player slot",
                ),
                (
                    "arg_i32(1) / ev.killer",
                    "i32 / Option<Player>",
                    "Killer slot (-1 = world)",
                ),
                ("arg_i32(2) / ev.weapon_id", "u8", "Killing WeaponId"),
                (
                    "arg_i32(3) / ev.hitgroup",
                    "u8",
                    "Fatal hitgroup (1 = Head)",
                ),
                ("arg_i32(4) / ev.headshot", "bool", "True on headshot kill"),
                (
                    "arg_i32(5) / ev.team_kill",
                    "bool",
                    "True on friendly fire kill",
                ),
                (
                    "arg_str(0) / ev.weapon_name",
                    "str",
                    "Killfeed weapon name (e.g. ak47)",
                ),
            ],
            "Pre can cancel death with Supercede; Post powers Vampire heal, GunGame level-up, StatsX, and VFX.",
            "if ev.killer and not ev.team_kill:\n        ev.killer.set_health(min(ev.killer.health + (20 if ev.headshot else 10), 100))\n        vfx.shockwave_ring(ev.victim.origin, 160.0, (255, 64, 64, 220))",
            "let killer = host.hook_arg_i32(1);\n    let hs = host.hook_arg_i32(4) != 0;\n    if killer >= 0 { Player::new(killer as u8).add_money(host, if hs { 500 } else { 300 }, true); }\n    HookAction::Continue",
            "int32_t k = cs16_hook_arg_i32(1);\n    if (k >= 0) cs16::Player(k).add_money(cs16_hook_arg_i32(4) ? 500 : 300);\n    return cs16::HookAction::Continue;",
            "if ev.KillerSlot >= 0 && !ev.TeamKill {\n        cs16mod.NewPlayer(ev.KillerSlot).AddMoney(300, true)\n    }\n    return cs16mod.Continue",
            "RegisterHam(Ham_Killed, \"player\", \"fw_PlayerKilled\", 1);",
        ),
        HookId::PlayerPreThink => (
            "PlayerPreThink",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            false,
            "PlayerPreThinkCtx",
            "client_PreThink / RG_CBasePlayer_PreThink",
            "Fired every tick before a player's physics/movement runs; exposes live IN_* buttons and oldbuttons (ideal for +use parachute or bhop).",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Player slot"),
                (
                    "arg_i32(1) / ev.buttons",
                    "u16",
                    "Current tick IN_* button flags",
                ),
                (
                    "arg_i32(2) / ev.oldbuttons",
                    "u16",
                    "Previous tick IN_* button flags",
                ),
            ],
            "Inspect ev.buttons & Button.USE and adjust velocity/gravity smoothly.",
            "if (ev.buttons & Button.USE) and not ev.player.on_ground:\n        vx, vy, vz = ev.player.velocity\n        if vz < -100.0:\n            ev.player.set_velocity((vx, vy, -100.0))",
            "let slot = host.hook_arg_i32(0) as u8;\n    let btns = host.hook_arg_i32(1);\n    if (btns & 32) != 0 { Player::new(slot).set_gravity(host, 0.35); }\n    HookAction::Continue",
            "if (cs16_hook_arg_i32(1) & 32) cs16::Player(cs16_hook_arg_i32(0)).set_gravity(0.35f);\n    return cs16::HookAction::Continue;",
            "if (cs16mod.HookArgI32(1) & 32) != 0 { cs16mod.NewPlayer(cs16mod.HookArgI32(0)).SetGravity(0.35) }\n    return cs16mod.Continue",
            "public client_PreThink(id) {\n    if (pev(id, pev_button) & IN_USE) { /* parachute */ }\n}",
        ),
        HookId::PlayerPostThink => (
            "PlayerPostThink",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Post",
            false,
            "PlayerPostThinkCtx",
            "client_PostThink / RG_CBasePlayer_PostThink",
            "Fired every tick after player physics/movement resolves (ideal for speedometer HUD or speed caps).",
            vec![("arg_i32(0) / ev.player", "u8 / Player", "Player slot")],
            "Post-physics inspection and HUD update hook.",
            "vx, vy, _ = ev.player.velocity\n    speed = int((vx * vx + vy * vy) ** 0.5)",
            "let _p = Player::new(host.hook_arg_i32(0) as u8);\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public client_PostThink(id) { }",
        ),
        HookId::PlayerJump => (
            "PlayerJump",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "PlayerJumpCtx",
            "RegisterHam(Ham_Player_Jump, \"player\", ...) / RG_CBasePlayer_Jump",
            "Fired when a player jumps (powers double-jump, auto-bhop, and jump sound/VFX).",
            vec![(
                "arg_i32(0) / ev.player",
                "u8 / Player",
                "Jumping player slot",
            )],
            "Return Supercede in Pre to disable jumping; Post can boost jump velocity.",
            "vx, vy, vz = ev.player.velocity\n    ev.player.set_velocity((vx * 1.1, vy * 1.1, vz))",
            "let _p = Player::new(host.hook_arg_i32(0) as u8);\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "RegisterHam(Ham_Player_Jump, \"player\", \"fw_Jump\", 1);",
        ),
        HookId::PlayersShouldCollide => (
            "PlayersShouldCollide",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre",
            true,
            "PlayersShouldCollideCtx",
            "FM_ShouldCollide / Semiclip",
            "Fired by GoldSrc movement collision when testing whether two players block each other.",
            vec![
                (
                    "arg_i32(0) / ev.player_a",
                    "u8 / Player",
                    "First player slot",
                ),
                (
                    "arg_i32(1) / ev.player_b",
                    "u8 / Player",
                    "Second player slot",
                ),
                (
                    "arg_i32(2) / ev.should_collide",
                    "bool",
                    "Default engine collision result (mutable!)",
                ),
            ],
            "Set ev.should_collide = False (or hook_set_override_i32(0)) and return Override for true zero-jitter teammate semiclip!",
            "if ev.player_a.team == ev.player_b.team:\n        ev.should_collide = False\n        return HookAction.OVERRIDE",
            "let a = Player::new(host.hook_arg_i32(0) as u8);\n    let b = Player::new(host.hook_arg_i32(1) as u8);\n    if a.team(host) == b.team(host) {\n        host.hook_set_override_i32(0);\n        return HookAction::Override;\n    }\n    HookAction::Continue",
            "if (cs16::Player(cs16_hook_arg_i32(0)).team() == cs16::Player(cs16_hook_arg_i32(1)).team()) {\n        cs16_hook_set_override_i32(0);\n        return cs16::HookAction::Override;\n    }\n    return cs16::HookAction::Continue;",
            "if cs16mod.NewPlayer(cs16mod.HookArgI32(0)).Team() == cs16mod.NewPlayer(cs16mod.HookArgI32(1)).Team() {\n        cs16mod.HookSetOverrideI32(0)\n        return cs16mod.Override\n    }\n    return cs16mod.Continue",
            "// Replaces complex AddToFullPack + Solid_Not semiclip hacks cleanly!",
        ),
        HookId::CanBuyItem => (
            "CanBuyItem",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "CanBuyItemCtx",
            "CS_OnBuyAttempt / restmenu.sma",
            "Fired whenever a player attempts to buy a weapon or equipment item from the BuyMenu or console alias.",
            vec![
                (
                    "arg_i32(0) / ev.player",
                    "u8 / Player",
                    "Buying player slot",
                ),
                (
                    "arg_i32(1) / ev.item_id",
                    "u8",
                    "WeaponId (1..30, 99) or equip (101=Kevlar, 102=VestHelm, 106=NVG, 107=Defuser)",
                ),
                (
                    "arg_i32(2) / ev.price",
                    "i32",
                    "Item cost in $ (mutable override!)",
                ),
            ],
            "Return Supercede in Pre to restrict a weapon (e.g. AWP); or override price via ev.price / hook_set_override_i32(price) + Override.",
            "if ev.item_id == WeaponId.AWP:\n        ev.player.chat('AWP is restricted on this server!')\n        return HookAction.SUPERCEDE",
            "if host.hook_arg_i32(1) == 18 {\n        return HookAction::Supercede;\n    }\n    HookAction::Continue",
            "if (cs16_hook_arg_i32(1) == 18) return cs16::HookAction::Supercede;\n    return cs16::HookAction::Continue;",
            "if cs16mod.HookArgI32(1) == 18 { return cs16mod.Supercede }\n    return cs16mod.Continue",
            "public CS_OnBuyAttempt(id, item) {\n    return item == CSI_AWP ? PLUGIN_HANDLED : PLUGIN_CONTINUE;\n}",
        ),
        HookId::CanHavePlayerItem => (
            "CanHavePlayerItem",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre",
            true,
            "CanHavePlayerItemCtx",
            "RG_CBasePlayer_HasRestrictItem / Ham_AddPlayerItem",
            "Fired before a player picks up a dropped weapon from the ground.",
            vec![
                (
                    "arg_i32(0) / ev.player",
                    "u8 / Player",
                    "Player attempting pickup",
                ),
                ("arg_i32(1) / ev.weapon_id", "u8", "Dropped WeaponId"),
            ],
            "Return Supercede in Pre to prevent picking up restricted weapons (e.g. knife-only arena).",
            "if ev.weapon_id == WeaponId.AWP:\n        return HookAction.SUPERCEDE",
            "if host.hook_arg_i32(1) == 18 { return HookAction::Supercede; }\n    HookAction::Continue",
            "if (cs16_hook_arg_i32(1) == 18) return cs16::HookAction::Supercede;\n    return cs16::HookAction::Continue;",
            "if cs16mod.HookArgI32(1) == 18 { return cs16mod.Supercede }\n    return cs16mod.Continue",
            "RegisterHam(Ham_AddPlayerItem, \"player\", \"fw_AddItem\", 0);",
        ),
        HookId::WeaponDeploy => (
            "WeaponDeploy",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "WeaponEventCtx",
            "RegisterHam(Ham_Item_Deploy, ...)",
            "Fired when a player switches to / deploys a weapon.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Weapon owner slot"),
                ("arg_i32(1) / ev.weapon_id", "u8", "Deployed WeaponId"),
            ],
            "Great for applying weapon-specific speed buffs (e.g. knife turbo) or custom HUD hints.",
            "if ev.weapon_id == WeaponId.KNIFE:\n        ev.player.set_maxspeed(280.0)",
            "if host.hook_arg_i32(1) == 29 {\n        Player::new(host.hook_arg_i32(0) as u8).set_maxspeed(host, 280.0);\n    }\n    HookAction::Continue",
            "if (cs16_hook_arg_i32(1) == 29) cs16::Player(cs16_hook_arg_i32(0)).set_maxspeed(280.0f);\n    return cs16::HookAction::Continue;",
            "if cs16mod.HookArgI32(1) == 29 { cs16mod.NewPlayer(cs16mod.HookArgI32(0)).SetMaxspeed(280) }\n    return cs16mod.Continue",
            "RegisterHam(Ham_Item_Deploy, \"weapon_knife\", \"fw_KnifeDeploy\", 1);",
        ),
        HookId::WeaponPrimaryAttack => (
            "WeaponPrimaryAttack",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "WeaponEventCtx",
            "RegisterHam(Ham_Weapon_PrimaryAttack, ...)",
            "Fired whenever a player pulls the trigger on a weapon.",
            vec![
                (
                    "arg_i32(0) / ev.player",
                    "u8 / Player",
                    "Firing player slot",
                ),
                ("arg_i32(1) / ev.weapon_id", "u8", "Active WeaponId"),
            ],
            "Pre can cancel firing with Supercede; Post can spawn laser tracers from origin to aim_origin!",
            "vfx.laser_beam(ev.player.origin, ev.player.aim_origin, (255, 215, 0, 220), life=3, width=4)",
            "let p = Player::new(host.hook_arg_i32(0) as u8);\n    if let (Some(a), Some(b)) = (p.origin(host), p.aim_origin(host)) {\n        vfx::laser_beam(host, a, b, [255, 215, 0, 220], 3, 4, 0, None);\n    }\n    HookAction::Continue",
            "cs16::Player p(cs16_hook_arg_i32(0));\n    cs16::Vec3 a, b;\n    if (p.origin(a) && p.aim_origin(b)) cs16::vfx::laser_beam(a, b, cs16::Color::Gold(), 3, 4);\n    return cs16::HookAction::Continue;",
            "p := cs16mod.NewPlayer(cs16mod.HookArgI32(0))\n    if a, ok1 := p.Origin(); ok1 { if b, ok2 := p.AimOrigin(); ok2 { cs16mod.VFX.LaserBeam(a, b, cs16mod.Color{255, 215, 0, 220}, 3, 4, 0, -1) } }\n    return cs16mod.Continue",
            "RegisterHam(Ham_Weapon_PrimaryAttack, \"weapon_ak47\", \"fw_Fire\", 1);",
        ),
        HookId::WeaponReload => (
            "WeaponReload",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "WeaponEventCtx",
            "RegisterHam(Ham_Weapon_Reload, ...)",
            "Fired when a player initiates weapon reload.",
            vec![
                (
                    "arg_i32(0) / ev.player",
                    "u8 / Player",
                    "Reloading player slot",
                ),
                ("arg_i32(1) / ev.weapon_id", "u8", "Reloaded WeaponId"),
            ],
            "Return Supercede in Pre to prevent reload (e.g. instant-refill arcade modes).",
            "mod.log(f'{ev.player.name} reloaded weapon {ev.weapon_id}')",
            "let _wid = host.hook_arg_i32(1);\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "RegisterHam(Ham_Weapon_Reload, \"weapon_ak47\", \"fw_Reload\", 0);",
        ),
        HookId::PlayerSay => (
            "PlayerSay",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "PlayerSayCtx",
            "register_clcmd(\"say\", ...) / say_team",
            "Fired whenever a player sends global or team chat; powers chat commands, anti-flood, and [VIP] prefixes.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Speaker slot"),
                (
                    "arg_i32(1) / ev.team_only",
                    "bool",
                    "1 for say_team, 0 for global say",
                ),
                (
                    "arg_str(0) / ev.message",
                    "str",
                    "Chat text (mutable in Pre!)",
                ),
            ],
            "Mutate ev.message / hook_set_override_str() + Override to rewrite chat, or return Supercede to hide slash commands.",
            "if ev.message.startswith('/rs'):\n        ev.player.reset_score()\n        return HookAction.SUPERCEDE",
            "if host.hook_arg_str(0).as_deref() == Some(\"/rs\") {\n        Player::new(host.hook_arg_i32(0) as u8).reset_score(host);\n        return HookAction::Supercede;\n    }\n    HookAction::Continue",
            "CS16_ON_CHAT(\"/rs\", on_rs) {\n    player.reset_score();\n    return cs16::HookAction::Supercede;\n}",
            "cs16mod.OnChat(\"/rs\", func(p cs16mod.Player, _ string) cs16mod.HookAction {\n    p.ResetScore()\n    return cs16mod.Supercede\n})",
            "register_clcmd(\"say /rs\", \"cmd_resetscore\");",
        ),
        HookId::MenuSelect => (
            "MenuSelect",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Post",
            false,
            "MenuSelectCtx",
            "newmenus callback / menuselect",
            "Fired when a player chooses an item on a menu (or use Menu(..., callback=fn) / cs_mod_menu).",
            vec![
                (
                    "arg_i32(0) / ev.player",
                    "u8 / Player",
                    "Selecting player slot",
                ),
                ("arg_i32(1) / ev.menu_id", "u32", "Host MenuManager menu ID"),
                (
                    "arg_i32(2) / ev.item_index",
                    "i32",
                    "0-based selectable item index (-3 = Exit)",
                ),
                ("arg_str(0) / ev.info", "str", "Opaque item info payload"),
            ],
            "Host MenuManager isolates menu ownership per slot automatically.",
            "if ev.info.startswith('weapon_'):\n        ev.player.give_item(ev.info)",
            "let _info = host.hook_arg_str(0).unwrap_or_default();\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public menu_handler(id, menu, item) {\n    if (item == MENU_EXIT) return PLUGIN_HANDLED;\n}",
        ),
        HookId::FreezeTimeEnd => (
            "FreezeTimeEnd",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Post",
            false,
            "EmptyCtx",
            "register_logevent(..., \"1=Round_Start\") / RG_CSGameRules_OnRoundFreezeEnd",
            "Fired the instant mp_freezetime expires and players can move/fire.",
            vec![("—", "none", "No arguments; inspect mod.players()")],
            "Ideal for launching round-live timers, AFK monitors, or zombie infection countdowns.",
            "mod.dhud('GO! GO! GO!', y=0.22, rgb=(116, 212, 116))",
            "host.center_print(None, \"Freeze time ended — GO!\");\n    HookAction::Continue",
            "cs16::server::center_print_all(\"Freeze time ended!\");\n    return cs16::HookAction::Continue;",
            "cs16mod.CenterPrintAll(\"Freeze time ended!\")\n    return cs16mod.Continue",
            "register_logevent(\"ev_freeze_end\", 2, \"1=Round_Start\");",
        ),
        HookId::RoundEnd => (
            "RoundEnd",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "RoundEndCtx",
            "RG_RoundEnd / SendAudio",
            "Fired when a round concludes with a winning team, reason code (1..16), and team cash bonus.",
            vec![
                (
                    "arg_i32(0) / ev.winner",
                    "u8",
                    "0 = Draw, 1 = Terrorists, 2 = Counter-Terrorists",
                ),
                (
                    "arg_i32(1) / ev.reason",
                    "u8",
                    "Win reason (1=TargetBombed, 7=BombDefused, 8=CTsWin, 9=TsWin, 11=HostagesRescued, 12=TargetSaved)",
                ),
                ("arg_i32(2) / ev.bonus", "i32", "Round end cash bonus"),
            ],
            "Pre can override round-end outcome or cancel with Supercede (e.g. infinite Deathmatch rounds!).",
            "side = 'CT' if ev.winner == 2 else 'T'\n    mod.chat_all(f'{Color.GREEN}[Round]{Color.DEFAULT} {side} wins! (+${ev.bonus})')",
            "let winner = host.hook_arg_i32(0);\n    host.print_all(&format!(\"Round ended, winner={winner}\"));\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "cs16mod.OnRoundEnd(func(winner, reason, bonus int32) cs16mod.HookAction {\n    return cs16mod.Continue\n})",
            "RegisterHookChain(RG_RoundEnd, \"fw_RoundEnd\", 1);",
        ),
        HookId::BombPlanting => (
            "BombPlanting",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "BombEventCtx",
            "bomb_planting (csx)",
            "Fired when a Terrorist starts planting C4 inside a bombsite.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Planter slot"),
                (
                    "arg_f32(0..2) / ev.origin",
                    "Vec3",
                    "Plant world origin (x, y, z)",
                ),
            ],
            "Return Supercede in Pre to disallow C4 planting (e.g. low-player warmup).",
            "mod.chat_all(f'{ev.player.name} is planting C4!')",
            "let _slot = host.hook_arg_i32(0);\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public bomb_planting(planter) { }",
        ),
        HookId::BombPlanted => (
            "BombPlanted",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Post",
            false,
            "BombEventCtx",
            "bomb_planted (csx)",
            "Fired when C4 is successfully armed on a bombsite.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Planter slot"),
                (
                    "arg_f32(0..2) / ev.origin",
                    "Vec3",
                    "Planted C4 world origin",
                ),
            ],
            "Spawn a pulsing shockwave ring or start a custom C4 countdown HUD!",
            "vfx.shockwave_ring(ev.origin, 220.0, (255, 64, 64, 240), life=12, width=14)",
            "host.center_print(None, \"The bomb has been planted!\");\n    HookAction::Continue",
            "cs16::server::center_print_all(\"Bomb planted!\");\n    return cs16::HookAction::Continue;",
            "cs16mod.CenterPrintAll(\"Bomb planted!\")\n    return cs16mod.Continue",
            "public bomb_planted(planter) { }",
        ),
        HookId::BombDefusing => (
            "BombDefusing",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "BombEventCtx",
            "bomb_defusing (csx)",
            "Fired when a Counter-Terrorist begins defusing the planted C4.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Defuser slot"),
                (
                    "arg_i32(1) / ev.has_kit",
                    "bool",
                    "True if player holds a defuse kit (5s vs 10s)",
                ),
                ("arg_f32(0..2) / ev.origin", "Vec3", "C4 world origin"),
            ],
            "Return Supercede in Pre to interrupt defuse.",
            "kit = 'with kit' if ev.has_kit else 'without kit'\n    mod.chat_all(f'{ev.player.name} defusing {kit}!')",
            "let _has_kit = host.hook_arg_i32(1) != 0;\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public bomb_defusing(defuser) { }",
        ),
        HookId::BombDefused => (
            "BombDefused",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Post",
            false,
            "BombEventCtx",
            "bomb_defused (csx)",
            "Fired when a CT completes defusing C4.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Defuser slot"),
                (
                    "arg_i32(1) / ev.has_kit",
                    "bool",
                    "True if defused with kit",
                ),
                ("arg_f32(0..2) / ev.origin", "Vec3", "C4 world origin"),
            ],
            "Reward clutch defuses with bonus cash, leaderboard points, and cyan shockwave VFX.",
            "vfx.shockwave_ring(ev.origin, 260.0, (0, 220, 255, 255))\n    ev.player.add_money(500)",
            "let p = Player::new(host.hook_arg_i32(0) as u8);\n    p.add_money(host, 500, true);\n    HookAction::Continue",
            "cs16::Player(cs16_hook_arg_i32(0)).add_money(500);\n    return cs16::HookAction::Continue;",
            "cs16mod.NewPlayer(cs16mod.HookArgI32(0)).AddMoney(500, true)\n    return cs16mod.Continue",
            "public bomb_defused(defuser) { }",
        ),
        HookId::BombExploded => (
            "BombExploded",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Post",
            false,
            "BombEventCtx",
            "bomb_explode (csx)",
            "Fired when the planted C4 detonates.",
            vec![
                (
                    "arg_i32(0) / ev.slot",
                    "i32",
                    "Original planter slot (-1 if disconnected)",
                ),
                ("arg_f32(0..2) / ev.origin", "Vec3", "Detonation origin"),
            ],
            "Trigger dramatic screen shakes or extra lava splash VFX.",
            "mod.screen_shake(amplitude=8.0, duration=1.8, frequency=3.0)",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public bomb_explode(planter, defuser) { }",
        ),
        HookId::HostageTouched => (
            "HostageTouched",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "HostageEventCtx",
            "register_logevent(\"Touched_A_Hostage\")",
            "Fired when a CT presses +use on a hostage to start escorting.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "CT player slot"),
                ("arg_i32(1) / ev.ent_index", "u16", "Hostage entity index"),
            ],
            "Return Supercede in Pre to block hostage follow.",
            "ev.player.chat('Lead the hostage to the rescue zone!')",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "register_logevent(\"ev_hostage_touch\", 3, \"2=Touched_A_Hostage\");",
        ),
        HookId::HostageRescued => (
            "HostageRescued",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Post",
            false,
            "HostageEventCtx",
            "register_logevent(\"Rescued_A_Hostage\")",
            "Fired when a hostage reaches `func_hostage_rescue`.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Rescuing CT slot"),
                ("arg_i32(1) / ev.ent_index", "u16", "Hostage entity index"),
            ],
            "Award extra rank points or VFX celebration.",
            "ev.player.add_money(250, flash=True)",
            "Player::new(host.hook_arg_i32(0) as u8).add_money(host, 250, true);\n    HookAction::Continue",
            "cs16::Player(cs16_hook_arg_i32(0)).add_money(250);\n    return cs16::HookAction::Continue;",
            "cs16mod.NewPlayer(cs16mod.HookArgI32(0)).AddMoney(250, true)\n    return cs16mod.Continue",
            "register_logevent(\"ev_hostage_rescue\", 3, \"2=Rescued_A_Hostage\");",
        ),
        HookId::GrenadeThrow => (
            "GrenadeThrow",
            "Gameplay / ReAPI (v2)",
            "gameplay",
            "Pre / Post",
            true,
            "GrenadeThrowCtx",
            "grenade_throw (csx)",
            "Fired when a player throws HE, Flashbang, or Smoke grenade.",
            vec![
                ("arg_i32(0) / ev.player", "u8 / Player", "Thrower slot"),
                (
                    "arg_i32(1) / ev.weapon_id",
                    "u8",
                    "4=HE, 9=Smoke, 25=Flashbang",
                ),
                (
                    "arg_i32(2) / ev.ent_index",
                    "u16",
                    "Spawned grenade projectile entity index",
                ),
            ],
            "Attach a colored TE_BEAMFOLLOW ribbon trail to ev.ent_index in Post!",
            "col = (255, 64, 64, 220) if ev.weapon_id == WeaponId.HEGRENADE else (100, 220, 255, 220)\n    vfx.follow_trail(ev.ent_index, col, life=14, width=5)",
            "let ent = host.hook_arg_i32(2) as u16;\n    vfx::follow_trail(host, ent, [255, 64, 64, 220], 14, 5, None);\n    HookAction::Continue",
            "cs16::vfx::follow_trail(cs16_hook_arg_i32(2), cs16::Color::Red(), 14, 5);\n    return cs16::HookAction::Continue;",
            "cs16mod.VFX.FollowTrail(cs16mod.HookArgI32(2), cs16mod.Color{255, 64, 64, 220}, 14, 5, -1)\n    return cs16mod.Continue",
            "public grenade_throw(id, greindex, wId) { /* TE_BEAMFOLLOW */ }",
        ),
        HookId::EngEmitSound => (
            "EngEmitSound",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "EmitSoundArgs",
            "FM_EmitSound",
            "Fired whenever the server emits a positional world/entity sound over Protocol 48.",
            vec![
                ("arg_i32(0) / ev.ent_index", "u16", "Source entity index"),
                (
                    "arg_str(0)",
                    "str",
                    "Sound sample path (e.g. weapons/knife_hit1.wav)",
                ),
            ],
            "Return Supercede in Pre to mute or replace vanilla sound samples.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_EmitSound(ent, chan, const sample[]) { return FMRES_IGNORED; }",
        ),
        HookId::EngSendUserMsg => (
            "EngSendUserMsg",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "UserMsgArgs",
            "register_message",
            "Fired before sending any GoldSrc UserMsg (SayText, DeathMsg, ScreenFade, TextMsg, etc.).",
            vec![
                ("arg_i32(0)", "i32", "UserMsg numeric ID"),
                ("arg_str(0)", "str", "UserMsg name"),
                ("arg_bytes()", "bytes", "Raw encoded UserMsg payload"),
            ],
            "Return Supercede in Pre to suppress a specific UserMsg packet.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "register_message(get_user_msgid(\"TextMsg\"), \"msg_text\");",
        ),
        HookId::EngClientPrint => (
            "EngClientPrint",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "ClientPrintArgs",
            "FM_ClientPrintf",
            "Fired when the engine prints console/chat text to a player.",
            vec![
                (
                    "arg_i32(0) / ev.slot",
                    "i32",
                    "Target slot (-1 for broadcast)",
                ),
                ("arg_str(0)", "str", "Message string"),
            ],
            "Return Supercede in Pre to suppress engine print output.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "// Intercepted via TextMsg in AMXX",
        ),
        HookId::EngCenterPrint => (
            "EngCenterPrint",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "ClientPrintArgs",
            "TextMsg (print_center)",
            "Fired when the engine sends a center-screen announcement.",
            vec![
                (
                    "arg_i32(0) / ev.slot",
                    "i32",
                    "Target slot (-1 for broadcast)",
                ),
                ("arg_str(0)", "str", "Center text"),
            ],
            "Return Supercede in Pre to replace default center banners with custom DHUD.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "// Intercepted via TextMsg print_center",
        ),
        HookId::EngCreateEntity => (
            "EngCreateEntity",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "EntitySpawnArgs",
            "FM_CreateNamedEntity",
            "Fired when the game DLL allocates a new entity edict.",
            vec![(
                "arg_str(0) / ev.classname",
                "str",
                "Requested entity classname",
            )],
            "Return Supercede in Pre to prevent allocating the entity.",
            "pass",
            "HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_CreateNamedEntity(iclass) { return FMRES_IGNORED; }",
        ),
        HookId::EngRemoveEntity => (
            "EngRemoveEntity",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "EntityIndexArgs",
            "FM_RemoveEntity",
            "Fired before an entity edict is freed.",
            vec![(
                "arg_i32(0) / ev.ent_index",
                "u16",
                "Entity index being removed",
            )],
            "Clean up custom entity references or beam trails attached to ent_index.",
            "vfx.kill_beam(ev.ent_index)",
            "let ent = host.hook_arg_i32(0) as u16;\n    vfx::kill_beam(host, ent, None);\n    HookAction::Continue",
            "cs16::vfx::kill_beam(cs16_hook_arg_i32(0));\n    return cs16::HookAction::Continue;",
            "cs16mod.VFX.KillBeam(cs16mod.HookArgI32(0), -1)\n    return cs16mod.Continue",
            "public fw_RemoveEntity(ent) { return FMRES_IGNORED; }",
        ),
        HookId::EngKick => (
            "EngKick",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "ClientKickArgs",
            "server_cmd(\"kick ...\")",
            "Fired when the engine or a plugin kicks a connected client.",
            vec![
                ("arg_i32(0) / ev.slot", "u8", "Target player slot"),
                ("arg_str(0)", "str", "Kick reason string"),
            ],
            "Return Supercede in Pre to protect immune admins (flag 'a') from kicks.",
            "if ev.player.has_flag('a'):\n        return HookAction.SUPERCEDE",
            "if Player::new(host.hook_arg_i32(0) as u8).has_flag(host, 'a') {\n        return HookAction::Supercede;\n    }\n    HookAction::Continue",
            "if (cs16::Player(cs16_hook_arg_i32(0)).has_flag('a')) return cs16::HookAction::Supercede;\n    return cs16::HookAction::Continue;",
            "if cs16mod.NewPlayer(cs16mod.HookArgI32(0)).HasFlag('a') { return cs16mod.Supercede }\n    return cs16mod.Continue",
            "// Protected via ADMIN_IMMUNITY check",
        ),
        HookId::EngChangeLevel => (
            "EngChangeLevel",
            "EngineApi (v1)",
            "engine",
            "Pre / Post",
            true,
            "ChangeLevelArgs",
            "FM_ChangeLevel",
            "Fired right before the server switches to another map.",
            vec![("arg_str(0) / ev.map_name", "str", "Destination map name")],
            "Return Supercede in Pre to defer map change until round end or vote completion.",
            "mod.chat_all(f'Changing level to {ev.map_name}...')",
            "let next = host.hook_arg_str(0).unwrap_or_default();\n    host.print_all(&format!(\"Changing map to {next}\"));\n    HookAction::Continue",
            "return cs16::HookAction::Continue;",
            "return cs16mod.Continue",
            "public fw_ChangeLevel(const s1[], const s2[]) { return FMRES_IGNORED; }",
        ),
    };

    let default_phase = if phases == "Post" { "post" } else { "pre" };
    let rust_phase = if phases == "Post" { "post" } else { "pre" };
    let cpp_phase = if phases == "Post" {
        "HookPhase::Post"
    } else {
        "HookPhase::Pre"
    };
    let go_phase = if phases == "Post" {
        "cs16mod.PhasePost"
    } else {
        "cs16mod.PhasePre"
    };

    let py_code = format!(
        "@mod.hook(\"{snake}\", phase=\"{default_phase}\")\ndef on_{snake}(ev):\n    {py_body}"
    );
    let rust_code = format!(
        "#[cs_mod_hook({pascal}, {rust_phase})]\npub fn on_{snake}(host: &mut impl HostBridge) -> HookAction {{\n    {rust_body}\n}}"
    );
    let cpp_code = if cpp_body.starts_with("CS16_ON_CHAT") {
        cpp_body.to_string()
    } else {
        format!(
            "CS16_ON_HOOK(cs16::HookId::{pascal}, cs16::{cpp_phase}, on_{snake}) {{\n    {cpp_body}\n}}"
        )
    };
    let go_code = if go_body.starts_with("cs16mod.On") {
        go_body.to_string()
    } else {
        format!(
            "cs16mod.OnHook(cs16mod.Hook{pascal}, {go_phase}, 0, func() cs16mod.HookAction {{\n    {go_body}\n}})"
        )
    };
    let amxx_code = amxx_body.to_string();

    HookDoc {
        id,
        pascal,
        snake,
        category,
        cat_key,
        phases,
        mutable_pre,
        ctx_struct,
        amxx_equiv,
        summary,
        args,
        override_note,
        py_code,
        rust_code,
        cpp_code,
        go_code,
        amxx_code,
    }
}

// ===========================================================================
// 2. Python Stub & Polyglot SDK API Introspection (`sdk/python/cs16.pyi`)
// ===========================================================================

#[derive(Debug, Clone)]
struct PyStubInfo {
    classes: BTreeMap<String, Vec<(String, String)>>,
}

fn introspect_python_stub(workspace_root: &Path) -> Result<PyStubInfo, String> {
    let stub_path = workspace_root.join("sdk/python/cs16.pyi");
    let src = fs::read_to_string(&stub_path).map_err(|e| io_err(&stub_path, &e))?;
    let mut classes: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut current_class = String::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("class ") {
            let cname = rest
                .split([':', '('])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            current_class = cname.clone();
            classes.entry(cname).or_default();
            i += 1;
            continue;
        }
        let trimmed = line.trim();
        if !current_class.is_empty() && trimmed.starts_with("def ") {
            let mut sig = trimmed.to_string();
            while !sig.contains("->") && i + 1 < lines.len() && !sig.ends_with("...") {
                i += 1;
                sig.push(' ');
                sig.push_str(lines[i].trim());
            }
            let sig_clean = sig
                .trim_end_matches("...")
                .trim_end_matches(':')
                .trim()
                .replace("self, ", "")
                .replace("(self)", "()");
            let name = trimmed
                .trim_start_matches("def ")
                .split('(')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !name.starts_with("__") || name == "__init__" {
                classes
                    .entry(current_class.clone())
                    .or_default()
                    .push((name, sig_clean));
            }
        }
        i += 1;
    }
    Ok(PyStubInfo { classes })
}

struct ApiMethodDoc {
    anchor: String,
    name: String,
    capability: &'static str,
    py_sig: String,
    rust_sig: String,
    cpp_sig: String,
    go_sig: String,
    summary: String,
}

struct ApiGroupDoc {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    entries: Vec<ApiMethodDoc>,
}

fn find_py_sig(py: &PyStubInfo, class_name: &str, method: &str, fallback: &str) -> String {
    if let Some(methods) = py.classes.get(class_name)
        && let Some((_, sig)) = methods.iter().find(|(n, _)| n == method)
    {
        return sig.clone();
    }
    fallback.to_string()
}

fn build_api_groups(py: &PyStubInfo) -> Vec<ApiGroupDoc> {
    let m = |group: &str,
             class_name: &str,
             name: &str,
             cap: &'static str,
             py_fallback: &str,
             rust_sig: &str,
             cpp_sig: &str,
             go_sig: &str,
             summary: &str| {
        ApiMethodDoc {
            anchor: slugify(&format!("{group}-{name}")),
            name: name.to_string(),
            capability: cap,
            py_sig: find_py_sig(py, class_name, name, py_fallback),
            rust_sig: rust_sig.to_string(),
            cpp_sig: cpp_sig.to_string(),
            go_sig: go_sig.to_string(),
            summary: summary.to_string(),
        }
    };

    vec![
        ApiGroupDoc {
            id: "mod-lifecycle",
            title: "Mod & Lifecycle",
            description: "Plugin metadata, decorators, hook/command registration, and server clock queries.",
            entries: vec![
                m(
                    "mod",
                    "Mod",
                    "info",
                    "none",
                    "mod.info(id, name, version, permissions=...)",
                    "mod.toml [mod]",
                    "CS16_PLUGIN_INFO(id, name, ver)",
                    "mod.toml [mod]",
                    "Declares plugin ID, metadata, and sandbox capability list.",
                ),
                m(
                    "mod",
                    "Mod",
                    "hook",
                    "none",
                    "mod.hook(hook, phase='pre', priority=0)",
                    "#[cs_mod_hook(HookId, pre|post, prio)]",
                    "CS16_ON_HOOK(HookId, Phase, fn)",
                    "cs16mod.OnHook(hook, phase, prio, fn)",
                    "Subscribes to any of the 52 ABI v1/v2 Pre or Post hooks.",
                ),
                m(
                    "mod",
                    "Mod",
                    "chat_command",
                    "commands",
                    "mod.chat_command('/vip', '!vip', suppress=True)",
                    "#[cs_mod_chat(\"/vip\", \"!vip\")]",
                    "CS16_ON_CHAT(\"/vip\", fn)",
                    "cs16mod.OnChat(\"/vip\", fn)",
                    "Registers case-insensitive chat triggers with optional message suppression.",
                ),
                m(
                    "mod",
                    "Mod",
                    "console_command",
                    "commands",
                    "mod.console_command('amx_kick')",
                    "#[cs_mod_cmd(\"amx_kick\")]",
                    "server::register_command(\"amx_kick\")",
                    "cs16mod.OnCommand(\"amx_kick\", fn)",
                    "Registers a client or server console command.",
                ),
                m(
                    "mod",
                    "Mod",
                    "players",
                    "entities",
                    "mod.players(alive_only=False, team=None) -> list[Player]",
                    "for s in 0..host.player_max_clients()",
                    "for (int i=0; i<server::max_clients(); ++i)",
                    "cs16mod.ActivePlayers()",
                    "Returns connected players optionally filtered by alive state or team.",
                ),
                m(
                    "mod",
                    "Mod",
                    "player",
                    "entities",
                    "mod.player(slot) -> Optional[Player]",
                    "Player::new(slot)",
                    "cs16::Player(slot)",
                    "cs16mod.NewPlayer(slot)",
                    "Looks up a connected player handle by 0-based slot (0..31).",
                ),
                m(
                    "mod",
                    "Mod",
                    "log",
                    "none",
                    "mod.log(*args) -> None",
                    "host.print_all(text)",
                    "cs16_log(ptr, len)",
                    "fmt.Println(...) // WASI fd_write",
                    "Writes structured plugin log lines captured by ResMon and Playground console.",
                ),
            ],
        },
        ApiGroupDoc {
            id: "player",
            title: "Player",
            description: "Read and mutate player vitals, economy, loadout, movement, rendering, team, and status.",
            entries: vec![
                m(
                    "player",
                    "Player",
                    "set_health",
                    "entities",
                    "player.set_health(hp: float)",
                    "p.set_health(host, hp)",
                    "p.set_health(hp)",
                    "p.SetHealth(hp)",
                    "Sets the player's current health points.",
                ),
                m(
                    "player",
                    "Player",
                    "set_max_health",
                    "entities",
                    "player.set_max_health(hp: float)",
                    "p.set_max_health(host, hp)",
                    "p.set_max_health(hp)",
                    "p.SetMaxHealth(hp)",
                    "Raises or lowers the player's maximum HP cap.",
                ),
                m(
                    "player",
                    "Player",
                    "set_armor",
                    "entities",
                    "player.set_armor(amount, kind=Armor.VESTHELM)",
                    "p.set_armor(host, amt, ArmorKind::VestHelm)",
                    "p.set_armor(amt, ArmorKind::VestHelm)",
                    "p.SetArmor(amt, ArmorVestHelm)",
                    "Grants Kevlar or Kevlar+Helmet armor protection.",
                ),
                m(
                    "player",
                    "Player",
                    "set_money",
                    "entities",
                    "player.set_money(amount, flash=True)",
                    "p.set_money(host, amt, true)",
                    "p.set_money(amt, true)",
                    "p.SetMoney(amt, true)",
                    "Sets exact CS 1.6 cash balance ($0..$16000) with optional HUD flash.",
                ),
                m(
                    "player",
                    "Player",
                    "add_money",
                    "entities",
                    "player.add_money(delta, flash=True)",
                    "p.add_money(host, delta, true)",
                    "p.add_money(delta, true)",
                    "p.AddMoney(delta, true)",
                    "Adds or subtracts cash from the player's account.",
                ),
                m(
                    "player",
                    "Player",
                    "set_score",
                    "entities",
                    "player.set_score(frags, deaths)",
                    "p.set_score(host, frags, deaths)",
                    "p.set_score(frags, deaths)",
                    "p.SetScore(frags, deaths)",
                    "Updates scoreboard frags and deaths immediately.",
                ),
                m(
                    "player",
                    "Player",
                    "reset_score",
                    "entities",
                    "player.reset_score()",
                    "p.reset_score(host)",
                    "p.reset_score()",
                    "p.ResetScore()",
                    "Resets player frags and deaths to 0 / 0.",
                ),
                m(
                    "player",
                    "Player",
                    "set_team",
                    "entities",
                    "player.set_team(team, model_class=None)",
                    "p.set_team(host, TeamKind::Ct, None)",
                    "p.set_team(Team::Ct, -1)",
                    "p.SetTeam(TeamCT, -1)",
                    "Moves a player between Terrorist, CT, and Spectator.",
                ),
                m(
                    "player",
                    "Player",
                    "respawn",
                    "entities",
                    "player.respawn()",
                    "p.respawn(host)",
                    "p.respawn()",
                    "p.Respawn()",
                    "Immediately respawns a dead or spectating player (CSDM / Arena).",
                ),
                m(
                    "player",
                    "Player",
                    "slap",
                    "entities",
                    "player.slap(damage=0, random_dir=True)",
                    "p.slap(host, dmg, true)",
                    "p.slap(dmg, true)",
                    "p.Slap(dmg, true)",
                    "Applies admin slap damage and random knockback impulse.",
                ),
                m(
                    "player",
                    "Player",
                    "slay",
                    "entities",
                    "player.slay(preserve_frags=False)",
                    "p.slay(host, false)",
                    "p.slay(false)",
                    "p.Slay(false)",
                    "Instantly slays the player.",
                ),
                m(
                    "player",
                    "Player",
                    "damage",
                    "entities",
                    "player.damage(amount, attacker=None, bits=0)",
                    "p.damage(host, attacker, dmg, bits)",
                    "p.damage(atk, dmg, bits)",
                    "p.Damage(atk, dmg, bits)",
                    "Deals programmatic damage through the PlayerTakeDamage hook chain.",
                ),
                m(
                    "player",
                    "Player",
                    "set_godmode",
                    "entities",
                    "player.set_godmode(enabled=True)",
                    "p.set_godmode(host, true)",
                    "p.set_godmode(true)",
                    "p.SetGodmode(true)",
                    "Toggles FL_GODMODE invulnerability.",
                ),
                m(
                    "player",
                    "Player",
                    "set_noclip",
                    "entities",
                    "player.set_noclip(enabled=True)",
                    "p.set_noclip(host, true)",
                    "p.set_noclip(true)",
                    "p.SetNoclip(true)",
                    "Toggles MOVETYPE_NOCLIP flight through walls.",
                ),
                m(
                    "player",
                    "Player",
                    "set_maxspeed",
                    "entities",
                    "player.set_maxspeed(speed: float)",
                    "p.set_maxspeed(host, speed)",
                    "p.set_maxspeed(speed)",
                    "p.SetMaxspeed(speed)",
                    "Overrides player max running speed in units/sec (default ~250).",
                ),
                m(
                    "player",
                    "Player",
                    "set_gravity",
                    "entities",
                    "player.set_gravity(gravity: float)",
                    "p.set_gravity(host, 0.5)",
                    "p.set_gravity(0.5f)",
                    "p.SetGravity(0.5)",
                    "Scales player gravity multiplier (1.0 = normal 800 sv_gravity).",
                ),
                m(
                    "player",
                    "Player",
                    "set_footsteps",
                    "entities",
                    "player.set_footsteps(silent=True)",
                    "p.set_footsteps(host, true)",
                    "p.set_footsteps(true)",
                    "p.SetFootsteps(true)",
                    "Enables or disables silent footsteps (e.g. ninja/assassin class).",
                ),
                m(
                    "player",
                    "Player",
                    "set_rendering",
                    "entities",
                    "player.set_rendering(fx, rgb, mode, amount)",
                    "p.set_rendering(host, fx, rgb, mode, amt)",
                    "p.set_rendering(fx, col, mode, amt)",
                    "p.SetRendering(fx, col, mode, amt)",
                    "Applies GoldSrc GlowShell aura, TransAlpha stealth, or TransAdd rendering.",
                ),
                m(
                    "player",
                    "Player",
                    "set_model",
                    "entities",
                    "player.set_model(model: str)",
                    "p.set_model(host, \"vip\")",
                    "p.set_model(\"vip\")",
                    "p.SetModel(\"vip\")",
                    "Overrides player third-person model name (reset with reset_model).",
                ),
                m(
                    "player",
                    "Player",
                    "set_origin",
                    "entities",
                    "player.set_origin((x, y, z))",
                    "host.ent_set_origin(p.edict_index(), pos)",
                    "p.set_origin(pos)",
                    "p.SetOrigin(pos)",
                    "Teleports the player to a 3D world coordinate.",
                ),
                m(
                    "player",
                    "Player",
                    "set_velocity",
                    "entities",
                    "player.set_velocity((vx, vy, vz))",
                    "host.ent_set_velocity(p.edict_index(), vel)",
                    "p.set_velocity(vel)",
                    "p.SetVelocity(vel)",
                    "Sets 3D velocity vector (parachute, knockback, jump pad).",
                ),
                m(
                    "player",
                    "Player",
                    "kick",
                    "commands",
                    "player.kick(reason='')",
                    "p.kick(host, reason)",
                    "p.kick(reason)",
                    "p.Kick(reason)",
                    "Disconnects the player with a reason string.",
                ),
            ],
        },
        ApiGroupDoc {
            id: "entity-world",
            title: "Entity & World",
            description: "Inspect and spawn world edicts, raycast BSP collision, query/mutate CVars, play sounds, and change maps.",
            entries: vec![
                m(
                    "world",
                    "Mod",
                    "entity",
                    "entities",
                    "mod.entity(index) -> Optional[Entity]",
                    "host.ent_classname(idx)",
                    "cs16_ent_get_origin(idx, xyz)",
                    "cs16mod.EntOrigin(idx)",
                    "Fetches a snapshot handle for any edict index.",
                ),
                m(
                    "world",
                    "Mod",
                    "entities",
                    "entities",
                    "mod.entities(classname=None) -> list[Entity]",
                    "for idx in 1..host.ent_count()",
                    "cs16_ent_count()",
                    "cs16mod.EntCount()",
                    "Lists all world entities, optionally filtered by classname.",
                ),
                m(
                    "world",
                    "Mod",
                    "create_entity",
                    "entities",
                    "mod.create_entity(classname: str)",
                    "host.ent_create(classname)",
                    "cs16_ent_create(ptr, len)",
                    "cs16mod.EntCreate(classname)",
                    "Spawns a new named entity on the server.",
                ),
                m(
                    "world",
                    "Mod",
                    "remove_entity",
                    "entities",
                    "mod.remove_entity(index: int)",
                    "host.ent_remove(index)",
                    "cs16_ent_remove(index)",
                    "cs16mod.EntRemove(index)",
                    "Removes an entity edict safely.",
                ),
                m(
                    "world",
                    "Mod",
                    "trace_fraction",
                    "traces",
                    "mod.trace_fraction(start, end) -> float",
                    "host.trace_line_fraction(a, b)",
                    "cs16_trace_line(ax,ay,az,bx,by,bz)",
                    "cs16mod.TraceLine(a, b)",
                    "Casts a BSP solid ray from start to end and returns hit fraction (0.0..1.0).",
                ),
                m(
                    "world",
                    "Mod",
                    "cvar",
                    "cvars",
                    "mod.cvar(name, default='') -> str",
                    "host.cvar_get(name)",
                    "cs16_cvar_get(name, buf, cap)",
                    "cs16mod.CvarGet(name)",
                    "Reads any server CVar string (also cvar_float / cvar_int).",
                ),
                m(
                    "world",
                    "Mod",
                    "set_cvar",
                    "cvars",
                    "mod.set_cvar(name, value)",
                    "host.cvar_set(name, value)",
                    "cs16_cvar_set(name, val)",
                    "cs16mod.CvarSet(name, val)",
                    "Updates a server CVar live.",
                ),
                m(
                    "world",
                    "Mod",
                    "precache_sound",
                    "sound",
                    "mod.precache_sound(sample: str)",
                    "host.precache_sound(sample)",
                    "cs16_precache_sound(ptr, len)",
                    "cs16mod.PrecacheSound(sample)",
                    "Precaches a .wav sample during init for reliable client playback.",
                ),
                m(
                    "world",
                    "Mod",
                    "emit_sound",
                    "sound",
                    "mod.emit_sound(ent, sample, channel=0, volume=1.0)",
                    "host.emit_sound(ent, sample, ch, vol)",
                    "cs16_emit_sound(ent, ptr, len, ch, vol)",
                    "cs16mod.EmitSound(ent, sample, ch, vol)",
                    "Emits a spatialized sound from an entity or player.",
                ),
                m(
                    "world",
                    "Mod",
                    "change_level",
                    "commands",
                    "mod.change_level(map_name: str)",
                    "host.change_level(map_name)",
                    "server::change_level(map)",
                    "cs16mod.ChangeLevel(mapName)",
                    "Triggers an immediate server map change.",
                ),
                m(
                    "world",
                    "Mod",
                    "server_exec",
                    "commands",
                    "mod.server_exec(line: str)",
                    "host.server_exec(line)",
                    "server::exec(line)",
                    "cs16mod.ServerExec(line)",
                    "Enqueues a server console command (e.g. sv_restart 1).",
                ),
            ],
        },
        ApiGroupDoc {
            id: "weapons",
            title: "Weapons & CustomWeaponBuilder",
            description: "Grant/strip vanilla CS 1.6 weapons, control clip/bpammo, and register declarative custom weapon archetypes.",
            entries: vec![
                m(
                    "weapons",
                    "Player",
                    "give_item",
                    "weapons",
                    "player.give_item('weapon_ak47')",
                    "p.give_weapon(host, \"ak47\")",
                    "p.give_item(\"weapon_ak47\")",
                    "p.GiveItem(\"weapon_ak47\")",
                    "Grants any CS 1.6 weapon or equipment item (weapon_*, item_*).",
                ),
                m(
                    "weapons",
                    "Player",
                    "strip_weapons",
                    "weapons",
                    "player.strip_weapons(keep_knife=False)",
                    "p.strip_weapons(host, false)",
                    "p.strip_weapons(false)",
                    "p.StripWeapons(false)",
                    "Removes carried weapons (optionally retaining the knife).",
                ),
                m(
                    "weapons",
                    "Player",
                    "set_weapon_clip",
                    "weapons",
                    "player.set_weapon_clip(weapon_id, clip)",
                    "p.set_weapon_clip(host, wid, clip)",
                    "p.set_weapon_clip(wid, clip)",
                    "p.SetWeaponClip(wid, clip)",
                    "Sets current magazine round count for a carried weapon.",
                ),
                m(
                    "weapons",
                    "Player",
                    "set_bpammo",
                    "weapons",
                    "player.set_bpammo(weapon_id, amount)",
                    "p.set_bpammo(host, wid, amt)",
                    "p.set_bpammo(wid, amt)",
                    "p.SetBpammo(wid, amt)",
                    "Sets reserve backpack ammo for a weapon.",
                ),
                m(
                    "weapons",
                    "Player",
                    "set_defuser",
                    "weapons",
                    "player.set_defuser(has=True)",
                    "p.set_defuser(host, true)",
                    "p.set_defuser(true)",
                    "p.SetDefuser(true)",
                    "Grants or removes the CT defuse kit.",
                ),
                m(
                    "weapons",
                    "Player",
                    "set_nvg",
                    "weapons",
                    "player.set_nvg(has=True)",
                    "p.set_nvg(host, true)",
                    "p.set_nvg(true)",
                    "p.SetNvg(true)",
                    "Grants or removes nightvision goggles.",
                ),
                m(
                    "weapons",
                    "Player",
                    "drop_slot",
                    "weapons",
                    "player.drop_slot(inv_slot: int)",
                    "p.drop_slot(host, slot)",
                    "p.drop_slot(slot)",
                    "p.DropSlot(slot)",
                    "Forces the player to drop a weapon inventory slot (1=Primary, 2=Secondary).",
                ),
                m(
                    "weapons",
                    "Mod",
                    "register_custom_weapon",
                    "weapons",
                    "mod.register_custom_weapon(CustomWeapon(...))",
                    "CustomWeaponBuilder::new(id, base).register(host)",
                    "cs16_register_custom_weapon(spec)",
                    "cs16mod.RegisterCustomWeapon(spec)",
                    "Registers a declarative custom weapon (v_/p_/w_ models, fire sound, damage_mult, clip_size, recoil_mult, knockback, cost).",
                ),
                m(
                    "weapons",
                    "Player",
                    "give_custom_weapon",
                    "weapons",
                    "player.give_custom_weapon('golden_ak47')",
                    "p.give_custom_weapon(host, \"golden_ak47\")",
                    "p.give_custom_weapon(\"golden_ak47\")",
                    "p.GiveCustomWeapon(\"golden_ak47\")",
                    "Grants a previously registered custom weapon archetype to the player.",
                ),
            ],
        },
        ApiGroupDoc {
            id: "menus",
            title: "Menus (PaginatedMenu & ShowMenu)",
            description: "Host-managed paginated GoldSrc menus (1..7, 8. Back, 9. More, 0. Exit) with authentic \\y/\\w/\\r/\\d/\\R escapes and zero menuselect collisions.",
            entries: vec![
                m(
                    "menus",
                    "Menu",
                    "__init__",
                    "menus",
                    "Menu(title, callback, per_page=7, allow_exit=True)",
                    "PaginatedMenu::new(title, cb_id)",
                    "Menu::create(title, cb_id)",
                    "cs16mod.NewMenu(title, cb)",
                    "Creates a host-managed paginated menu in MenuManager.",
                ),
                m(
                    "menus",
                    "Menu",
                    "add_item",
                    "menus",
                    "menu.add_item(label, info='', enabled=True)",
                    ".item_enabled(label, info, enabled)",
                    "menu.add_item(label, info, enabled)",
                    "menu.AddItem(label, info, enabled)",
                    "Appends a selectable numbered menu entry with opaque callback info.",
                ),
                m(
                    "menus",
                    "Menu",
                    "add_blank",
                    "menus",
                    "menu.add_blank()",
                    ".blank()",
                    "menu.add_blank()",
                    "menu.AddBlank()",
                    "Inserts a blank spacer line without consuming a key slot.",
                ),
                m(
                    "menus",
                    "Menu",
                    "add_text",
                    "menus",
                    "menu.add_text(label: str)",
                    ".text(line)",
                    "menu.add_text(line)",
                    "menu.AddText(line)",
                    "Inserts a non-selectable descriptive text line.",
                ),
                m(
                    "menus",
                    "Menu",
                    "show",
                    "menus",
                    "menu.show(player, page=0, time=-1)",
                    ".display_to(host, slot, 0, 15)",
                    "menu.display(player, 0, 15)",
                    "menu.Display(player, 0, 15)",
                    "Displays the menu to a player slot and binds menuselect ownership.",
                ),
                m(
                    "menus",
                    "Menu",
                    "destroy",
                    "menus",
                    "menu.destroy()",
                    "host.menu_destroy(menu_id)",
                    "menu.destroy()",
                    "menu.Destroy()",
                    "Releases the menu definition from MenuManager.",
                ),
            ],
        },
        ApiGroupDoc {
            id: "hud-vfx",
            title: "HUD, DHUD, MOTD & 1-Line VFX",
            description: "Classic chat colors, TE_TEXTMESSAGE HUD, bold DHUD banners, VGUI2 MOTD windows, ScreenFade/Shake, and all 12 Protocol 48 TE_* VFX emitters.",
            entries: vec![
                m(
                    "hud",
                    "Player",
                    "chat",
                    "messages",
                    "player.chat(text) / mod.chat_all(text)",
                    "p.say_text(host, text)",
                    "p.say_text(text)",
                    "p.SayText(text)",
                    "Sends CS 1.6 colored SayText chat (\\x01 yellow, \\x03 team, \\x04 green).",
                ),
                m(
                    "hud",
                    "Player",
                    "center_print",
                    "messages",
                    "player.center_print(text)",
                    "host.center_print(Some(slot), text)",
                    "p.center_print(text)",
                    "p.CenterPrint(text)",
                    "Displays centered white GoldSrc banner text.",
                ),
                m(
                    "hud",
                    "Player",
                    "hud",
                    "messages",
                    "player.hud(text, x=-1.0, y=0.35, channel=1, rgba=...)",
                    "host.hud_message(&HudMessageSpec { .. })",
                    "p.hud_message(text, x, y, col, ch)",
                    "p.HudMessage(text, x, y, col, ch)",
                    "Displays a TE_TEXTMESSAGE HUD overlay on channel 0..4.",
                ),
                m(
                    "hud",
                    "Player",
                    "dhud",
                    "messages",
                    "player.dhud(text, x=-1.0, y=0.25, rgb=...)",
                    "host.dhud_message(&DhudMessageSpec { .. })",
                    "p.dhud_message(text, x, y, col)",
                    "p.DhudMessage(text, x, y, col)",
                    "Displays a large bold Director HUD (DRC_CMD_MESSAGE) banner.",
                ),
                m(
                    "hud",
                    "Player",
                    "show_motd",
                    "messages",
                    "player.show_motd(title, body_html)",
                    "p.show_motd(host, title, html)",
                    "p.show_motd(title, html)",
                    "p.ShowMotd(title, html)",
                    "Opens the VGUI2 MOTD.res HTML/text viewer window.",
                ),
                m(
                    "hud",
                    "Player",
                    "screen_fade",
                    "messages",
                    "player.screen_fade(duration, hold, flags, rgba)",
                    "p.screen_fade(host, dur, hold, flags, rgba)",
                    "p.screen_fade(dur, hold, flags, col)",
                    "p.ScreenFade(dur, hold, flags, col)",
                    "Flashes or tints a player's screen via ScreenFade UserMsg.",
                ),
                m(
                    "hud",
                    "Player",
                    "screen_shake",
                    "messages",
                    "player.screen_shake(amplitude, duration, frequency)",
                    "p.screen_shake(host, amp, dur, freq)",
                    "p.screen_shake(amp, dur, freq)",
                    "p.ScreenShake(amp, dur, freq)",
                    "Shakes the player's camera view via ScreenShake UserMsg.",
                ),
                m(
                    "hud",
                    "Player",
                    "bar_time",
                    "messages",
                    "player.bar_time(seconds: int)",
                    "p.bar_time(host, secs)",
                    "p.bar_time(secs)",
                    "p.BarTime(secs)",
                    "Displays the classic CS 1.6 C4-style progress bar (0 cancels).",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "shockwave_ring",
                    "vfx",
                    "vfx.shockwave_ring(origin, radius, rgba, life, width)",
                    "vfx::shockwave_ring(host, pos, r, rgba, life, w, slot)",
                    "vfx::shockwave_ring(pos, r, col, life, w)",
                    "VFX.ShockwaveRing(pos, r, col, life, w, slot)",
                    "TE_BEAMCYLINDER expanding shockwave ring.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "laser_beam",
                    "vfx",
                    "vfx.laser_beam(start, end, rgba, life, width, noise)",
                    "vfx::laser_beam(host, a, b, rgba, life, w, noise, slot)",
                    "vfx::laser_beam(a, b, col, life, w, noise)",
                    "VFX.LaserBeam(a, b, col, life, w, noise, slot)",
                    "TE_BEAMPOINTS laser/tracer/tripwire beam.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "follow_trail",
                    "vfx",
                    "vfx.follow_trail(ent, rgba, life, width)",
                    "vfx::follow_trail(host, ent, rgba, life, w, slot)",
                    "vfx::follow_trail(ent, col, life, w)",
                    "VFX.FollowTrail(ent, col, life, w, slot)",
                    "TE_BEAMFOLLOW ribbon trail attached to an entity or grenade.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "kill_beam",
                    "vfx",
                    "vfx.kill_beam(ent)",
                    "vfx::kill_beam(host, ent, slot)",
                    "vfx::kill_beam(ent)",
                    "VFX.KillBeam(ent, slot)",
                    "TE_KILLBEAM removes beams attached to an entity.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "aura_light",
                    "vfx",
                    "vfx.aura_light(origin, radius, rgb, life, decay)",
                    "vfx::aura_light(host, pos, r, rgb, life, decay, slot)",
                    "vfx::aura_light(pos, r, col, life, decay)",
                    "VFX.AuraLight(pos, r, col, life, decay, slot)",
                    "TE_DLIGHT dynamic point illumination glow.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "sparks",
                    "vfx",
                    "vfx.sparks(origin)",
                    "vfx::sparks(host, pos, slot)",
                    "vfx::sparks(pos)",
                    "VFX.Sparks(pos, slot)",
                    "TE_SPARKS metallic impact spark shower.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "implosion",
                    "vfx",
                    "vfx.implosion(origin, radius, count, life)",
                    "vfx::implosion(host, pos, r, cnt, life, slot)",
                    "vfx::implosion(pos, r, cnt, life)",
                    "VFX.Implosion(pos, r, cnt, life, slot)",
                    "TE_IMPLOSION inward-converging particle burst.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "teleport_splash",
                    "vfx",
                    "vfx.teleport_splash(origin)",
                    "vfx::teleport_splash(host, pos, slot)",
                    "vfx::teleport_splash(pos)",
                    "VFX.TeleportSplash(pos, slot)",
                    "TE_TELEPORT GoldSrc teleport particle cloud.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "lava_splash",
                    "vfx",
                    "vfx.lava_splash(origin)",
                    "vfx::lava_splash(host, pos, slot)",
                    "vfx::lava_splash(pos)",
                    "VFX.LavaSplash(pos, slot)",
                    "TE_LAVASPLASH red/orange particle fountain.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "explosion",
                    "vfx",
                    "vfx.explosion(origin, scale, flags)",
                    "vfx::explosion(host, pos, scale, flags, slot)",
                    "vfx::explosion(pos, scale, flags)",
                    "VFX.Explosion(pos, scale, flags, slot)",
                    "TE_EXPLOSION fireball sprite, particles, and flash.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "blood_stream",
                    "vfx",
                    "vfx.blood_stream(origin, dir, color, speed)",
                    "vfx::blood_stream(host, pos, dir, col, spd, slot)",
                    "vfx::blood_stream(pos, dir, col, spd)",
                    "VFX.BloodStream(pos, dir, col, spd, slot)",
                    "TE_BLOODSTREAM directional crimson blood jet.",
                ),
                m(
                    "vfx",
                    "Vfx",
                    "world_decal",
                    "vfx",
                    "vfx.world_decal(origin, '{scorch1')",
                    "vfx::world_decal(host, pos, name, slot)",
                    "vfx::world_decal(pos, name)",
                    "VFX.WorldDecal(pos, name, slot)",
                    "TE_WORLDDECAL stamps a permanent WAD decal onto BSP geometry.",
                ),
            ],
        },
        ApiGroupDoc {
            id: "timers-storage-leaderboard",
            title: "Timers, Storage (KV) & Leaderboard",
            description: "Deterministic server-tick timers, reload-safe KV storage, and shared O(log N) sorted leaderboards.",
            entries: vec![
                m(
                    "timers",
                    "Mod",
                    "timer",
                    "timers",
                    "@mod.timer(interval, repeat=True, arg=0)",
                    "host.timer_repeat(ms, cb_id, arg)",
                    "server::timer_repeat(ms, cb_id, arg)",
                    "cs16mod.TimerRepeat(ms, fn)",
                    "Schedules a repeating or one-shot timer on the 100 Hz tick loop.",
                ),
                m(
                    "timers",
                    "Mod",
                    "set_timer",
                    "timers",
                    "mod.set_timer(delay_sec, callback, repeat=False)",
                    "host.timer_after(ms, cb_id, arg)",
                    "server::timer_after(ms, cb_id, arg)",
                    "cs16mod.TimerAfter(ms, fn)",
                    "Dynamically schedules a callback and returns its timer_id.",
                ),
                m(
                    "timers",
                    "Mod",
                    "cancel_timer",
                    "timers",
                    "mod.cancel_timer(timer_id: int)",
                    "host.timer_cancel(timer_id)",
                    "cs16_timer_cancel(timer_id)",
                    "cs16mod.TimerCancel(timerID)",
                    "Cancels a pending or repeating timer.",
                ),
                m(
                    "storage",
                    "Storage",
                    "get",
                    "storage",
                    "mod.storage.get(key, default=None)",
                    "host.storage_get(key)",
                    "cs16_storage_get(key, buf, cap)",
                    "cs16mod.StorageGet(key)",
                    "Reads a persistent per-plugin key preserved across live hot-reloads.",
                ),
                m(
                    "storage",
                    "Storage",
                    "set",
                    "storage",
                    "mod.storage.set(key, value)",
                    "host.storage_set(key, val)",
                    "cs16_storage_set(key, val)",
                    "cs16mod.StorageSet(key, val)",
                    "Persists a key-value entry backed by ConfigStorage / NVault.",
                ),
                m(
                    "storage",
                    "Storage",
                    "delete",
                    "storage",
                    "mod.storage.delete(key) -> bool",
                    "host.storage_delete(key)",
                    "cs16_storage_delete(key)",
                    "cs16mod.StorageDelete(key)",
                    "Removes a key from plugin KV storage.",
                ),
                m(
                    "leaderboard",
                    "Leaderboard",
                    "upsert",
                    "storage",
                    "mod.leaderboard.upsert(table, key, score, payload)",
                    "host.leaderboard_upsert(tbl, k, score, json)",
                    "cs16_leaderboard_upsert(...)",
                    "cs16mod.LeaderboardUpsert(tbl, k, score, json)",
                    "Inserts or updates a ranked entry in O(log N) time.",
                ),
                m(
                    "leaderboard",
                    "Leaderboard",
                    "rank",
                    "storage",
                    "mod.leaderboard.rank(table, key)",
                    "host.leaderboard_rank(tbl, key)",
                    "cs16_leaderboard_rank(...)",
                    "cs16mod.LeaderboardRank(tbl, key)",
                    "Returns 1-based rank, total count, score, and JSON payload.",
                ),
                m(
                    "leaderboard",
                    "Leaderboard",
                    "top",
                    "storage",
                    "mod.leaderboard.top(table, offset=0, limit=15)",
                    "host.leaderboard_top(tbl, off, lim)",
                    "cs16_leaderboard_top(...)",
                    "cs16mod.LeaderboardTop(tbl, off, lim)",
                    "Fetches paginated top-N leaderboard rows (e.g. /top15).",
                ),
            ],
        },
        ApiGroupDoc {
            id: "webhooks-rpc-i18n-admin",
            title: "Webhooks, Polyglot RPC, i18n & Admin RBAC",
            description: "Allowlisted async HTTP webhooks, cross-language synchronous RPC, per-player translations, and unified AMXX a..z access flags.",
            entries: vec![
                m(
                    "webhooks",
                    "Mod",
                    "http_post_json",
                    "http:<host>",
                    "mod.http_post_json(url, body, callback=None)",
                    "host.http_post_json(url, json, cb_id)",
                    "server::http_post_json(url, json, cb)",
                    "cs16mod.HttpPostJSON(url, json, cb)",
                    "Sends a non-blocking HTTP POST JSON request off the 100 Hz tick thread (strictly host-allowlisted).",
                ),
                m(
                    "rpc",
                    "Mod",
                    "export_fn",
                    "rpc",
                    "@mod.export_fn('vip.is_vip')",
                    "host.rpc_export(name, cb_id)",
                    "cs16_rpc_export(name, cb_id)",
                    "cs16mod.ExportFn(name, fn)",
                    "Exports a named function callable synchronously by Python, Rust, C++, Go, and AMXX plugins.",
                ),
                m(
                    "rpc",
                    "Mod",
                    "call_fn",
                    "rpc",
                    "mod.call_fn('vip.is_vip', slot)",
                    "host.rpc_call_i32(name, args)",
                    "cs16_rpc_call(name, args)",
                    "cs16mod.CallFn(name, args...)",
                    "Invokes any exported polyglot RPC function synchronously.",
                ),
                m(
                    "rpc",
                    "Mod",
                    "emit_event",
                    "rpc",
                    "mod.emit_event('vip.purchased', payload)",
                    "host.rpc_emit_event(name, json)",
                    "cs16_rpc_emit_event(name, json)",
                    "cs16mod.EmitEvent(name, json)",
                    "Broadcasts a structured event to all subscribed plugins.",
                ),
                m(
                    "rpc",
                    "Mod",
                    "on_event",
                    "rpc",
                    "@mod.on_event('vip.purchased')",
                    "host.rpc_subscribe(name, cb_id)",
                    "cs16_rpc_subscribe(name, cb_id)",
                    "cs16mod.OnEvent(name, fn)",
                    "Subscribes a callback to a cross-plugin broadcast event.",
                ),
                m(
                    "i18n",
                    "Player",
                    "tr",
                    "messages",
                    "player.tr('KEY', name=player.name)",
                    "p.tr(host, \"KEY\", &[(\"name\", &n)])",
                    "p.tr(\"KEY\", args)",
                    "p.Tr(\"KEY\", cs16mod.Arg(\"name\", n))",
                    "Translates a LangCatalog key in the player's chosen language (/lang en|ru).",
                ),
                m(
                    "i18n",
                    "Mod",
                    "load_lang_toml",
                    "messages",
                    "mod.load_lang_toml(toml_src)",
                    "host.lang_load_toml(src)",
                    "server::lang_load_toml(src)",
                    "cs16mod.LangLoadTOML(src)",
                    "Loads localized translation dictionaries into the shared LangCatalog.",
                ),
                m(
                    "admin",
                    "Player",
                    "has_flag",
                    "admins",
                    "player.has_flag('t')",
                    "p.has_flag(host, 't')",
                    "p.has_flag('t')",
                    "p.HasFlag('t')",
                    "Checks whether the player holds AMXX RBAC access flag(s) from configs/admins.toml or users.ini.",
                ),
            ],
        },
    ]
}

// ===========================================================================
// 3. AMX Mod X Native Introspection (`crates/cs-amxx/src/natives.rs`)
// ===========================================================================

#[derive(Debug, Clone)]
struct AmxxNativeDoc {
    native: String,
    module: &'static str,
    py_equiv: &'static str,
    rust_equiv: &'static str,
    notes: &'static str,
}

fn introspect_amxx_natives(workspace_root: &Path) -> Result<Vec<AmxxNativeDoc>, String> {
    let path = workspace_root.join("crates/cs-amxx/src/natives.rs");
    let src = fs::read_to_string(&path).map_err(|e| io_err(&path, &e))?;
    let mut names = BTreeSet::new();
    for line in src.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('"') || !trimmed.contains("=>") {
            continue;
        }
        let Some((lhs, _)) = trimmed.split_once("=>") else {
            continue;
        };
        for arm in lhs.split('|') {
            let tok = arm.trim();
            if tok.len() >= 3 && tok.starts_with('"') && tok.ends_with('"') {
                let inner = &tok[1..tok.len() - 1];
                if inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    names.insert(inner.to_string());
                }
            }
        }
    }
    let out = names.into_iter().map(classify_amxx_native).collect();
    Ok(out)
}

fn classify_amxx_native(native: String) -> AmxxNativeDoc {
    let (module, py_equiv, rust_equiv, notes) = match native.as_str() {
        "register_plugin" => (
            "amxmodx",
            "@mod.info(id=..., name=..., version=...)",
            "mod.toml [mod]",
            "Declares plugin metadata",
        ),
        "register_clcmd" => (
            "amxmodx",
            "@mod.chat_command(...) / @mod.console_command(...)",
            "#[cs_mod_chat(...)] / #[cs_mod_cmd(...)]",
            "Client chat or console command",
        ),
        "register_concmd" => (
            "amxmodx",
            "@mod.console_command(...)",
            "#[cs_mod_cmd(...)]",
            "Admin console command with RBAC flag check",
        ),
        "register_srvcmd" => (
            "amxmodx",
            "@mod.hook('server_command', 'pre')",
            "#[cs_mod_hook(ServerCommand, pre)]",
            "Server/RCON console command",
        ),
        "register_event" => (
            "amxmodx",
            "@mod.hook('round_start'|'player_killed', ...)",
            "#[cs_mod_hook(RoundStart, post)]",
            "Event forward -> typed ABI v2 hook",
        ),
        "register_message" => (
            "amxmodx",
            "@mod.hook('eng_send_usermsg', 'pre')",
            "#[cs_mod_hook(EngSendUserMsg, pre)]",
            "Intercepts outgoing Protocol 48 UserMsg",
        ),
        "get_user_msgid" => (
            "amxmodx",
            "Built-in typed UserMsg helpers",
            "HostBridge typed methods",
            "No raw msgid lookup needed in ABI v2",
        ),
        "precache_model" | "precache_generic" => (
            "amxmodx",
            "CustomWeapon(v_model=...) / mod.toml assets",
            "CustomWeaponBuilder::v_model()",
            "Host precaches declared assets automatically",
        ),
        "precache_sound" => (
            "amxmodx",
            "mod.precache_sound(sample)",
            "host.precache_sound(sample)",
            "Precaches sound during init",
        ),
        "register_cvar" | "get_cvar_pointer" | "get_cvar_string" | "get_pcvar_string" => (
            "amxmodx",
            "mod.cvar(name, default)",
            "host.cvar_get(name)",
            "Reads CVar string value",
        ),
        "get_cvar_num" | "get_pcvar_num" => (
            "amxmodx",
            "mod.cvar_int(name, default)",
            "host.cvar_get_i32(name)",
            "Reads CVar integer value",
        ),
        "get_cvar_float" | "get_pcvar_float" => (
            "amxmodx",
            "mod.cvar_float(name, default)",
            "host.cvar_get_f32(name)",
            "Reads CVar float value",
        ),
        "set_cvar_num" | "set_cvar_float" | "set_cvar_string" | "set_pcvar_num"
        | "set_pcvar_float" | "set_pcvar_string" => (
            "amxmodx",
            "mod.set_cvar(name, value)",
            "host.cvar_set(name, val)",
            "Writes CVar value live",
        ),
        "set_task" => (
            "amxmodx",
            "mod.set_timer(delay, cb, repeat=...)",
            "host.timer_after / timer_repeat",
            "Schedules deterministic server timer",
        ),
        "remove_task" | "task_exists" => (
            "amxmodx",
            "mod.cancel_timer(timer_id)",
            "host.timer_cancel(timer_id)",
            "Cancels active timer handle",
        ),
        "client_print" => (
            "amxmodx",
            "player.chat(msg) / player.center_print(msg)",
            "p.say_text(host, msg)",
            "Print to chat, center, or console",
        ),
        "console_print" => (
            "amxmodx",
            "player.print(msg)",
            "host.print_client(slot, msg)",
            "Prints to client console",
        ),
        "server_print" | "log_amx" => (
            "amxmodx",
            "mod.log(msg)",
            "host.print_all(msg)",
            "Structured plugin log output",
        ),
        "server_cmd" => (
            "amxmodx",
            "mod.server_exec(line)",
            "host.server_exec(line)",
            "Executes server console command",
        ),
        "get_user_name" => (
            "amxmodx",
            "player.name",
            "p.name(host)",
            "Player display name",
        ),
        "get_user_authid" => (
            "amxmodx",
            "player.steamid",
            "p.steamid(host)",
            "Player SteamID",
        ),
        "get_user_ip" => ("amxmodx", "player.ip", "p.ip(host)", "Player IP address"),
        "get_user_health" => ("amxmodx", "player.health", "p.health(host)", "Current HP"),
        "get_user_armor" => (
            "amxmodx",
            "player.armor",
            "p.armor(host)",
            "Current armor value",
        ),
        "get_user_team" | "cs_get_user_team" => (
            "cstrike",
            "player.team",
            "p.team(host)",
            "TeamKind (T/CT/Spec)",
        ),
        "get_user_origin" => (
            "amxmodx",
            "player.origin",
            "p.origin(host)",
            "3D world origin Vec3",
        ),
        "get_user_weapon" => (
            "amxmodx",
            "player.active_weapon / player.clip / player.bpammo",
            "p.active_weapon(host)",
            "Active WeaponId and ammo",
        ),
        "get_user_flags" | "set_user_flags" => (
            "amxmodx",
            "player.has_flag('t') / player.admin_flags",
            "p.has_flag(host, 't')",
            "Unified AMXX a..z RBAC flags",
        ),
        "is_user_alive" => ("amxmodx", "player.alive", "p.is_alive(host)", "Alive check"),
        "is_user_connected" => (
            "amxmodx",
            "player.active",
            "p.is_active(host)",
            "Connected slot check",
        ),
        "is_user_bot" | "is_user_hltv" => (
            "amxmodx",
            "player.flags & FL_FAKECLIENT",
            "host.ent_get_flags(edict)",
            "Bot / HLTV proxy flag",
        ),
        "get_players" => (
            "amxmodx",
            "mod.players(alive_only=..., team=...)",
            "0..host.player_max_clients()",
            "Iterates active players",
        ),
        "get_maxplayers" => (
            "amxmodx",
            "mod.max_clients",
            "host.player_max_clients()",
            "Max server player slots",
        ),
        "get_gametime" => (
            "amxmodx",
            "mod.time",
            "host.time_ms() as f32 / 1000.0",
            "Server uptime in seconds",
        ),
        "read_argc" | "read_argv" | "read_args" => (
            "amxmodx",
            "args parameter in @mod.chat_command / ev.args",
            "host.hook_arg_str(0)",
            "Direct command argument string",
        ),
        "menu_create" => (
            "newmenus",
            "Menu(title, callback=fn)",
            "PaginatedMenu::new(title, cb_id)",
            "Creates shared paginated GoldSrc menu",
        ),
        "menu_additem" => (
            "newmenus",
            "menu.add_item(label, info, enabled)",
            ".item_enabled(label, info, en)",
            "Adds selectable menu entry",
        ),
        "menu_addblank" | "menu_addblank2" => (
            "newmenus",
            "menu.add_blank()",
            ".blank()",
            "Adds spacer line",
        ),
        "menu_addtext" | "menu_addtext2" => (
            "newmenus",
            "menu.add_text(label)",
            ".text(label)",
            "Adds static text line",
        ),
        "menu_setprop" => (
            "newmenus",
            "Menu(..., per_page=7, allow_exit=True)",
            ".per_page(7).allow_exit(true)",
            "Configures menu pagination/labels",
        ),
        "menu_display" => (
            "newmenus",
            "menu.show(player, page=0, time=-1)",
            ".display_to(host, slot, 0, -1)",
            "Displays menu to player",
        ),
        "menu_item_getinfo" => (
            "newmenus",
            "callback(player, info) parameter",
            "host.hook_arg_str(0)",
            "Passed directly to menu callback",
        ),
        "menu_destroy" => (
            "newmenus",
            "menu.destroy()",
            "host.menu_destroy(menu_id)",
            "Frees menu handle",
        ),
        "player_menu_info" => (
            "newmenus",
            "Managed automatically by MenuManager",
            "MenuManager::player_menu_info",
            "Per-slot menu isolation",
        ),
        "register_dictionary" | "LookupLangKey" => (
            "lang",
            "mod.load_lang_toml(...) / player.tr('KEY')",
            "p.tr(host, \"KEY\", &[])",
            "Shared EN/RU LangCatalog i18n",
        ),
        "get_stats" | "get_user_stats" | "get_statsnum" => (
            "csx",
            "mod.leaderboard.top / rank / count",
            "host.leaderboard_top / rank",
            "O(log N) shared LeaderboardStore",
        ),
        "nvault_open" | "nvault_get" | "nvault_lookup" | "nvault_set" | "nvault_pset"
        | "nvault_remove" | "nvault_prune" | "nvault_close" => (
            "nvault",
            "mod.storage.get / set / delete",
            "host.storage_get / set / delete",
            "Persistent KV store preserved on hot-reload",
        ),
        "cs_get_user_money" => (
            "cstrike",
            "player.money",
            "p.money(host)",
            "Player cash balance",
        ),
        "cs_set_user_money" => (
            "cstrike",
            "player.set_money(amt, flash=True)",
            "p.set_money(host, amt, true)",
            "Sets player cash",
        ),
        "cs_set_user_team" => (
            "cstrike",
            "player.set_team(Team.CT)",
            "p.set_team(host, TeamKind::Ct, None)",
            "Changes player team",
        ),
        "cs_get_user_armor" => (
            "cstrike",
            "player.armor / player.armor_kind",
            "p.armor(host) / p.armor_type(host)",
            "Reads armor amount & kind",
        ),
        "cs_set_user_armor" => (
            "cstrike",
            "player.set_armor(100, Armor.VESTHELM)",
            "p.set_armor(host, 100, ArmorKind::VestHelm)",
            "Sets Kevlar/VestHelm armor",
        ),
        "cs_get_user_deaths" => (
            "cstrike",
            "player.deaths",
            "p.deaths(host)",
            "Reads death count",
        ),
        "cs_set_user_deaths" => (
            "cstrike",
            "player.set_score(player.frags, deaths)",
            "p.set_score(host, frags, deaths)",
            "Updates scoreboard deaths",
        ),
        "set_user_health" => (
            "fun",
            "player.set_health(hp)",
            "p.set_health(host, hp)",
            "Sets player HP",
        ),
        "set_user_gravity" => (
            "fun",
            "player.set_gravity(mult)",
            "p.set_gravity(host, mult)",
            "Sets player gravity multiplier",
        ),
        "set_user_maxspeed" => (
            "fun",
            "player.set_maxspeed(speed)",
            "p.set_maxspeed(host, speed)",
            "Overrides max movement speed",
        ),
        "give_item" => (
            "fun",
            "player.give_item('weapon_ak47')",
            "p.give_weapon(host, \"ak47\")",
            "Grants weapon or equipment",
        ),
        "strip_user_weapons" => (
            "fun",
            "player.strip_weapons(keep_knife=False)",
            "p.strip_weapons(host, false)",
            "Strips carried inventory",
        ),
        "set_user_godmode" => (
            "fun",
            "player.set_godmode(True)",
            "p.set_godmode(host, true)",
            "Toggles invulnerability",
        ),
        "pev" | "set_pev" => (
            "fakemeta",
            "player.* / entity.* properties & setters",
            "host.ent_get_* / ent_set_*",
            "Typed entity field accessors (origin, velocity, health, flags, buttons)",
        ),
        s if s.starts_with("entity_get_") || s.starts_with("entity_set_") => (
            "engine",
            "entity.origin / velocity / health / flags",
            "host.ent_get_* / ent_set_*",
            "Direct typed entity accessors",
        ),
        "RegisterHam" => (
            "hamsandwich",
            "@mod.hook('player_spawn'|'player_take_damage'|'player_killed', ...)",
            "#[cs_mod_hook(PlayerTakeDamage, pre)]",
            "Maps Ham_* virtual hooks to ABI v2 HookId",
        ),
        "SetHamParamFloat" | "SetHamParamInteger" | "SetHamParamEntity" => (
            "hamsandwich",
            "ev.damage = new_dmg; return HookAction.OVERRIDE",
            "host.hook_set_override_f32(dmg); HookAction::Override",
            "Mutable Pre hook parameter override",
        ),
        "ExecuteHam" | "ExecuteHamB" => (
            "hamsandwich",
            "player.respawn() / player.damage(...)",
            "p.respawn(host) / p.damage(host, ...)",
            "Direct typed gameplay action call",
        ),
        "rg_give_item" => (
            "reapi",
            "player.give_item(name)",
            "p.give_weapon(host, name)",
            "Grants weapon with append/replace semantics",
        ),
        "rg_remove_all_items" => (
            "reapi",
            "player.strip_weapons()",
            "p.strip_weapons(host, false)",
            "Clears player weapons",
        ),
        "rg_set_user_armor" => (
            "reapi",
            "player.set_armor(amt, Armor.VESTHELM)",
            "p.set_armor(host, amt, kind)",
            "Sets armor + helmet",
        ),
        "rg_add_account" => (
            "reapi",
            "player.add_money(delta) / set_money(amt)",
            "p.add_money(host, delta, true)",
            "Adjusts player account balance",
        ),
        "rg_round_respawn" => (
            "reapi",
            "player.respawn()",
            "p.respawn(host)",
            "Instantly respawns player",
        ),
        "get_member" | "set_member" => (
            "reapi",
            "player.money / armor / has_defuser / clip / team",
            "Player typed getters/setters",
            "Clean typed fields replacing raw CBasePlayer offsets",
        ),
        "register_native" | "get_param" | "get_param_f" | "get_string" | "set_string" => (
            "rpc",
            "@mod.export_fn('ns.fn') / mod.call_fn(...)",
            "host.rpc_export / host.rpc_call",
            "Polyglot synchronous RPC across Python/Rust/C++/Go/AMXX",
        ),
        "CreateMultiForward" | "ExecuteForward" | "DestroyForward" => (
            "rpc",
            "@mod.on_event('name') / mod.emit_event(...)",
            "host.rpc_subscribe / rpc_emit_event",
            "Cross-plugin broadcast event bus",
        ),
        _ => (
            "amxmodx",
            "Native Python 3 built-ins (str, int, float, math)",
            "Native Rust std types",
            "Standard string/math helper built natively into host language",
        ),
    };

    AmxxNativeDoc {
        native,
        module,
        py_equiv,
        rust_equiv,
        notes,
    }
}

// ===========================================================================
// 4. Full 736/736 CVar & Console Command Introspection
// ===========================================================================

#[derive(Debug, Clone)]
struct CVarDoc {
    id: String,
    kind: &'static str,
    default_value: String,
    flags: Vec<&'static str>,
    category: &'static str,
    status: &'static str,
    description: String,
}

fn introspect_cvars(workspace_root: &Path) -> Result<Vec<CVarDoc>, String> {
    let toml_path = workspace_root.join("docs/parity/cvars.toml");
    let raw = fs::read_to_string(&toml_path).map_err(|e| io_err(&toml_path, &e))?;
    let cat = parity::parse_category("cvars", &raw).map_err(|e| e.to_string())?;
    let registry = CVarRegistry::new_with_defaults();
    let known_cmds: BTreeSet<String> = CVarRegistry::known_command_names()
        .map(|s| s.to_ascii_lowercase())
        .collect();

    let mut out = Vec::with_capacity(cat.items.len());
    for item in cat.items {
        let lower = item.id.to_ascii_lowercase();
        let status = match item.status {
            parity::Status::Verified => "verified",
            parity::Status::Tested => "tested",
            parity::Status::Implemented => "implemented",
            parity::Status::Missing => "missing",
        };
        let category = categorize_cvar(&lower);
        if let Some(cv) = registry.vars().get(&lower) {
            let flags = decode_cvar_flags(cv.flags);
            let desc = if cv.description.trim().is_empty() {
                format!("GoldSrc / Counter-Strike 1.6 CVar `{}`.", item.id)
            } else {
                cv.description.clone()
            };
            out.push(CVarDoc {
                id: item.id,
                kind: "cvar",
                default_value: cv.default_value.clone(),
                flags,
                category,
                status,
                description: desc,
            });
        } else {
            let kind = if known_cmds.contains(&lower) {
                "command"
            } else {
                "cvar"
            };
            let desc = infer_command_description(&lower);
            out.push(CVarDoc {
                id: item.id,
                kind,
                default_value: String::new(),
                flags: Vec::new(),
                category,
                status,
                description: desc,
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

fn decode_cvar_flags(flags: CVarFlags) -> Vec<&'static str> {
    let defs = [
        (CVarFlags::ARCHIVE, "ARCHIVE"),
        (CVarFlags::SERVER, "SERVER"),
        (CVarFlags::USERINFO, "USERINFO"),
        (CVarFlags::REPLICATED, "REPLICATED"),
        (CVarFlags::EXTDLL, "EXTDLL"),
        (CVarFlags::CLIENTDLL, "CLIENTDLL"),
        (CVarFlags::PROTECTED, "PROTECTED"),
        (CVarFlags::SPONLY, "SPONLY"),
        (CVarFlags::CHEAT, "CHEAT"),
        (CVarFlags::UNLOGGED, "UNLOGGED"),
    ];
    defs.iter()
        .filter_map(|&(flag, label)| flags.contains(flag).then_some(label))
        .collect()
}

fn categorize_cvar(name: &str) -> &'static str {
    if name.starts_with("sv_") {
        "sv_"
    } else if name.starts_with("mp_") {
        "mp_"
    } else if name.starts_with("cl_") {
        "cl_"
    } else if name.starts_with("bot_") {
        "bot_"
    } else if name.starts_with("gl_")
        || name.starts_with("r_")
        || name.starts_with("vid_")
        || name.starts_with("mat_")
    {
        "render"
    } else if name.starts_with("hud_")
        || name.starts_with("scr_")
        || name.starts_with("con_")
        || name.starts_with("vgui")
        || name.starts_with("_vgui")
    {
        "hud_ui"
    } else if name.starts_with("m_")
        || name.starts_with("joy")
        || name.starts_with("sensitivity")
        || name.starts_with("zoom_sensitivity")
        || name.starts_with('+')
        || name.starts_with('-')
    {
        "input"
    } else if name.starts_with("net_")
        || name.starts_with("rate")
        || name.starts_with("ex_")
        || name.starts_with("rcon")
    {
        "network"
    } else if name.starts_with("s_")
        || name.starts_with("snd_")
        || name.starts_with("voice_")
        || name.starts_with("volume")
        || name.starts_with("mp3")
        || name.starts_with("bgm")
    {
        "audio"
    } else if name.starts_with("amx_") || name.starts_with("mod_") {
        "modding"
    } else {
        "general"
    }
}

fn infer_command_description(name: &str) -> String {
    if let Some(act) = name.strip_prefix('+') {
        format!("Hold-button input press command for `{act}`.")
    } else if let Some(act) = name.strip_prefix('-') {
        format!("Hold-button input release command for `{act}`.")
    } else if let Some(slot) = name.strip_prefix("slot") {
        format!("Selects HUD weapon bucket / GoldSrc menu slot {slot}.")
    } else if name.starts_with("radio") {
        "Opens Counter-Strike 1.6 tactical radio command menu.".to_string()
    } else if name.starts_with("bot_") {
        "ZBot management console command.".to_string()
    } else if name.starts_with("mod_") || name.starts_with("amx_") {
        "Polyglot modding host / AMXX administration command.".to_string()
    } else {
        format!("GoldSrc / Counter-Strike 1.6 console command `{name}`.")
    }
}

// ===========================================================================
// 5. Search Index JSON & CVar Data JSON Serialization
// ===========================================================================

struct SearchEntry {
    title: String,
    category: &'static str,
    url: String,
    summary: String,
    keywords: String,
}

fn collect_guide_search_entries(out: &mut Vec<SearchEntry>) {
    let items = [
        (
            "60-Second Polyglot Quickstart",
            "./index.html#quickstart",
            "Write your first CS 1.6 plugin in Python 3, Rust, C++20, Go, or AMXX.",
        ),
        (
            "Zero-Compile Python 3 Hot-Reload (< 5 ms)",
            "./index.html#python-hot-reload",
            "Drop .py scripts into mods/, get VS Code IntelliSense via cs16.pyi, hot-reload preserving mod.storage.",
        ),
        (
            "Rust WASM SDK & MockHostBridge",
            "./index.html#rust-sdk",
            "cargo xtask mod new, safe #[cs_mod_*] macros, cs16_wasm_exports!, fast unit testing.",
        ),
        (
            "C++20 Header-Only & Go WASI SDKs",
            "./index.html#cpp-go-sdk",
            "Compile freestanding C++20 and Go wasip1 plugins to server.wasm.",
        ),
        (
            "Sandbox Security, Fuel & Capabilities (mod.toml)",
            "./index.html#security-capabilities",
            "17 fine-grained capabilities, fuel caps, memory limits, SHA-256 .csmod bundles.",
        ),
        (
            "CustomWeaponBuilder (Declarative Weapons)",
            "./index.html#custom-weapons",
            "Custom v_/p_/w_ models, fire sound, damage multiplier, clip size, recoil, knockback, cost.",
        ),
        (
            "Per-Player i18n (configs/lang/*.toml)",
            "./index.html#i18n-rbac",
            "Live /lang en|ru translations and unified a..z RBAC admin flags.",
        ),
        (
            "Cross-Language Synchronous RPC & Event Bus",
            "./index.html#rpc-bus",
            "Call Rust/Go/C++ exports from Python and AMXX synchronously.",
        ),
        (
            "Time-Travel Crash Replay (ModFaultDump)",
            "./index.html#crash-replay",
            "cargo xtask mod replay reproduces production traps offline in < 10 ms.",
        ),
        (
            "Interactive GoldSrc Menu Visual Designer",
            "./vfx-menus.html#menu-studio",
            "Preview \\y/\\w/\\r/\\d menus and generate Python/Rust/C++20/Go code live.",
        ),
        (
            "Interactive HUD & DHUD Designer",
            "./vfx-menus.html#hud-studio",
            "Click-to-place HUD/DHUD text with color pickers and fade animation preview.",
        ),
        (
            "Interactive Protocol 48 VFX Studio",
            "./vfx-menus.html#vfx-studio",
            "Live canvas preview of shockwave_ring, laser_beam, aura_light, particle burst, screen fade & shake.",
        ),
        (
            "5-Second Legacy AMX Mod X Migrator",
            "./amxx-migration.html#amxx-cli",
            "cargo xtask import-amxx upgrades 25 stock plugins and mounts custom .amxx files.",
        ),
        (
            "Interactive .sma -> Python 3 / Rust Converter",
            "./amxx-migration.html#sma-converter",
            "Paste any AMXX .sma plugin and convert it live to Python 3 or Rust.",
        ),
        (
            "Dedicated Server, HTTP/3 WebTransport & Docker Deployment",
            "./cvars-deployment.html#deployment",
            "cs-server CLI flags, TLS wss://, --webtransport-port, Docker Compose, Web Admin API.",
        ),
    ];
    for (title, url, summary) in items {
        out.push(SearchEntry {
            title: title.to_string(),
            category: "guide",
            url: url.to_string(),
            summary: summary.to_string(),
            keywords: format!("{title} {summary}"),
        });
    }
}

fn encode_search_index_json(entries: &[SearchEntry]) -> String {
    let mut out = String::with_capacity(entries.len() * 140);
    out.push('[');
    for (idx, e) in entries.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"t\":\"{}\",\"c\":\"{}\",\"u\":\"{}\",\"s\":\"{}\",\"k\":\"{}\"}}",
            json_escape(&e.title),
            json_escape(e.category),
            json_escape(&e.url),
            json_escape(&e.summary),
            json_escape(&e.keywords),
        );
    }
    out.push(']');
    out
}

fn encode_cvars_json(cvars: &[CVarDoc]) -> String {
    let mut out = String::with_capacity(cvars.len() * 120);
    out.push('[');
    for (idx, c) in cvars.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let mut flags_json = String::from("[");
        for (fi, f) in c.flags.iter().enumerate() {
            if fi > 0 {
                flags_json.push(',');
            }
            let _ = write!(flags_json, "\"{}\"", json_escape(f));
        }
        flags_json.push(']');
        let _ = write!(
            out,
            "{{\"id\":\"{}\",\"kind\":\"{}\",\"def\":\"{}\",\"flags\":{},\"cat\":\"{}\",\"status\":\"{}\",\"desc\":\"{}\"}}",
            json_escape(&c.id),
            json_escape(c.kind),
            json_escape(&c.default_value),
            flags_json,
            json_escape(c.category),
            json_escape(c.status),
            json_escape(&c.description),
        );
    }
    out.push(']');
    out
}

// ===========================================================================
// 6. Page Renderers (`index.html`, `hooks.html`, `api.html`, `vfx-menus.html`,
//    `amxx-migration.html`, `cvars-deployment.html`)
// ===========================================================================

fn render_index_page(
    hook_count: usize,
    api_count: usize,
    cvar_count: usize,
    amxx_count: usize,
) -> String {
    let toc = [
        ("architecture", "Architecture Overview"),
        ("quickstart", "60-Second Quickstart"),
        ("python-hot-reload", "Python 3 Hot-Reload"),
        ("rust-sdk", "Rust WASM SDK & Tests"),
        ("cpp-go-sdk", "C++20 & Go WASI SDKs"),
        ("security-capabilities", "Capabilities & Fuel Caps"),
        ("custom-weapons", "CustomWeaponBuilder"),
        ("i18n-rbac", "i18n & Admin RBAC"),
        ("rpc-bus", "Polyglot RPC & Events"),
        ("crash-replay", "Time-Travel Crash Replay"),
    ];

    let quickstart_tabs = render_code_tabs(&[
        (
            "py",
            "Python 3",
            r#"# mods/community/vampire_vip.py — zero compile, < 5 ms hot-reload!
from cs16 import mod, vfx, Color, HookAction, Armor

@mod.info(
    id="vampire_vip",
    name="Vampire VIP",
    version="1.0.0",
    permissions=["entities", "messages", "vfx", "admins", "commands"],
)
def _meta():
    pass

@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    if ev.player.has_flag("t"):
        ev.player.set_armor(100, Armor.VESTHELM)

@mod.hook("player_killed", phase="post")
def on_kill(ev):
    if ev.killer and not ev.team_kill:
        bonus = 20 if ev.headshot else 10
        ev.killer.set_health(min(ev.killer.health + bonus, 100.0))
        vfx.shockwave_ring(ev.victim.origin, 160.0, (255, 196, 80, 220))
        ev.killer.chat(f"{Color.GREEN}[Vampire]{Color.DEFAULT} +{bonus} HP!")"#,
        ),
        (
            "rust",
            "Rust",
            r#"use cs_mod_sdk::{
    ArmorKind, HookAction, HookId, HookPhase, HostBridge, Player,
    cs_mod_hook, cs_mod_init, vfx,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

#[cs_mod_init]
pub fn init(host: &mut impl HostBridge) -> i32 {
    host.hook_register(HookId::PlayerSpawn, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerKilled, HookPhase::Post, 0);
    0
}

#[cs_mod_hook(PlayerSpawn, post)]
pub fn on_spawn(host: &mut impl HostBridge) -> HookAction {
    let p = Player::new(host.hook_arg_i32(0) as u8);
    if p.has_flag(host, 't') {
        p.set_armor(host, 100, ArmorKind::VestHelm);
    }
    HookAction::Continue
}

#[cs_mod_hook(PlayerKilled, post)]
pub fn on_kill(host: &mut impl HostBridge) -> HookAction {
    let killer = host.hook_arg_i32(1);
    if killer >= 0 && host.hook_arg_i32(5) == 0 {
        let p = Player::new(killer as u8);
        let bonus = if host.hook_arg_i32(4) != 0 { 20.0 } else { 10.0 };
        p.set_health(host, (p.health(host) + bonus).min(100.0));
        if let Some(pos) = Player::new(host.hook_arg_i32(0) as u8).origin(host) {
            vfx::shockwave_ring(host, pos, 160.0, [255, 196, 80, 220], 8, 12, None);
        }
    }
    HookAction::Continue
}

cs_mod_sdk::cs16_wasm_exports! {
    init: init,
    hooks: [
        (HookId::PlayerSpawn, HookPhase::Post) => on_spawn,
        (HookId::PlayerKilled, HookPhase::Post) => on_kill,
    ],
}"#,
        ),
        (
            "cpp",
            "C++20",
            r#"#include "cs16_mod.hpp"

CS16_PLUGIN_INFO("vampire_vip", "Vampire VIP", "1.0.0")

CS16_ON_HOOK(cs16::HookId::PlayerSpawn, cs16::HookPhase::Post, on_spawn) {
    cs16::Player p(cs16_hook_arg_i32(0));
    if (p.has_flag('t')) {
        p.set_armor(100, cs16::ArmorKind::VestHelm);
    }
    return cs16::HookAction::Continue;
}

CS16_ON_HOOK(cs16::HookId::PlayerKilled, cs16::HookPhase::Post, on_kill) {
    int32_t killer = cs16_hook_arg_i32(1);
    if (killer >= 0 && cs16_hook_arg_i32(5) == 0) {
        cs16::Player p(killer);
        float bonus = cs16_hook_arg_i32(4) ? 20.0f : 10.0f;
        float hp = p.health() + bonus;
        p.set_health(hp > 100.0f ? 100.0f : hp);
        cs16::Vec3 pos;
        if (cs16::Player(cs16_hook_arg_i32(0)).origin(pos)) {
            cs16::vfx::shockwave_ring(pos, 160.0f, cs16::Color::Gold());
        }
    }
    return cs16::HookAction::Continue;
}

CS16_EXPORT_PLUGIN()"#,
        ),
        (
            "go",
            "Go",
            r#"//go:build wasm
package main

import "github.com/cs16-rs/cs16-rs/sdk/go/cs16mod"

func init() {
    cs16mod.OnPlayerSpawn(cs16mod.PhasePost, func(p cs16mod.Player) cs16mod.HookAction {
        if p.HasFlag('t') {
            p.SetArmor(100, cs16mod.ArmorVestHelm)
        }
        return cs16mod.Continue
    })
    cs16mod.OnPlayerKilled(cs16mod.PhasePost, func(ev cs16mod.KilledEvent) cs16mod.HookAction {
        if ev.KillerSlot >= 0 && !ev.TeamKill {
            k := cs16mod.NewPlayer(ev.KillerSlot)
            bonus := float32(10.0)
            if ev.Headshot {
                bonus = 20.0
            }
            hp := k.Health() + bonus
            if hp > 100.0 {
                hp = 100.0
            }
            k.SetHealth(hp)
            if pos, ok := ev.Victim.Origin(); ok {
                cs16mod.VFX.ShockwaveRing(pos, 160.0, cs16mod.Color{255, 196, 80, 220}, 8, 12, -1)
            }
        }
        return cs16mod.Continue
    })
}

func main() {}"#,
        ),
        (
            "amxx",
            "AMXX .sma",
            r#"#include <amxmodx>
#include <cstrike>
#include <fun>
#include <hamsandwich>

public plugin_init() {
    register_plugin("Vampire VIP", "1.0.0", "cs16-rs");
    RegisterHam(Ham_Spawn, "player", "fw_Spawn", 1);
    RegisterHam(Ham_Killed, "player", "fw_Killed", 1);
}

public fw_Spawn(id) {
    if (is_user_alive(id) && (get_user_flags(id) & ADMIN_LEVEL_H)) {
        cs_set_user_armor(id, 100, CS_ARMOR_VESTHELM);
    }
}

public fw_Killed(victim, attacker) {
    if (is_user_alive(attacker) && victim != attacker) {
        new hp = min(get_user_health(attacker) + 10, 100);
        set_user_health(attacker, hp);
        client_print(attacker, print_chat, "[Vampire] +10 HP!");
    }
}"#,
        ),
    ]);

    let custom_weapon_tabs = render_code_tabs(&[
        (
            "py",
            "Python 3",
            r#"from cs16 import mod, CustomWeapon, WeaponId

golden_ak = CustomWeapon(
    id_name="golden_ak47",
    base_weapon=WeaponId.AK47,
    display_name="Golden AK-47",
    v_model="models/v_golden_ak47.mdl",
    fire_sound="weapons/golden_ak_fire.wav",
    damage_mult=1.35,
    clip_size=35,
    recoil_mult=0.85,
    knockback=1.4,
    cost=4500,
)
mod.register_custom_weapon(golden_ak)

@mod.chat_command("/goldak")
def give_gold(player, args):
    player.give_custom_weapon("golden_ak47")"#,
        ),
        (
            "rust",
            "Rust",
            r#"CustomWeaponBuilder::new("golden_ak47", weapon_id::AK47)
    .display_name("Golden AK-47")
    .v_model("models/v_golden_ak47.mdl")
    .fire_sound("weapons/golden_ak_fire.wav")
    .damage_mult(1.35)
    .clip_size(35)
    .recoil_mult(0.85)
    .knockback(1.4)
    .cost(4500)
    .register(host);

// Later on spawn or purchase:
Player::new(slot).give_custom_weapon(host, "golden_ak47");"#,
        ),
        (
            "cpp",
            "C++20",
            r#"const char *spec =
    "id=golden_ak47\nbase_weapon=28\ndisplay_name=Golden AK-47\n"
    "damage_mult=1.35\nclip_size=35\nrecoil_mult=0.85\nknockback=1.4\ncost=4500\n";
cs16_register_custom_weapon(spec, static_cast<int32_t>(cs16::StrView(spec).size()));
cs16::Player(slot).give_custom_weapon("golden_ak47");"#,
        ),
        (
            "go",
            "Go",
            r#"spec := "id=golden_ak47\nbase_weapon=28\ndisplay_name=Golden AK-47\ndamage_mult=1.35\nclip_size=35\ncost=4500\n"
cs16mod.RegisterCustomWeaponRaw(spec)
cs16mod.NewPlayer(slot).GiveCustomWeapon("golden_ak47")"#,
        ),
        (
            "amxx",
            "AMXX .sma",
            r#"// In legacy AMXX, custom weapons required hooking Ham_Item_Deploy,
// Ham_TakeDamage, CurWeapon, and FM_EmitSound manually.
// cs16-rs replaces all 4 hooks with 1 declarative CustomWeaponSpec!"#,
        ),
    ]);

    let rpc_tabs = render_code_tabs(&[
        (
            "py",
            "Python 3",
            r#"# Export a synchronous RPC function callable from Rust, Go, C++, or AMXX:
@mod.export_fn("vip.is_vip")
def rpc_is_vip(slot):
    p = mod.player(int(slot))
    return 1 if (p and p.has_flag("t")) else 0

# Call another plugin's export & emit cross-plugin events:
rank = mod.call_fn("statsx.get_rank", player.slot)
mod.emit_event("vip.purchased", {"slot": player.slot, "weapon": "ak47"})"#,
        ),
        (
            "rust",
            "Rust",
            r#"// Export from Rust WASM:
host.rpc_export("statsx.get_rank", 1);

// Synchronously call Python's `vip.is_vip` export:
let is_vip = host.rpc_call_i32("vip.is_vip", &[RpcValue::Int(slot as i32)]);
host.rpc_emit_event("statsx.rank_up", "{\"slot\":0,\"rank\":1}");"#,
        ),
        (
            "cpp",
            "C++20",
            r#"// Call Python's vip.is_vip synchronously from C++20 WASM:
int32_t is_vip = cs16_rpc_call_i32("vip.is_vip", 10, slot);
cs16_rpc_emit_event("restmenu.updated", 16, "{\"awp\":true}", 12);"#,
        ),
        (
            "go",
            "Go",
            r#"cs16mod.ExportFn("mapmanager.nextmap", func(_ int32, _ string) (int32, string) {
    return 0, currentNextMap
})
cs16mod.EmitEvent("mapmanager.vote_started", `{"candidates":["de_dust2","de_inferno"]}`)"#,
        ),
        (
            "amxx",
            "AMXX .sma",
            r#"// AMXX register_native & CreateMultiForward bridge directly onto RpcBus!
public plugin_natives() {
    register_native("my_amxx_bonus", "native_bonus");
}
// Any Python/Rust/Go/C++ plugin can call "my_amxx_bonus" synchronously!"#,
        ),
    ]);

    let mut perm_rows = String::new();
    for perm in Permission::ALL_VARIANTS {
        let name = perm.as_str();
        let bit = perm as u32;
        let desc = permission_description(perm);
        let _ = write!(
            perm_rows,
            "<tr><td><code>{name}</code></td><td><code>0x{bit:05x}</code></td><td>{desc}</td></tr>"
        );
    }

    let admin_flag_defs = [
        (
            'a',
            "IMMUNITY",
            "Cannot be kicked, slapped, slayed, or banned by other admins",
        ),
        ('b', "RESERVATION", "Reserved server slot bypass"),
        ('c', "KICK", "Access to amx_kick / kick player menu"),
        ('d', "BAN", "Permanent ban / banid access"),
        ('e', "SLAY", "Access to amx_slay and amx_slap"),
        ('f', "MAP", "Access to amx_map / changelevel"),
        ('g', "CVAR", "Modify server CVars via amx_cvar"),
        ('h', "CFG", "Execute server .cfg files"),
        ('i', "CHAT", "Admin HUD/tsay/csay/psay announcements"),
        ('j', "VOTE", "Start map/kick/custom votes"),
        ('k', "PASSWORD", "Modify sv_password"),
        ('l', "RCON", "Full RCON command execution"),
        ('m', "LEVEL_A", "Custom plugin group flag A"),
        ('n', "LEVEL_B", "Custom plugin group flag B"),
        ('o', "LEVEL_C", "Custom plugin group flag C"),
        ('p', "LEVEL_D", "Custom plugin group flag D"),
        ('q', "LEVEL_E", "Custom plugin group flag E"),
        ('r', "LEVEL_F", "Custom plugin group flag F"),
        ('s', "LEVEL_G", "Custom plugin group flag G"),
        (
            't',
            "LEVEL_H",
            "Standard VIP privilege flag (vip_system.py)",
        ),
        ('u', "MENU", "Access to amxmodmenu admin front-end"),
        ('v', "BAN_TEMP", "Temporary timed bans only"),
        ('y', "ADMIN", "Default admin marker flag"),
        ('z', "USER", "Regular non-admin player (default)"),
    ];
    let mut admin_rows = String::new();
    for (ch, cname, desc) in admin_flag_defs {
        let bit = AdminFlags::bit_for_char(ch).unwrap_or(0);
        let _ = write!(
            admin_rows,
            "<tr><td><code>{ch}</code></td><td><code>ADMIN_{cname}</code></td><td><code>0x{bit:07x}</code></td><td>{desc}</td></tr>"
        );
    }

    let body = format!(
        r#"<section class="hero-panel">
  <div class="hero-kicker">COUNTER-STRIKE 1.6 IN SAFE RUST &middot; POLYGLOT MODDING ABI v{abi}</div>
  <h1>cs16-rs Developer &amp; Engine Portal</h1>
  <p class="hero-lead">Build sandboxed Counter-Strike 1.6 plugins in <strong>Python 3</strong>, <strong>Rust</strong>, <strong>C++20</strong>, <strong>Go</strong>, or legacy <strong>AMX Mod X (.amxx)</strong> on one 100 Hz Protocol 48 engine — with &lt;5 ms hot-reload, 1-line VFX, time-travel crash replay, and zero <code>unsafe</code> code.</p>
  <div class="stat-strip">
    <div class="stat-tile"><span class="stat-num">{hook_count}</span><span class="stat-label">ABI v2 Hooks</span></div>
    <div class="stat-tile"><span class="stat-num">{api_count}</span><span class="stat-label">SDK API Methods</span></div>
    <div class="stat-tile"><span class="stat-num">{cvar_count}</span><span class="stat-label">CVars &amp; Commands</span></div>
    <div class="stat-tile"><span class="stat-num">{amxx_count}</span><span class="stat-label">AMXX Natives</span></div>
    <div class="stat-tile"><span class="stat-num">17</span><span class="stat-label">Sandbox Capabilities</span></div>
  </div>
  <div class="hero-actions">
    <a class="vgui-btn accent" href="../playground/">&#9654; Launch Live Python Playground</a>
    <a class="vgui-btn" href="./hooks.html">Explore All {hook_count} Hooks</a>
    <a class="vgui-btn" href="./vfx-menus.html">Open VFX &amp; Menu Designer</a>
    <a class="vgui-btn" href="../mods/">Browse Plugin Hub</a>
  </div>
</section>

<section class="doc-section" id="architecture">
  <h2>1. Unified Polyglot Architecture</h2>
  <p>Every plugin runtime shares a single deterministic <code>SharedPlatformServices</code> bus inside <code>cs-server</code> and <code>cs-local</code> (including browser WebAssembly!). A Python 3 script, a compiled Rust/C++/Go <code>.wasm</code> module, and a legacy <code>.amxx</code> binary share the same paginated GoldSrc menus, EN/RU translations, RBAC admin table, NVault KV store, O(log N) leaderboards, and synchronous RPC calls.</p>
  <div class="arch-diagram"><pre>+--------------------------------------------------------------------------------+
|             100 Hz Protocol 48 Engine (cs-server / cs-local / cs-web)          |
+--------------------------------------------------------------------------------+
         |  52 ReAPI / GameDll / EngineApi Hooks (Pre &amp; Post, Priority Chain)
         v
+-----------------------+  +------------------------+  +-------------------------+
| cs-mod-py (Python 3)  |  | wasmi WASM Sandbox     |  | cs-amxx (Pawn 3.2 VM)   |
| Zero-compile .py      |  | Rust · C++20 · Go WASI |  | Legacy .amxx binaries   |
| &lt; 5 ms hot-reload     |  | Deterministic fuel cap |  | Ham / ReAPI / FM / CSX  |
+-----------+-----------+  +-----------+------------+  +------------+------------+
            \                          |                           /
             v                         v                          v
+--------------------------------------------------------------------------------+
|                     SharedPlatformServices (Zero-Unsafe Rust)                  |
|  MenuManager · LangCatalog (en/ru) · AdminRegistry (a..z) · NVault KV Store    |
|  O(log N) LeaderboardStore · Synchronous RpcBus · Allowlisted Async Webhooks   |
|  ModFaultDump Black-Box Recorder (&lt;= 256 KiB JSON, 32-slot ring, &lt;10ms replay) |
+--------------------------------------------------------------------------------+</pre></div>
</section>

<section class="doc-section" id="quickstart">
  <h2>2. 60-Second Polyglot Quickstart</h2>
  <p>Switch languages below — your choice persists automatically across the entire documentation portal:</p>
  {quickstart_tabs}
</section>

<section class="doc-section" id="python-hot-reload">
  <h2>3. Zero-Compile Python 3 Plugins (&lt; 5 ms Hot-Reload)</h2>
  <p><code>cs-mod-py</code> embeds a pure-Rust, fuel-metered Python 3 interpreter directly into <code>cs-server</code>, <code>cs-local</code>, and <code>cs-web</code> (no external Python installation or C FFI!). Drop any <code>.py</code> file into <code>mods/community/</code> and enable live auto-reload:</p>
  {py_cli}
  <ul class="vgui-list">
    <li><strong>Instant Hot-Reload:</strong> Saving a file on disk or inside <code>/admin.html</code> &rarr; <em>Mods &amp; ResMon Studio</em> recompiles and swaps hooks in <strong>&lt; 5 ms</strong> while preserving <code>mod.storage</code>!</li>
    <li><strong>Full IDE IntelliSense:</strong> Point VS Code / Pyright / Pylance <code>python.analysis.extraPaths</code> to <code>sdk/python</code> to get full type hints from <code>sdk/python/cs16.pyi</code>.</li>
    <li><strong>Browser Testing:</strong> Click <a href="../playground/">Live Playground</a> to write and hot-reload Python 3 plugins against live 5v5 ZBots right inside your browser!</li>
  </ul>
</section>

<section class="doc-section" id="rust-sdk">
  <h2>4. Rust WASM SDK (<code>cs-mod-sdk</code> &amp; <code>MockHostBridge</code>)</h2>
  <p>Scaffold, compile to <code>wasm32-unknown-unknown</code>, and pack into a SHA-256 verified <code>.csmod</code> archive in two commands:</p>
  {rust_cli}
  <p>Zero-dependency proc macros validate hook names and phases at compile time while keeping <code>#![forbid(unsafe_code)]</code>: <code>#[cs_mod_init]</code>, <code>#[cs_mod_hook(PlayerTakeDamage, pre, -5)]</code>, <code>#[cs_mod_chat("/guns", "!guns")]</code>, <code>#[cs_mod_cmd("amx_kick")]</code>, and <code>#[cs_mod_hud]</code>. On native targets, test every hook in milliseconds with <code>MockHostBridge</code>:</p>
  {rust_test}
</section>

<section class="doc-section" id="cpp-go-sdk">
  <h2>5. C++20 Header-Only &amp; Go WASI SDKs</h2>
  <p>Author zero-dependency freestanding C++20 plugins with <code>sdk/cpp/cs16_mod.hpp</code> or Go <code>&gt;= 1.24</code> WASI plugins with <code>sdk/go/cs16mod</code>:</p>
  {cpp_go_cli}
  <p>The WASM host supplies minimal safe <code>wasi_snapshot_preview1</code> shims (<code>fd_write</code> routed to plugin logs, deterministic PRNG <code>random_get</code>, <code>clock_time_get</code>, and isolated <code>proc_exit</code>) with zero host filesystem or network escape.</p>
</section>

<section class="doc-section" id="security-capabilities">
  <h2>6. Sandbox Fuel Caps, Memory Limits &amp; <code>mod.toml</code> Capabilities</h2>
  <p>Plugins default to <strong>zero permissions</strong> (deny-by-default). Every host capability must be declared explicitly in <code>mod.toml</code> (or <code>@mod.info(permissions=[...])</code>). Resource caps enforce 100 Hz server stability:</p>
  <div class="callout-grid">
    <div class="callout"><strong>Default Fuel / Frame:</strong> <code>{def_fuel}</code> instructions (max <code>{max_fuel}</code>). Infinite loops trap with <code>OutOfFuel</code> and isolate only that plugin.</div>
    <div class="callout"><strong>Default Memory Cap:</strong> <code>{def_mem} MiB</code> linear WASM / Python heap (max <code>{max_mem} MiB</code>).</div>
    <div class="callout"><strong>Webhook Allowlisting:</strong> Declare <code>permissions = ["http:discord.com"]</code> — requests to any other hostname are rejected by the host.</div>
  </div>
  <div class="table-wrap">
    <table class="vgui-table">
      <thead><tr><th>Capability</th><th>Bitmask</th><th>Gated Host APIs</th></tr></thead>
      <tbody>{perm_rows}</tbody>
    </table>
  </div>
</section>

<section class="doc-section" id="custom-weapons">
  <h2>7. Declarative <code>CustomWeaponBuilder</code></h2>
  <p>Register custom CS 1.6 weapons declaratively during init. The host precaches declared <code>v_</code>/<code>p_</code>/<code>w_</code> models and fire sounds, applies damage/clip/recoil/knockback multipliers automatically inside <code>cs-game</code>, and replicates client views over Protocol 48:</p>
  {custom_weapon_tabs}
</section>

<section class="doc-section" id="i18n-rbac">
  <h2>8. Per-Player <code>i18n</code> (<code>configs/lang/*.toml</code>) &amp; RBAC Flags (<code>a..z</code>)</h2>
  <p>Players toggle their language live via <code>/lang en</code> or <code>/lang ru</code> (default controlled by <code>amx_language</code>). Both <code>configs/lang/*.toml</code> and legacy AMXX <code>data/lang/*.txt</code> dictionaries merge into one shared <code>LangCatalog</code>:</p>
  {i18n_admin_toml}
  <h3>Unified AMX Mod X RBAC Flags (<code>AdminFlags</code>)</h3>
  <div class="table-wrap">
    <table class="vgui-table">
      <thead><tr><th>Flag</th><th>Constant</th><th>Bitmask</th><th>Privilege</th></tr></thead>
      <tbody>{admin_rows}</tbody>
    </table>
  </div>
</section>

<section class="doc-section" id="rpc-bus">
  <h2>9. Cross-Language Synchronous RPC &amp; Event Bus</h2>
  <p>Plugins written in different languages call each other synchronously via <code>RpcBus</code> without serialization boilerplate:</p>
  {rpc_tabs}
</section>

<section class="doc-section" id="crash-replay">
  <h2>10. Time-Travel Black-Box Crash Replay (<code>&lt; 10 ms</code>)</h2>
  <p>Whenever a <code>.wasm</code>, <code>.py</code>, or <code>.amxx</code> plugin traps or exceeds fuel, <code>cs-server</code> isolates that plugin, preserves the 100 Hz match, and writes a bounded JSON snapshot (<code>&lt;= 256 KiB</code>, 32-dump ring buffer) to <code>logs/mod_faults/</code>. Replay any production crash offline without launching the game:</p>
  {replay_cli}
</section>"#,
        abi = ABI_VERSION,
        py_cli = render_single_code(
            "Server Console / Hot-Reload",
            "sv_mod_autoreload 1\nmod_list             # Unified table across Rust, C++, Go, Python, AMXX\nmod_stats            # Live ResMon CPU (avg/p99/max us), fuel, memory, faults\nmod_reload vip_system\nmod_pause afk_control",
        ),
        rust_cli = render_single_code(
            "Rust WASM Workflow",
            "cargo xtask mod new my_gamemode --lang rust --side both\ncargo xtask mod build\n# Emits SHA-256 verified dist/mods/my_gamemode.csmod",
        ),
        rust_test = render_single_code(
            "Native MockHostBridge Unit Test",
            "let mut host = MockHostBridge::default();\nhost.add_player(0, \"Alice\");\nhost.set_player_admin_flags(0, AdminFlags::parse_str(\"t\"));\nhost.set_hook_args_i32(&[0]);\nassert_eq!(on_spawn(&mut host), HookAction::Continue);\nassert_eq!(Player::new(0).armor(&host), 100);",
        ),
        cpp_go_cli = render_single_code(
            "C++20 & Go WASM Compilation",
            "# C++20 freestanding WASM:\ncargo xtask mod new my_cpp_mod --lang cpp\nclang++ --target=wasm32-unknown-unknown -O2 -std=c++20 -nostdlib \\\n  -Wl,--no-entry -Wl,--allow-undefined -Wl,--export-dynamic \\\n  -I sdk/cpp mods/my_cpp_mod/main.cpp -o mods/my_cpp_mod/server.wasm\n\n# Go >= 1.24 WASI:\ncargo xtask mod new my_go_mod --lang go\ncd mods/my_go_mod && GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -o server.wasm .",
        ),
        def_fuel = DEFAULT_FUEL_PER_FRAME,
        max_fuel = MAX_FUEL_PER_FRAME,
        def_mem = DEFAULT_MEMORY_BYTES / (1024 * 1024),
        max_mem = MAX_MEMORY_CAP / (1024 * 1024),
        i18n_admin_toml = render_single_code(
            "configs/lang/vip.toml & configs/admins.toml",
            "# configs/lang/vip.toml\n[en]\nVIP_GREET = \"Welcome VIP {name}!\"\n[ru]\nVIP_GREET = \"Добро пожаловать, VIP {name}!\"\n\n# configs/admins.toml\n[[admin]]\nauth = \"STEAM_0:0:123456\"\nflags = \"abcdefghijklmnopqrstu\"\npassword = \"\"\naccount_flags = \"ce\"",
        ),
        replay_cli = render_single_code(
            "Offline Crash Reproduction & Fix Verification",
            "# Reproduce the exact failing tick & stack trace in < 10 ms:\ncargo xtask mod replay logs/mod_faults/1732560000_my_mod_player_take_damage_pre.json\n\n# Verify your patched plugin passes against the exact same snapshot:\ncargo xtask mod replay logs/mod_faults/1732560000_my_mod_player_take_damage_pre.json \\\n  --mod-path mods/community/my_mod.py",
        ),
    );

    render_docs_shell(
        "index",
        "Overview & 60-Second Quickstart",
        "Complete cs16-rs polyglot modding and engine developer documentation portal.",
        &toc,
        &body,
        &[],
    )
}

fn permission_description(perm: Permission) -> &'static str {
    match perm {
        Permission::Entities => {
            "Read/mutate players, health, armor, money, origin, velocity, and spawn/remove entities"
        }
        Permission::Messages => {
            "Chat SayText, center_print, HUD/DHUD messages, MOTD windows, ScreenFade, ScreenShake, BarTime, i18n"
        }
        Permission::Cvars => "Read and modify server CVars (cvar_get / cvar_set)",
        Permission::Commands => {
            "Register chat/console commands, server_exec, change_level, and player kick"
        }
        Permission::Menus => "Create, display, and destroy host-managed paginated GoldSrc menus",
        Permission::Timers => "Schedule and cancel deterministic 100 Hz server timers",
        Permission::Traces => "Execute BSP solid raycasts (trace_fraction / trace_line)",
        Permission::Sound => "Precache and emit spatialized .wav samples",
        Permission::Storage => {
            "Persistent per-plugin KV storage (mod.storage / NVault) and O(log N) LeaderboardStore"
        }
        Permission::Hud => {
            "Client-side WASM VGUI2 HudCanvas draw commands (Rect, Text, ProgressBar, Panel)"
        }
        Permission::Input => "Client-side WASM input inspection hooks",
        Permission::NetCustom => {
            "Send and receive custom UserMsg binary packets between server and client mods"
        }
        Permission::Vfx => {
            "Emit Protocol 48 TE_* VFX (shockwave_ring, laser_beam, aura_light, explosion, decals, etc.)"
        }
        Permission::Weapons => {
            "Give/strip weapons, set clip/bpammo, defuser/NVG, and register_custom_weapon"
        }
        Permission::Admins => {
            "Inspect unified RBAC admin flags (has_flag) and manage admin privileges"
        }
        Permission::Rpc => {
            "Export/call synchronous cross-language RPC functions and emit/subscribe to events"
        }
        Permission::Http => {
            "Send non-blocking async HTTP POST JSON webhooks strictly to allowlisted http:<host> domains"
        }
    }
}

fn render_hooks_page(hooks: &[HookDoc]) -> String {
    let gameplay_count = hooks.iter().filter(|h| h.cat_key == "gameplay").count();
    let gamedll_count = hooks.iter().filter(|h| h.cat_key == "gamedll").count();
    let engine_count = hooks.iter().filter(|h| h.cat_key == "engine").count();
    let mutable_count = hooks.iter().filter(|h| h.mutable_pre).count();

    let toc = [
        ("hook-semantics", "HookPhase & HookAction"),
        ("hook-filter", "Filter All 52 Hooks"),
        ("hooks-list", "Complete Hook Reference"),
    ];

    let mut cards = String::with_capacity(hooks.len() * 1400);
    for h in hooks {
        let mut arg_rows = String::new();
        for (slot, ty, desc) in &h.args {
            let _ = write!(
                arg_rows,
                "<tr><td><code>{slot}</code></td><td><code>{ty}</code></td><td>{desc}</td></tr>",
                slot = html_escape(slot),
                ty = html_escape(ty),
                desc = html_escape(desc),
            );
        }
        let code_tabs = render_code_tabs(&[
            ("py", "Python 3", &h.py_code),
            ("rust", "Rust", &h.rust_code),
            ("cpp", "C++20", &h.cpp_code),
            ("go", "Go", &h.go_code),
            ("amxx", "AMXX .sma", &h.amxx_code),
        ]);
        let mut_badge = if h.mutable_pre {
            r#"<span class="pill pill-amber">Mutable / Cancellable Pre</span>"#
        } else {
            r#"<span class="pill">Observer</span>"#
        };
        let _ = write!(
            cards,
            r##"<article class="hook-card" id="hook-{snake}" data-hook-cat="{cat_key}" data-hook-mut="{mut_flag}" data-filter-text="{filter_txt}">
  <div class="hook-head">
    <div class="hook-title-wrap">
      <h3><code>{pascal}</code> <span class="hook-snake">"{snake}"</span></h3>
      <div class="hook-badges">
        <span class="pill pill-id">ID {id}</span>
        <span class="pill">{category}</span>
        <span class="pill">{phases}</span>
        {mut_badge}
      </div>
    </div>
    <a class="anchor-link" href="#hook-{snake}" title="Permalink">#</a>
  </div>
  <p class="hook-summary">{summary}</p>
  <div class="hook-meta-line">
    <span><strong>ABI Ctx:</strong> <code>{ctx}</code></span>
    <span><strong>AMXX / ReAPI Equiv:</strong> <code>{amxx}</code></span>
  </div>
  <div class="hook-override-note"><strong>Return / Override Semantics:</strong> {override_note}</div>
  <div class="table-wrap">
    <table class="vgui-table compact">
      <thead><tr><th>Argument / Field</th><th>Type</th><th>Description</th></tr></thead>
      <tbody>{arg_rows}</tbody>
    </table>
  </div>
  {code_tabs}
</article>"##,
            snake = h.snake,
            cat_key = h.cat_key,
            mut_flag = if h.mutable_pre { "1" } else { "0" },
            filter_txt = html_escape(
                &format!(
                    "{} {} {} {} {}",
                    h.pascal, h.snake, h.id, h.amxx_equiv, h.summary
                )
                .to_ascii_lowercase()
            ),
            pascal = h.pascal,
            id = h.id,
            category = html_escape(h.category),
            phases = html_escape(h.phases),
            summary = html_escape(h.summary),
            ctx = html_escape(h.ctx_struct),
            amxx = html_escape(h.amxx_equiv),
            override_note = html_escape(h.override_note),
        );
    }

    let body = format!(
        r#"<section class="page-head">
  <div class="hero-kicker">INTROSPECTED DIRECTLY FROM <code>cs_mod_api::HookId</code></div>
  <h1>All {total} ABI v2 Hooks Encyclopedia</h1>
  <p class="hero-lead">Every single Pre &amp; Post hook across <code>GameDll</code> ({gamedll_count}), CS 1.6 ReAPI Gameplay ({gameplay_count}), and <code>EngineApi</code> ({engine_count}) — complete with argument tables, mutable Pre override semantics, and synchronized 5-language examples.</p>
</section>

<section class="doc-section" id="hook-semantics">
  <h2>HookPhase &amp; HookAction Control Flow</h2>
  <div class="table-wrap">
    <table class="vgui-table">
      <thead><tr><th>Directive</th><th>Wire Value</th><th>Chain Behavior</th></tr></thead>
      <tbody>
        <tr><td><code>HookAction::Continue</code></td><td><code>0</code></td><td>Run remaining hooks and execute the underlying engine/game handler unchanged.</td></tr>
        <tr><td><code>HookAction::Handled</code></td><td><code>1</code></td><td>Mark event handled while allowing remaining hooks and inner handler to proceed.</td></tr>
        <tr><td><code>HookAction::Override</code></td><td><code>2</code></td><td>Apply mutated Pre override values (<code>ev.damage</code>, <code>ev.should_collide</code>, <code>ev.message</code>, <code>ev.price</code>) and invoke the inner handler.</td></tr>
        <tr><td><code>HookAction::Supercede</code></td><td><code>3</code></td><td>Short-circuit and cancel the underlying inner handler (e.g. block AWP purchase, suppress slash command, prevent damage).</td></tr>
      </tbody>
    </table>
  </div>
  <p>Hooks run in ascending numeric <code>priority</code> order (e.g. <code>-10</code> runs before <code>0</code>, which runs before <code>+10</code>). Every hook supports <code>HookPhase::Pre</code> (<code>0</code>) and/or <code>HookPhase::Post</code> (<code>1</code>).</p>
</section>

<section class="doc-section" id="hook-filter">
  <h2>Filter Hooks</h2>
  <div class="filter-bar">
    <input type="search" id="hook-search" class="vgui-input" placeholder="Filter by hook name, ID, AMXX equivalent, or keyword (e.g. damage, bomb, semiclip)...">
    <div class="filter-chips" id="hook-cat-chips">
      <button type="button" class="cat-chip active" data-hook-filter="all">All ({total})</button>
      <button type="button" class="cat-chip" data-hook-filter="gameplay">Gameplay / ReAPI ({gameplay_count})</button>
      <button type="button" class="cat-chip" data-hook-filter="gamedll">GameDll ({gamedll_count})</button>
      <button type="button" class="cat-chip" data-hook-filter="engine">EngineApi ({engine_count})</button>
      <button type="button" class="cat-chip" data-hook-filter="mutable">Mutable Pre ({mutable_count})</button>
    </div>
    <span class="filter-count" id="hook-count">Showing {total} of {total} hooks</span>
  </div>
</section>

<section class="doc-section" id="hooks-list">
  {cards}
</section>"#,
        total = hooks.len(),
    );

    render_docs_shell(
        "hooks",
        "All 52 Hooks Encyclopedia",
        "Complete ABI v2 Pre & Post hooks encyclopedia for cs16-rs with 5-language examples.",
        &toc,
        &body,
        &[],
    )
}

fn render_api_page(groups: &[ApiGroupDoc], py_stub: &PyStubInfo) -> String {
    let toc: Vec<(&str, &str)> = groups.iter().map(|g| (g.id, g.title)).collect();
    let total_methods: usize = groups.iter().map(|g| g.entries.len()).sum();

    let mut sections = String::new();
    for g in groups {
        let mut rows = String::new();
        for m in &g.entries {
            let filter_txt = format!(
                "{} {} {} {} {} {} {}",
                g.title, m.name, m.capability, m.py_sig, m.rust_sig, m.cpp_sig, m.summary
            )
            .to_ascii_lowercase();
            let _ = write!(
                rows,
                r#"<tr id="api-{anchor}" data-api-row data-filter-text="{ft}">
  <td><code>{name}</code><div class="cell-sub"><span class="pill pill-cap">{cap}</span></div></td>
  <td><code>{py}</code></td>
  <td><code>{rust}</code></td>
  <td><code>{cpp}</code></td>
  <td><code>{go}</code></td>
  <td>{summary}</td>
</tr>"#,
                anchor = html_escape(&m.anchor),
                ft = html_escape(&filter_txt),
                name = html_escape(&m.name),
                cap = html_escape(m.capability),
                py = html_escape(&m.py_sig),
                rust = html_escape(&m.rust_sig),
                cpp = html_escape(&m.cpp_sig),
                go = html_escape(&m.go_sig),
                summary = html_escape(&m.summary),
            );
        }
        let _ = write!(
            sections,
            r#"<section class="doc-section" id="{id}">
  <h2>{title}</h2>
  <p>{desc}</p>
  <div class="table-wrap">
    <table class="vgui-table api-table">
      <thead><tr><th>Method &amp; Cap</th><th>Python 3 (<code>cs16.pyi</code>)</th><th>Rust (<code>cs-mod-sdk</code>)</th><th>C++20 (<code>cs16_mod.hpp</code>)</th><th>Go (<code>cs16mod</code>)</th><th>Description</th></tr></thead>
      <tbody>{rows}</tbody>
    </table>
  </div>
</section>"#,
            id = g.id,
            title = html_escape(g.title),
            desc = html_escape(g.description),
        );
    }

    let stub_class_list: String = py_stub
        .classes
        .keys()
        .map(|k| format!("<code>{}</code>", html_escape(k)))
        .collect::<Vec<_>>()
        .join(" &middot; ");

    let body = format!(
        r#"<section class="page-head">
  <div class="hero-kicker">AUTO-VERIFIED FROM <code>sdk/python/cs16.pyi</code>, <code>cs-mod-sdk</code>, <code>cs16_mod.hpp</code> &amp; <code>sdk/go/cs16mod</code></div>
  <h1>Complete Host &amp; SDK API Reference</h1>
  <p class="hero-lead">{total_methods} documented polyglot methods across Player, Entity, World, Weapons, Paginated Menus, HUD/VFX, Timers, Storage, O(log N) Leaderboards, Async Webhooks, Polyglot RPC, i18n, and Admin RBAC — with exact <code>mod.toml</code> capability requirements.</p>
  <div class="meta-strip">Introspected Python stub classes: {stub_class_list}</div>
</section>

<section class="doc-section">
  <div class="filter-bar">
    <input type="search" id="api-search" class="vgui-input" placeholder="Filter API methods by name, signature, or capability (e.g. vfx, leaderboard, armor, webhook)...">
    <span class="filter-count" id="api-count">Showing {total_methods} of {total_methods} methods</span>
  </div>
</section>

{sections}"#
    );

    render_docs_shell(
        "api",
        "Host & SDK API Reference",
        "Complete cs16-rs polyglot host and SDK API reference with mod.toml capabilities.",
        &toc,
        &body,
        &[],
    )
}

fn render_vfx_menus_page() -> String {
    let toc = [
        ("menu-studio", "GoldSrc Menu Designer"),
        ("hud-studio", "HUD & DHUD Designer"),
        ("vfx-studio", "Protocol 48 VFX Studio"),
    ];

    let body = r##"<section class="page-head">
  <div class="hero-kicker">INTERACTIVE GOLDSRC VISUAL STUDIO &middot; LIVE 4-LANGUAGE CODE GENERATOR</div>
  <h1>VFX, HUD &amp; GoldSrc Menu Visual Designer</h1>
  <p class="hero-lead">Visually design authentic CS 1.6 <code>\y</code>/<code>\w</code>/<code>\r</code>/<code>\d</code> paginated menus, click-to-place HUD &amp; DHUD messages, and Protocol 48 <code>TE_*</code> VFX — while generating copy-ready <strong>Python 3</strong>, <strong>Rust</strong>, <strong>C++20</strong>, and <strong>Go</strong> code in real time.</p>
</section>

<section class="doc-section studio-block" id="menu-studio">
  <h2>1. GoldSrc Paginated Menu Designer</h2>
  <div class="studio-grid">
    <div class="studio-controls">
      <div class="form-row"><label for="menu-title">Menu Title (supports \y \w \r \d \R)</label><input id="menu-title" class="vgui-input" type="text" value="\yVIP Arsenal \d(Round 2+)"></div>
      <div class="form-row-inline">
        <div><label for="menu-per-page">Items / Page (1..7)</label><input id="menu-per-page" class="vgui-input" type="number" min="1" max="7" value="7"></div>
        <div><label for="menu-time">Timeout (sec, -1=∞)</label><input id="menu-time" class="vgui-input" type="number" min="-1" max="60" value="15"></div>
        <div><label for="menu-num-col">Number Color</label><select id="menu-num-col" class="vgui-input"><option value="\r">\r Red (AMXX)</option><option value="\y">\y Amber</option><option value="\w">\w White</option></select></div>
      </div>
      <div class="form-row">
        <label>Enabled Key Mask &amp; Navigation</label>
        <div class="keymask-row" id="menu-keymask">
          <label><input type="checkbox" data-key="1" checked> 1</label>
          <label><input type="checkbox" data-key="2" checked> 2</label>
          <label><input type="checkbox" data-key="3" checked> 3</label>
          <label><input type="checkbox" data-key="4" checked> 4</label>
          <label><input type="checkbox" data-key="5" checked> 5</label>
          <label><input type="checkbox" data-key="6" checked> 6</label>
          <label><input type="checkbox" data-key="7" checked> 7</label>
          <label><input type="checkbox" id="menu-allow-exit" checked> 0. Exit</label>
        </div>
      </div>
      <div class="form-row">
        <label for="menu-items-text">Menu Items (one per line: <code>Label | info</code>, or <code>---</code> blank, or <code># text</code>)</label>
        <textarea id="menu-items-text" class="vgui-textarea" rows="8">\wAK-47 \y+ Deagle \R\rVIP | weapon_ak47
\wM4A1 Silenced \y+ Nades \R\rVIP | weapon_m4a1
\wAWP Sniper \d($4750 free) | weapon_awp
\wFAMAS Burst | weapon_famas
\wGalil Assault | weapon_galil
---
# \dTip: Type /vipmenu anytime on Round 2+
\wDesert Eagle Only | weapon_deagle
\wArmor + Defuse Kit | equip_kit
\wFull Grenade Pack | equip_nades</textarea>
      </div>
      <div class="studio-actions">
        <button type="button" class="vgui-btn" id="menu-prev-page">8. Back</button>
        <button type="button" class="vgui-btn" id="menu-next-page">9. More</button>
        <button type="button" class="vgui-btn accent" id="btn-menu-playground">&#9654; Test Menu in Playground</button>
      </div>
    </div>
    <div class="studio-preview">
      <div class="preview-header"><span>GoldSrc 1.6 ShowMenu Viewport</span><span id="menu-page-badge" class="pill">Page 1 / 2 · Mask 0x037f</span></div>
      <div class="goldsrc-viewport menu-viewport">
        <div class="goldsrc-menu-box" id="goldsrc-menu-preview"></div>
        <div class="viewport-status" id="menu-click-log">Click any numbered item in the preview to simulate menuselect!</div>
      </div>
    </div>
  </div>
  <div class="code-tabs" id="menu-code-tabs">
    <div class="code-tab-bar">
      <button type="button" class="code-tab-btn active" data-lang="py">Python 3</button>
      <button type="button" class="code-tab-btn" data-lang="rust">Rust</button>
      <button type="button" class="code-tab-btn" data-lang="cpp">C++20</button>
      <button type="button" class="code-tab-btn" data-lang="go">Go</button>
    </div>
    <div class="code-panel active" data-lang="py"><button type="button" class="copy-btn">Copy</button><pre><code id="menu-code-py"></code></pre></div>
    <div class="code-panel" data-lang="rust"><button type="button" class="copy-btn">Copy</button><pre><code id="menu-code-rust"></code></pre></div>
    <div class="code-panel" data-lang="cpp"><button type="button" class="copy-btn">Copy</button><pre><code id="menu-code-cpp"></code></pre></div>
    <div class="code-panel" data-lang="go"><button type="button" class="copy-btn">Copy</button><pre><code id="menu-code-go"></code></pre></div>
  </div>
</section>

<section class="doc-section studio-block" id="hud-studio">
  <h2>2. HUD &amp; DHUD Message Designer</h2>
  <div class="studio-grid">
    <div class="studio-controls">
      <div class="form-row-inline">
        <div><label for="hud-mode">Message Type</label><select id="hud-mode" class="vgui-input"><option value="dhud">DHUD (Large Bold Director HUD)</option><option value="hud">HUD (TE_TEXTMESSAGE Channel 0..4)</option></select></div>
        <div><label for="hud-channel">Channel (0..4)</label><input id="hud-channel" class="vgui-input" type="number" min="0" max="4" value="1"></div>
        <div><label for="hud-effect">Effect</label><select id="hud-effect" class="vgui-input"><option value="0">0 — Fade In/Out</option><option value="1">1 — Flicker</option><option value="2">2 — Write-Out Scan</option></select></div>
      </div>
      <div class="form-row"><label for="hud-text">HUD Text (supports multi-line)</label><textarea id="hud-text" class="vgui-textarea" rows="3">HEADSHOT BONUS! +20 HP
DOMINATING STREAK: 4 KILLS</textarea></div>
      <div class="form-row-inline">
        <div><label for="hud-x">X (<span id="hud-x-val">-1.00 (Center)</span>)</label><input id="hud-x" type="range" min="-1" max="1" step="0.01" value="-1"></div>
        <div><label for="hud-y">Y (<span id="hud-y-val">0.24</span>)</label><input id="hud-y" type="range" min="0" max="0.95" step="0.01" value="0.24"></div>
      </div>
      <div class="form-row-inline">
        <div><label for="hud-color">Primary Color</label><input id="hud-color" type="color" value="#ffb000"></div>
        <div><label for="hud-alpha">Alpha (<span id="hud-alpha-val">255</span>)</label><input id="hud-alpha" type="range" min="40" max="255" value="255"></div>
        <div><label for="hud-hold">Hold (<span id="hud-hold-val">2.5s</span>)</label><input id="hud-hold" type="range" min="0.5" max="10" step="0.1" value="2.5"></div>
      </div>
      <div class="form-row-inline">
        <div><label for="hud-fadein">Fade-In (<span id="hud-fadein-val">0.15s</span>)</label><input id="hud-fadein" type="range" min="0" max="2" step="0.05" value="0.15"></div>
        <div><label for="hud-fadeout">Fade-Out (<span id="hud-fadeout-val">0.35s</span>)</label><input id="hud-fadeout" type="range" min="0" max="3" step="0.05" value="0.35"></div>
      </div>
      <div class="studio-actions">
        <button type="button" class="vgui-btn" id="btn-hud-center">Center X (-1.0)</button>
        <button type="button" class="vgui-btn" id="btn-hud-replay">Replay Fade</button>
        <button type="button" class="vgui-btn accent" id="btn-hud-playground">&#9654; Test HUD in Playground</button>
      </div>
    </div>
    <div class="studio-preview">
      <div class="preview-header"><span>16:9 CS 1.6 HUD Viewport (Click to place!)</span><span id="hud-pos-badge" class="pill">x=-1.00, y=0.24</span></div>
      <div class="goldsrc-viewport hud-viewport" id="hud-stage">
        <div class="hud-crosshair"></div>
        <div class="hud-sample-text" id="hud-live-text"></div>
        <div class="hud-bottom-bar"><span>+ 100</span><span>&#9670; 100</span><span>01:45</span><span>$ 16000</span><span>30 | 90</span></div>
      </div>
    </div>
  </div>
  <div class="code-tabs" id="hud-code-tabs">
    <div class="code-tab-bar">
      <button type="button" class="code-tab-btn active" data-lang="py">Python 3</button>
      <button type="button" class="code-tab-btn" data-lang="rust">Rust</button>
      <button type="button" class="code-tab-btn" data-lang="cpp">C++20</button>
      <button type="button" class="code-tab-btn" data-lang="go">Go</button>
    </div>
    <div class="code-panel active" data-lang="py"><button type="button" class="copy-btn">Copy</button><pre><code id="hud-code-py"></code></pre></div>
    <div class="code-panel" data-lang="rust"><button type="button" class="copy-btn">Copy</button><pre><code id="hud-code-rust"></code></pre></div>
    <div class="code-panel" data-lang="cpp"><button type="button" class="copy-btn">Copy</button><pre><code id="hud-code-cpp"></code></pre></div>
    <div class="code-panel" data-lang="go"><button type="button" class="copy-btn">Copy</button><pre><code id="hud-code-go"></code></pre></div>
  </div>
</section>

<section class="doc-section studio-block" id="vfx-studio">
  <h2>3. Protocol 48 <code>VfxOp</code> &amp; Screen FX Studio</h2>
  <div class="studio-grid">
    <div class="studio-controls">
      <div class="form-row"><label for="vfx-kind">VFX Primitive</label>
        <select id="vfx-kind" class="vgui-input">
          <option value="shockwave_ring">shockwave_ring (TE_BEAMCYLINDER)</option>
          <option value="laser_beam">laser_beam (TE_BEAMPOINTS)</option>
          <option value="dynamic_light">aura_light / dynamic_light (TE_DLIGHT)</option>
          <option value="particle_burst">implosion / particle_burst (TE_IMPLOSION)</option>
          <option value="screen_fade">screen_fade (ScreenFade UserMsg)</option>
          <option value="screen_shake">screen_shake (ScreenShake UserMsg)</option>
        </select>
      </div>
      <div class="form-row-inline">
        <div><label for="vfx-color">Color</label><input id="vfx-color" type="color" value="#00dcff"></div>
        <div><label for="vfx-alpha">Alpha (<span id="vfx-alpha-val">230</span>)</label><input id="vfx-alpha" type="range" min="30" max="255" value="230"></div>
      </div>
      <div class="form-row-inline">
        <div><label for="vfx-radius">Radius / Amp (<span id="vfx-radius-val">200</span>)</label><input id="vfx-radius" type="range" min="10" max="400" value="200"></div>
        <div><label for="vfx-width">Width / Count (<span id="vfx-width-val">14</span>)</label><input id="vfx-width" type="range" min="2" max="64" value="14"></div>
      </div>
      <div class="form-row-inline">
        <div><label for="vfx-life">Life (decisec: <span id="vfx-life-val">10</span>)</label><input id="vfx-life" type="range" min="2" max="40" value="10"></div>
        <div><label for="vfx-noise">Noise / Freq (<span id="vfx-noise-val">6</span>)</label><input id="vfx-noise" type="range" min="0" max="40" value="6"></div>
      </div>
      <div class="studio-actions">
        <button type="button" class="vgui-btn" id="btn-vfx-trigger">Trigger Pulse</button>
        <button type="button" class="vgui-btn accent" id="btn-vfx-playground">&#9654; Test VFX in Playground</button>
      </div>
    </div>
    <div class="studio-preview">
      <div class="preview-header"><span>Live Protocol 48 TempEntity Preview</span><span id="vfx-wire-badge" class="pill">TE_BEAMCYLINDER</span></div>
      <div class="goldsrc-viewport canvas-viewport">
        <canvas id="vfx-canvas" width="520" height="290"></canvas>
      </div>
    </div>
  </div>
  <div class="code-tabs" id="vfx-code-tabs">
    <div class="code-tab-bar">
      <button type="button" class="code-tab-btn active" data-lang="py">Python 3</button>
      <button type="button" class="code-tab-btn" data-lang="rust">Rust</button>
      <button type="button" class="code-tab-btn" data-lang="cpp">C++20</button>
      <button type="button" class="code-tab-btn" data-lang="go">Go</button>
    </div>
    <div class="code-panel active" data-lang="py"><button type="button" class="copy-btn">Copy</button><pre><code id="vfx-code-py"></code></pre></div>
    <div class="code-panel" data-lang="rust"><button type="button" class="copy-btn">Copy</button><pre><code id="vfx-code-rust"></code></pre></div>
    <div class="code-panel" data-lang="cpp"><button type="button" class="copy-btn">Copy</button><pre><code id="vfx-code-cpp"></code></pre></div>
    <div class="code-panel" data-lang="go"><button type="button" class="copy-btn">Copy</button><pre><code id="vfx-code-go"></code></pre></div>
  </div>
</section>"##;

    render_docs_shell(
        "vfx-menus",
        "VFX, HUD & GoldSrc Menu Visual Designer",
        "Interactive GoldSrc menu, HUD/DHUD, and Protocol 48 VFX visual studio with live 4-language code generation.",
        &toc,
        body,
        &["./vfx-designer.js"],
    )
}

fn render_amxx_page(natives: &[AmxxNativeDoc]) -> String {
    let toc = [
        ("sma-converter", "Interactive .sma Converter"),
        ("amxx-cli", "5-Second Server Migrator"),
        ("stock-plugins", "25 Stock Plugin Upgrades"),
        ("natives-table", "AMXX Native Mapping Table"),
    ];

    let mut stock_rows = String::new();
    for entry in STOCK_AMXX_PLUGINS {
        let note = entry.note.unwrap_or("Drop-in native replacement");
        let _ = write!(
            stock_rows,
            "<tr><td><code>{}</code></td><td><code>{}</code></td><td>{}</td></tr>",
            html_escape(entry.amxx_name),
            html_escape(entry.native_mod),
            html_escape(note),
        );
    }

    let mut modules = BTreeMap::<&str, usize>::new();
    for n in natives {
        *modules.entry(n.module).or_default() += 1;
    }
    let mut mod_chips = format!(
        r#"<button type="button" class="cat-chip active" data-amxx-mod="all">All ({})</button>"#,
        natives.len()
    );
    for (m, cnt) in &modules {
        let _ = write!(
            mod_chips,
            r#"<button type="button" class="cat-chip" data-amxx-mod="{m}">{m} ({cnt})</button>"#
        );
    }

    let mut native_rows = String::with_capacity(natives.len() * 180);
    for n in natives {
        let anchor = slugify(&n.native);
        let ft = format!("{} {} {} {}", n.native, n.module, n.py_equiv, n.rust_equiv)
            .to_ascii_lowercase();
        let _ = write!(
            native_rows,
            r#"<tr id="amxx-{anchor}" data-amxx-row data-amxx-mod="{module}" data-filter-text="{ft}">
  <td><code>{native}</code></td>
  <td><span class="pill">{module}</span></td>
  <td><code>{py}</code></td>
  <td><code>{rust}</code></td>
  <td>{notes}</td>
</tr>"#,
            anchor = html_escape(&anchor),
            module = n.module,
            ft = html_escape(&ft),
            native = html_escape(&n.native),
            py = html_escape(n.py_equiv),
            rust = html_escape(n.rust_equiv),
            notes = html_escape(n.notes),
        );
    }

    let body = format!(
        r#"<section class="page-head">
  <div class="hero-kicker">PAWN 3.2 COMPATIBILITY + 5-SECOND SERVER MIGRATION + LIVE CONVERTER</div>
  <h1>AMX Mod X Migration &amp; Interactive <code>.sma</code> Converter</h1>
  <p class="hero-lead">Migrate an existing <code>addons/amxmodx/</code> server in 5 seconds via <code>cargo xtask import-amxx</code>, run custom <code>.amxx</code> binaries directly on our safe Rust Pawn VM (<strong>{native_count}</strong> natives), or paste any <code>.sma</code> source below to convert it live into idiomatic <strong>Python 3</strong> or <strong>Rust</strong>!</p>
</section>

<section class="doc-section" id="sma-converter">
  <h2>1. Interactive <code>.sma</code> &rarr; Python 3 / Rust Converter</h2>
  <div class="preset-bar">
    <span>Load Preset <code>.sma</code>:</span>
    <button type="button" class="vgui-btn" data-sma-preset="vip">VIP System</button>
    <button type="button" class="vgui-btn" data-sma-preset="resetscore">ResetScore</button>
    <button type="button" class="vgui-btn" data-sma-preset="damager">Hit Damager</button>
    <button type="button" class="vgui-btn" data-sma-preset="admincmd">Admin Slap/Slay</button>
  </div>
  <div class="converter-grid">
    <div class="converter-pane">
      <div class="pane-head"><span>Input AMX Mod X Pawn (<code>.sma</code>)</span><span class="pill" id="sma-line-count">0 lines</span></div>
      <textarea id="sma-input" class="vgui-textarea code-editor" rows="20" spellcheck="false"></textarea>
    </div>
    <div class="converter-pane">
      <div class="pane-head">
        <div class="out-tab-bar">
          <button type="button" class="out-tab active" data-out-lang="py">Python 3 (<code>cs16</code>)</button>
          <button type="button" class="out-tab" data-out-lang="rust">Rust (<code>cs-mod-sdk</code>)</button>
        </div>
        <div class="pane-actions">
          <button type="button" class="vgui-btn" id="btn-copy-converted">Copy</button>
          <button type="button" class="vgui-btn" id="btn-download-converted">Download</button>
          <button type="button" class="vgui-btn accent" id="btn-sma-playground">&#9654; Open in Playground</button>
        </div>
      </div>
      <pre class="converted-output"><code id="sma-output"></code></pre>
    </div>
  </div>
  <div class="diagnostics-box" id="sma-diagnostics"></div>
</section>

<section class="doc-section" id="amxx-cli">
  <h2>2. 5-Second Legacy Server Migrator (<code>cargo xtask import-amxx</code>)</h2>
  <p>Point the migrator at any existing <code>cstrike/</code> or <code>addons/amxmodx/</code> directory. It parses <code>users.ini</code>, <code>plugins.ini</code>, <code>plugins-*.ini</code>, <code>maps.ini</code>, <code>amxx.cfg</code>, and <code>data/lang/*.txt</code> into native <code>configs/*.toml</code> while mounting custom third-party <code>.amxx</code> binaries automatically on <code>cs-amxx</code>:</p>
  {cli_code}
</section>

<section class="doc-section" id="stock-plugins">
  <h2>3. Automatic Stock Plugin Upgrades ({stock_count} Plugins)</h2>
  <p>Introspected directly from <code>cs_mod_host::amxx_import::STOCK_AMXX_PLUGINS</code>:</p>
  <div class="table-wrap">
    <table class="vgui-table compact">
      <thead><tr><th>Legacy AMXX Plugin</th><th>Native <code>cs16-rs</code> Replacement</th><th>Migration Note</th></tr></thead>
      <tbody>{stock_rows}</tbody>
    </table>
  </div>
</section>

<section class="doc-section" id="natives-table">
  <h2>4. AMXX Native &rarr; <code>cs16-rs</code> ABI v2 Mapping ({native_count} Natives)</h2>
  <p>Introspected directly from <code>crates/cs-amxx/src/natives.rs</code>:</p>
  <div class="filter-bar">
    <input type="search" id="amxx-search" class="vgui-input" placeholder="Search AMXX natives (e.g. RegisterHam, cs_set_user_money, nvault, rg_give_item)...">
    <div class="filter-chips" id="amxx-mod-chips">{mod_chips}</div>
    <span class="filter-count" id="amxx-count">Showing {native_count} of {native_count} natives</span>
  </div>
  <div class="table-wrap">
    <table class="vgui-table compact">
      <thead><tr><th>AMXX Native</th><th>Module</th><th>Python 3 Equivalent</th><th>Rust SDK Equivalent</th><th>Notes</th></tr></thead>
      <tbody>{native_rows}</tbody>
    </table>
  </div>
</section>"#,
        native_count = natives.len(),
        stock_count = STOCK_AMXX_PLUGINS.len(),
        cli_code = render_single_code(
            "One-Command AMXX Server Migration",
            "cargo xtask import-amxx /path/to/cstrike/addons/amxmodx --out-dir .\n# Boots all upgraded native plugins + custom .amxx binaries side-by-side:\ncargo run --release -p cs-server -- --map de_dust2 --plugins-toml configs/plugins.toml --admin",
        ),
    );

    render_docs_shell(
        "amxx-migration",
        "AMX Mod X Migration & .sma Converter",
        "AMX Mod X native mapping reference, cargo xtask import-amxx guide, and interactive .sma to Python 3 / Rust converter.",
        &toc,
        &body,
        &["./amxx-converter.js"],
    )
}

fn render_cvars_deployment_page(cvars: &[CVarDoc]) -> String {
    let toc = [
        ("cvar-explorer", "CVar & Command Explorer"),
        ("deployment", "Dedicated Server CLI"),
        ("webtransport-tls", "HTTP/3 WebTransport & TLS"),
        ("docker-ghcr", "Docker & GHCR"),
        ("web-admin", "Web Admin API & Paths"),
    ];

    let cvar_only = cvars.iter().filter(|c| c.kind == "cvar").count();
    let cmd_only = cvars.len() - cvar_only;

    // Pre-render the complete index table cleanly (also enhanced with instant multi-facet JS filtering).
    let mut cvar_rows = String::with_capacity(cvars.len() * 140);
    for c in cvars {
        let flags_str = if c.flags.is_empty() {
            String::new()
        } else {
            c.flags
                .iter()
                .map(|f| format!(r#"<span class="pill pill-flag">{f}</span>"#))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let flags_data = c.flags.join(" ");
        let def_cell = if c.default_value.is_empty() {
            "&mdash;".to_string()
        } else {
            format!("<code>{}</code>", html_escape(&c.default_value))
        };
        let ft = format!(
            "{} {} {} {} {}",
            c.id, c.kind, c.category, flags_data, c.description
        )
        .to_ascii_lowercase();
        let _ = write!(
            cvar_rows,
            r#"<tr id="cvar-{slug}" data-cvar-row data-kind="{kind}" data-cat="{cat}" data-flags="{flags_data}" data-filter-text="{ft}">
  <td><code>{id}</code></td>
  <td><span class="pill">{kind}</span></td>
  <td>{def_cell}</td>
  <td>{flags_str}</td>
  <td>{desc}</td>
</tr>"#,
            slug = html_escape(&slugify(&c.id)),
            kind = c.kind,
            cat = c.category,
            ft = html_escape(&ft),
            id = html_escape(&c.id),
            desc = html_escape(&c.description),
        );
    }

    let body = format!(
        r#"<section class="page-head">
  <div class="hero-kicker">AUTO-GENERATED FROM <code>docs/parity/cvars.toml</code> &amp; <code>cs_cvar::CVarRegistry</code></div>
  <h1>{total}/{total} CVar &amp; Console Command Encyclopedia + Deployment Manual</h1>
  <p class="hero-lead">Complete searchable reference for all <strong>{cvar_only}</strong> CVars and <strong>{cmd_only}</strong> console commands, plus production deployment guides for single-binary <code>cs-server</code>, HTTP/3 WebTransport, TLS <code>wss://</code>, Docker/GHCR, and Web Admin.</p>
</section>

<section class="doc-section" id="cvar-explorer">
  <h2>1. Searchable CVar &amp; Console Command Explorer ({total} Entries)</h2>
  <div class="filter-bar">
    <input type="search" id="cvar-search" class="vgui-input" placeholder="Filter all {total} CVars &amp; commands (e.g. sv_airaccelerate, mp_freezetime, cl_righthand, bot_quota)...">
    <div class="filter-row-selects">
      <select id="cvar-kind-filter" class="vgui-input">
        <option value="all">All Types ({total})</option>
        <option value="cvar">CVars Only ({cvar_only})</option>
        <option value="command">Commands Only ({cmd_only})</option>
      </select>
      <select id="cvar-cat-filter" class="vgui-input">
        <option value="all">All Categories</option>
        <option value="sv_">sv_* (Server)</option>
        <option value="mp_">mp_* (Match Rules)</option>
        <option value="cl_">cl_* (Client)</option>
        <option value="bot_">bot_* (ZBots)</option>
        <option value="render">gl_* / r_* (Renderer)</option>
        <option value="hud_ui">hud_* / vgui (HUD &amp; UI)</option>
        <option value="input">m_* / +/- (Input &amp; Mouse)</option>
        <option value="network">net_* / rate / rcon</option>
        <option value="audio">snd_* / voice_* / volume</option>
        <option value="modding">amx_* / mod_* (Modding)</option>
        <option value="general">General</option>
      </select>
      <select id="cvar-flag-filter" class="vgui-input">
        <option value="all">Any Flag</option>
        <option value="ARCHIVE">ARCHIVE</option>
        <option value="SERVER">SERVER</option>
        <option value="USERINFO">USERINFO</option>
        <option value="REPLICATED">REPLICATED</option>
        <option value="CHEAT">CHEAT</option>
        <option value="PROTECTED">PROTECTED</option>
        <option value="SPONLY">SPONLY</option>
      </select>
    </div>
    <span class="filter-count" id="cvar-count">Showing {total} of {total} entries</span>
  </div>
  <div class="table-wrap cvar-table-wrap">
    <table class="vgui-table compact">
      <thead><tr><th>Name</th><th>Kind</th><th>Default</th><th>Flags</th><th>Description</th></tr></thead>
      <tbody id="cvar-tbody">{cvar_rows}</tbody>
    </table>
  </div>
</section>

<section class="doc-section" id="deployment">
  <h2>2. Dedicated 100 Hz Server (<code>cs-server</code>)</h2>
  <p><code>cs-server</code> runs the full Protocol 48 UDP game server, browser WebSocket/HTTP static host, optional HTTP/3 WebTransport server, HLDEMO v5 per-round recorder, and polyglot mod host in a single zero-<code>unsafe</code> Rust binary:</p>
  {server_cli}
</section>

<section class="doc-section" id="webtransport-tls">
  <h2>3. HTTP/3 WebTransport (<code>--webtransport-port</code>) &amp; TLS <code>wss://</code></h2>
  <p>For lowest-latency browser play with unreliable UDP-style datagrams (zero head-of-line blocking), enable HTTP/3 WebTransport alongside TLS WebSockets:</p>
  {tls_code}
  <p>When connecting from an HTTPS origin (such as GitHub Pages), browsers require encrypted <code>wss://</code> or HTTP/3 WebTransport rather than plaintext <code>ws://</code>.</p>
</section>

<section class="doc-section" id="docker-ghcr">
  <h2>4. Docker, Docker Compose &amp; GHCR</h2>
  <p>Mount your local Half-Life / Counter-Strike 1.6 directory read-only into <code>/gamedir</code> (no Valve assets are ever baked into container images):</p>
  {docker_code}
</section>

<section class="doc-section" id="web-admin">
  <h2>5. Web Admin Panel (<code>/admin.html</code>), REST API &amp; Asset Paths</h2>
  <p>Authenticate with <code>Authorization: Bearer &lt;rcon_password&gt;</code> (constant-time verified):</p>
  <div class="table-wrap">
    <table class="vgui-table compact">
      <thead><tr><th>Endpoint</th><th>Method</th><th>Purpose</th></tr></thead>
      <tbody>
        <tr><td><code>/api/status</code></td><td>GET</td><td>Server status, tickrate, current map, and available maps JSON</td></tr>
        <tr><td><code>/api/players</code></td><td>GET</td><td>Connected human players &amp; ZBots with ping, team, score</td></tr>
        <tr><td><code>/api/stats</code></td><td>GET</td><td>Live match leaderboard (K/D, HS%, ADR, assists, plants, defuses, MVPs)</td></tr>
        <tr><td><code>/api/replays</code> &amp; <code>/api/replays/&lt;id&gt;.dem</code></td><td>GET</td><td>List and download HLDEMO v5 round replay files</td></tr>
        <tr><td><code>/api/rcon</code></td><td>POST</td><td>Execute server console command <code>{{"command":"..."}}</code></td></tr>
        <tr><td><code>/api/mods</code> &amp; <code>/api/mods/faults/&lt;id&gt;.json</code></td><td>GET</td><td>Live ResMon metrics &amp; black-box <code>ModFaultDump</code> downloads</td></tr>
        <tr><td><code>/api/mods/source/&lt;file&gt;</code></td><td>GET / PUT</td><td>Sandboxed live editor for <code>mods/*.py</code> &amp; <code>configs/*.toml</code> with &lt;5 ms hot-reload</td></tr>
        <tr><td><code>/api/invite</code></td><td>GET</td><td>Generate shareable <code>?join=host:port&amp;room=code</code> web lobby links</td></tr>
      </tbody>
    </table>
  </div>
  <h3>Cross-Platform Half-Life Asset Discovery</h3>
  <ul class="vgui-list">
    <li><strong>Linux (XDG):</strong> <code>$XDG_CONFIG_HOME/cs16-rs</code>, auto-scans Steam &amp; Flatpak <code>libraryfolders.vdf</code>.</li>
    <li><strong>Steam Deck / SteamOS:</strong> Auto-detects <code>gamescope</code> / SteamOS and microSD mounts under <code>/run/media/mmcblk0p1/SteamLibrary</code>.</li>
    <li><strong>macOS:</strong> <code>~/Library/Application Support/cs16-rs</code> &amp; Steam <code>libraryfolders.vdf</code>.</li>
    <li><strong>Override anywhere:</strong> Set <code>CS16_GAMEDIR=/path/to/Half-Life</code> or pass <code>--gamedir</code> (or <code>--demo</code> for clean-room <code>dm_rusthouse</code>).</li>
  </ul>
</section>"#,
        total = cvars.len(),
        server_cli = render_single_code(
            "Single-Binary Production Launch",
            "cargo build --release -p cs-server --bin cs-server\n./target/release/cs-server \\\n  --gamedir /srv/steam/Half-Life \\\n  --port 27015 \\\n  --web-port 27016 \\\n  --web-dir ./dist/web \\\n  --webtransport-port 27017 \\\n  --wt-cert /etc/ssl/fullchain.pem \\\n  --wt-key /etc/ssl/privkey.pem \\\n  --plugins-toml configs/plugins.toml \\\n  --admin \\\n  --map de_dust2 --maxplayers 16 --bots 4 \\\n  --hostname \"cs16-rs 100Hz Dedicated\"",
        ),
        tls_code = render_single_code(
            "Caddy Reverse Proxy (TLS wss:// + /docs/ + /playground/ + /mods/)",
            "play.example.com {\n    reverse_proxy 127.0.0.1:27016\n}\n# UDP 27015: Native GoldSrc / cs-client Protocol 48 & A2S\n# UDP 27017: HTTP/3 WebTransport datagrams (--webtransport-port 27017)\n# TCP 27016: Static portal (/, /docs/, /playground/, /mods/, /admin.html) + /ws",
        ),
        docker_code = render_single_code(
            "Docker Compose & GHCR",
            "export CS16_GAMEDIR=/srv/steam/Half-Life\nexport CS16_MAP=de_dust2\ndocker compose up -d --build\n\n# Or run directly:\ndocker run -d --name cs16-server \\\n  -p 27015:27015/udp -p 27016:27016/tcp -p 27017:27017/udp \\\n  -v /srv/steam/Half-Life:/gamedir:ro \\\n  ghcr.io/maybewewill/cs16-rs:latest \\\n  --gamedir /gamedir --port 27015 --web-port 27016 --web-dir /app/web --map de_dust2 --admin",
        ),
    );

    render_docs_shell(
        "cvars-deployment",
        "736 CVars & Deployment Manual",
        "Searchable 736/736 Counter-Strike 1.6 CVar and console command reference plus Dedicated Server, Docker, HTTP/3 WebTransport, and Web Admin deployment manual.",
        &toc,
        &body,
        &[],
    )
}

// ===========================================================================
// 7. Shared Steam-2003 VGUI2 CSS & Client-Side JS Assets
// ===========================================================================

/// Shared Steam-2003 Olive/Charcoal & Amber VGUI2 Developer Portal stylesheet reused by `/docs/` and `/mods/`.
pub(crate) const PORTAL_CSS: &str = r##":root {
  --bg: #1d211a;
  --inset: #2c3327;
  --terminal: #141812;
  --panel: #3e4637;
  --panel-alt: #4c5844;
  --panel-hover: #5a6a50;
  --border-light: #889180;
  --border-dark: #282e22;
  --text: #d8ded3;
  --text-bright: #ecefe9;
  --muted: #a0aa95;
  --amber: #c4b550;
  --hud-amber: #ffb000;
  --green: #74d474;
  --red: #ff5b5b;
  --ct: #99ccff;
}

* { box-sizing: border-box; margin: 0; padding: 0; border-radius: 0 !important; }

html { scroll-behavior: smooth; }

body {
  background: var(--bg);
  color: var(--text);
  font-family: Tahoma, Verdana, "Segoe UI", sans-serif;
  font-size: 13px;
  line-height: 1.55;
  min-height: 100vh;
}

a { color: var(--amber); text-decoration: none; }
a:hover { text-decoration: underline; color: #f0df78; }

code, pre, textarea.code-editor {
  font-family: Consolas, "Cascadia Mono", "Courier New", monospace;
}

p code, li code, td code, h1 code, h2 code, h3 code {
  background: var(--inset);
  border: 1px solid var(--border-dark);
  padding: 1px 4px;
  font-size: 12px;
  color: #f0df78;
}

/* Shared Steam-2003 Top Navbar */
.portal-navbar {
  position: sticky;
  top: 0;
  z-index: 60;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  background: var(--panel-alt);
  border-top: 1px solid var(--border-light);
  border-bottom: 2px solid var(--border-dark);
  padding: 6px 14px;
}

.navbar-left, .navbar-links, .navbar-right {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.brand-home {
  display: flex;
  align-items: center;
  gap: 8px;
  text-decoration: none !important;
  color: var(--text-bright);
}

.brand-logo { width: 24px; height: 24px; display: block; object-fit: contain; }
.brand-name { font-weight: bold; font-size: 15px; color: var(--amber); letter-spacing: 0.4px; }
.brand-sub { font-size: 11px; color: var(--muted); border-left: 1px solid var(--border-dark); padding-left: 8px; }

.vgui-nav-link, .vgui-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--panel);
  color: var(--text-bright);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 4px 10px;
  font-size: 12px;
  font-family: inherit;
  text-decoration: none !important;
  cursor: pointer;
}

.vgui-nav-link:hover, .vgui-btn:hover {
  background: var(--panel-hover);
  color: var(--amber);
}

.vgui-nav-link.active, .vgui-btn.accent {
  background: #4e5938;
  color: var(--amber);
  font-weight: bold;
  border-top: 1px solid var(--amber);
  border-left: 1px solid var(--amber);
}

.vgui-btn.full-btn { width: 100%; justify-content: center; margin-bottom: 6px; }

.search-trigger kbd {
  background: var(--inset);
  border: 1px solid var(--border-dark);
  padding: 1px 4px;
  font-size: 10px;
  color: var(--amber);
}

.mobile-drawer-btn { display: none; }

/* 3-Column Portal Layout */
.portal-layout {
  display: grid;
  grid-template-columns: 246px minmax(0, 1fr) 210px;
  max-width: 1540px;
  margin: 0 auto;
  min-height: calc(100vh - 46px);
}

.portal-sidebar {
  position: sticky;
  top: 44px;
  height: calc(100vh - 44px);
  overflow-y: auto;
  background: var(--panel);
  border-right: 1px solid var(--border-dark);
  padding: 12px 10px;
}

.sidebar-group-title, .toc-title {
  color: var(--amber);
  font-size: 11px;
  font-weight: bold;
  text-transform: uppercase;
  letter-spacing: 0.6px;
  border-bottom: 1px solid var(--border-dark);
  padding-bottom: 4px;
  margin: 12px 0 8px 0;
}

.sidebar-group-title:first-child, .toc-title { margin-top: 0; }

.sidebar-nav { display: flex; flex-direction: column; gap: 4px; }

.side-link {
  display: flex;
  flex-direction: column;
  padding: 6px 8px;
  background: var(--inset);
  border-top: 1px solid var(--border-dark);
  border-left: 1px solid var(--border-dark);
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  text-decoration: none !important;
}

.side-link:hover { background: #364030; }
.side-link.active {
  background: var(--panel-alt);
  border-left: 3px solid var(--amber);
}
.side-title { color: var(--text-bright); font-weight: bold; font-size: 12px; }
.side-link.active .side-title { color: var(--amber); }
.side-hint { color: var(--muted); font-size: 11px; }

.lang-switcher-box {
  background: var(--inset);
  border: 1px solid var(--border-dark);
  padding: 8px;
}
.lang-hint { font-size: 11px; color: var(--muted); margin-bottom: 6px; }
.global-lang-pills { display: flex; flex-wrap: wrap; gap: 4px; }
.lang-pill {
  background: var(--panel);
  color: var(--text);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 2px 6px;
  font-size: 11px;
  cursor: pointer;
}
.lang-pill.active { background: var(--amber); color: #161a14; font-weight: bold; }

/* Main Content */
.portal-main {
  padding: 18px 24px 36px 24px;
  min-width: 0;
}

.hero-panel, .page-head, .doc-section {
  background: var(--panel);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 16px 18px;
  margin-bottom: 16px;
}

.hero-kicker {
  color: var(--amber);
  font-size: 11px;
  font-weight: bold;
  letter-spacing: 0.7px;
  margin-bottom: 6px;
}

h1 { font-size: 23px; color: var(--text-bright); margin-bottom: 8px; line-height: 1.25; }
h2 {
  font-size: 16px;
  color: var(--amber);
  border-bottom: 1px solid var(--border-dark);
  padding-bottom: 5px;
  margin-bottom: 10px;
}
h3 { font-size: 14px; color: var(--text-bright); margin: 12px 0 6px 0; }
p { margin-bottom: 10px; }
.hero-lead { font-size: 13.5px; color: var(--text-bright); max-width: 920px; }

.stat-strip {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(125px, 1fr));
  gap: 8px;
  margin: 14px 0;
}

.stat-tile {
  background: var(--inset);
  border-top: 1px solid var(--border-dark);
  border-left: 1px solid var(--border-dark);
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
}
.stat-num { font-size: 20px; font-weight: bold; color: var(--amber); }
.stat-label { font-size: 11px; color: var(--muted); }

.hero-actions { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 10px; }

.arch-diagram pre {
  background: var(--terminal);
  border: 1px solid var(--border-dark);
  padding: 12px;
  overflow-x: auto;
  font-size: 12px;
  line-height: 1.4;
  color: #cfe0c2;
}

.vgui-list { margin: 8px 0 10px 20px; }
.vgui-list li { margin-bottom: 5px; }

.callout-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 8px;
  margin: 10px 0 12px 0;
}
.callout {
  background: var(--inset);
  border-left: 3px solid var(--amber);
  padding: 8px 10px;
  font-size: 12px;
}

.meta-strip {
  margin-top: 8px;
  padding: 6px 8px;
  background: var(--inset);
  border: 1px solid var(--border-dark);
  font-size: 12px;
  color: var(--muted);
}

/* Sticky Right TOC */
.portal-toc {
  position: sticky;
  top: 44px;
  height: calc(100vh - 44px);
  overflow-y: auto;
  background: #232820;
  border-left: 1px solid var(--border-dark);
  padding: 12px 10px;
}

.toc-nav { display: flex; flex-direction: column; gap: 3px; }
.toc-link {
  font-size: 12px;
  color: var(--muted);
  padding: 3px 6px;
  border-left: 2px solid transparent;
}
.toc-link:hover, .toc-link.active {
  color: var(--amber);
  border-left-color: var(--amber);
  background: rgba(196, 181, 80, 0.08);
  text-decoration: none;
}

/* Synchronized Code Tabs & Single Code Boxes */
.code-tabs, .code-box {
  background: var(--terminal);
  border-top: 1px solid var(--border-dark);
  border-left: 1px solid var(--border-dark);
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  margin: 10px 0;
}

.code-tab-bar, .code-box-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 4px;
  background: var(--inset);
  border-bottom: 1px solid var(--border-dark);
  padding: 4px 6px;
}

.code-tab-bar { justify-content: flex-start; }

.code-tab-btn {
  background: var(--panel);
  color: var(--muted);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 3px 9px;
  font-size: 11.5px;
  cursor: pointer;
}
.code-tab-btn.active {
  background: var(--panel-alt);
  color: var(--amber);
  font-weight: bold;
  border-top-color: var(--amber);
}

.code-panel { display: none; position: relative; }
.code-panel.active { display: block; }

.code-panel pre, .code-box pre, .converted-output {
  padding: 12px 14px;
  overflow-x: auto;
  font-size: 12px;
  line-height: 1.48;
  color: #e4ecd9;
}

.copy-btn {
  background: var(--panel);
  color: var(--text-bright);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 2px 8px;
  font-size: 11px;
  cursor: pointer;
}
.copy-btn:hover { background: var(--panel-hover); color: var(--amber); }
.code-panel .copy-btn { position: absolute; top: 6px; right: 8px; z-index: 2; }

/* Tables */
.table-wrap { overflow-x: auto; margin: 8px 0; }
.cvar-table-wrap { max-height: 620px; overflow-y: auto; border: 1px solid var(--border-dark); }

.vgui-table {
  width: 100%;
  border-collapse: collapse;
  background: var(--inset);
  font-size: 12px;
}
.vgui-table th, .vgui-table td {
  border: 1px solid var(--border-dark);
  padding: 6px 8px;
  text-align: left;
  vertical-align: top;
}
.vgui-table th {
  position: sticky;
  top: 0;
  background: var(--panel-alt);
  color: var(--amber);
  font-weight: bold;
  z-index: 2;
}
.vgui-table.compact td, .vgui-table.compact th { padding: 4px 7px; }
.vgui-table tbody tr:hover { background: #343d2e; }
.cell-sub { margin-top: 4px; }

/* Pills & Badges */
.pill {
  display: inline-block;
  background: var(--inset);
  border: 1px solid var(--border-light);
  color: var(--text);
  padding: 1px 6px;
  font-size: 10.5px;
  white-space: nowrap;
}
.pill-amber { border-color: var(--amber); color: var(--amber); }
.pill-id { background: var(--panel-alt); color: var(--amber); font-weight: bold; }
.pill-cap { border-color: var(--green); color: var(--green); }
.pill-flag { border-color: var(--ct); color: var(--ct); font-size: 10px; }

/* Filter Bars & Inputs */
.filter-bar { display: flex; flex-direction: column; gap: 8px; }
.filter-chips { display: flex; flex-wrap: wrap; gap: 5px; }
.filter-row-selects { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 6px; }
.filter-count { font-size: 12px; color: var(--amber); font-weight: bold; }

.vgui-input, .vgui-textarea {
  width: 100%;
  background: var(--inset);
  color: var(--text-bright);
  border-top: 1px solid var(--border-dark);
  border-left: 1px solid var(--border-dark);
  border-right: 1px solid var(--border-light);
  border-bottom: 1px solid var(--border-light);
  padding: 6px 8px;
  font-size: 12.5px;
  font-family: inherit;
}

.cat-chip {
  background: var(--inset);
  color: var(--text);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 3px 9px;
  font-size: 11.5px;
  cursor: pointer;
}
.cat-chip.active, .cat-chip:hover {
  background: var(--panel-alt);
  color: var(--amber);
  border-top-color: var(--amber);
}

/* Hook Cards */
.hook-card {
  background: var(--inset);
  border-top: 1px solid var(--border-light);
  border-left: 1px solid var(--border-light);
  border-right: 1px solid var(--border-dark);
  border-bottom: 1px solid var(--border-dark);
  padding: 12px 14px;
  margin-bottom: 14px;
}
.hook-head { display: flex; justify-content: space-between; align-items: flex-start; gap: 8px; margin-bottom: 6px; }
.hook-title-wrap { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.hook-snake { color: var(--muted); font-weight: normal; font-size: 12px; }
.hook-badges { display: flex; flex-wrap: wrap; gap: 4px; }
.anchor-link { color: var(--muted); font-weight: bold; font-size: 15px; }
.hook-meta-line { display: flex; flex-wrap: wrap; gap: 14px; font-size: 12px; color: var(--muted); margin-bottom: 6px; }
.hook-override-note {
  background: rgba(196, 181, 80, 0.08);
  border-left: 3px solid var(--amber);
  padding: 5px 8px;
  font-size: 12px;
  margin-bottom: 8px;
}

/* Interactive Visual Designers */
.studio-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.1fr);
  gap: 14px;
  margin-bottom: 12px;
}
.studio-controls { display: flex; flex-direction: column; gap: 8px; }
.form-row { display: flex; flex-direction: column; gap: 4px; }
.form-row label, .form-row-inline label { font-size: 11.5px; color: var(--amber); font-weight: bold; display: block; margin-bottom: 2px; }
.form-row-inline { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 8px; align-items: end; }
.keymask-row { display: flex; flex-wrap: wrap; gap: 8px; background: var(--inset); padding: 6px 8px; border: 1px solid var(--border-dark); }
.keymask-row label { display: inline-flex; align-items: center; gap: 3px; color: var(--text); font-weight: normal; margin: 0; }
.studio-actions { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }

.studio-preview { display: flex; flex-direction: column; }
.preview-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: var(--inset);
  border: 1px solid var(--border-dark);
  padding: 5px 8px;
  font-size: 11.5px;
  color: var(--amber);
}
.goldsrc-viewport {
  position: relative;
  min-height: 290px;
  background:
    radial-gradient(circle at 60% 42%, rgba(164, 134, 86, 0.25), rgba(14, 18, 12, 0.94) 70%),
    linear-gradient(180deg, #272c22 0%, #141712 52%, #242118 52%, #181611 100%);
  border: 1px solid var(--border-dark);
  padding: 14px;
  overflow: hidden;
}
.goldsrc-menu-box {
  background: rgba(8, 10, 8, 0.72);
  border: 1px solid rgba(196, 181, 80, 0.35);
  padding: 10px 12px;
  max-width: 340px;
  font-family: Tahoma, Verdana, sans-serif;
  font-size: 13px;
  line-height: 1.5;
}
.gs-line { display: flex; justify-content: space-between; gap: 12px; padding: 1px 4px; }
.gs-line.clickable { cursor: pointer; }
.gs-line.clickable:hover { background: rgba(196, 181, 80, 0.2); }
.gs-y { color: #ffb000; }
.gs-w { color: #ffffff; }
.gs-r { color: #ff4444; }
.gs-d { color: #8c9488; }
.viewport-status {
  position: absolute;
  left: 10px;
  right: 10px;
  bottom: 8px;
  font-size: 11px;
  color: var(--amber);
  background: rgba(0, 0, 0, 0.65);
  padding: 3px 6px;
}

.hud-viewport { cursor: crosshair; user-select: none; }
.hud-crosshair {
  position: absolute;
  left: 50%;
  top: 50%;
  width: 14px;
  height: 14px;
  transform: translate(-50%, -50%);
  pointer-events: none;
}
.hud-crosshair::before, .hud-crosshair::after {
  content: "";
  position: absolute;
  background: rgba(0, 255, 64, 0.85);
}
.hud-crosshair::before { left: 6px; top: 0; width: 2px; height: 14px; }
.hud-crosshair::after { top: 6px; left: 0; height: 2px; width: 14px; }
.hud-sample-text {
  position: absolute;
  white-space: pre-wrap;
  pointer-events: none;
  text-shadow: 1px 1px 0 #000;
  transition: opacity 0.25s ease;
}
.hud-sample-text.dhud { font-size: 18px; font-weight: bold; letter-spacing: 0.5px; }
.hud-sample-text.hud { font-size: 13px; font-family: Consolas, monospace; }
.hud-bottom-bar {
  position: absolute;
  left: 12px;
  right: 12px;
  bottom: 8px;
  display: flex;
  justify-content: space-between;
  color: var(--hud-amber);
  font-weight: bold;
  font-size: 14px;
  text-shadow: 1px 1px 0 #000;
  pointer-events: none;
}

.canvas-viewport { padding: 0; display: flex; align-items: center; justify-content: center; }
#vfx-canvas { width: 100%; height: 290px; display: block; background: #10140f; }

/* AMXX Converter */
.preset-bar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin-bottom: 10px; }
.converter-grid { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 12px; }
.converter-pane { display: flex; flex-direction: column; background: var(--terminal); border: 1px solid var(--border-dark); }
.pane-head {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  align-items: center;
  gap: 6px;
  background: var(--inset);
  border-bottom: 1px solid var(--border-dark);
  padding: 5px 8px;
}
.out-tab-bar, .pane-actions { display: flex; gap: 4px; flex-wrap: wrap; }
.out-tab {
  background: var(--panel);
  color: var(--muted);
  border: 1px solid var(--border-dark);
  padding: 3px 8px;
  font-size: 11.5px;
  cursor: pointer;
}
.out-tab.active { background: var(--panel-alt); color: var(--amber); font-weight: bold; }
.converted-output { margin: 0; min-height: 320px; max-height: 460px; overflow: auto; }
.diagnostics-box {
  margin-top: 10px;
  background: var(--inset);
  border-left: 3px solid var(--amber);
  padding: 8px 10px;
  font-size: 12px;
}

/* Search Modal */
.search-backdrop {
  position: fixed;
  inset: 0;
  z-index: 200;
  background: rgba(10, 13, 9, 0.8);
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 8vh;
}
.search-backdrop.hidden { display: none; }
.search-dialog {
  width: 720px;
  max-width: calc(100vw - 24px);
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  background: var(--panel);
  border-top: 2px solid var(--border-light);
  border-left: 2px solid var(--border-light);
  border-right: 2px solid var(--border-dark);
  border-bottom: 2px solid var(--border-dark);
  padding: 10px;
  gap: 8px;
}
.search-header { display: flex; gap: 6px; }
.search-header input {
  flex: 1;
  background: var(--inset);
  color: var(--text-bright);
  border: 1px solid var(--border-light);
  padding: 7px 10px;
  font-size: 14px;
}
.search-cats { display: flex; flex-wrap: wrap; gap: 4px; }
.search-meta { font-size: 11.5px; color: var(--muted); }
.search-results {
  overflow-y: auto;
  max-height: 54vh;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.search-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 7px 10px;
  background: var(--inset);
  border: 1px solid var(--border-dark);
  text-decoration: none !important;
}
.search-item.selected, .search-item:hover {
  background: var(--panel-alt);
  border-color: var(--amber);
}
.search-item-head { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
.search-item-title { color: var(--amber); font-weight: bold; font-size: 13px; }
.search-item-summary { color: var(--text); font-size: 11.5px; }

/* Footer */
.portal-footer {
  margin-top: 20px;
  padding: 12px 14px;
  background: var(--inset);
  border: 1px solid var(--border-dark);
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 8px;
  font-size: 11.5px;
  color: var(--muted);
}

/* Responsive */
@media (max-width: 1100px) {
  .portal-layout { grid-template-columns: 230px minmax(0, 1fr); }
  .portal-toc { display: none; }
}
@media (max-width: 820px) {
  .mobile-drawer-btn { display: inline-flex; }
  .portal-layout { grid-template-columns: minmax(0, 1fr); }
  .portal-sidebar {
    display: none;
    position: fixed;
    top: 44px;
    left: 0;
    width: 265px;
    z-index: 90;
    border-right: 2px solid var(--amber);
  }
  .portal-sidebar.open { display: block; }
  .studio-grid, .converter-grid { grid-template-columns: minmax(0, 1fr); }
  .portal-main { padding: 12px; }
}
"##;

const DOCS_JS: &str = r##"(function () {
  "use strict";

  const LANG_STORAGE_KEY = "cs16_docs_lang";
  const VALID_LANGS = ["py", "rust", "cpp", "go", "amxx"];

  function getSavedLang() {
    try {
      const v = localStorage.getItem(LANG_STORAGE_KEY);
      if (v && VALID_LANGS.includes(v)) return v;
    } catch (_) {}
    return "py";
  }

  function applyGlobalLang(lang, persist) {
    if (!VALID_LANGS.includes(lang)) return;
    if (persist) {
      try { localStorage.setItem(LANG_STORAGE_KEY, lang); } catch (_) {}
    }
    document.querySelectorAll("[data-set-lang]").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.setLang === lang);
    });
    document.querySelectorAll(".code-tabs").forEach((group) => {
      const targetBtn = group.querySelector(`.code-tab-btn[data-lang="${lang}"]`);
      if (!targetBtn) return;
      group.querySelectorAll(".code-tab-btn").forEach((b) => {
        b.classList.toggle("active", b.dataset.lang === lang);
      });
      group.querySelectorAll(".code-panel").forEach((p) => {
        p.classList.toggle("active", p.dataset.lang === lang);
      });
    });
  }

  function initTabsAndCopy() {
    applyGlobalLang(getSavedLang(), false);

    document.addEventListener("click", (ev) => {
      const langPill = ev.target.closest("[data-set-lang]");
      if (langPill) {
        applyGlobalLang(langPill.dataset.setLang, true);
        return;
      }
      const tabBtn = ev.target.closest(".code-tab-btn");
      if (tabBtn && tabBtn.dataset.lang) {
        applyGlobalLang(tabBtn.dataset.lang, true);
        return;
      }
      const copyBtn = ev.target.closest(".copy-btn");
      if (copyBtn) {
        const container = copyBtn.closest(".code-panel, .code-box");
        const codeEl = container ? container.querySelector("pre code") : null;
        if (!codeEl) return;
        const text = codeEl.textContent || "";
        const flash = () => {
          const prev = copyBtn.textContent;
          copyBtn.textContent = "Copied!";
          setTimeout(() => { copyBtn.textContent = prev || "Copy"; }, 1400);
        };
        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(text).then(flash).catch(() => fallbackCopy(text, flash));
        } else {
          fallbackCopy(text, flash);
        }
      }
    });
  }

  function fallbackCopy(text, onDone) {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand("copy"); } catch (_) {}
    document.body.removeChild(ta);
    onDone();
  }

  function initDrawerAndToc() {
    const drawerBtn = document.getElementById("btn-toggle-drawer");
    const sidebar = document.getElementById("portal-sidebar");
    if (drawerBtn && sidebar) {
      drawerBtn.addEventListener("click", () => sidebar.classList.toggle("open"));
    }

    const tocLinks = Array.from(document.querySelectorAll("#toc-nav .toc-link"));
    if (!tocLinks.length) return;
    const targets = tocLinks
      .map((a) => document.getElementById((a.getAttribute("href") || "").replace(/^#/, "")))
      .filter(Boolean);
    if (!targets.length) return;

    const updateToc = () => {
      let currentId = targets[0].id;
      const offset = 110;
      for (const el of targets) {
        if (el.getBoundingClientRect().top <= offset) currentId = el.id;
      }
      tocLinks.forEach((a) => {
        a.classList.toggle("active", a.getAttribute("href") === `#${currentId}`);
      });
    };
    window.addEventListener("scroll", updateToc, { passive: true });
    updateToc();
  }

  /* Ctrl+K / Slash Search Modal */
  function initSearchModal() {
    const modal = document.getElementById("search-modal");
    const input = document.getElementById("search-input");
    const resultsEl = document.getElementById("search-results");
    const metaEl = document.getElementById("search-meta");
    const openBtn = document.getElementById("btn-open-search");
    const closeBtn = document.getElementById("btn-close-search");
    if (!modal || !input || !resultsEl) return;

    let indexData = null;
    let loading = false;
    let activeCat = "all";
    let selectedIdx = 0;
    let renderedItems = [];

    async function ensureIndex() {
      if (indexData || loading) return;
      loading = true;
      metaEl.textContent = "Loading search index...";
      try {
        const res = await fetch("./search-index.json");
        indexData = await res.json();
        runQuery();
      } catch (err) {
        metaEl.textContent = "Could not load search-index.json (" + String(err) + ")";
      } finally {
        loading = false;
      }
    }

    function openModal() {
      modal.classList.remove("hidden");
      input.focus();
      input.select();
      ensureIndex();
    }

    function closeModal() {
      modal.classList.add("hidden");
    }

    function scoreItem(item, q) {
      if (!q) return 1;
      const t = (item.t || "").toLowerCase();
      const s = (item.s || "").toLowerCase();
      const k = (item.k || "").toLowerCase();
      if (t === q) return 100;
      if (t.startsWith(q)) return 60;
      if (t.includes(q)) return 35;
      if (k.includes(q)) return 20;
      if (s.includes(q)) return 10;
      return 0;
    }

    function renderSelection() {
      const nodes = resultsEl.querySelectorAll(".search-item");
      nodes.forEach((n, idx) => n.classList.toggle("selected", idx === selectedIdx));
      const active = nodes[selectedIdx];
      if (active) active.scrollIntoView({ block: "nearest" });
    }

    function runQuery() {
      if (!indexData) return;
      const q = (input.value || "").trim().toLowerCase();
      const scored = [];
      for (const item of indexData) {
        if (activeCat !== "all" && item.c !== activeCat) continue;
        const sc = scoreItem(item, q);
        if (sc > 0) scored.push({ item, sc });
      }
      scored.sort((a, b) => b.sc - a.sc || a.item.t.localeCompare(b.item.t));
      const total = scored.length;
      renderedItems = scored.slice(0, 60).map((x) => x.item);
      selectedIdx = 0;

      metaEl.textContent = q
        ? `${total} matches (showing top ${renderedItems.length}) · Use ↑/↓ and Enter`
        : `${total} indexed items across guides, hooks, SDK API, CVars & AMXX natives`;

      resultsEl.textContent = "";
      for (let i = 0; i < renderedItems.length; i++) {
        const it = renderedItems[i];
        const a = document.createElement("a");
        a.className = "search-item" + (i === 0 ? " selected" : "");
        a.href = it.u;
        const head = document.createElement("div");
        head.className = "search-item-head";
        const title = document.createElement("span");
        title.className = "search-item-title";
        title.textContent = it.t;
        const badge = document.createElement("span");
        badge.className = "pill pill-amber";
        badge.textContent = it.c.toUpperCase();
        head.appendChild(title);
        head.appendChild(badge);
        const sum = document.createElement("div");
        sum.className = "search-item-summary";
        sum.textContent = it.s;
        a.appendChild(head);
        a.appendChild(sum);
        a.addEventListener("click", closeModal);
        resultsEl.appendChild(a);
      }
    }

    if (openBtn) openBtn.addEventListener("click", openModal);
    if (closeBtn) closeBtn.addEventListener("click", closeModal);
    modal.addEventListener("click", (ev) => {
      if (ev.target === modal) closeModal();
    });

    document.querySelectorAll("[data-search-cat]").forEach((btn) => {
      btn.addEventListener("click", () => {
        activeCat = btn.dataset.searchCat || "all";
        document.querySelectorAll("[data-search-cat]").forEach((b) => {
          b.classList.toggle("active", b === btn);
        });
        runQuery();
      });
    });

    input.addEventListener("input", runQuery);
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "ArrowDown") {
        ev.preventDefault();
        if (renderedItems.length) {
          selectedIdx = (selectedIdx + 1) % renderedItems.length;
          renderSelection();
        }
      } else if (ev.key === "ArrowUp") {
        ev.preventDefault();
        if (renderedItems.length) {
          selectedIdx = (selectedIdx - 1 + renderedItems.length) % renderedItems.length;
          renderSelection();
        }
      } else if (ev.key === "Enter") {
        const chosen = renderedItems[selectedIdx];
        if (chosen) {
          ev.preventDefault();
          closeModal();
          window.location.href = chosen.u;
        }
      } else if (ev.key === "Escape") {
        closeModal();
      }
    });

    document.addEventListener("keydown", (ev) => {
      const tag = (ev.target && ev.target.tagName) || "";
      const typing = tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
      if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "k") {
        ev.preventDefault();
        if (modal.classList.contains("hidden")) openModal();
        else closeModal();
      } else if (!typing && ev.key === "/") {
        ev.preventDefault();
        openModal();
      } else if (ev.key === "Escape" && !modal.classList.contains("hidden")) {
        closeModal();
      }
    });

    if (window.location.hash === "#search") openModal();
  }

  /* Page-specific filters */
  function initPageFilters() {
    // Hooks page filter
    const hookSearch = document.getElementById("hook-search");
    const hookCount = document.getElementById("hook-count");
    if (hookSearch) {
      let hookCat = "all";
      const cards = Array.from(document.querySelectorAll(".hook-card"));
      const apply = () => {
        const q = hookSearch.value.trim().toLowerCase();
        let shown = 0;
        for (const c of cards) {
          const catOk =
            hookCat === "all" ||
            (hookCat === "mutable" ? c.dataset.hookMut === "1" : c.dataset.hookCat === hookCat);
          const qOk = !q || (c.dataset.filterText || "").includes(q);
          const vis = catOk && qOk;
          c.style.display = vis ? "" : "none";
          if (vis) shown++;
        }
        if (hookCount) hookCount.textContent = `Showing ${shown} of ${cards.length} hooks`;
      };
      hookSearch.addEventListener("input", apply);
      document.querySelectorAll("[data-hook-filter]").forEach((btn) => {
        btn.addEventListener("click", () => {
          hookCat = btn.dataset.hookFilter || "all";
          document.querySelectorAll("[data-hook-filter]").forEach((b) => b.classList.toggle("active", b === btn));
          apply();
        });
      });
    }

    // API page filter
    const apiSearch = document.getElementById("api-search");
    const apiCount = document.getElementById("api-count");
    if (apiSearch) {
      const rows = Array.from(document.querySelectorAll("[data-api-row]"));
      apiSearch.addEventListener("input", () => {
        const q = apiSearch.value.trim().toLowerCase();
        let shown = 0;
        for (const r of rows) {
          const vis = !q || (r.dataset.filterText || "").includes(q);
          r.style.display = vis ? "" : "none";
          if (vis) shown++;
        }
        if (apiCount) apiCount.textContent = `Showing ${shown} of ${rows.length} methods`;
      });
    }

    // AMXX natives filter
    const amxxSearch = document.getElementById("amxx-search");
    const amxxCount = document.getElementById("amxx-count");
    if (amxxSearch) {
      let activeMod = "all";
      const rows = Array.from(document.querySelectorAll("[data-amxx-row]"));
      const apply = () => {
        const q = amxxSearch.value.trim().toLowerCase();
        let shown = 0;
        for (const r of rows) {
          const modOk = activeMod === "all" || r.dataset.amxxMod === activeMod;
          const qOk = !q || (r.dataset.filterText || "").includes(q);
          const vis = modOk && qOk;
          r.style.display = vis ? "" : "none";
          if (vis) shown++;
        }
        if (amxxCount) amxxCount.textContent = `Showing ${shown} of ${rows.length} natives`;
      };
      amxxSearch.addEventListener("input", apply);
      document.querySelectorAll("[data-amxx-mod]").forEach((btn) => {
        btn.addEventListener("click", () => {
          activeMod = btn.dataset.amxxMod || "all";
          document.querySelectorAll("[data-amxx-mod]").forEach((b) => b.classList.toggle("active", b === btn));
          apply();
        });
      });
    }

    // 736 CVars & Commands filter
    const cvarSearch = document.getElementById("cvar-search");
    const cvarKind = document.getElementById("cvar-kind-filter");
    const cvarCat = document.getElementById("cvar-cat-filter");
    const cvarFlag = document.getElementById("cvar-flag-filter");
    const cvarCount = document.getElementById("cvar-count");
    if (cvarSearch) {
      const rows = Array.from(document.querySelectorAll("[data-cvar-row]"));
      const apply = () => {
        const q = cvarSearch.value.trim().toLowerCase();
        const k = cvarKind ? cvarKind.value : "all";
        const cat = cvarCat ? cvarCat.value : "all";
        const flag = cvarFlag ? cvarFlag.value : "all";
        let shown = 0;
        for (const r of rows) {
          const ok =
            (k === "all" || r.dataset.kind === k) &&
            (cat === "all" || r.dataset.cat === cat) &&
            (flag === "all" || (r.dataset.flags || "").split(" ").includes(flag)) &&
            (!q || (r.dataset.filterText || "").includes(q));
          r.style.display = ok ? "" : "none";
          if (ok) shown++;
        }
        if (cvarCount) cvarCount.textContent = `Showing ${shown} of ${rows.length} entries`;
      };
      cvarSearch.addEventListener("input", apply);
      if (cvarKind) cvarKind.addEventListener("change", apply);
      if (cvarCat) cvarCat.addEventListener("change", apply);
      if (cvarFlag) cvarFlag.addEventListener("change", apply);
    }
  }

  document.addEventListener("DOMContentLoaded", () => {
    initTabsAndCopy();
    initDrawerAndToc();
    initSearchModal();
    initPageFilters();
  });
})();
"##;

const VFX_DESIGNER_JS: &str = r##"(function () {
  "use strict";

  function hexToRgb(hex) {
    const clean = (hex || "#ffb000").replace("#", "");
    const num = parseInt(clean.length === 3 ? clean.split("").map((c) => c + c).join("") : clean, 16);
    return [(num >> 16) & 255, (num >> 8) & 255, num & 255];
  }

  function openInPlayground(id, name, code) {
    try {
      localStorage.setItem("cs16_playground_import", JSON.stringify({ id, name, code }));
    } catch (_) {}
    window.location.href = "../playground/?mod=" + encodeURIComponent(id);
  }

  function renderGoldSrcSegments(container, raw) {
    let cls = "gs-w";
    const parts = raw.split(/(\\y|\\w|\\r|\\d)/g);
    for (const part of parts) {
      if (part === "\\y") { cls = "gs-y"; continue; }
      if (part === "\\w") { cls = "gs-w"; continue; }
      if (part === "\\r") { cls = "gs-r"; continue; }
      if (part === "\\d") { cls = "gs-d"; continue; }
      if (!part) continue;
      const span = document.createElement("span");
      span.className = cls;
      span.textContent = part;
      container.appendChild(span);
    }
  }

  function makeGoldSrcLine(raw, clickable, onClick) {
    const row = document.createElement("div");
    row.className = "gs-line" + (clickable ? " clickable" : "");
    const sides = raw.split("\\R");
    const left = document.createElement("span");
    renderGoldSrcSegments(left, sides[0] || "");
    row.appendChild(left);
    if (sides.length > 1) {
      const right = document.createElement("span");
      renderGoldSrcSegments(right, sides.slice(1).join(" "));
      row.appendChild(right);
    }
    if (clickable && onClick) row.addEventListener("click", onClick);
    return row;
  }

  /* 1. GoldSrc Paginated Menu Studio */
  function initMenuStudio() {
    const titleEl = document.getElementById("menu-title");
    const perPageEl = document.getElementById("menu-per-page");
    const timeEl = document.getElementById("menu-time");
    const numColEl = document.getElementById("menu-num-col");
    const allowExitEl = document.getElementById("menu-allow-exit");
    const itemsEl = document.getElementById("menu-items-text");
    const previewEl = document.getElementById("goldsrc-menu-preview");
    const badgeEl = document.getElementById("menu-page-badge");
    const logEl = document.getElementById("menu-click-log");
    if (!titleEl || !previewEl) return;

    let currentPage = 0;

    function parseItems() {
      return (itemsEl.value || "").split(/\r?\n/).filter((l) => l.trim().length > 0).map((line) => {
        const t = line.trim();
        if (t === "---") return { kind: "blank" };
        if (t.startsWith("#")) return { kind: "text", label: t.slice(1).trim() };
        const [label, info] = t.split("|").map((s) => s.trim());
        return { kind: "item", label: label || "Item", info: info || "item" };
      });
    }

    function getEnabledKeys() {
      const set = new Set();
      document.querySelectorAll("#menu-keymask input[data-key]").forEach((cb) => {
        if (cb.checked) set.add(Number(cb.dataset.key));
      });
      return set;
    }

    function update() {
      const title = titleEl.value || "\\yMenu";
      const perPage = Math.max(1, Math.min(7, Number(perPageEl.value) || 7));
      const timeSec = Number(timeEl.value);
      const numCol = numColEl.value || "\\r";
      const allowExit = Boolean(allowExitEl.checked);
      const allEntries = parseItems();
      const selectable = allEntries.filter((e) => e.kind === "item");
      const totalPages = Math.max(1, Math.ceil(selectable.length / perPage));
      if (currentPage >= totalPages) currentPage = totalPages - 1;
      if (currentPage < 0) currentPage = 0;

      const enabledKeys = getEnabledKeys();
      const start = currentPage * perPage;
      const pageItems = selectable.slice(start, start + perPage);

      let mask = 0;
      previewEl.textContent = "";
      previewEl.appendChild(makeGoldSrcLine(`${title} \\d${currentPage + 1}/${totalPages}`, false));
      previewEl.appendChild(makeGoldSrcLine(" ", false));

      pageItems.forEach((it, idx) => {
        const keyNum = idx + 1;
        const enabled = enabledKeys.has(keyNum);
        if (enabled) mask |= 1 << (keyNum - 1);
        const lineText = enabled
          ? `${numCol}${keyNum}. \\w${it.label}`
          : `\\d${keyNum}. ${it.label.replace(/\\[ywrd]/g, "")}`;
        previewEl.appendChild(
          makeGoldSrcLine(lineText, enabled, () => {
            logEl.textContent = `menuselect ${keyNum} -> callback(player, info="${it.info}")`;
          })
        );
      });

      const nonSel = allEntries.filter((e) => e.kind === "text");
      if (nonSel.length) {
        previewEl.appendChild(makeGoldSrcLine(" ", false));
        nonSel.slice(0, 2).forEach((t) => previewEl.appendChild(makeGoldSrcLine(t.label, false)));
      }

      previewEl.appendChild(makeGoldSrcLine(" ", false));
      if (totalPages > 1) {
        const hasBack = currentPage > 0;
        const hasNext = currentPage + 1 < totalPages;
        if (hasBack) mask |= 1 << 7;
        if (hasNext) mask |= 1 << 8;
        previewEl.appendChild(makeGoldSrcLine(hasBack ? `${numCol}8. \\wBack` : "\\d8. Back", hasBack, () => { currentPage--; update(); }));
        previewEl.appendChild(makeGoldSrcLine(hasNext ? `${numCol}9. \\wMore` : "\\d9. More", hasNext, () => { currentPage++; update(); }));
      }
      if (allowExit) {
        mask |= 1 << 9;
        previewEl.appendChild(makeGoldSrcLine(`${numCol}0. \\wExit`, true, () => {
          logEl.textContent = "menuselect 10 -> MENU_EXIT (-3)";
        }));
      }

      badgeEl.textContent = `Page ${currentPage + 1} / ${totalPages} · Mask 0x${mask.toString(16).padStart(4, "0")}`;

      const esc = (s) => s.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
      const pyLines = [
        "from cs16 import mod, Menu, HookAction",
        "",
        "def on_menu_pick(player, info):",
        '    player.chat(f"Selected: {info}")',
        '    if info.startswith("weapon_"):',
        "        player.give_item(info)",
        "",
        '@mod.chat_command("/menu")',
        "def cmd_menu(player, args):",
        `    menu = Menu("${esc(title)}", callback=on_menu_pick, per_page=${perPage}, allow_exit=${allowExit ? "True" : "False"}, number_color="${esc(numCol)}")`,
      ];
      allEntries.forEach((e, idx) => {
        if (e.kind === "blank") pyLines.push("    menu.add_blank()");
        else if (e.kind === "text") pyLines.push(`    menu.add_text("${esc(e.label)}")`);
        else {
          const en = enabledKeys.has((idx % perPage) + 1);
          pyLines.push(`    menu.add_item("${esc(e.label)}", "${esc(e.info)}", enabled=${en ? "True" : "False"})`);
        }
      });
      pyLines.push(`    menu.show(player, page=0, time=${timeSec})`);
      pyLines.push("    return HookAction.SUPERCEDE");

      const rustLines = [
        `PaginatedMenu::new("${esc(title)}", 1)`,
        `    .per_page(${perPage})`,
        `    .allow_exit(${allowExit})`,
        `    .number_color("${esc(numCol)}")`,
      ];
      allEntries.forEach((e) => {
        if (e.kind === "blank") rustLines.push("    .blank()");
        else if (e.kind === "text") rustLines.push(`    .text("${esc(e.label)}")`);
        else rustLines.push(`    .item("${esc(e.label)}", "${esc(e.info)}")`);
      });
      rustLines.push(`    .display_to(host, slot, 0, ${timeSec});`);

      const cppLines = [
        `auto menu = cs16::Menu::create("${esc(title)}", 1);`,
        `menu.set_per_page(${perPage}).set_allow_exit(${allowExit});`,
      ];
      allEntries.forEach((e) => {
        if (e.kind === "blank") cppLines.push("menu.add_blank();");
        else if (e.kind === "text") cppLines.push(`menu.add_text("${esc(e.label)}");`);
        else cppLines.push(`menu.add_item("${esc(e.label)}", "${esc(e.info)}");`);
      });
      cppLines.push(`menu.display(player, 0, ${timeSec});`);

      const goLines = [
        `m := cs16mod.NewMenu("${esc(title)}", onPick).SetPerPage(${perPage}).SetAllowExit(${allowExit})`,
      ];
      allEntries.forEach((e) => {
        if (e.kind === "blank") goLines.push("m.AddBlank()");
        else if (e.kind === "text") goLines.push(`m.AddText("${esc(e.label)}")`);
        else goLines.push(`m.AddItem("${esc(e.label)}", "${esc(e.info)}")`);
      });
      goLines.push(`m.Display(player, 0, ${timeSec})`);

      document.getElementById("menu-code-py").textContent = pyLines.join("\n");
      document.getElementById("menu-code-rust").textContent = rustLines.join("\n");
      document.getElementById("menu-code-cpp").textContent = cppLines.join("\n");
      document.getElementById("menu-code-go").textContent = goLines.join("\n");
    }

    [titleEl, perPageEl, timeEl, numColEl, allowExitEl, itemsEl].forEach((el) => {
      el.addEventListener("input", update);
      el.addEventListener("change", update);
    });
    document.querySelectorAll("#menu-keymask input[data-key]").forEach((cb) => cb.addEventListener("change", update));
    document.getElementById("menu-prev-page").addEventListener("click", () => { currentPage--; update(); });
    document.getElementById("menu-next-page").addEventListener("click", () => { currentPage++; update(); });
    document.getElementById("btn-menu-playground").addEventListener("click", () => {
      const code = document.getElementById("menu-code-py").textContent;
      openInPlayground("menu_studio", "Custom GoldSrc Menu", code);
    });
    update();
  }

  /* 2. HUD & DHUD Designer */
  function initHudStudio() {
    const modeEl = document.getElementById("hud-mode");
    const chanEl = document.getElementById("hud-channel");
    const effectEl = document.getElementById("hud-effect");
    const textEl = document.getElementById("hud-text");
    const xEl = document.getElementById("hud-x");
    const yEl = document.getElementById("hud-y");
    const colEl = document.getElementById("hud-color");
    const alphaEl = document.getElementById("hud-alpha");
    const holdEl = document.getElementById("hud-hold");
    const fadeInEl = document.getElementById("hud-fadein");
    const fadeOutEl = document.getElementById("hud-fadeout");
    const stageEl = document.getElementById("hud-stage");
    const liveEl = document.getElementById("hud-live-text");
    const badgeEl = document.getElementById("hud-pos-badge");
    if (!modeEl || !stageEl) return;

    function update() {
      const mode = modeEl.value;
      const ch = Number(chanEl.value) || 1;
      const eff = Number(effectEl.value) || 0;
      const rawText = textEl.value || "CS 1.6 HUD";
      const x = Number(xEl.value);
      const y = Number(yEl.value);
      const [r, g, b] = hexToRgb(colEl.value);
      const a = Number(alphaEl.value);
      const hold = Number(holdEl.value);
      const fi = Number(fadeInEl.value);
      const fo = Number(fadeOutEl.value);

      document.getElementById("hud-x-val").textContent = x < 0 ? "-1.00 (Center)" : x.toFixed(2);
      document.getElementById("hud-y-val").textContent = y.toFixed(2);
      document.getElementById("hud-alpha-val").textContent = String(a);
      document.getElementById("hud-hold-val").textContent = hold.toFixed(1) + "s";
      document.getElementById("hud-fadein-val").textContent = fi.toFixed(2) + "s";
      document.getElementById("hud-fadeout-val").textContent = fo.toFixed(2) + "s";
      badgeEl.textContent = `x=${x < 0 ? "-1.00" : x.toFixed(2)}, y=${y.toFixed(2)} · rgba(${r},${g},${b},${a})`;

      liveEl.className = "hud-sample-text " + mode;
      liveEl.textContent = rawText;
      liveEl.style.color = `rgba(${r}, ${g}, ${b}, ${(a / 255).toFixed(2)})`;
      liveEl.style.top = `${Math.round(y * 100)}%`;
      if (x < 0) {
        liveEl.style.left = "50%";
        liveEl.style.transform = "translateX(-50%)";
        liveEl.style.textAlign = "center";
      } else {
        liveEl.style.left = `${Math.round(x * 100)}%`;
        liveEl.style.transform = "none";
        liveEl.style.textAlign = "left";
      }

      const esc = rawText.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n");
      const xStr = x < 0 ? "-1.0" : x.toFixed(2);
      const yStr = y.toFixed(2);

      if (mode === "dhud") {
        document.getElementById("hud-code-py").textContent =
          `player.dhud(\n    "${esc}",\n    x=${xStr}, y=${yStr},\n    rgb=(${r}, ${g}, ${b}),\n    effect=${eff}, fadein=${fi.toFixed(2)}, hold=${hold.toFixed(1)}, fadeout=${fo.toFixed(2)},\n)`;
        document.getElementById("hud-code-rust").textContent =
          `host.dhud_message(&DhudMessageSpec {\n    slot: Some(slot), x: ${xStr}, y: ${yStr},\n    rgb: [${r}, ${g}, ${b}], effect: ${eff},\n    fadein: ${fi.toFixed(2)}, hold: ${hold.toFixed(1)}, fadeout: ${fo.toFixed(2)}, fxtime: 0.0,\n    text: "${esc}".into(),\n});`;
        document.getElementById("hud-code-cpp").textContent =
          `player.dhud_message("${esc}", ${xStr}f, ${yStr}f, cs16::Color{${r}, ${g}, ${b}, 255}, ${eff}, ${fi.toFixed(2)}f, ${hold.toFixed(1)}f, ${fo.toFixed(2)}f);`;
        document.getElementById("hud-code-go").textContent =
          `player.DhudMessage("${esc}", ${xStr}, ${yStr}, cs16mod.Color{R: ${r}, G: ${g}, B: ${b}, A: 255}, ${eff}, ${fi.toFixed(2)}, ${hold.toFixed(1)}, ${fo.toFixed(2)})`;
      } else {
        document.getElementById("hud-code-py").textContent =
          `player.hud(\n    "${esc}",\n    x=${xStr}, y=${yStr}, channel=${ch},\n    rgba=(${r}, ${g}, ${b}, ${a}),\n    effect=${eff}, fadein=${fi.toFixed(2)}, hold=${hold.toFixed(1)}, fadeout=${fo.toFixed(2)},\n)`;
        document.getElementById("hud-code-rust").textContent =
          `host.hud_message(&HudMessageSpec {\n    slot: Some(slot), channel: ${ch}, x: ${xStr}, y: ${yStr},\n    rgba1: [${r}, ${g}, ${b}, ${a}], rgba2: [255, 255, 255, 255], effect: ${eff},\n    fadein: ${fi.toFixed(2)}, hold: ${hold.toFixed(1)}, fadeout: ${fo.toFixed(2)}, fxtime: 0.0,\n    text: "${esc}".into(),\n});`;
        document.getElementById("hud-code-cpp").textContent =
          `player.hud_message("${esc}", ${xStr}f, ${yStr}f, cs16::Color{${r}, ${g}, ${b}, ${a}}, ${ch}, ${eff}, ${fi.toFixed(2)}f, ${hold.toFixed(1)}f, ${fo.toFixed(2)}f);`;
        document.getElementById("hud-code-go").textContent =
          `player.HudMessage("${esc}", ${xStr}, ${yStr}, cs16mod.Color{R: ${r}, G: ${g}, B: ${b}, A: ${a}}, ${ch}, ${eff}, ${fi.toFixed(2)}, ${hold.toFixed(1)}, ${fo.toFixed(2)})`;
      }
    }

    stageEl.addEventListener("click", (ev) => {
      const rect = stageEl.getBoundingClientRect();
      const nx = Math.max(0, Math.min(0.95, (ev.clientX - rect.left) / rect.width));
      const ny = Math.max(0.02, Math.min(0.88, (ev.clientY - rect.top) / rect.height));
      xEl.value = nx.toFixed(2);
      yEl.value = ny.toFixed(2);
      update();
    });

    document.getElementById("btn-hud-center").addEventListener("click", () => {
      xEl.value = "-1";
      update();
    });

    document.getElementById("btn-hud-replay").addEventListener("click", () => {
      liveEl.style.opacity = "0";
      setTimeout(() => { liveEl.style.opacity = "1"; }, 180);
    });

    document.getElementById("btn-hud-playground").addEventListener("click", () => {
      const snippet = document.getElementById("hud-code-py").textContent;
      const plugin = `from cs16 import mod, HookAction\n\n@mod.hook("player_spawn", phase="post")\ndef on_spawn(ev):\n    player = ev.player\n    ${snippet.replace(/\n/g, "\n    ")}\n\n@mod.chat_command("/hud")\ndef cmd_hud(player, args):\n    ${snippet.replace(/\n/g, "\n    ")}\n    return HookAction.SUPERCEDE\n`;
      openInPlayground("hud_studio", "Custom HUD Banner", plugin);
    });

    [modeEl, chanEl, effectEl, textEl, xEl, yEl, colEl, alphaEl, holdEl, fadeInEl, fadeOutEl].forEach((el) => {
      el.addEventListener("input", update);
      el.addEventListener("change", update);
    });
    update();
  }

  /* 3. Protocol 48 VFX & Screen FX Canvas Studio */
  function initVfxStudio() {
    const kindEl = document.getElementById("vfx-kind");
    const colEl = document.getElementById("vfx-color");
    const alphaEl = document.getElementById("vfx-alpha");
    const radEl = document.getElementById("vfx-radius");
    const widthEl = document.getElementById("vfx-width");
    const lifeEl = document.getElementById("vfx-life");
    const noiseEl = document.getElementById("vfx-noise");
    const wireEl = document.getElementById("vfx-wire-badge");
    const canvas = document.getElementById("vfx-canvas");
    if (!kindEl || !canvas) return;

    const ctx = canvas.getContext("2d");
    let phase = 0;

    function updateCode() {
      const kind = kindEl.value;
      const [r, g, b] = hexToRgb(colEl.value);
      const a = Number(alphaEl.value);
      const radius = Number(radEl.value);
      const width = Number(widthEl.value);
      const life = Number(lifeEl.value);
      const noise = Number(noiseEl.value);

      document.getElementById("vfx-alpha-val").textContent = String(a);
      document.getElementById("vfx-radius-val").textContent = String(radius);
      document.getElementById("vfx-width-val").textContent = String(width);
      document.getElementById("vfx-life-val").textContent = String(life);
      document.getElementById("vfx-noise-val").textContent = String(noise);

      let py = "", rust = "", cpp = "", go = "", wire = "TE_BEAMCYLINDER";
      if (kind === "shockwave_ring") {
        wire = "TE_BEAMCYLINDER";
        py = `vfx.shockwave_ring(player.origin, radius=${radius}.0, rgba=(${r}, ${g}, ${b}, ${a}), life=${life}, width=${width})`;
        rust = `vfx::shockwave_ring(host, origin, ${radius}.0, [${r}, ${g}, ${b}, ${a}], ${life}, ${width}, None);`;
        cpp = `cs16::vfx::shockwave_ring(origin, ${radius}.0f, cs16::Color{${r}, ${g}, ${b}, ${a}}, ${life}, ${width});`;
        go = `cs16mod.VFX.ShockwaveRing(origin, ${radius}.0, cs16mod.Color{R: ${r}, G: ${g}, B: ${b}, A: ${a}}, ${life}, ${width}, -1)`;
      } else if (kind === "laser_beam") {
        wire = "TE_BEAMPOINTS";
        py = `vfx.laser_beam(player.origin, player.aim_origin, rgba=(${r}, ${g}, ${b}, ${a}), life=${life}, width=${width}, noise=${noise})`;
        rust = `vfx::laser_beam(host, start, end, [${r}, ${g}, ${b}, ${a}], ${life}, ${width}, ${noise}, None);`;
        cpp = `cs16::vfx::laser_beam(start, end, cs16::Color{${r}, ${g}, ${b}, ${a}}, ${life}, ${width}, ${noise});`;
        go = `cs16mod.VFX.LaserBeam(start, end, cs16mod.Color{R: ${r}, G: ${g}, B: ${b}, A: ${a}}, ${life}, ${width}, ${noise}, -1)`;
      } else if (kind === "dynamic_light") {
        wire = "TE_DLIGHT";
        const radByte = Math.min(255, Math.max(8, Math.round(radius / 10)));
        py = `vfx.aura_light(player.origin, radius=${radByte}, rgb=(${r}, ${g}, ${b}), life=${life}, decay=${Math.max(5, noise)})`;
        rust = `vfx::aura_light(host, origin, ${radByte}, [${r}, ${g}, ${b}], ${life}, ${Math.max(5, noise)}, None);`;
        cpp = `cs16::vfx::aura_light(origin, ${radByte}, cs16::Color{${r}, ${g}, ${b}, 255}, ${life}, ${Math.max(5, noise)});`;
        go = `cs16mod.VFX.AuraLight(origin, ${radByte}, cs16mod.Color{R: ${r}, G: ${g}, B: ${b}, A: 255}, ${life}, ${Math.max(5, noise)}, -1)`;
      } else if (kind === "particle_burst") {
        wire = "TE_IMPLOSION";
        const radByte = Math.min(255, radius);
        py = `vfx.implosion(player.origin, radius=${radByte}, count=${width}, life=${life})`;
        rust = `vfx::implosion(host, origin, ${radByte}, ${width}, ${life}, None);`;
        cpp = `cs16::vfx::implosion(origin, ${radByte}, ${width}, ${life});`;
        go = `cs16mod.VFX.Implosion(origin, ${radByte}, ${width}, ${life}, -1)`;
      } else if (kind === "screen_fade") {
        wire = "UserMsg: ScreenFade";
        const dur = (life / 10).toFixed(2);
        py = `player.screen_fade(duration=${dur}, hold=0.15, flags=0, rgba=(${r}, ${g}, ${b}, ${Math.min(180, a)}))`;
        rust = `p.screen_fade(host, ${dur}, 0.15, 0, [${r}, ${g}, ${b}, ${Math.min(180, a)}]);`;
        cpp = `player.screen_fade(${dur}f, 0.15f, 0, cs16::Color{${r}, ${g}, ${b}, ${Math.min(180, a)}});`;
        go = `player.ScreenFade(${dur}, 0.15, 0, cs16mod.Color{R: ${r}, G: ${g}, B: ${b}, A: ${Math.min(180, a)}})`;
      } else {
        wire = "UserMsg: ScreenShake";
        const amp = (radius / 25).toFixed(1);
        const dur = (life / 10).toFixed(2);
        const freq = Math.max(1.0, noise / 4).toFixed(1);
        py = `player.screen_shake(amplitude=${amp}, duration=${dur}, frequency=${freq})`;
        rust = `p.screen_shake(host, ${amp}, ${dur}, ${freq});`;
        cpp = `player.screen_shake(${amp}f, ${dur}f, ${freq}f);`;
        go = `player.ScreenShake(${amp}, ${dur}, ${freq})`;
      }

      wireEl.textContent = wire;
      document.getElementById("vfx-code-py").textContent = py;
      document.getElementById("vfx-code-rust").textContent = rust;
      document.getElementById("vfx-code-cpp").textContent = cpp;
      document.getElementById("vfx-code-go").textContent = go;
    }

    function drawFrame() {
      phase = (phase + 0.018) % 1;
      const w = canvas.width;
      const h = canvas.height;
      const kind = kindEl.value;
      const [r, g, b] = hexToRgb(colEl.value);
      const a = Number(alphaEl.value) / 255;
      const radius = Number(radEl.value);
      const width = Number(widthEl.value);
      const noise = Number(noiseEl.value);

      ctx.save();
      if (kind === "screen_shake") {
        const amp = Math.min(12, radius / 30) * (1 - phase);
        ctx.translate((Math.random() - 0.5) * amp * 2, (Math.random() - 0.5) * amp * 2);
      }
      ctx.fillStyle = "#121610";
      ctx.fillRect(-20, -20, w + 40, h + 40);

      // Perspective floor grid
      ctx.strokeStyle = "rgba(136, 145, 128, 0.16)";
      ctx.lineWidth = 1;
      for (let gx = 0; gx <= w; gx += 40) {
        ctx.beginPath(); ctx.moveTo(gx, 0); ctx.lineTo(gx, h); ctx.stroke();
      }
      for (let gy = 0; gy <= h; gy += 36) {
        ctx.beginPath(); ctx.moveTo(0, gy); ctx.lineTo(w, gy); ctx.stroke();
      }

      const cx = w / 2;
      const cy = h / 2 + 12;

      if (kind === "shockwave_ring") {
        const rx = 18 + phase * Math.min(220, radius * 0.75);
        const ry = rx * 0.42;
        ctx.strokeStyle = `rgba(${r}, ${g}, ${b}, ${(a * (1 - phase)).toFixed(3)})`;
        ctx.lineWidth = Math.max(2, width / 2.2);
        ctx.beginPath();
        ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
        ctx.stroke();
      } else if (kind === "laser_beam") {
        ctx.strokeStyle = `rgba(${r}, ${g}, ${b}, ${a.toFixed(2)})`;
        ctx.lineWidth = Math.max(2, width / 2.4);
        ctx.beginPath();
        const segs = 16;
        for (let i = 0; i <= segs; i++) {
          const t = i / segs;
          const x = 48 + t * (w - 96);
          const jitter = i === 0 || i === segs ? 0 : Math.sin(t * 25 + phase * 30) * noise * 0.75;
          const y = cy + jitter;
          if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
        }
        ctx.stroke();
      } else if (kind === "dynamic_light") {
        const rad = Math.min(140, radius * 0.5) * (0.85 + 0.15 * Math.sin(phase * Math.PI * 2));
        const grad = ctx.createRadialGradient(cx, cy, 4, cx, cy, rad);
        grad.addColorStop(0, `rgba(${r}, ${g}, ${b}, ${a.toFixed(2)})`);
        grad.addColorStop(1, `rgba(${r}, ${g}, ${b}, 0)`);
        ctx.fillStyle = grad;
        ctx.beginPath(); ctx.arc(cx, cy, rad, 0, Math.PI * 2); ctx.fill();
      } else if (kind === "particle_burst") {
        const count = Math.max(8, width);
        for (let i = 0; i < count; i++) {
          const ang = (i / count) * Math.PI * 2 + phase * 0.6;
          const dist = (1 - phase) * Math.min(130, radius * 0.55);
          ctx.fillStyle = `rgba(${r}, ${g}, ${b}, ${(a * (0.3 + phase * 0.7)).toFixed(2)})`;
          ctx.fillRect(cx + Math.cos(ang) * dist - 2, cy + Math.sin(ang) * dist - 2, 4, 4);
        }
      } else if (kind === "screen_fade") {
        ctx.fillStyle = `rgba(${r}, ${g}, ${b}, ${(Math.min(0.75, a) * (1 - phase)).toFixed(3)})`;
        ctx.fillRect(0, 0, w, h);
      } else if (kind === "screen_shake") {
        ctx.strokeStyle = "#ffb000";
        ctx.lineWidth = 2;
        ctx.strokeRect(20, 20, w - 40, h - 40);
        ctx.fillStyle = "#ffb000";
        ctx.font = "bold 14px Tahoma, sans-serif";
        ctx.fillText("SCREEN SHAKE IMPULSE", cx - 86, cy);
      }
      ctx.restore();
      requestAnimationFrame(drawFrame);
    }

    [kindEl, colEl, alphaEl, radEl, widthEl, lifeEl, noiseEl].forEach((el) => {
      el.addEventListener("input", updateCode);
      el.addEventListener("change", updateCode);
    });
    document.getElementById("btn-vfx-trigger").addEventListener("click", () => { phase = 0; });
    document.getElementById("btn-vfx-playground").addEventListener("click", () => {
      const call = document.getElementById("vfx-code-py").textContent;
      const plugin = `from cs16 import mod, vfx, HookAction\n\n@mod.chat_command("/vfx")\ndef cmd_vfx(player, args):\n    ${call}\n    return HookAction.SUPERCEDE\n\n@mod.hook("player_killed", phase="post")\ndef on_kill(ev):\n    player = ev.killer or ev.victim\n    ${call}\n`;
      openInPlayground("vfx_studio", "Custom Protocol 48 VFX", plugin);
    });

    updateCode();
    requestAnimationFrame(drawFrame);
  }

  document.addEventListener("DOMContentLoaded", () => {
    initMenuStudio();
    initHudStudio();
    initVfxStudio();
  });
})();
"##;

const AMXX_CONVERTER_JS: &str = r##"(function () {
  "use strict";

  const PRESETS = {
    vip: `#include <amxmodx>
#include <cstrike>
#include <fun>
#include <hamsandwich>

public plugin_init() {
    register_plugin("VIP Bonus", "1.0", "Author");
    register_clcmd("say /vip", "cmd_vip");
    RegisterHam(Ham_Spawn, "player", "fw_Spawn", 1);
    RegisterHam(Ham_TakeDamage, "player", "fw_TakeDamage", 0);
}

public fw_Spawn(id) {
    if (is_user_alive(id) && (get_user_flags(id) & ADMIN_LEVEL_H)) {
        cs_set_user_armor(id, 100, CS_ARMOR_VESTHELM);
        give_item(id, "weapon_hegrenade");
        set_user_health(id, 115);
    }
}

public fw_TakeDamage(victim, inflictor, attacker, Float:damage, bits) {
    if (is_user_alive(attacker) && (get_user_flags(attacker) & ADMIN_LEVEL_H)) {
        SetHamParamFloat(4, damage * 1.15);
        return HAM_OVERRIDE;
    }
    return HAM_IGNORED;
}

public cmd_vip(id) {
    new menu = menu_create("\\yVIP Kit", "menu_handler");
    menu_additem(menu, "AK-47 + Deagle", "weapon_ak47");
    menu_additem(menu, "M4A1 + Deagle", "weapon_m4a1");
    menu_display(id, menu, 0);
    return PLUGIN_HANDLED;
}`,
    resetscore: `#include <amxmodx>
#include <cstrike>
#include <fun>

public plugin_init() {
    register_plugin("ResetScore", "1.0", "Author");
    register_clcmd("say /rs", "cmd_rs");
    register_clcmd("say /resetscore", "cmd_rs");
}

public cmd_rs(id) {
    set_user_frags(id, 0);
    cs_set_user_deaths(id, 0);
    client_print(id, print_chat, "[RS] Your score has been reset to 0-0!");
    return PLUGIN_HANDLED;
}`,
    damager: `#include <amxmodx>
#include <hamsandwich>

public plugin_init() {
    register_plugin("Hit Damager", "1.0", "Author");
    RegisterHam(Ham_TakeDamage, "player", "fw_Damage", 1);
}

public fw_Damage(victim, inflictor, attacker, Float:damage, bits) {
    if (is_user_alive(attacker) && victim != attacker) {
        set_hudmessage(0, 200, 255, -1.0, 0.55, 0, 0.1, 1.0, 0.1, 0.2, 2);
        show_hudmessage(attacker, "-%d HP", floatround(damage));
    }
    return HAM_IGNORED;
}`,
    admincmd: `#include <amxmodx>
#include <fun>

public plugin_init() {
    register_plugin("Admin Slap/Slay", "1.0", "Author");
    register_concmd("amx_slap", "cmd_slap", ADMIN_SLAY);
    register_concmd("amx_heal", "cmd_heal", ADMIN_SLAY);
}

public cmd_slap(id) {
    if (get_user_flags(id) & ADMIN_SLAY) {
        user_slap(id, 10);
        client_print(id, print_chat, "[Admin] Slapped player for 10 HP.");
    }
    return PLUGIN_HANDLED;
}

public cmd_heal(id) {
    if (get_user_flags(id) & ADMIN_SLAY) {
        set_user_health(id, 150);
        client_print(id, print_chat, "[Admin] Healed to 150 HP.");
    }
    return PLUGIN_HANDLED;
}`,
  };

  function analyzeSma(src) {
    const info = {
      name: "Converted AMXX Plugin",
      version: "1.0.0",
      author: "amxx-import",
      id: "converted_amxx",
      chatCmds: [],
      conCmds: [],
      hams: [],
      events: [],
      perms: new Set(["entities", "messages"]),
      notes: [],
    };

    const plugMatch = src.match(/register_plugin\s*\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)"/);
    if (plugMatch) {
      info.name = plugMatch[1];
      info.version = plugMatch[2];
      info.author = plugMatch[3];
      info.id = plugMatch[1].toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "") || "converted_amxx";
    }

    const clcmdRe = /register_clcmd\s*\(\s*"([^"]+)"\s*,\s*"([^"]+)"/g;
    let m;
    while ((m = clcmdRe.exec(src)) !== null) {
      info.perms.add("commands");
      const raw = m[1].trim();
      if (raw.toLowerCase().startsWith("say ")) {
        info.chatCmds.push({ trigger: raw.slice(4).trim(), fn: m[2] });
      } else {
        info.conCmds.push({ cmd: raw, fn: m[2] });
      }
    }

    const concmdRe = /register_concmd\s*\(\s*"([^"]+)"\s*,\s*"([^"]+)"/g;
    while ((m = concmdRe.exec(src)) !== null) {
      info.perms.add("commands");
      info.perms.add("admins");
      info.conCmds.push({ cmd: m[1].trim(), fn: m[2] });
    }

    const hamRe = /RegisterHam\s*\(\s*Ham_(\w+)\s*,\s*"[^"]+"\s*,\s*"([^"]+)"(?:\s*,\s*(\d+))?/g;
    const hamMap = {
      Spawn: { py: "player_spawn", rust: "PlayerSpawn" },
      TakeDamage: { py: "player_take_damage", rust: "PlayerTakeDamage" },
      Killed: { py: "player_killed", rust: "PlayerKilled" },
      Player_Jump: { py: "player_jump", rust: "PlayerJump" },
      Item_Deploy: { py: "weapon_deploy", rust: "WeaponDeploy" },
      Weapon_PrimaryAttack: { py: "weapon_primary_attack", rust: "WeaponPrimaryAttack" },
    };
    while ((m = hamRe.exec(src)) !== null) {
      const mapped = hamMap[m[1]] || { py: "player_spawn", rust: "PlayerSpawn" };
      const post = m[3] === "1";
      info.hams.push({ ham: m[1], fn: m[2], pyHook: mapped.py, rustHook: mapped.rust, post });
    }

    if (/menu_create|menu_additem/.test(src)) info.perms.add("menus");
    if (/give_item|strip_user_weapons|rg_give_item/.test(src)) info.perms.add("weapons");
    if (/get_user_flags|ADMIN_/.test(src)) info.perms.add("admins");
    if (/nvault_/.test(src)) info.perms.add("storage");
    if (/set_task/.test(src)) info.perms.add("timers");
    if (/get_cvar|set_cvar|pcvar/.test(src)) info.perms.add("cvars");

    if (/SetHamParamFloat/.test(src)) {
      info.notes.push("Mapped SetHamParamFloat(4, ...) to mutable ev.damage + HookAction.OVERRIDE.");
    }
    if (/menu_create/.test(src)) {
      info.notes.push("Mapped newmenus calls to shared host MenuManager (zero menuselect collisions).");
    }
    if (/ADMIN_LEVEL_H/.test(src)) {
      info.notes.push("Mapped ADMIN_LEVEL_H to RBAC flag 't' (configs/admins.toml).");
    }
    if (/ADMIN_SLAY/.test(src)) {
      info.notes.push("Mapped ADMIN_SLAY to RBAC flag 'e' (configs/admins.toml).");
    }
    return info;
  }

  function extractFnBody(src, fnName) {
    const re = new RegExp(`public\\s+${fnName}\\s*\\([^)]*\\)\\s*\\{`);
    const match = re.exec(src);
    if (!match) return "";
    let depth = 1;
    let i = match.index + match[0].length;
    const start = i;
    while (i < src.length && depth > 0) {
      if (src[i] === "{") depth++;
      else if (src[i] === "}") depth--;
      i++;
    }
    return src.slice(start, i - 1).trim();
  }

  function convertBodyToPy(body, targetVar) {
    const lines = [];
    const p = targetVar || "player";
    if (/ADMIN_LEVEL_H/.test(body)) lines.push(`if not ${p}.has_flag("t"): return HookAction.CONTINUE`);
    else if (/ADMIN_SLAY/.test(body)) lines.push(`if not ${p}.has_flag("e"): return HookAction.SUPERCEDE`);

    const armor = body.match(/cs_set_user_armor\s*\([^,]+,\s*(\d+)/);
    if (armor) lines.push(`${p}.set_armor(${armor[1]}, Armor.VESTHELM)`);
    const hp = body.match(/set_user_health\s*\([^,]+,\s*(\d+)/);
    if (hp) lines.push(`${p}.set_health(${hp[1]}.0)`);
    const giveRe = /give_item\s*\([^,]+,\s*"([^"]+)"/g;
    let gm;
    while ((gm = giveRe.exec(body)) !== null) lines.push(`${p}.give_item("${gm[1]}")`);
    if (/set_user_frags\s*\([^,]+,\s*0\)/.test(body) || /cs_set_user_deaths\s*\([^,]+,\s*0\)/.test(body)) {
      lines.push(`${p}.reset_score()`);
    }
    const slap = body.match(/user_slap\s*\([^,]+,\s*(\d+)/);
    if (slap) lines.push(`${p}.slap(${slap[1]})`);
    const hamDmg = body.match(/SetHamParamFloat\s*\(\s*4\s*,\s*([^)]+)\)/);
    if (hamDmg) {
      lines.push(`ev.damage = ${hamDmg[1].replace(/\bdamage\b/g, "ev.damage")}`);
      lines.push("return HookAction.OVERRIDE");
    }
    const hud = body.match(/show_hudmessage\s*\([^,]+,\s*"([^"]+)"/);
    if (hud) {
      const msg = hud[1].replace(/%d/g, "{int(ev.damage)}");
      lines.push(`${p}.hud(f"${msg}", x=-1.0, y=0.55, rgba=(0, 200, 255, 255), hold=1.0)`);
    }
    const menuMatch = body.match(/menu_create\s*\(\s*"([^"]+)"/);
    if (menuMatch) {
      lines.push(`menu = Menu("${menuMatch[1]}", callback=lambda pl, info: pl.give_item(info))`);
      const itemRe = /menu_additem\s*\([^,]+,\s*"([^"]+)"\s*,\s*"([^"]+)"/g;
      let im;
      while ((im = itemRe.exec(body)) !== null) {
        lines.push(`menu.add_item("${im[1]}", "${im[2]}")`);
      }
      lines.push(`${p}.show_menu(menu)`);
    }
    const printRe = /client_print\s*\([^,]+,\s*print_chat\s*,\s*"([^"]+)"/g;
    let pm;
    while ((pm = printRe.exec(body)) !== null) {
      lines.push(`${p}.chat(f"{Color.GREEN}${pm[1]}{Color.DEFAULT}")`);
    }
    if (/PLUGIN_HANDLED/.test(body)) lines.push("return HookAction.SUPERCEDE");
    if (!lines.length) lines.push("return HookAction.CONTINUE");
    return lines;
  }

  function buildPython(src, info) {
    const permsList = Array.from(info.perms).map((p) => `"${p}"`).join(", ");
    const out = [
      `# Auto-converted from AMX Mod X (.sma) -> cs16-rs Python 3 ABI v2`,
      `from cs16 import mod, Color, HookAction, Armor, Menu`,
      "",
      `@mod.info(`,
      `    id="${info.id}",`,
      `    name="${info.name}",`,
      `    version="${info.version}",`,
      `    author="${info.author}",`,
      `    permissions=[${permsList}],`,
      `)`,
      `def _meta():`,
      `    pass`,
      "",
    ];

    // Group chat commands sharing the same handler
    const byFn = new Map();
    for (const c of info.chatCmds) {
      if (!byFn.has(c.fn)) byFn.set(c.fn, []);
      byFn.get(c.fn).push(c.trigger);
    }
    for (const [fn, triggers] of byFn.entries()) {
      const trigArgs = triggers.map((t) => `"${t}"`).join(", ");
      const bodyLines = convertBodyToPy(extractFnBody(src, fn), "player");
      out.push(`@mod.chat_command(${trigArgs})`);
      out.push(`def ${fn}(player, args):`);
      bodyLines.forEach((l) => out.push(`    ${l}`));
      out.push("");
    }

    for (const c of info.conCmds) {
      const bodyLines = convertBodyToPy(extractFnBody(src, c.fn), "player");
      out.push(`@mod.console_command("${c.cmd}")`);
      out.push(`def ${c.fn}(player, args):`);
      bodyLines.forEach((l) => out.push(`    ${l}`));
      out.push("");
    }

    for (const h of info.hams) {
      const phase = h.post ? "post" : "pre";
      const target = h.pyHook === "player_take_damage" ? "ev.attacker" : "ev.player";
      const bodyLines = convertBodyToPy(extractFnBody(src, h.fn), target);
      out.push(`@mod.hook("${h.pyHook}", phase="${phase}")`);
      out.push(`def ${h.fn}(ev):`);
      if (target === "ev.attacker") {
        out.push("    if ev.attacker is None:");
        out.push("        return HookAction.CONTINUE");
      }
      bodyLines.forEach((l) => out.push(`    ${l}`));
      out.push("");
    }
    return out.join("\n");
  }

  function buildRust(src, info) {
    const out = [
      `// Auto-converted from AMX Mod X (.sma) -> cs16-rs Rust WASM SDK (#![forbid(unsafe_code)])`,
      `use cs_mod_sdk::{ArmorKind, HookAction, HookId, HookPhase, HostBridge, Player, cs_mod_hook, cs_mod_init};`,
      "",
      `cs_mod_sdk::cs16_wasm_host_bridge!();`,
      "",
      `#[cs_mod_init]`,
      `pub fn init_mod(host: &mut impl HostBridge) -> i32 {`,
    ];
    for (const h of info.hams) {
      const phase = h.post ? "HookPhase::Post" : "HookPhase::Pre";
      out.push(`    host.hook_register(HookId::${h.rustHook}, ${phase}, 0);`);
    }
    if (info.chatCmds.length) {
      out.push(`    host.hook_register(HookId::PlayerSay, HookPhase::Pre, 0);`);
    }
    out.push("    0");
    out.push("}");
    out.push("");

    for (const h of info.hams) {
      const phase = h.post ? "post" : "pre";
      const body = extractFnBody(src, h.fn);
      out.push(`#[cs_mod_hook(${h.rustHook}, ${phase})]`);
      out.push(`pub fn ${h.fn}(host: &mut impl HostBridge) -> HookAction {`);
      if (h.rustHook === "PlayerTakeDamage") {
        out.push("    let atk = host.hook_arg_i32(1);");
        out.push("    if atk < 0 { return HookAction::Continue; }");
        out.push("    let player = Player::new(atk as u8);");
        if (/SetHamParamFloat/.test(body)) {
          out.push("    if player.has_flag(host, 't') {");
          out.push("        host.hook_set_override_f32(host.hook_arg_f32(0) * 1.15);");
          out.push("        return HookAction::Override;");
          out.push("    }");
        }
      } else {
        out.push("    let player = Player::new(host.hook_arg_i32(0) as u8);");
        if (/cs_set_user_armor/.test(body)) {
          out.push("    player.set_armor(host, 100, ArmorKind::VestHelm);");
        }
        if (/set_user_health/.test(body)) {
          out.push("    player.set_health(host, 115.0);");
        }
      }
      out.push("    HookAction::Continue");
      out.push("}");
      out.push("");
    }

    out.push("cs_mod_sdk::cs16_wasm_exports! {");
    out.push("    init: init_mod,");
    out.push("    hooks: [");
    for (const h of info.hams) {
      const phase = h.post ? "HookPhase::Post" : "HookPhase::Pre";
      out.push(`        (HookId::${h.rustHook}, ${phase}) => ${h.fn},`);
    }
    out.push("    ],");
    out.push("}");
    return out.join("\n");
  }

  function initConverter() {
    const inputEl = document.getElementById("sma-input");
    const outEl = document.getElementById("sma-output");
    const diagEl = document.getElementById("sma-diagnostics");
    const lineCountEl = document.getElementById("sma-line-count");
    if (!inputEl || !outEl) return;

    let outLang = "py";
    let lastPy = "";
    let lastRust = "";
    let lastInfo = null;

    function convert() {
      const src = inputEl.value || "";
      lineCountEl.textContent = `${src.split(/\r?\n/).length} lines`;
      lastInfo = analyzeSma(src);
      lastPy = buildPython(src, lastInfo);
      lastRust = buildRust(src, lastInfo);
      outEl.textContent = outLang === "py" ? lastPy : lastRust;

      const perms = Array.from(lastInfo.perms).join(", ");
      const notes = lastInfo.notes.length ? " · " + lastInfo.notes.join(" ") : "";
      diagEl.textContent = `Detected "${lastInfo.name}" (${lastInfo.id}) · Hooks: ${lastInfo.hams.length} · Chat Cmds: ${lastInfo.chatCmds.length} · Console Cmds: ${lastInfo.conCmds.length} · Permissions: [${perms}]${notes}`;
    }

    document.querySelectorAll("[data-sma-preset]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const key = btn.dataset.smaPreset;
        if (PRESETS[key]) {
          inputEl.value = PRESETS[key];
          convert();
        }
      });
    });

    document.querySelectorAll("[data-out-lang]").forEach((btn) => {
      btn.addEventListener("click", () => {
        outLang = btn.dataset.outLang || "py";
        document.querySelectorAll("[data-out-lang]").forEach((b) => b.classList.toggle("active", b === btn));
        outEl.textContent = outLang === "py" ? lastPy : lastRust;
      });
    });

    document.getElementById("btn-copy-converted").addEventListener("click", () => {
      const txt = outEl.textContent || "";
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(txt).catch(() => {});
      }
    });

    document.getElementById("btn-download-converted").addEventListener("click", () => {
      const ext = outLang === "py" ? "py" : "rs";
      const blob = new Blob([outEl.textContent || ""], { type: "text/plain;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${(lastInfo && lastInfo.id) || "converted"}.${ext}`;
      a.click();
      URL.revokeObjectURL(url);
    });

    document.getElementById("btn-sma-playground").addEventListener("click", () => {
      const id = (lastInfo && lastInfo.id) || "amxx_converted";
      const name = (lastInfo && lastInfo.name) || "Converted AMXX Plugin";
      try {
        localStorage.setItem("cs16_playground_import", JSON.stringify({ id, name, code: lastPy }));
      } catch (_) {}
      window.location.href = "../playground/?mod=" + encodeURIComponent(id);
    });

    inputEl.addEventListener("input", convert);
    inputEl.value = PRESETS.vip;
    convert();
  }

  document.addEventListener("DOMContentLoaded", initConverter);
})();
"##;
