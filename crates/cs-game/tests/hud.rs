//! The user messages and the HUD of a joining player, on a `Server<Rig>` (a `CsGame` that also
//! runs closures the test queued, at the start of the next frame) with probe clients.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cs_assets::bsp::Entity;
use cs_cvar::CVarRegistry;
use cs_engine::{ClientState, EdictId, EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::{ArmorType, Team};
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::{CsUserMsg, USERMSG_BASE_ID, UserMsgRegistry, encode_cs_usermsg};
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};

type Action = Box<dyn FnOnce(&mut CsGame, &mut EngineApi<'_>)>;

/// `CsGame` plus the closures the test queued, which run at the start of the next frame.
struct Rig {
    game: CsGame,
    actions: Vec<Action>,
}

impl GameDll for Rig {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.game.usermsgs()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_activate(eng);
    }
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_deactivate(eng);
    }
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.game.spawn_entity(eng, id, classname, keys)
    }
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.game.client_connect(eng, slot)
    }
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_put_in_server(eng, slot);
    }
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_disconnect(eng, slot);
    }
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.game.client_command(eng, slot, argv);
    }
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_userinfo_changed(eng, slot);
    }
    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        self.game.run_usercmd(eng, slot, cmd, random_seed);
    }
    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        self.game.post_usercmd(eng, slot, cmd);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        for action in std::mem::take(&mut self.actions) {
            action(&mut self.game, eng);
        }
        self.game.start_frame(eng, frametime);
    }
    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.game.think(eng, id);
    }
    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        self.game.touch(eng, toucher, other);
    }
    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        self.game.blocked(eng, pusher, other);
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.game.update_client_data(eng, slot)
    }
}

/// A server on a stub map running a [`Rig`], with probe clients, and its clock.
struct Bench {
    server: Server<Rig>,
    hub: LoopbackHub,
    probes: Vec<ProbeClient<LoopbackEndpoint>>,
    now: f64,
}

impl Bench {
    /// A server of `slots` slots whose game has `motd`, with no client yet.
    fn new(slots: u8, motd: &[u8]) -> Self {
        let cfg = ServerConfig {
            max_clients: slots,
            max_edicts: 256,
            ..ServerConfig::default()
        };
        let mut game = CsGame::new();
        game.state_mut().motd = motd.to_vec();
        let rig = Rig {
            game,
            actions: Vec::new(),
        };
        let mut entities = vec![entity(&[("classname", "worldspawn")])];
        for i in 0..20 {
            for class in ["info_player_start", "info_player_deathmatch"] {
                let y = if class == "info_player_start" { 0 } else { 500 };
                entities.push(entity(&[
                    ("classname", class),
                    ("origin", &format!("{} {} 0", 100 + i * 100, y)),
                    ("angles", "0 90 0"),
                ]));
            }
        }
        let map = ServerMap::stub_with_entities("test", entities);
        let server = Server::new(
            cfg,
            map,
            rig,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        Self {
            server,
            hub: LoopbackHub::new(),
            probes: Vec::new(),
            now: 0.0,
        }
    }

    /// A one-probe server of `slots` slots, the probe in the game.
    fn with_one_client(slots: u8, motd: &[u8]) -> Self {
        let mut b = Self::new(slots, motd);
        b.join();
        b
    }

    /// Connects one more probe and runs until it is in the game.
    fn join(&mut self) -> usize {
        let endpoint = self.hub.connect();
        let cfg = ProbeConfig {
            name: format!("p{}", self.probes.len()),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(format!("probe{}", self.probes.len()).as_bytes()),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        probe.connect();
        self.probes.push(probe);
        let n = self.probes.len() - 1;
        for _ in 0..600 {
            self.step(1);
            if self.probes[n].state() == &ProbeState::InGame {
                break;
            }
        }
        assert_eq!(self.probes[n].state(), &ProbeState::InGame);
        n
    }

    /// Runs `frames` server frames (10 ms each) with every probe pumped.
    fn step(&mut self, frames: usize) {
        for _ in 0..frames {
            for probe in &mut self.probes {
                probe.pump(self.now);
            }
            self.now = ((self.now / 0.01).round() + 1.0) * 0.01;
            self.server.frame(&mut self.hub, self.now);
        }
    }

    /// Runs `f` as the game at the start of the next frame and returns what it returned.
    fn run<T: 'static>(
        &mut self,
        f: impl FnOnce(&mut CsGame, &mut EngineApi<'_>) -> T + 'static,
    ) -> T {
        let out = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&out);
        self.server
            .game_mut()
            .actions
            .push(Box::new(move |game, eng| {
                *sink.borrow_mut() = Some(f(game, eng))
            }));
        self.step(1);
        let result = out.borrow_mut().take();
        result.expect("the game ran the action")
    }

    /// The user messages probe `n` has received, in order, as (name, decoded).
    fn messages(&self, n: usize) -> Vec<(String, CsUserMsg)> {
        self.probes[n]
            .messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::UserMsg { name, decoded, .. } => Some((name.clone(), decoded.clone())),
                _ => None,
            })
            .collect()
    }

    /// The names of the user messages probe `n` has received since its message number `from`.
    fn names_from(&self, n: usize, from: usize) -> Vec<String> {
        self.messages(n)
            .into_iter()
            .skip(from)
            .map(|(name, _)| name)
            .collect()
    }
}

