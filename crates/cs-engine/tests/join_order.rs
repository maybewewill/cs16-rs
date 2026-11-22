//! What the server sends a client that signs on, and in which order, as ReHLDS does it:
//! `SV_Spawn_f` and `SV_WriteSpawn` (the signon buffer, then the time, the userinfos, the light
//! styles, the view, the client data, `svc_signonnum`, the voice codec), what `ClientPutInServer`
//! sends to the joiner alone being thrown away, `MSG_ALL` reaching the clients at the end of the
//! frame, `fixangle` and `svc_choke` in the client data, and `SV_CleanupEnts`.

mod common;

use common::{CLIENTDATA_PRINT, Harness, JOINER_PRINT, SHOUT, TestGame, print_bytes};
use cs_engine::{ClientState, MsgDest};
use cs_probe::ProbeState;
use cs_proto48::messages::{SvcMessage, svc};

fn joined_with(game: TestGame) -> (Harness, cs_probe::ProbeClient<cs_transport::LoopbackEndpoint>) {
    let mut h = Harness::with_game(4, game);
    let mut p = h.add_probe("A");
    h.step(&mut [&mut p], 200);
    assert_eq!(p.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    (h, p)
}

/// The opcodes of the `spawn` answer: from the spawn baselines to `svc_voiceinit`.
fn spawn_answer(p: &cs_probe::ProbeClient<cs_transport::LoopbackEndpoint>) -> Vec<u8> {
    let log = p.opcode_log();
    let start = log.iter().position(|&op| op == svc::SPAWNBASELINE).expect("baselines");
    let end = log.iter().position(|&op| op == svc::VOICEINIT).expect("voiceinit");
    log[start..=end].to_vec()
}

#[test]
fn the_spawn_answer_is_that_of_sv_write_spawn() {
    let game = TestGame {
        activate_sends: vec![(MsgDest::Init, print_bytes("init-message"))],
        fixangle_on_put_in_server: 1,
        ..TestGame::default()
    };
    let (_h, p) = joined_with(game);
    // The init buffer comes first, then the baselines (the signon buffer of `SV_Spawn_f`).
    let messages: Vec<&SvcMessage> = p.messages().iter().collect();
    let init = messages
        .iter()
        .position(|m| **m == SvcMessage::Print("init-message".to_string()))
        .expect("the init message");
    let baselines = messages
        .iter()
        .position(|m| matches!(m, SvcMessage::SpawnBaseline { .. }))
        .expect("baselines");
    assert!(init < baselines);
    // Then `SV_WriteSpawn`: time, userinfos, the 64 light styles, the view of `v_angle`, the
    // `fixangle` the game set (a second `svc_setangle`, from the client data), the client data,
    // `svc_signonnum 1` and the voice codec.
    let answer = spawn_answer(&p);
    let mut kinds: Vec<u8> = Vec::new();
    for op in answer {
        if kinds.last() != Some(&op) {
            kinds.push(op);
        }
    }
    assert_eq!(
        kinds,
        [
            svc::SPAWNBASELINE,
            svc::TIME,
            svc::UPDATEUSERINFO,
            svc::LIGHTSTYLE,
            svc::SETANGLE,
            svc::CLIENTDATA,
            svc::SIGNONNUM,
            svc::VOICEINIT,
        ]
    );
}

#[test]
fn a_forced_view_goes_out_once_with_the_client_data() {
    let game = TestGame {
        fixangle_on_put_in_server: 1,
        ..TestGame::default()
    };
    let (mut h, mut p) = joined_with(game);
    // The `spawn` answer carries the two views: `v_angle` (`SV_WriteSpawn`) and the forced one
    // (`SV_WriteClientdataToMessage`: `angles`, roll included). Nothing more afterwards: the
    // engine cleared `fixangle`.
    let views: Vec<_> = p
        .messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::SetAngle(_)))
        .collect();
    assert_eq!(views.len(), 2, "{views:?}");
    let SvcMessage::SetAngle(forced) = views[1] else { unreachable!() };
    assert!((forced.x - 10.0).abs() < 0.01 && (forced.y - 90.0).abs() < 0.01, "{forced:?}");
    assert!((forced.z - 5.0).abs() < 0.01, "the roll of `angles` is sent: {forced:?}");
    let id = h.server.edicts().client(0);
    assert_eq!(h.server.edicts().get(id).unwrap().vars.fixangle, 0);
    p.clear_messages();
    h.step(&mut [&mut p], 30);
    assert!(!p.messages().iter().any(|m| matches!(m, SvcMessage::SetAngle(_))));
}

