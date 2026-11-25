//! Integration tests verifying the complete localized polyglot AMXX+ showcase mod suite shipped
//! in `mods/` (`std_admin`, `std_mapmanager` Go/WASI, `std_statsx`, `std_restmenu` C++20,
//! `csdm`, `gungame`, `zombie_plague`, `semiclip` C++20) with ZERO `.wat` files remaining in `mods/`.

use std::path::{Path, PathBuf};

use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_mod_api::{CsModArchive, HudDrawCmd, MOD_CAPABILITY_KEY};
use cs_mod_host::{
    ApprovalPolicy, ClientModHost, ClientModStatus, ClientModTransfer, ModStatus, ModdedGame,
    ServerModHost, discover_packages, load_package_csmod, load_package_dir,
};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{LoopbackHub, PeerAddr};
use cs_vfs::Vfs;

fn mods_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("mods")
}

fn count_wat_files(dir: &Path) -> usize {
    let mut n = 0usize;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                n += count_wat_files(&p);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("wat"))
            {
                n += 1;
            }
        }
    }
    n
}

fn connect_player(
    mod_rel_path: &str,
    supports_mods: bool,
    with_admin: bool,
) -> (
    Server<ModdedGame<WalkGame>>,
    LoopbackHub,
    Connection<cs_transport::LoopbackEndpoint>,
    f64,
) {
    let pkg = load_package_dir(&mods_dir().join(mod_rel_path)).unwrap();
    let mut host = ServerModHost::new();
    if with_admin {
        host.services().lock().admins.load_admins_toml_str(
            r#"
[[admins]]
auth = "player"
access = "abcdefghijklmnopqrstu"
"#,
        );
    }
    host.load_package(pkg).unwrap();

    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        ServerMap::stub("de_dust2"),
        ModdedGame::new(WalkGame::new(MoveParams::default()), host),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );

    let mut hub = LoopbackHub::new();
    let ep = hub.connect();
    let mut conn = Connection::new(
        ep,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("player", 76_561_197_960_265_730, &md5_hex(b"refmod")),
    );
    if supports_mods {
        conn.set_userinfo(MOD_CAPABILITY_KEY, "1");
    }
    conn.connect();

    let mut now = 0.0;
    for _ in 0..250 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut hub, now);
        if conn.state() == &ConnState::InGame {
            break;
        }
    }
    assert_eq!(conn.state(), &ConnState::InGame);
    (server, hub, conn, now)
}

fn pump_pair(
    server: &mut Server<ModdedGame<WalkGame>>,
    hub: &mut LoopbackHub,
    conn: &mut Connection<cs_transport::LoopbackEndpoint>,
    now: &mut f64,
    steps: usize,
) {
    for _ in 0..steps {
        *now += 0.01;
        conn.pump(*now);
        server.frame(hub, *now);
    }
}

#[test]
fn all_six_reference_mods_discover_and_repack_cleanly_as_csmod() {
    assert_eq!(count_wat_files(&mods_dir()), 0, "zero raw .wat files may remain in mods/");

    let pkgs = discover_packages(&mods_dir()).unwrap();
    let ids: Vec<&str> = pkgs.iter().map(|p| p.manifest.id.as_str()).collect();
    for expected in [
        "std_admin",
        "std_mapmanager",
        "std_statsx",
        "std_restmenu",
        "csdm",
        "gungame",
        "zombie_plague",
        "semiclip",
    ] {
        assert!(ids.contains(&expected), "missing showcase mod {expected} in {ids:?}");
    }
    for pkg in pkgs {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let mut manifest = pkg.manifest.clone();
        if pkg.server_wasm.is_some() {
            manifest.entry_server = Some("server.wasm".to_string());
        }
        if pkg.client_wasm.is_some() {
            manifest.entry_client = Some("client.wasm".to_string());
        }
        entries.push(("mod.toml".to_string(), manifest.to_toml().into_bytes()));
        if let Some(ref s) = pkg.server_wasm {
            entries.push(("server.wasm".to_string(), s.clone()));
        }
        if let Some(ref c) = pkg.client_wasm {
            entries.push(("client.wasm".to_string(), c.clone()));
        }
        for (k, v) in &pkg.assets {
            entries.push((format!("assets/{k}"), v.clone()));
        }
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
        let zip = CsModArchive::write_zip(refs).unwrap();
        let reparsed = load_package_csmod(&zip, None).unwrap();
        assert_eq!(reparsed.manifest.id, pkg.manifest.id);
    }
}

