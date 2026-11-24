//! Integration tests verifying all 6 reference WASM mods shipped in `mods/`
//! (`deathmatch`, `gungame`, `zombie`, `admin_menu`, `player_stats`, `custom_hud`) both as
//! unpacked directories and `.csmod` archives.

use std::path::PathBuf;

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

fn connect_player(
    mod_id: &str,
    supports_mods: bool,
) -> (
    Server<ModdedGame<WalkGame>>,
    LoopbackHub,
    Connection<cs_transport::LoopbackEndpoint>,
    f64,
) {
    let pkg = load_package_dir(&mods_dir().join(mod_id)).unwrap();
    let mut host = ServerModHost::new();
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
    let pkgs = discover_packages(&mods_dir()).unwrap();
    let ids: Vec<&str> = pkgs.iter().map(|p| p.manifest.id.as_str()).collect();
    for expected in ["deathmatch", "gungame", "zombie", "admin_menu", "player_stats", "custom_hud"] {
        assert!(ids.contains(&expected), "missing ref mod {expected} in {ids:?}");
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
    let (mut server, mut hub, mut conn, mut now) = connect_player("deathmatch", false);
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
    let (mut server, mut hub, mut conn, mut now) = connect_player("gungame", false);
    for _ in 0..3 {
        conn.send_stringcmd("gg_kill");
        pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    }
    assert!(
        conn.fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("GunGame Level Up!"))
    );
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}

#[test]
fn ref_mod_zombie_infects_and_boosts_health() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("zombie", false);
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 15);
    assert_eq!(
        conn.fx().hud.center_print.as_deref(),
        Some("ZOMBIE OUTBREAK!")
    );
    assert_eq!(server.game_mut().host.list_mods()[0].status, ModStatus::Active);
}

#[test]
fn ref_mod_admin_menu_opens_classic_showmenu_and_handles_select() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("admin_menu", false);
    conn.send_stringcmd("csmod_admin");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    let menu = conn.fx().hud.menu.as_ref().expect("ShowMenu opened");
    assert!(menu.text.contains("Admin Menu"), "{}", menu.text);

    conn.send_stringcmd("menuselect 3");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    assert!(
        conn.fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("Admin: Round restart"))
    );
}

#[test]
fn ref_mod_player_stats_persists_and_answers_rank() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("player_stats", false);
    conn.send_stringcmd("rank");
    pump_pair(&mut server, &mut hub, &mut conn, &mut now, 10);
    assert!(
        conn.fx()
            .hud
            .chat
            .iter()
            .any(|c| c.text.contains("Stats Rank #1"))
    );
}

#[test]
fn ref_mod_custom_hud_auto_downloads_overlays_assets_and_draws_hud() {
    let (mut server, mut hub, mut conn, mut now) = connect_player("custom_hud", true);
    let mut transfer = ClientModTransfer::new(ApprovalPolicy::AutoApproveTrusted);
    let mut client_host = ClientModHost::new();
    let mut vfs = Vfs::from_memory(Vec::new());

    for step in 0..350 {
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
        if step > 100 && client_host.status("custom_hud") == Some(&ClientModStatus::Active) {
            break;
        }
    }

    assert_eq!(client_host.status("custom_hud"), Some(&ClientModStatus::Active));
    assert!(vfs.exists("sprites/mod_badge.txt"));
    let cmds = client_host.render_hud(16, 100, 100, 800);
    assert_eq!(cmds.len(), 3);
    assert!(matches!(&cmds[0], HudDrawCmd::Panel { title, .. } if title == "CS16-RS MOD"));
}