#[test]
fn a_turn_of_two_is_sent_as_addangle_and_zeroed() {
    let (mut h, mut p) = joined_with(TestGame::default());
    p.clear_messages();
    h.server.game_mut().act(|eng| {
        let id = eng.client_edict(0);
        let v = eng.ent_mut(id).unwrap();
        v.fixangle = 2;
        v.avelocity = glam::Vec3::new(0.0, 45.0, 0.0);
    });
    h.step(&mut [&mut p], 30);
    let turns: Vec<_> = p
        .messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::AddAngle(_)))
        .collect();
    assert_eq!(turns.len(), 1, "{turns:?}");
    let SvcMessage::AddAngle(yaw) = turns[0] else { unreachable!() };
    assert!((yaw - 45.0).abs() < 0.01, "{yaw}");
    let id = h.server.edicts().client(0);
    let vars = &h.server.edicts().get(id).unwrap().vars;
    assert_eq!((vars.fixangle, vars.avelocity.y), (0, 0.0));
}

#[test]
fn what_the_game_sends_the_joiner_in_put_in_server_is_thrown_away() {
    let game = TestGame {
        print_to_joiner_on_put_in_server: true,
        shout_on_put_in_server: true,
        ..TestGame::default()
    };
    let (_h, p) = joined_with(game);
    let count = |text: &str| {
        p.messages()
            .iter()
            .filter(|m| **m == SvcMessage::Print(text.to_string()))
            .count()
    };
    // `SZ_Clear(&host_client->netchan.message)` after `pfnClientPutInServer`: the message to the
    // joiner alone is gone; the broadcast is `sv.reliable_datagram`, which stays.
    assert_eq!(count(JOINER_PRINT), 0);
    assert_eq!(count(SHOUT), 1);
}

/// `SV_WriteSpawn` puts the answer in `netchan.message` and only then does the game's
/// `UpdateClientData` run, so what it sends the client alone follows the answer.
#[test]
fn what_update_client_data_sends_follows_the_spawn_answer() {
    let game = TestGame { print_in_update_client_data: true, ..TestGame::default() };
    let (_h, p) = joined_with(game);
    // Not inside the answer (baselines to the voice codec), but right behind it.
    assert!(!spawn_answer(&p).contains(&svc::PRINT), "no print inside the answer");
    let log = p.opcode_log();
    let voiceinit = log.iter().position(|&op| op == svc::VOICEINIT).expect("voiceinit");
    assert_eq!(log[voiceinit + 1], svc::PRINT, "the print follows the answer");
    assert!(p.messages().iter().any(|m| *m == SvcMessage::Print(CLIENTDATA_PRINT.to_string())));
}

#[test]
fn the_broadcast_of_a_frame_follows_what_went_to_single_clients() {
    let (mut h, mut p) = joined_with(TestGame::default());
    p.clear_messages();
    h.server.game_mut().act(|eng| {
        eng.send(MsgDest::All, &print_bytes("to all"));
        eng.send(MsgDest::One(0), &print_bytes("to one"));
    });
    h.step(&mut [&mut p], 5);
    let prints: Vec<String> = p
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Print(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    // `MSG_ALL` goes to `sv.reliable_datagram`, `MSG_ONE` straight to the client's buffer:
    // `SV_UpdateToReliableMessages` appends the first at the end of the frame, after the second.
    assert_eq!(prints, ["to one", "to all"]);
}

#[test]
fn one_frame_effects_are_cleared_after_the_updates() {
    // `SV_CleanupEnts`: `EF_NOINTERP` and `EF_MUZZLEFLASH` live for one frame.
    let game = TestGame {
        effects_on_put_in_server: 32 | 2 | 128,
        ..TestGame::default()
    };
    let (h, _p) = joined_with(game);
    let id = h.server.edicts().client(0);
    assert_eq!(h.server.edicts().get(id).unwrap().vars.effects, 128);
}

#[test]
fn the_physics_info_goes_to_the_client_in_its_client_data() {
    let game = TestGame {
        physinfo_on_put_in_server: true,
        ..TestGame::default()
    };
    let (_h, p) = joined_with(game);
    // `SET_PHYSICS_KEY_VALUE` in `client_put_in_server`, `ENGINE_GETPHYSINFO` in the game's client
    // data: the string reaches the client (the golden capture of HLDS shows its `slj` and `hl`
    // keys).
    let data = p.client_data().expect("client data");
    assert_eq!(data.physinfo, r"\slj\0");
}

#[test]
fn private_userinfo_keys_are_not_sent() {
    let game = TestGame::default();
    let mut h = Harness::with_game(4, game);
    let mut p = h.add_probe("A");
    p.set_userinfo("_vgui_menus", "1");
    p.set_userinfo("model", "gordon");
    h.step(&mut [&mut p], 200);
    assert_eq!(p.state(), &ProbeState::InGame);
    let info = p.userinfos().get(&0).expect("own userinfo");
    assert!(info.contains("\\model\\gordon"), "{info}");
    assert!(!info.contains("_vgui_menus"), "{info}");
}
