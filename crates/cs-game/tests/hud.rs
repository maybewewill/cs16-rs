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
        let map = ServerMap::stub_with_entities(
            "test",
            vec![
                entity(&[("classname", "worldspawn")]),
                entity(&[
                    ("classname", "info_player_start"),
                    ("origin", "100 0 0"),
                    ("angles", "0 90 0"),
                ]),
                entity(&[
                    ("classname", "info_player_deathmatch"),
                    ("origin", "-100 0 0"),
                    ("angles", "0 90 0"),
                ]),
            ],
        );
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
    "HideWeapon",
    "SetFOV",
    "Health",
    "Battery",
    "Damage",
    "Train",
];

#[test]
fn init_hud_sequence_as_regamedll() {
    let mut b = Bench::with_one_client(2, b"");
    b.step(20);
    let names = b.names_from(0, 0);
    assert_eq!(names, FIRST_UPDATE, "the first player's HUD, once");

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
    assert_eq!(
        find("TeamInfo"),
        [CsUserMsg::TeamInfo { player_id: 1, team_name: "UNASSIGNED".into() }]
    );
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
    assert_eq!(find("ScoreAttrib")[0], CsUserMsg::ScoreAttrib { player_id: 1, flags: 1 });
    assert_eq!(find("SetFOV"), [CsUserMsg::SetFov(90)]);
    assert_eq!(find("HideWeapon"), [CsUserMsg::HideWeapon(0)]);
    assert_eq!(find("Health"), [CsUserMsg::Health(100)]);
    assert_eq!(find("Battery"), [CsUserMsg::Battery(0)]);
    assert_eq!(
        find("Damage"),
        [CsUserMsg::Damage {
            armor_damage: 0,
            health_damage: 0,
            damage_bits: 0,
            origin: glam::Vec3::ZERO
        }]
    );

    // A second player joins: it gets the whole welcome, with both players on the scoreboard;
    // the first one hears nothing of it.
    let before = b.messages(0).len();
    let second = b.join();
    b.step(20);
    let names = b.names_from(second, 0);
    let scoreboard = names.iter().filter(|n| *n == "TeamInfo").count();
    assert_eq!(scoreboard, 2);
    assert_eq!(names.iter().filter(|n| *n == "InitHUD").count(), 1);
    assert_eq!(names.iter().filter(|n| *n == "ScoreInfo").count(), 3);
    // The newcomer's marks go to every player (`SetBombIcon` ends in `SetScoreboardAttributes`).
    assert_eq!(b.names_from(0, before), ["ScoreAttrib"]);
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
    // `PlayerSpawn`'s empty weapon after the scoreboard line, then the HUD again, but not
    // InitHUD or the rules' welcome.
    assert_eq!(
        names,
        [
            "ScoreInfo", "CurWeapon", "ResetHUD", "Money", "StatusIcon", "ScoreAttrib",
            "RoundTime", "TeamScore", "TeamScore", "HideWeapon", "SetFOV", "Health", "Battery",
            "Damage", "Train",
        ]
    );
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
    b.step(20);
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
    assert_eq!(names, ["Health", "Battery", "Damage", "Damage"]);
    // `ARMOR_RATIO` 0.5, `ARMOR_BONUS` 0.5: 10 points to the health (90 left), 5 to the armor.
    assert_eq!(after[0].1, CsUserMsg::Health(90));
    assert_eq!(after[1].1, CsUserMsg::Battery(45));
    match &after[2].1 {
        CsUserMsg::Damage {
            armor_damage,
            health_damage,
            damage_bits,
            origin,
        } => {
            assert_eq!((*armor_damage, *health_damage), (10, 10));
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
    // against the 0 `Killed` cached, as the original does), and then nothing more.
    b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, 0));
    b.step(20);
    let health: Vec<CsUserMsg> = b
        .messages(0)
        .into_iter()
        .skip(before)
        .filter(|(n, _)| n == "Health")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(health, [CsUserMsg::Health(0), CsUserMsg::Health(0)]);
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
        b.run(move |game, _| {
            let p = game.state_mut().players[n].as_mut().unwrap();
            p.frags = n as i32;
            p.deaths = 3;
        });
    }
    b.step(20);

    // The 32nd joiner got the whole welcome: all 32 on the scoreboard, in one go.
    let last = b.messages(31);
    let teams = last.iter().filter(|(n, _)| n == "TeamInfo").count();
    assert_eq!(teams, 32);
    let motd = last.iter().filter(|(n, _)| n == "MOTD").count();
    assert_eq!(motd, 26);
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