#[test]
fn ref_mod_deathmatch_respawns_and_announces() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("gamemodes/csdm", false, false);
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 15);
    assert_eq!(
        conn.fx().hud.center_print.as_deref(),
        Some("DEATHMATCH RESPAWN")
    );
    conn.send_stringcmd("dm_respawn");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}

#[test]
fn ref_mod_gungame_tracks_levels_and_announces_victory() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("gamemodes/gungame", false, false);
    for _ in 0..3 {
        conn.send_stringcmd("gg_kill");
        pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    }
    assert!(
        conn.fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("[GunGame]") && c.text.contains("Level"))
    );
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}

#[test]
fn ref_mod_zombie_infects_and_boosts_health() {
    let (mut server, mut hub, mut conn, mut now) =
        connect_player("gamemodes/zombie_plague", false, false);
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 15);
    assert_eq!(
        conn.fx().hud.center_print.as_deref(),
        Some("ZOMBIE OUTBREAK!")
    );
    conn.send_stringcmd("zp_shop");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    let menu = conn.fx().hud.menu.as_ref().expect("ZP shop menu opened");
    assert!(menu.text.contains("Golden AK-47"), "{}", menu.text);
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}

#[test]
fn ref_mod_admin_menu_opens_classic_showmenu_and_handles_select() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("std/admin", false, true);
    conn.send_stringcmd("amxmodmenu");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    let menu = conn.fx().hud.menu.as_ref().expect("ShowMenu opened");
    assert!(menu.text.contains("AMX Mod Menu"), "{}", menu.text);

    // Page 2 (`9. More`) -> item 1 (`Restart Round`)
    conn.send_stringcmd("menuselect 9");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    conn.send_stringcmd("menuselect 1");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    assert!(
        conn.fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("restarted the round"))
    );
}

#[test]
fn ref_mod_player_stats_persists_and_answers_rank() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("std/statsx", false, false);
    conn.send_stringcmd("rank");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    assert!(
        conn.fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("[STATS]") && c.text.contains("#1"))
    );
    conn.send_stringcmd("top15");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}

#[test]
fn ref_mod_custom_hud_auto_downloads_overlays_assets_and_draws_hud() {
    let (mut server, mut hub, mut conn, mut now) =
        connect_player("gamemodes/zombie_plague", true, false);
    let mut transfer = ClientModTransfer::new(ApprovalPolicy::AutoApproveTrusted);
    let mut client_host = ClientModHost::new();
    let mut vfs = Vfs::from_memory(Vec::new());

    for step in 0..450 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut hub, now);
        while let Some(um) = conn.fx_mut().hud.unknown_messages.pop_front() {
            if !transfer.ingest_usermsg(&um.name, &um.payload, &mut client_host, Some(&mut vfs)) {
                client_host.on_usermsg(&um.name, &um.payload);
            }
        }
        for cmd in transfer.take_outgoing_commands() {
            conn.send_stringcmd(&cmd);
        }
        if step > 100 && client_host.status("zombie_plague") == Some(&ClientModStatus::Active) {
            break;
        }
    }

    assert_eq!(client_host.status("zombie_plague"), Some(&ClientModStatus::Active));
    assert!(vfs.exists("sprites/zp_biohazard.txt"));
    let cmds = client_host.render_hud(16, 100, 100, 800);
    assert!(
        cmds.iter().any(|c| matches!(c, HudDrawCmd::Panel { title, .. } if title == "BIOHAZARD ZP")),
        "expected VGUI2 BIOHAZARD ZP panel in {cmds:?}"
    );
}

#[test]
fn ref_mod_go_mapmanager_instantiates_and_runs_rtv_and_nominate() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("std/mapmanager", false, false);
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);

    conn.send_stringcmd("say \"/maps\"");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}
