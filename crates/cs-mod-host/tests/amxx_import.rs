//! End-to-end integration tests for the 5-Second Legacy ReHLDS / AMX Mod X Server Migrator
//! (`cs_mod_host::amxx_import`), proving realistic multi-file `addons/amxmodx/` fixtures migrate
//! cleanly into `configs/` and boot directly into `AdminRegistry`, `LangCatalog`, `MapCycle`,
//! and `ServerModHost` + `AmxxRuntime`, while strictly rejecting path traversal, symlink escapes,
//! and oversized files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cs_amxx::amxxfile::build_test_amxx_plugin;
use cs_amxx::vm::op::*;
use cs_assets::bsp::Entity;
use cs_cvar::CVarRegistry;
use cs_engine::{ConsoleOutcome, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::mapcycle::MapCycle;
use cs_mod_host::admins::load_admins_from_dir;
use cs_mod_host::amxx_import::{
    ImportError, ImportOptions, import_amxx_dir, import_amxx_dir_with_options,
};
use cs_mod_host::lang::load_lang_dir;
use cs_mod_host::{AdminFlags, AdminRegistry, LangCatalog, ModdedGame, ServerModHost};
use cs_proto48::delta::DeltaRegistry;
use cs_vfs::Vfs;

static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(1);

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(label: &str) -> Self {
        let id = NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cs16_rs_amxx_import_{label}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp test dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Builds a minimal valid Pawn 3.2 `.amxx` binary whose `plugin_init()` registers itself,
/// registers `"custom.txt"` dictionary, and prints `boot_marker` to the server console.
fn build_greeter_amxx(plugin_title: &str, boot_marker: &str) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::new();
    let mut add_str = |s: &str| -> u32 {
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let addr = data.len() as u32;
        for &b in s.as_bytes() {
            data.extend_from_slice(&i32::from(b).to_le_bytes());
        }
        data.extend_from_slice(&0i32.to_le_bytes());
        addr
    };

    let s_title = add_str(plugin_title);
    let s_ver = add_str("1.2");
    let s_author = add_str("legacy-owner");
    let s_dict = add_str("custom.txt");
    let s_boot = add_str(boot_marker);

    let natives = ["register_plugin", "register_dictionary", "server_print"];
    let mut cells: Vec<u32> = vec![OP_HALT, 0];

    let init_off = (cells.len() * 4) as u32;
    cells.push(OP_PROC);

    let push_call = |cells: &mut Vec<u32>, native_idx: u32, args: &[u32]| {
        for &a in args.iter().rev() {
            cells.push(OP_PUSH_C);
            cells.push(a);
        }
        let param_bytes = (args.len() * 4) as u32;
        cells.push(OP_PUSH_C);
        cells.push(param_bytes);
        cells.push(OP_SYSREQ_C);
        cells.push(native_idx);
        cells.push(OP_STACK);
        cells.push(param_bytes + 4);
    };

    push_call(&mut cells, 0, &[s_title, s_ver, s_author]);
    push_call(&mut cells, 1, &[s_dict]);
    push_call(&mut cells, 2, &[s_boot]);
    cells.push(OP_CONST_PRI);
    cells.push(0);
    cells.push(OP_RETN);

    build_test_amxx_plugin(
        &[("plugin_init", init_off)],
        &natives,
        &[],
        &cells,
        &data,
        4096,
        0,
    )
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
fn realistic_legacy_amxx_fixture_migrates_and_boots_cleanly() {
    let tmp = TempDirGuard::new("full_fixture");
    let src = tmp.path().join("legacy_cstrike");
    let out = tmp.path().join("migrated_cs16rs");

    let amxx_cfg_dir = src.join("addons").join("amxmodx").join("configs");
    let amxx_plugins_dir = src.join("addons").join("amxmodx").join("plugins");
    let amxx_lang_dir = src.join("addons").join("amxmodx").join("data").join("lang");
    let metamod_dir = src.join("addons").join("metamod");

    fs::create_dir_all(&amxx_cfg_dir).unwrap();
    fs::create_dir_all(&amxx_plugins_dir).unwrap();
    fs::create_dir_all(&amxx_lang_dir).unwrap();
    fs::create_dir_all(&metamod_dir).unwrap();

    // 1. users.ini with SteamID, VALVE_, IP, CIDR, ClanTag, Name+_pw, comments, all a..z flags
    fs::write(
        amxx_cfg_dir.join("users.ini"),
        "\u{FEFF}; Legacy AMX Mod X users.ini\n\
// Owner & staff\n\
\"STEAM_0:1:424242\" \"\" \"abcdefghijklmnopqrstu\" \"ce\" ; founder\n\
\"VALVE_0:0:11111\" \"ignored\" \"bt\" \"ce\"\n\
\"192.168.10.20\" \"\" \"cdef\" \"de\"\n\
\"10.20.0.0/16\" \"\" \"b\" \"de\"\n\
\"[VIP]\" \"vip_pw\" \"mt\" \"bk\"\n\
\"BossAdmin\" \"top_secret\" \"abcdefghijklmnopqrstuvwxyz\" \"a\"\n",
    )
    .unwrap();

    // 2. plugins.ini + plugins-custom.ini (stock + custom + disabled + explicit disabled override)
    fs::write(
        amxx_cfg_dir.join("plugins.ini"),
        "; AMX Mod X plugins\n\
admin.amxx\n\
;admin_sql.amxx ; SQL disabled\n\
admincmd.amxx\n\
adminslots.amxx\n\
multilingual.amxx\n\
plmenu.amxx\n\
antiflood.amxx\n\
imessage.amxx\n\
//scrollmsg.amxx\n\
nextmap.amxx\n\
mapchooser.amxx\n\
timeleft.amxx\n\
statsx.amxx\n\
miscstats.amxx\n\
restmenu.amxx\n\
custom_greeter.amxx debug ; custom plugin with binary\n\
;legacy_disabled.amxx\n",
    )
    .unwrap();
    fs::write(
        amxx_cfg_dir.join("plugins-custom.ini"),
        "telemenu.amxx\n\
missing_optional.amxx\n\
old_disabled_override.amxx disabled\n",
    )
    .unwrap();

    // Write real compiled Pawn 3.2 bytecode for `custom_greeter.amxx`
    let greeter_bytes = build_greeter_amxx("Custom Greeter", "MIGRATED_AMXX_BOOTED");
    fs::write(amxx_plugins_dir.join("custom_greeter.amxx"), &greeter_bytes).unwrap();

    // 3. maps.ini + cstrike/mapcycle.txt
    fs::write(
        amxx_cfg_dir.join("maps.ini"),
        "; Map list\n\
de_dust2 \"Dust II\"\n\
de_inferno \"Inferno\"\n\
;cs_italy \"Disabled Map\"\n",
    )
    .unwrap();
    fs::write(
        src.join("mapcycle.txt"),
        "// Server rotation\n\
de_dust2 \\minplayers\\2\\maxplayers\\32\n\
de_nuke \\minplayers\\6\\maxplayers\\24\n\
// cs_militia\n",
    )
    .unwrap();

    // 4. configs/amxx.cfg
    fs::write(
        amxx_cfg_dir.join("amxx.cfg"),
        "// AMX Mod X Configuration\n\
amx_language \"ru\"\n\
amx_flood_time 0.75\n\
amx_scrollmsg \"Welcome to Classic Pub!\" 600\n\
amx_sql_host \"127.0.0.1\"\n\
exec ../../outside_escape.cfg\n",
    )
    .unwrap();

    // 5. data/lang/custom.txt (UTF-8 BOM + en/ru + AMXX ^4/^1 color codes)
    fs::write(
        amxx_lang_dir.join("custom.txt"),
        "\u{FEFF}; Custom plugin translations\n\
[en]\n\
CUSTOM_HELLO = ^4Hello %s from migrated AMXX!^1\n\
[ru]\n\
CUSTOM_HELLO = ^4Привет, %s, из перенесённого AMXX!^1\n",
    )
    .unwrap();

    // 6. configs/modules.ini + addons/metamod/plugins.ini
    fs::write(
        amxx_cfg_dir.join("modules.ini"),
        "; AMXX modules\n\
fun\n\
cstrike\n\
hamsandwich\n\
reapi\n\
mysql\n\
orpheu\n",
    )
    .unwrap();
    fs::write(
        metamod_dir.join("plugins.ini"),
        "; Metamod plugins\n\
win32 addons/amxmodx/dlls/amxmodx_mm.dll\n\
linux addons/resemiclip/resemiclip_mm_i386.so\n\
linux addons/revoice/revoice_mm_i386.so\n\
win32 addons/yapb/bin/yapb.dll\n\
linux addons/legacy_hook/legacy_mm.so\n\
;linux addons/disabled/old.so\n",
    )
    .unwrap();

    // --- Execute the 5-second migrator ---
    let report = import_amxx_dir(&src, &out).expect("migration must succeed");

    // Verify MigrationReport numbers & formatted Display output
    assert_eq!(report.admins_imported, 6);
    assert_eq!(report.upgraded_stock_plugins.len(), 14);
    assert_eq!(
        report.native_mods_enabled,
        vec![
            "std_admin",
            "std_mapmanager",
            "std_statsx",
            "std_restmenu",
            "antiflood_admanager.py",
        ]
    );
    assert_eq!(report.custom_amxx_plugins.len(), 2);
    assert!(report.custom_amxx_plugins[0].binary_copied);
    assert!(!report.custom_amxx_plugins[1].binary_copied);
    assert_eq!(report.disabled_plugins.len(), 4);
    assert_eq!(report.maps_imported.len(), 3);
    assert_eq!(report.maps_imported[0].name, "de_dust2");
    assert_eq!(report.maps_imported[0].display_name.as_deref(), Some("Dust II"));
    assert_eq!(report.maps_imported[0].min_players, Some(2));
    assert_eq!(report.maps_imported[0].max_players, Some(32));
    assert_eq!(report.maps_imported[1].name, "de_inferno");
    assert_eq!(report.maps_imported[2].name, "de_nuke");
    assert_eq!(report.cvars_imported, 4); // exec ../../outside_escape.cfg was rejected!
    assert_eq!(report.imported_lang_files.len(), 1);
    assert_eq!(report.imported_lang_files[0].filename, "custom.txt");
    assert_eq!(report.imported_lang_files[0].entries_count, 2);
    assert_eq!(
        report.native_amxx_modules,
        vec!["fun", "cstrike", "hamsandwich", "reapi"]
    );
    assert_eq!(report.unsupported_native_modules.len(), 7); // 2 from modules.ini + 5 from metamod

    let summary = report.summary();
    assert!(summary.contains("Admins imported:        6"), "{summary}");
    assert!(summary.contains("Stock AMXX upgraded:    14 -> 5"), "{summary}");
    assert!(summary.contains("Custom .amxx mounted:   2"), "{summary}");
    assert!(summary.contains("Language files indexed: 1"), "{summary}");
    assert!(summary.contains("resemiclip_mm_i386.so"), "{summary}");
    assert!(summary.contains("Unsupported native modules (7)"), "{summary}");

    // --- Verify generated configs/admins.toml loads cleanly into AdminRegistry ---
    let mut admins = AdminRegistry::new();
    assert_eq!(load_admins_from_dir(&mut admins, &out), 6);
    assert!(
        admins
            .authenticate("STEAM_0:1:424242", "", "Someone", "")
            .unwrap()
            .contains(AdminFlags::IMMUNITY)
    );
    assert!(
        admins
            .authenticate("VALVE_0:0:11111", "", "Someone", "")
            .unwrap()
            .contains(AdminFlags::LEVEL_H)
    );
    assert!(
        admins
            .authenticate("", "192.168.10.20:27005", "Someone", "")
            .unwrap()
            .contains(AdminFlags::BAN)
    );
    assert!(
        admins
            .authenticate("", "10.20.99.5:27005", "Someone", "")
            .unwrap()
            .contains(AdminFlags::RESERVATION)
    );
    assert!(
        admins
            .authenticate("", "", "[VIP] Player", "vip_pw")
            .unwrap()
            .has_char('m')
    );
    assert_eq!(
        admins
            .authenticate("", "", "[vip] Player", "vip_pw")
            .unwrap(),
        AdminFlags::USER
    );
    assert!(admins.authenticate("", "", "BossAdmin", "wrong").is_err());
    assert_eq!(
        admins
            .authenticate("", "", "BossAdmin", "top_secret")
            .unwrap()
            .to_flag_string(),
        "abcdefghijklmnopqrstuvwxyz"
    );

    // --- Verify generated configs/lang/amxx/ loads directly into LangCatalog ---
    let mut direct_lang = LangCatalog::new();
    let loaded_lang = load_lang_dir(&mut direct_lang, &out.join("configs").join("lang").join("amxx"));
    assert_eq!(loaded_lang, 2);
    direct_lang.set_player_lang(0, None, "en");
    direct_lang.set_player_lang(1, None, "ru");
    assert_eq!(
        direct_lang.tr(Some(0), "CUSTOM_HELLO", &[("0", "Neo")]),
        "\x04Hello Neo from migrated AMXX!\x01"
    );
    assert_eq!(
        direct_lang.tr(Some(1), "CUSTOM_HELLO", &[("0", "Neo")]),
        "\x04Привет, Neo, из перенесённого AMXX!\x01"
    );

    // --- Verify generated configs/imported_amxx.cfg ---
    let imported_cfg = fs::read_to_string(out.join("configs").join("imported_amxx.cfg")).unwrap();
    assert!(imported_cfg.contains("amx_language \"ru\""));
    assert!(imported_cfg.contains("amx_flood_time 0.75"));
    assert!(imported_cfg.contains("// amx_sql_host"));
    assert!(!imported_cfg.contains("outside_escape.cfg"));

    // --- Verify generated mapcycle.txt loads cleanly into cs-game MapCycle ---
    let mapcycle_raw = fs::read(out.join("mapcycle.txt")).unwrap();
    let vfs = Vfs::from_memory([
        ("mapcycle.txt".to_string(), mapcycle_raw),
        ("maps/de_dust2.bsp".to_string(), Vec::new()),
        ("maps/de_inferno.bsp".to_string(), Vec::new()),
        ("maps/de_nuke.bsp".to_string(), Vec::new()),
    ]);
    let cycle = MapCycle::load(&vfs);
    assert_eq!(cycle.next_map(), Some("de_dust2"));

    // --- Verify generated configs/plugins.toml boots cleanly into ServerModHost + AmxxRuntime ---
    let mut host = ServerModHost::new();
    let (wasm_cnt, amxx_cnt) = host
        .load_plugins_toml(&out.join("configs").join("plugins.toml"))
        .expect("migrated configs/plugins.toml must boot cleanly in ServerModHost");
    assert_eq!(wasm_cnt, 0);
    assert_eq!(amxx_cnt, 1);
    assert_eq!(host.amxx_plugin_count(), 1);

    // Host's unified SharedPlatformServices already loaded both AdminRegistry & LangCatalog!
    {
        let svc = host.services();
        let mut guard = svc.lock();
        assert!(
            guard
                .admins
                .authenticate("STEAM_0:1:424242", "", "Founder", "")
                .unwrap()
                .contains(AdminFlags::RCON)
        );
        guard.lang.set_player_lang(0, None, "en");
        guard.lang.set_player_lang(1, None, "ru");
        assert_eq!(
            guard.lang.tr(Some(0), "CUSTOM_HELLO", &[("0", "Player")]),
            "\x04Hello Player from migrated AMXX!\x01"
        );
        assert_eq!(
            guard.lang.tr(Some(1), "CUSTOM_HELLO", &[("0", "Player")]),
            "\x04Привет, Player, из перенесённого AMXX!\x01"
        );
    }

    // Boot a live Server with ModdedGame<CsGame> and confirm the migrated .amxx plugin executes!
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
    let prints = srv.game().host.amxx_server_prints();
    assert!(
        prints.iter().any(|p| p == "MIGRATED_AMXX_BOOTED"),
        "expected migrated custom_greeter.amxx plugin_init to run, prints={prints:?}"
    );
    let ConsoleOutcome::Output(amxx_out) = srv.console_command("amxx plugins") else {
        panic!("expected `amxx plugins` output");
    };
    assert!(amxx_out.contains("Custom Greeter"), "amxx_out={amxx_out}");
}

#[test]
fn direct_amxmodx_root_directory_migrates_without_cstrike_parent() {
    let tmp = TempDirGuard::new("direct_amxx");
    let amxx = tmp.path().join("amxmodx");
    let out = tmp.path().join("out");
    fs::create_dir_all(amxx.join("configs")).unwrap();
    fs::write(
        amxx.join("configs").join("users.ini"),
        "\"STEAM_0:0:777\" \"\" \"bcdef\" \"ce\"\n",
    )
    .unwrap();
    fs::write(
        amxx.join("configs").join("plugins.ini"),
        "admin.amxx\nmapchooser.amxx\n",
    )
    .unwrap();

    let report = import_amxx_dir(&amxx, &out).expect("direct amxmodx dir should migrate");
    assert_eq!(report.admins_imported, 1);
    assert_eq!(report.native_mods_enabled, vec!["std_admin", "std_mapmanager"]);
    assert!(out.join("configs").join("admins.toml").is_file());
    assert!(out.join("configs").join("plugins.toml").is_file());
}

#[test]
fn rejects_dotdot_traversal_in_plugins_ini() {
    let tmp = TempDirGuard::new("traversal");
    let src = tmp.path().join("amxmodx");
    let out = tmp.path().join("out");
    fs::create_dir_all(src.join("configs")).unwrap();
    fs::write(
        src.join("configs").join("plugins.ini"),
        "admin.amxx\n../../secret_escape.amxx\n",
    )
    .unwrap();

    let err = import_amxx_dir(&src, &out).unwrap_err();
    assert!(
        matches!(err, ImportError::PathTraversal(_)),
        "expected PathTraversal error, got {err:?}"
    );
}

#[test]
fn enforces_bounded_file_sizes_for_configs_and_amxx_binaries() {
    let tmp = TempDirGuard::new("size_bounds");
    let src = tmp.path().join("amxmodx");
    let out = tmp.path().join("out");
    fs::create_dir_all(src.join("configs")).unwrap();
    fs::create_dir_all(src.join("plugins")).unwrap();

    // Config file larger than cap
    let huge_cfg = "a".repeat(2048);
    fs::write(src.join("configs").join("users.ini"), &huge_cfg).unwrap();
    let small_opts = ImportOptions {
        max_config_file_bytes: 512,
        max_amxx_plugin_bytes: 1024,
        copy_custom_binaries: true,
    };
    let err = import_amxx_dir_with_options(&src, &out, &small_opts).unwrap_err();
    assert!(
        matches!(err, ImportError::FileTooLarge { .. }),
        "expected FileTooLarge for oversized users.ini, got {err:?}"
    );

    // Replace users.ini with tiny file, test oversized .amxx binary
    fs::write(
        src.join("configs").join("users.ini"),
        "\"STEAM_0:0:1\" \"\" \"a\" \"ce\"\n",
    )
    .unwrap();
    fs::write(src.join("configs").join("plugins.ini"), "huge_mod.amxx\n").unwrap();
    fs::write(src.join("plugins").join("huge_mod.amxx"), vec![0u8; 2048]).unwrap();

    let err = import_amxx_dir_with_options(&src, &out, &small_opts).unwrap_err();
    assert!(
        matches!(err, ImportError::FileTooLarge { .. }),
        "expected FileTooLarge for oversized .amxx binary, got {err:?}"
    );
}

#[test]
fn rejects_symlink_or_junction_escape_outside_source_dir() {
    let tmp = TempDirGuard::new("symlink_escape");
    let outside = tmp.path().join("outside_secret");
    let src = tmp.path().join("amxmodx");
    let out = tmp.path().join("out");

    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "[en]\nSECRET = leaked\n").unwrap();
    fs::create_dir_all(src.join("configs")).unwrap();
    fs::create_dir_all(src.join("data")).unwrap();
    fs::write(src.join("configs").join("plugins.ini"), "admin.amxx\n").unwrap();

    let link_path = src.join("data").join("lang");

    #[cfg(unix)]
    let link_created = std::os::unix::fs::symlink(&outside, &link_path).is_ok();

    #[cfg(windows)]
    let link_created = std::os::windows::fs::symlink_dir(&outside, &link_path).is_ok()
        || std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link_path)
            .arg(&outside)
            .output()
            .is_ok_and(|o| o.status.success());

    assert!(link_created, "test harness must create symlink or junction");

    let err = import_amxx_dir(&src, &out).unwrap_err();
    assert!(
        matches!(err, ImportError::SymlinkEscape(_)),
        "expected SymlinkEscape error, got {err:?}"
    );

    // Remove the junction/symlink cleanly before TempDirGuard cleans up
    let _ = fs::remove_dir(&link_path).or_else(|_| fs::remove_file(&link_path));
}