fn entity(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

/// `LinkUserMessages` of ReGameDLL `client.cpp` (read 2026-11-22), written out once more here,
/// so a slip in the table of the game shows.
const REGAMEDLL_ORDER: &[(&str, i8)] = &[
    ("CurWeapon", 3), ("Geiger", 1), ("Flashlight", 2), ("FlashBat", 1), ("Health", 1),
    ("Damage", 12), ("Battery", 2), ("Train", 1), ("HudTextPro", -1), ("HudText", -1),
    ("SayText", -1), ("TextMsg", -1), ("WeaponList", -1), ("ResetHUD", 0), ("InitHUD", 0),
    ("ViewMode", 0), ("GameTitle", 1), ("DeathMsg", -1), ("ScoreAttrib", 2), ("ScoreInfo", 9),
    ("TeamInfo", -1), ("TeamScore", -1), ("GameMode", 1), ("MOTD", -1), ("ServerName", -1),
    ("AmmoPickup", 2), ("WeapPickup", 1), ("ItemPickup", -1), ("HideWeapon", 1), ("SetFOV", 1),
    ("ShowMenu", -1), ("ScreenShake", 6), ("ScreenFade", 10), ("AmmoX", 2), ("SendAudio", -1),
    ("RoundTime", 2), ("Money", 5), ("ArmorType", 1), ("BlinkAcct", 1), ("StatusValue", -1),
    ("StatusText", -1), ("StatusIcon", -1), ("BarTime", 2), ("ReloadSound", 2), ("Crosshair", 1),
    ("NVGToggle", 1), ("Radar", 7), ("Spectator", 2), ("VGUIMenu", -1), ("TutorText", -1),
    ("TutorLine", -1), ("TutorState", -1), ("TutorClose", -1), ("AllowSpec", 1), ("BombDrop", 7),
    ("BombPickup", 0), ("ClCorpse", -1), ("HostagePos", 8), ("HostageK", 1), ("HLTV", 2),
    ("SpecHealth", 1), ("ForceCam", 3), ("ADStop", 0), ("ReceiveW", 1), ("CZCareer", -1),
    ("CZCareerHUD", -1), ("ShadowIdx", 4), ("TaskTime", 4), ("Scenario", -1), ("BotVoice", 2),
    ("BuyClose", 0), ("SpecHealth2", 2), ("BarTime2", 4), ("ItemStatus", 1), ("Location", -1),
    ("BotProgress", -1), ("Brass", -1), ("Fog", 7), ("ShowTimer", 0), ("HudTextArgs", -1),
];

#[test]
fn usermsgs_registered_in_regamedll_order() {
    let reg = CsGame::new().usermsgs();
    let got: Vec<(String, i8, u8)> = reg
        .iter()
        .map(|d| (d.name.clone(), d.size, d.id))
        .collect();
    let want: Vec<(String, i8, u8)> = REGAMEDLL_ORDER
        .iter()
        .enumerate()
        .map(|(i, &(n, s))| (n.to_string(), s, USERMSG_BASE_ID + i as u8))
        .collect();
    assert_eq!(got, want);

    // Every message the codec can encode is in the table, with the size its payload has.
    let samples = [
        CsUserMsg::Health(1),
        CsUserMsg::Battery(1),
        CsUserMsg::Money { amount: 1, flash: false },
        CsUserMsg::ScoreInfo { player_id: 1, frags: 0, deaths: 0, class_id: 0, team_id: 0 },
        CsUserMsg::ScoreAttrib { player_id: 1, flags: 0 },
        CsUserMsg::CurWeapon { is_active: false, weapon_id: 0, clip_ammo: 0 },
        CsUserMsg::RoundTime(1),
        CsUserMsg::SetFov(90),
        CsUserMsg::Radar { player_id: 1, origin: glam::Vec3::ZERO },
    ];
    for msg in samples {
        let (name, payload) = encode_cs_usermsg(&msg);
        let id = reg.id_for_name(name).unwrap_or_else(|| panic!("{name} registered"));
        let size = reg.get_by_id(id).unwrap().size;
        assert!(size < 0 || size as usize == payload.len(), "{name}");
    }
}

/// What is left of the user messages of `Spawn()` inside `ClientPutInServer`. The original clears
/// what the game sent to the joiner alone (`NVGToggle`, `BarTime`, `ScoreAttrib`, `ScreenFade`,
/// `RoundTime`: `SZ_Clear` of `netchan.message` after `pfnClientPutInServer`, `SV_WriteSpawn`,
/// ReHLDS `sv_main.cpp`, read 2026-11-22; the golden capture of HLDS has none of them), and what
/// it sent to everyone (`MSG_ALL`) stays: `TeamInfo` and `ScoreInfo` (player.cpp:6106, 6112), and
/// then `ClientPutInServer`'s `#Game_connected` (`TextMsg`). They reach the joiner at the end of
/// the frame of its `spawn` (`sv.reliable_datagram`), whatever the commands that follow send.
pub const SPAWN_ON_CONNECT: &[&str] = &["TeamInfo", "ScoreInfo"];

/// What the first command's `PlayerPreThink` sends before `UpdateClientData`: the join menus
/// (`JoiningThink`: the MOTD window is reset, the defuser icon and the item status hidden). The
/// team menu (`PlayerThink`) is the second command's, after the first update: `VGUIMenu`.
const FIRST_MENU: &[&str] = &["ShowMenu", "StatusIcon", "ItemStatus"];

/// The messages a first player gets when its first client data update runs.
const FIRST_UPDATE: &[&str] = &[
    // CBasePlayer::UpdateClientData, m_fInitHUD
    "ResetHUD",
    "InitHUD",
    // CHalfLifeMultiplay::InitHUD
    "GameMode",
    "ViewMode",
    "ScoreInfo",
    "ShadowIdx",
    "ServerName",
    "ScoreInfo",
    "TeamScore",
    "TeamScore",
    "AllowSpec",
    "ForceCam",
    "TeamInfo",
    "ScoreAttrib",
    // back in UpdateClientData
    "Money",
    "StatusIcon",
    "ScoreAttrib",
    "RoundTime",
    "TeamScore",
    "TeamScore",
    // `PreThink` ran before this update and set the flashlight bit (`mp_flashlight` is 0).
    "HideWeapon",
    "SetFOV",
    "Health",
    "Battery",
    "Damage",
    // The flashlight clock is due after the `Spawn()` (`m_flFlashLightTime = 1`,
    // `m_iFlashBattery = 99`): the battery charges to 100 and the client is told (`FlashBat`).
    "FlashBat",
    "Train",
    // `m_rgAmmoLast` is -1 after the `Spawn()` of the join: all 32 ammo counts, zeros too
    // (`SendAmmoUpdate`).
    "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX",
    "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX",
    "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX",
    "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX", "AmmoX",
];

/// What a joining client is sent up to the team menu, as ReGameDLL sends it: the `Spawn()` of
/// `ClientPutInServer`, `#Game_connected`, the menus of the first command's `PreThink`, the first
/// `UpdateClientData` (ResetHUD, InitHUD, the rules' `InitHUD`, Money, ..., RoundTime, HideWeapon,
/// ...). Exact order, and the wire's `ClientData.weapons` has no `WEAPON_SUIT` bit before the
/// player has joined (`PlayerSpawn` gives the suit, and it returns at once for a joiner).
/// What the original client does with the round timer it is told here is not decided by this
/// test (see the join-screen report).
#[test]
fn join_hud_messages_as_regamedll() {
    let mut b = Bench::with_one_client(2, b"");
    b.step(20);
    let all = b.names_from(0, 0);
    // The weapon table comes first, in the `spawn` answer (the init buffer).
    let weapon_lists = all.iter().take_while(|n| *n == "WeaponList").count();
    assert_eq!(weapon_lists, cs_game::weapons::item_infos().len());
    let names = &all[weapon_lists..];
    let mut expected: Vec<&str> = SPAWN_ON_CONNECT.to_vec();
    expected.push("TextMsg");
    expected.extend(FIRST_MENU);
    expected.extend(FIRST_UPDATE);
    // The team menu is the second command's `PlayerThink`, after the first update.
    expected.push("VGUIMenu");
    assert_eq!(names[..expected.len()], *expected);
    // What follows is the team menu and the second `FlashBat 100`: the clock of the full battery
    // runs out 0.2 s after the first (`m_flFlashLightTime = 0`), as HLDS sent it twice.
    assert!(names[expected.len()..]
        .iter()
        .all(|n| n == "ShowMenu" || n == "VGUIMenu" || n == "FlashBat"));
    assert_eq!(names.iter().filter(|n| *n == "FlashBat").count(), 2);
    // `RoundTime` once, from the first update (the `Spawn()` of the join sent one the engine
    // cleared); no `ShowTimer`; `HideWeapon` once, the flashlight bit only.
    assert_eq!(names.iter().filter(|n| *n == "RoundTime").count(), 1);
    assert!(!names.iter().any(|n| n == "ShowTimer"));
    let hides: Vec<CsUserMsg> = b
        .messages(0)
        .into_iter()
        .filter(|(n, _)| n == "HideWeapon")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(hides, [CsUserMsg::HideWeapon(2)]);
    // The client data as the probe decoded it from the wire (the last one received, before any
    // team was chosen): no `WEAPON_SUIT` bit.
    let data = b.probes[0].client_data().expect("the probe received client data");
    assert_eq!(data.weapons & (1 << 31), 0, "WEAPON_SUIT before joining");
}

#[test]
fn init_hud_sequence_as_regamedll() {
    let mut b = Bench::with_one_client(2, b"");
    b.step(20);
    let all = b.names_from(0, 0);
    // The weapon table of `WriteSigonMessages` is in the init (sign-on) buffer every client is
    // sent at `spawn`, so all of it comes before what `ClientPutInServer` sends.
    let weapon_lists = all.iter().filter(|n| *n == "WeaponList").count();
    assert_eq!(weapon_lists, cs_game::weapons::item_infos().len());
    assert!(all[..weapon_lists].iter().all(|n| n == "WeaponList"), "WeaponList first: {all:?}");
    let names = &all[weapon_lists..];
    // What `ClientPutInServer` sends to everyone survives the engine's clearing of the joiner's
    // own messages: `TeamInfo` and `ScoreInfo` of its `Spawn()`, and `#Game_connected` to
    // everyone, the newcomer included. They are the broadcast of the frame of its `spawn`, which
    // the engine delivers at the end of it (`SV_UpdateToReliableMessages`). Then the client's
    // first command runs (the engine runs the moves that follow `sendents` in the same packet, as
    // ReHLDS runs them after `begin`): `PlayerPreThink` runs for every command, so the join menus
    // (`teams::pre_think`) and the first update (`UpdateClientData`, in `PreThink` too) come out
    // in it, the HUD once; the team menu is the second command's.
    assert_eq!(names[..SPAWN_ON_CONNECT.len()], *SPAWN_ON_CONNECT, "Spawn() of ClientPutInServer");
    let names = &names[SPAWN_ON_CONNECT.len()..];
    assert_eq!(names[0], "TextMsg");
    let names = &names[1..];
    assert_eq!(names[..FIRST_MENU.len()], *FIRST_MENU, "the first command's PreThink");
    let names = &names[FIRST_MENU.len()..];
    assert_eq!(names[..FIRST_UPDATE.len()], *FIRST_UPDATE, "the first player's HUD");
    assert_eq!(names[FIRST_UPDATE.len()], "VGUIMenu", "the second command's team menu");
    // The team menu, and the second `FlashBat 100` 0.2 s after the first (the golden capture of
    // HLDS has two).
    assert!(names[FIRST_UPDATE.len()..]
        .iter()
        .all(|n| n == "ShowMenu" || n == "VGUIMenu" || n == "FlashBat"));
    assert_eq!(names.iter().filter(|n| *n == "FlashBat").count(), 2);

    let msgs = b.messages(0);
    let find = |name: &str| -> Vec<CsUserMsg> {
        msgs.iter()
            .filter(|(n, _)| n == name)
            .map(|(_, m)| m.clone())
            .collect()
    };
    // The scoreboard line of the newcomer: edict index 1, nothing earned, unassigned.
    assert_eq!(
        find("ScoreInfo")[0],
        CsUserMsg::ScoreInfo { player_id: 1, frags: 0, deaths: 0, class_id: 0, team_id: 0 }
    );
    // Once from `Spawn()` of the join (to everyone, so it stayed), once from `InitHUD`.
    let unassigned = CsUserMsg::TeamInfo { player_id: 1, team_name: "UNASSIGNED".into() };
    assert_eq!(find("TeamInfo"), [unassigned.clone(), unassigned]);
    // The terrorists' score first, then the counter-terrorists' (InitHUD), the other way
    // round in UpdateClientData.
    let scores: Vec<String> = find("TeamScore")
        .into_iter()
        .map(|m| match m {
            CsUserMsg::TeamScore { team_name, score: 0 } => team_name,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(scores, ["TERRORIST", "CT", "CT", "TERRORIST"]);
    // The start money, no flash; the server name is the hostname; the dead mark: unassigned
    // players are not alive.
    assert_eq!(find("Money"), [CsUserMsg::Money { amount: 800, flash: false }]);
    assert_eq!(
        find("ServerName"),
        [CsUserMsg::ServerName("Counter-Strike 1.6 Rust".into())]
    );
    // `Spawn()` shows the player alive (it is `DEAD_NO` for the length of `Spawn`), which the
    // engine cleared; `InitHUD` and the first update say dead.
    assert_eq!(find("ScoreAttrib").len(), 2);
    assert_eq!(find("ScoreAttrib")[0], CsUserMsg::ScoreAttrib { player_id: 1, flags: 1 });
    assert_eq!(find("ScoreAttrib")[1], CsUserMsg::ScoreAttrib { player_id: 1, flags: 1 });
    assert_eq!(find("SetFOV"), [CsUserMsg::SetFov(90)]);
    // A joiner's cached hide-HUD value is 0 (vanilla), and the first command's `PreThink` set
    // the flashlight bit before this first update (`flashlight_cvar_hides_the_flashlight_hud`),
    // so the update tells it, once.
    assert_eq!(find("HideWeapon"), [CsUserMsg::HideWeapon(2)]);
    assert_eq!(find("Health"), [CsUserMsg::Health(100)]);
    assert_eq!(find("Battery"), [CsUserMsg::Battery(0)]);
    assert_eq!(
        find("Damage"),
        [CsUserMsg::Damage {
            armor_damage: 0,
            health_damage: 0,
            damage_bits: 0,
            // From inside the player, which waits at a counter-terrorist spawn point: the first
            // one after the terrorist point `Spawn()` of the join recorded as the last (this
            // map lists them alternately: vanilla's search goes on from that entity).
            origin: glam::Vec3::new(200.0, 0.0, 1.0)
        }]
    );

    // A second player joins: it gets the whole welcome, with both players on the scoreboard;
    // the first one hears nothing of it.
    let before = b.messages(0).len();
    let second = b.join();
    b.step(20);
    let names = b.names_from(second, 0);
    let scoreboard = names.iter().filter(|n| *n == "TeamInfo").count();
    assert_eq!(scoreboard, 3);
    assert_eq!(names.iter().filter(|n| *n == "InitHUD").count(), 1);
    assert_eq!(names.iter().filter(|n| *n == "ScoreInfo").count(), 4);
    // The newcomer's marks go to every player (`SetBombIcon` ends in `SetScoreboardAttributes`).
    // The first one hears the `Spawn()` of the second's join (its mark, `TeamInfo`, `ScoreInfo`),
    // the connect line (`#Game_connected`) and the mark of the second's first update.
    assert_eq!(
        b.names_from(0, before),
        ["ScoreAttrib", "TeamInfo", "ScoreInfo", "TextMsg", "ScoreAttrib"]
    );
    assert_eq!(
        b.messages(0).last().unwrap().1,
        CsUserMsg::ScoreAttrib { player_id: 2, flags: 1 }
    );
}

#[test]
fn a_spawn_starts_the_hud_over_without_the_welcome() {
    let mut b = Bench::with_one_client(2, b"");
    b.step(20);
    let before = b.messages(0).len();
    assert!(b.run(|game, eng| game.spawn_player(eng, 0, Team::Ct, 0)));
    b.step(20);
    let names = b.names_from(0, before);
    // The order of `Spawn` (player.cpp): the goggles go off (`NVGToggle`, 6064), `PlayerSpawn`'s
    // empty weapon (`CurWeapon`, 6085), `SetScoreboardAttributes` (6104), `TeamInfo` (6106), the
    // scoreboard line (`ScoreInfo`, 6112), the fade-in that clears a death fade (6132) and
    // `SyncRoundTimer` (6133), then the HUD again, but not InitHUD or the rules' welcome. The stub
    // map has no buy zone, so standing at a spawn point counts as one and the last update of the
    // HUD shows its icon.
    // A player that was not killed keeps what it carries, so `OnSpawnEquip` does not run
    // `RemoveAllItems` (and its `CurWeapon`); the client is told every ammo count again
    // (`m_rgAmmoLast` is -1: one `AmmoX` for each of the 32 ammo slots).
    // What goes to everyone (`TeamInfo`, `ScoreInfo`) reaches the player at the end of the frame
    // of the spawn, after what went to it alone; the first command after it runs the update.
    // `Spawn()` sends `BarTime 0` (`SetProgressBarTime(0)`, 6090) and leaves `m_iClientHealth`
    // alone: the client knows the health, no `Health`.
    let mut expected: Vec<&str> = vec![
        "NVGToggle", "BarTime", "ScoreAttrib", "ScreenFade", "RoundTime", "TeamInfo", "ScoreInfo",
        "ResetHUD", "Money", "StatusIcon", "ScoreAttrib", "RoundTime", "TeamScore", "TeamScore",
        "SetFOV", "Battery", "Damage", "FlashBat", "Train",
    ];
    expected.extend(std::iter::repeat_n("AmmoX", 32));
    expected.push("StatusIcon");
    // The first command that could buy queued `#Hint_press_buy_to_purchase` (a spawn point counts
    // as a buy zone on the stub map); the next command's `PreThink` shows it.
    expected.push("HudTextArgs");
    assert_eq!(names, expected);
    // `SVC_ROOMTYPE` (`room_type`, 0 here) goes first of all, before the user messages.
    let room_types = b.probes[0]
        .messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::RoomType(0)))
        .count();
    // This one: the join's `Spawn()` sent one to the joiner alone, which the engine cleared.
    assert_eq!(room_types, 1);
}

#[test]
fn motd_is_sent_in_chunks() {
    let text: Vec<u8> = (0..150u32).map(|i| b'a' + (i % 26) as u8).collect();
    let mut b = Bench::with_one_client(2, &text);
    b.step(20);
    let motd: Vec<CsUserMsg> = b
        .messages(0)
        .into_iter()
        .filter(|(n, _)| n == "MOTD")
        .map(|(_, m)| m)
        .collect();
    let want: Vec<CsUserMsg> = text
        .chunks(60)
        .enumerate()
        .map(|(i, c)| CsUserMsg::Motd {
            finished: i == 2,
            text: String::from_utf8(c.to_vec()).unwrap(),
        })
        .collect();
    assert_eq!(motd, want);
    assert_eq!(motd.len(), 3);
    // The name of the server comes before the first chunk.
    let names = b.names_from(0, 0);
    let name_at = names.iter().position(|n| n == "ServerName").unwrap();
    let motd_at = names.iter().position(|n| n == "MOTD").unwrap();
    assert!(name_at < motd_at);

    // A text over the limit is cut after the chunk that crosses it.
    let long = vec![b'x'; 4000];
    let mut b = Bench::with_one_client(2, &long);
    b.step(20);
    let chunks: Vec<CsUserMsg> = b
        .messages(0)
        .into_iter()
        .filter(|(n, _)| n == "MOTD")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(chunks.len(), 26);
    let finished: Vec<bool> = chunks
        .iter()
        .map(|m| matches!(m, CsUserMsg::Motd { finished: true, .. }))
        .collect();
    assert_eq!(finished.iter().filter(|f| **f).count(), 1);
    assert!(finished[25]);

    // No file, no message of the day, but still the server name.
    let mut b = Bench::with_one_client(2, b"");
    b.step(20);
    let names = b.names_from(0, 0);
    assert!(!names.iter().any(|n| n == "MOTD"));
    assert!(names.iter().any(|n| n == "ServerName"));
}

#[test]
fn health_and_armor_follow_the_player() {
    let mut b = Bench::with_one_client(2, b"");
    assert!(b.run(|game, eng| game.spawn_player(eng, 0, Team::Ct, 0)));
    // A teammate stays alive, or the death would end the round and the next round would bring
    // the dead player back before its death camera.
    b.join();
    assert!(b.run(|game, eng| game.spawn_player(eng, 1, Team::Ct, 0)));
    b.step(40);
    // Nothing changes, nothing is sent.
    let before = b.messages(0).len();
    b.step(30);
    assert_eq!(b.names_from(0, before), Vec::<String>::new());

    // Armor: a vest of 50 points.
    b.run(|game, _| {
        let p = game.state_mut().players[0].as_mut().unwrap();
        p.armor = 50.0;
        p.kevlar = ArmorType::Vest;
    });
    b.step(20);
    let msgs = b.messages(0);
    assert_eq!(msgs.last().unwrap().1, CsUserMsg::Battery(50));

    // A hit of 20 from the world: armor takes some, the health the rest; the HUD is told once.
    let before = b.messages(0).len();
    b.run(|game, eng| game.damage_player(eng, 0, None, 20.0, 1 << 1));
    b.step(20);
    let after: Vec<(String, CsUserMsg)> = b.messages(0).into_iter().skip(before).collect();
    let names: Vec<&str> = after.iter().map(|(n, _)| n.as_str()).collect();
    // The second `Damage` is the original's: the kinds the HUD was told (a bullet) differ from
    // those kept (a bullet is not a time-based kind), so it is sent once more, empty.
    // The spawn is over two seconds old: `m_flgeigerDelay` is due, and the Geiger counter tells
    // the client its starting range (1000 / 4 against the 1000 it was never told).
    assert_eq!(names, ["Health", "Battery", "Damage", "Damage", "Geiger"]);
    // `ARMOR_RATIO` 0.5, `ARMOR_BONUS` 0.5: 10 points to the health (90 left), 5 to the armor.
    // The message's armor byte is always 0: nothing sets `dmg_save` in the original.
    assert_eq!(after[0].1, CsUserMsg::Health(90));
    assert_eq!(after[1].1, CsUserMsg::Battery(45));
    match &after[2].1 {
        CsUserMsg::Damage {
            armor_damage,
            health_damage,
            damage_bits,
            origin,
        } => {
            assert_eq!((*armor_damage, *health_damage), (0, 10));
            // A bullet has no art on the HUD.
            assert_eq!(*damage_bits, 0);
            // From inside the player (no inflictor): its own position.
            assert!(origin.x.abs() > 0.0 || origin.y.abs() > 0.0 || origin.z.abs() > 0.0);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        after[3].1,
        CsUserMsg::Damage { armor_damage: 0, health_damage: 0, damage_bits: 0, .. }
    ));
    let before = b.messages(0).len();
    b.step(30);
    assert_eq!(b.names_from(0, before), Vec::<String>::new());

    // Death: the client is told 0 (by `Killed`, and by the update, which sees a health of -410
    // against the 0 `Killed` cached, as the original does), and 1 when `StartObserver` gives the
    // observer a health of 1.
    b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, 0));
    b.step(20);
    // Hiding the weapons and the health is `StartObserver`'s: it comes with the death camera,
    // which `PlayerDeathThink` starts from the dead player's commands, 3 seconds after the death
    // (and sets the health to 1, which the client is told too).
    for _ in 0..800 {
        b.run(|game, eng| {
            let cmd = GoldSrcUserCmd {
                msec: 10,
                ..Default::default()
            };
            run_cmd(game, eng, 0, &cmd, 0);
        });
    }
    let health: Vec<CsUserMsg> = b
        .messages(0)
        .into_iter()
        .skip(before)
        .filter(|(n, _)| n == "Health")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(
        health,
        [CsUserMsg::Health(0), CsUserMsg::Health(0), CsUserMsg::Health(1)]
    );
    // The flashlight bit of `PreThink` (2) went first; `StartObserver` then sets the hidden
    // elements to the weapons and the health (1 | 8) and the next command's `PreThink` puts the
    // flashlight bit back (11).
    // (From the start of the messages: the flashlight bit was told in the first update, which
    // follows the first command's `PreThink`, before the window of `before`.)
    // Re-baselined for `SV_RunCmd` (ledger CARRY 53): the engine's 0-ms command runs `PreThink`
    // too, so `PlayerDeathThink` starts the camera on that command. As in vanilla, `PreThink`
    // sets the flashlight bit again before `UpdateClientData` sends the hidden elements, so the
    // client is never told the intermediate 9.
    let hide: Vec<CsUserMsg> = b
        .messages(0)
        .into_iter()
        .filter(|(n, _)| n == "HideWeapon")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(hide, [CsUserMsg::HideWeapon(2), CsUserMsg::HideWeapon(11)]);
    // A player killed before gets the HUD elements back on its next spawn, and is told.
    let before_respawn = b.messages(0).len();
    assert!(b.run(|game, eng| game.respawn_player(eng, 0)));
    b.step(20);
    let hide: Vec<CsUserMsg> = b
        .messages(0)
        .into_iter()
        .skip(before_respawn)
        .filter(|(n, _)| n == "HideWeapon")
        .map(|(_, m)| m)
        .collect();
    // (the flashlight bit stays: `mp_flashlight` is 0)
    assert_eq!(hide, [CsUserMsg::HideWeapon(2)]);
    let before = b.messages(0).len();
    b.step(30);
    assert!(!b.names_from(0, before).iter().any(|n| n == "Health"));
}

#[test]
fn full_server_init_hud_fits() {
    let long_motd = vec![b'm'; 4000];
    let mut b = Bench::new(32, &long_motd);
    for n in 0..32 {
        assert_eq!(b.join(), n);
        // Every player is on the scoreboard with a score to tell.
        // Alive on a team (so `Radar` is sent), with a score to tell.
        b.run(move |game, eng| {
            let team = if n % 2 == 0 { Team::Ct } else { Team::Terrorist };
            assert!(game.spawn_player(eng, n as u8, team, 0));
            let p = game.state_mut().players[n].as_mut().unwrap();
            p.frags = n as i32;
            p.deaths = 3;
        });
    }
    b.step(20);

    // The 32nd joiner got the whole welcome: all 32 on the scoreboard, in one go.
    let last = b.messages(31);
    // (and its own spawn's `TeamInfo` to everyone, which `Spawn` sends before the first update)
    let teams = last.iter().filter(|(n, _)| n == "TeamInfo").count();
    assert_eq!(teams, 34);
    let motd = last.iter().filter(|(n, _)| n == "MOTD").count();
    assert_eq!(motd, 26);
    // The 31 players alive when it came in are on its radar.
    let radar = last.iter().filter(|(n, _)| n == "Radar").count();
    assert_eq!(radar, 31);
    // What InitHUD put on its reliable stream, as the bytes of the messages on the wire (the id,
    // the length byte of a variable message, the payload): well inside the reliable buffer of
    // a netchan, and the engine neither dropped nor kicked anyone.
    let reg = CsGame::new().usermsgs();
    let welcome: usize = last
        .iter()
        .map(|(name, msg)| {
            let size = reg.get_by_id(reg.id_for_name(name).unwrap()).unwrap().size;
            let payload = match msg {
                CsUserMsg::Unknown { payload, .. } => payload.len(),
                CsUserMsg::Motd { text, .. } => 1 + text.len() + 1,
                other => encode_cs_usermsg(other).1.len(),
            };
            1 + usize::from(size < 0) + payload
        })
        .sum();
    eprintln!("the 32nd joiner's user messages: {welcome} bytes");
    assert!(welcome < 8 * 1024, "{welcome} bytes");
    assert!(welcome < cs_engine::MAX_SIGNON_BUFFER / 8);
    assert_eq!(b.server.net_stats().outbox_dropped, 0);
    for slot in 0..32 {
        assert_eq!(b.server.client_state(slot), ClientState::Active, "slot {slot}");
    }
}

/// A command with time in it from the player of `slot`, run as the game.
fn command_of(b: &mut Bench, slot: u8) {
    b.run(move |game, eng| {
        let cmd = GoldSrcUserCmd {
            msec: 10,
            ..Default::default()
        };
        run_cmd(game, eng, slot, &cmd, 0);
    });
    b.step(2);
}

#[test]
fn flashlight_cvar_hides_the_flashlight_hud() {
    // `PreThink` sets `HIDEHUD_FLASHLIGHT` (bit 1) unless `FAllowFlashlight()` (`mp_flashlight`,
    // default 0), before `UpdateClientData` tells the client (player.cpp:4825, read 2026-11-22).
    let mut b = Bench::with_one_client(2, b"");
    b.step(20);
    assert!(b.run(|game, eng| game.spawn_player(eng, 0, Team::Ct, 0)));
    b.step(5);
    let hide = |b: &Bench, from: usize| -> Vec<CsUserMsg> {
        b.messages(0)
            .into_iter()
            .skip(from)
            .filter(|(n, _)| n == "HideWeapon")
            .map(|(_, m)| m)
            .collect()
    };
    // The bit was set by the first command's `PreThink` (the sign-on's placeholder move) and
    // told in the first update, so it is in the messages from the start; a later command does
    // not tell it again.
    let before = b.messages(0).len();
    command_of(&mut b, 0);
    assert_eq!(hide(&b, 0), [CsUserMsg::HideWeapon(2)]);
    assert_eq!(hide(&b, before), []);
    // The cvar allows it: the bit goes, the client is told once.
    b.run(|_, eng| eng.set_cvar("mp_flashlight", "1"));
    let before = b.messages(0).len();
    command_of(&mut b, 0);
    command_of(&mut b, 0);
    assert_eq!(hide(&b, before), [CsUserMsg::HideWeapon(0)]);
    b.run(|_, eng| eng.set_cvar("mp_flashlight", "0"));
    let before = b.messages(0).len();
    command_of(&mut b, 0);
    assert_eq!(hide(&b, before), [CsUserMsg::HideWeapon(2)]);
}

#[test]
fn every_hit_sends_the_damage_message_with_the_integer_health_lost() {
    // `CBasePlayer::TakeDamage` ends with `m_bitsHUDDamage = -1`, so the next update sends
    // `Damage` even when the kind of damage is the one it last sent; the health byte is
    // `int(pev->dmg_take)`.
    let mut b = Bench::with_one_client(2, b"");
    b.step(20);
    assert!(b.run(|game, eng| game.spawn_player(eng, 0, Team::Ct, 0)));
    b.step(10);
    let damage = |b: &Bench, from: usize| -> Vec<CsUserMsg> {
        b.messages(0)
            .into_iter()
            .skip(from)
            .filter(|(n, _)| n == "Damage")
            .map(|(_, m)| m)
            .collect()
    };
    let before = b.messages(0).len();
    b.run(|game, eng| game.damage_player(eng, 0, None, 33.7, 1 << 1));
    b.step(10);
    let first = damage(&b, before);
    assert!(!first.is_empty());
    assert!(matches!(first[0], CsUserMsg::Damage { armor_damage: 0, health_damage: 33, damage_bits: 0, .. }));
    // Quiet again: nothing more is sent while nothing is hit.
    let before = b.messages(0).len();
    b.step(10);
    assert!(damage(&b, before).is_empty());
    // A hit of no kind that costs no whole point (damage 0.5, `dmg_take` stays 0, the bits are
    // the ones the client was last told) still reaches the client: `m_bitsHUDDamage = -1`.
    let before = b.messages(0).len();
    b.run(|game, eng| game.damage_player(eng, 0, None, 0.5, 0));
    b.step(10);
    let second = damage(&b, before);
    assert!(!second.is_empty());
    assert!(second.iter().all(|m| matches!(m, CsUserMsg::Damage { health_damage: 0, .. })));
}

/// One command as the engine runs it: `run_usercmd`, then `post_usercmd`.
fn run_cmd(game: &mut CsGame, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd, seed: u32) {
    game.run_usercmd(eng, slot, cmd, seed);
    game.post_usercmd(eng, slot, cmd);
}
