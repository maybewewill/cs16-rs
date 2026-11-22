//! Fake clients (bots): `EngineApi::create_fake_client` and `EngineApi::run_player_move`, and
//! what the engine skips for them (ReHLDS `pr_cmds.cpp` `CreateFakeClient_internal` and
//! `PF_RunPlayerMove_I`, `sv_user.cpp` `SV_RunCmd`, `sv_main.cpp`, read 2026-11-23).

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use common::{FRAME, Harness, TestGame, print_bytes, send_chunked};
use cs_engine::event_flags::FEV_GLOBAL;
use cs_engine::{ClientState, ConsoleOutcome, MsgDest, ServerMap, consts};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::info::InfoString;
use cs_proto48::messages::SvcMessage;
use cs_proto48::query::{A2S_INFO, A2S_INFO_PAYLOAD, A2sInfoReply, decode_a2s_info_reply, encode_a2s_info_request};
use cs_transport::{LoopbackEndpoint, PacketTransport, PeerAddr};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

const AK47: &str = "events/ak47.sc";

fn game() -> TestGame {
    TestGame {
        precache_events: vec![AK47.to_string()],
        ..TestGame::default()
    }
}

/// Adds a probe and runs the harness, pumping `others` too, until the probe is in the game.
fn joined(h: &mut Harness, name: &str, others: &mut [&mut Probe]) -> Probe {
    let mut probe = h.add_probe(name);
    for _ in 0..300 {
        let mut all: Vec<&mut Probe> = others.iter_mut().map(|p| &mut **p).collect();
        all.push(&mut probe);
        h.step(&mut all, 1);
        if probe.state() == &ProbeState::InGame {
            return probe;
        }
    }
    panic!("{name} did not get in the game: {:?}", probe.state());
}

/// What the game does for a bot it made: `ClientPutInServer` puts it in the world.
fn put_in_world(eng: &mut cs_engine::EngineApi<'_>, bot: u8) {
    let id = eng.client_edict(bot);
    let v = eng.ent_mut(id).expect("the bot's edict");
    v.origin = Vec3::new(100.0 * f32::from(bot + 1), 0.0, 0.0);
    v.modelindex = 2;
    v.health = 100.0;
    v.solid = consts::SOLID_SLIDEBOX;
    v.movetype = consts::MOVETYPE_WALK;
}

/// Makes a bot called `name` in the next frame and returns its slot.
fn spawn_bot(h: &mut Harness, name: &str) -> u8 {
    let made = Rc::new(Cell::new(None));
    let (cell, name) = (made.clone(), name.to_string());
    h.server.game_mut().act(move |eng| {
        let bot = eng.create_fake_client(&name).expect("a free slot");
        put_in_world(eng, bot);
        cell.set(Some(bot));
    });
    h.step(&mut [], 1);
    made.get().expect("the game ran")
}

fn origin_of(h: &Harness, slot: u8) -> Vec3 {
    let id = h.server.edicts().client(slot);
    h.server.edicts().get(id).expect("edict").vars.origin
}

fn console(h: &mut Harness, line: &str) -> String {
    match h.server.console_command(line) {
        ConsoleOutcome::Output(text) => text,
        other => panic!("{line:?}: expected Output, got {other:?}"),
    }
}

#[test]
fn a_fake_client_takes_a_slot_without_a_peer() {
    let mut h = Harness::with_game(3, game());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let (log, made) = (seen.clone(), Rc::new(Cell::new(None)));
    let cell = made.clone();
    h.server.game_mut().act(move |eng| {
        let bot = eng.create_fake_client("Bot").expect("a free slot");
        cell.set(Some(bot));
        put_in_world(eng, bot);
        let mut log = log.borrow_mut();
        for key in ["name", "model", "topcolor", "bottomcolor", "*bot"] {
            log.push(eng.client_key(bot, key).map(str::to_string));
        }
        log.push(Some(eng.is_fake_client(bot).to_string()));
        log.push(Some(eng.is_client_active(bot).to_string()));
        // A second bot of the same name is made unique as a connecting client's is.
        let second = eng.create_fake_client("Bot").expect("a free slot");
        log.push(eng.client_key(second, "name").map(str::to_string));
    });
    h.step(&mut [], 1);
    let bot = made.get().unwrap();
    assert_eq!(bot, 0);
    let strings = |values: &[&str]| values.iter().map(|s| Some(s.to_string())).collect::<Vec<_>>();
    assert_eq!(
        *seen.borrow(),
        strings(&["Bot", "gordon", "1", "1", "1", "true", "true", "(1)Bot"])
    );

    // The slot is in the game with no address: no `ClientConnect`, no put in by the engine.
    assert_eq!(h.server.client_state(bot), ClientState::Active);
    assert_eq!(h.server.client_count(), 2);
    assert_eq!(h.server.fake_client_count(), 2);
    assert!(h.server.is_fake_client(bot));
    assert_eq!(h.server.client_userid(0), Some(1));
    assert_eq!(h.server.client_userid(1), Some(2));
    assert_eq!(h.server.game().client_connect_calls, 0);
    assert_eq!(h.server.game().client_put_in_server_calls, 0);
    let edict = h.server.edicts().get(h.server.edicts().client(bot)).expect("edict");
    assert_eq!(edict.vars.flags, consts::FL_FAKECLIENT | consts::FL_CLIENT);

    // A real client takes the next slot and sees the bot: its userinfo and its entity.
    let mut a = joined(&mut h, "Alice", &mut []);
    h.step(&mut [&mut a], 20);
    assert_eq!(a.player_slot(), Some(2));
    let info = InfoString::parse(&a.userinfos()[&bot]);
    assert_eq!(info.get("name"), Some("Bot"));
    assert_eq!(info.get("model"), Some("gordon"));
    assert!(a.entities().contains_key(&(u16::from(bot) + 1)));

    // The server is full: the next bot is refused and so is a connecting client.
    let refused = Rc::new(Cell::new(true));
    let r = refused.clone();
    h.server.game_mut().act(move |eng| r.set(eng.create_fake_client("Third").is_none()));
    h.step(&mut [&mut a], 1);
    assert!(refused.get());
    let mut late = h.add_probe("Late");
    h.step(&mut [&mut a, &mut late], 100);
    assert!(matches!(late.state(), ProbeState::Rejected(_)), "{:?}", late.state());

    // A kick releases the bot as any client's: the game hears `client_disconnect`.
    h.server.kick(bot, "bye");
    assert_eq!(h.server.client_state(bot), ClientState::Free);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    assert_eq!(h.server.fake_client_count(), 1);
    h.step(&mut [&mut a], 5);
    assert_eq!(a.userinfos()[&bot], "");
    assert!(!a.entities().contains_key(&(u16::from(bot) + 1)));
    // The slot is free again for a new bot.
    assert_eq!(spawn_bot(&mut h, "Again"), bot);
}

#[test]
fn run_player_move_moves_the_player() {
    let mut h = Harness::with_game(2, game());
    let bot = spawn_bot(&mut h, "Bot");
    let start = origin_of(&h, bot);
    let angles = Vec3::new(10.0, 20.0, 0.0);
    h.server
        .game_mut()
        .act(move |eng| eng.run_player_move(bot, angles, 250.0, -50.0, 3.0, 0x21, 7, 10));
    h.step(&mut [], 1);

    let game = h.server.game();
    assert_eq!(game.run_usercmd_calls, 1);
    let cmd = &game.usercmd_cmds[0];
    assert_eq!(
        (cmd.viewangles, cmd.forwardmove, cmd.sidemove, cmd.upmove),
        (angles, 250.0, -50.0, 3.0)
    );
    assert_eq!((cmd.buttons, cmd.impulse, cmd.msec), (0x21, 7, 10));
    // `SV_RunCmd(&cmd, 0, FALSE)`: no random seed, and every other field zero.
    assert_eq!(game.random_seeds, vec![0]);
    assert_eq!((cmd.lerp_msec, cmd.lightlevel), (0, 0));
    // `svtimebase = host_frametime + sv.time - msec / 1000`, then the command's own time added:
    // the end of the frame.
    let sv_time = *game.start_frame_times.last().unwrap();
    assert!((game.usercmd_times[0] - (sv_time + FRAME)).abs() < 1e-9, "{:?}", game.usercmd_times);
    assert!((f64::from(game.usercmd_frametimes[0]) - 0.010).abs() < 1e-6);
    assert_eq!(game.post_usercmd_marks.len(), 1);
    // The game's `run_usercmd` moved the player: 250 units a second for 10 ms.
    assert!((origin_of(&h, bot).x - start.x - 2.5).abs() < 1e-4);
    // `lastcmd` is the command and the bot is still where it was put in the game.
    assert_eq!(h.server.client_state(bot), ClientState::Active);

    // A long command is split in halves as `SV_RunCmd` splits it, the impulse kept in the first;
    // commands queued together run in the order queued.
    h.server.game_mut().act(move |eng| {
        eng.run_player_move(bot, Vec3::ZERO, 1.0, 0.0, 0.0, 0, 9, 200);
        eng.run_player_move(bot, Vec3::ZERO, 2.0, 0.0, 0.0, 0, 0, 20);
    });
    h.step(&mut [], 1);
    let game = h.server.game();
    let tail: Vec<(u8, u8, f32)> = game.usercmd_cmds[1..]
        .iter()
        .map(|c| (c.msec, c.impulse, c.forwardmove))
        .collect();
    assert_eq!(
        tail,
        vec![(50, 9, 1.0), (50, 0, 1.0), (50, 0, 1.0), (50, 0, 1.0), (20, 0, 2.0)]
    );
    assert_eq!(game.post_usercmd_marks.len(), 6);

    // Refused: a real client's slot, a free slot, a slot out of range, numbers that are not
    // finite. (The probe sends its own commands, none with these values.)
    let mut a = joined(&mut h, "Alice", &mut []);
    let real = a.player_slot().unwrap();
    let before = h.server.game().usercmd_cmds.len();
    h.server.game_mut().act(move |eng| {
        eng.run_player_move(real, Vec3::ZERO, 321.0, 0.0, 0.0, 0, 0, 33);
        eng.run_player_move(31, Vec3::ZERO, 321.0, 0.0, 0.0, 0, 0, 33);
        eng.run_player_move(200, Vec3::ZERO, 321.0, 0.0, 0.0, 0, 0, 33);
        eng.run_player_move(bot, Vec3::ZERO, f32::NAN, 0.0, 0.0, 0, 0, 33);
        eng.run_player_move(bot, Vec3::new(0.0, f32::INFINITY, 0.0), 1.0, 0.0, 0.0, 0, 0, 33);
    });
    h.step(&mut [&mut a], 3);
    assert!(h.server.game().usercmd_cmds[before..].iter().all(|c| c.msec != 33));
}

#[test]
fn fake_clients_are_never_sent_or_timed_out() {
    let mut h = Harness::with_game(3, game());
    let bot = spawn_bot(&mut h, "Bot");
    // Past the timeout a client that says nothing is dropped; the bot says nothing for ever.
    let mut sent = 0;
    for _ in 0..(75.0 / FRAME) as usize {
        sent += h.step_tapped(&mut []).sent.len();
    }
    assert_eq!(sent, 0, "nothing is built or sent for a fake client");
    assert_eq!(h.server.client_state(bot), ClientState::Active);
    assert_eq!(h.server.client_frame_count(bot), Some(0));
    assert_eq!(h.server.net_stats().send_failures, 0);

    // What the game writes to it is dropped, whatever the destination, and never grows a
    // backlog that would get it kicked as a real client's would.
    let mut a = joined(&mut h, "Alice", &mut []);
    let hello = print_bytes("hello all");
    for _ in 0..10 {
        let hello = hello.clone();
        h.server.game_mut().act(move |eng| {
            send_chunked(eng, MsgDest::One(bot), &vec![1; 100_000]);
            send_chunked(eng, MsgDest::OneUnreliable(bot), &vec![1; 1_000]);
            eng.send(MsgDest::All, &hello);
            eng.send(MsgDest::Broadcast, &[1; 100]);
            eng.send(MsgDest::Pvs(Vec3::ZERO), &[1; 100]);
        });
        h.step(&mut [&mut a], 1);
    }
    h.step(&mut [&mut a], 20);
    assert_eq!(h.server.client_reliable_backlog(bot), 0);
    assert_eq!(h.server.client_state(bot), ClientState::Active);
    assert_eq!(h.server.client_frame_count(bot), Some(0));
    // The real client still got what went to everyone.
    let prints = a
        .messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::Print(t) if t == "hello all"))
        .count();
    assert_eq!(prints, 10);
    assert_eq!(h.server.client_state(a.player_slot().unwrap()), ClientState::Active);
}

#[test]
fn fake_clients_skip_unlag_and_events() {
    let mut h = Harness::with_game(3, game());
    let mut a = h.add_probe("Alice");
    a.set_userinfo("cl_lc", "1");
    h.step(&mut [&mut a], 200);
    assert_eq!(a.state(), &ProbeState::InGame);
    let real = a.player_slot().unwrap();
    let bot = spawn_bot(&mut h, "Bot");

    // `SV_IsLagCompensationActive` of a real client with `cl_lw` and `cl_lc`; a bot with the same
    // userinfo takes no part (`SV_SetupMove` and `SV_RestoreMove` are skipped for it).
    assert!(h.server.lag_compensation_active(real));
    h.server.game_mut().act(move |eng| {
        eng.set_client_key(bot, "cl_lw", "1");
        eng.set_client_key(bot, "cl_lc", "1");
    });
    h.step(&mut [&mut a], 1);
    assert!(!h.server.lag_compensation_active(bot));
    // Its moves run all the same.
    h.server
        .game_mut()
        .act(move |eng| eng.run_player_move(bot, Vec3::ZERO, 100.0, 0.0, 0.0, 0, 0, 10));
    h.step(&mut [&mut a], 1);
    assert!(h.server.game().usercmd_cmds.iter().any(|c| c.forwardmove == 100.0));

    // An event for everyone reaches the real client and is never queued for the bot (which
    // would never send it); a reliable one is not written to the bot's stream either.
    h.server.game_mut().act(move |eng| {
        let play = |eng: &mut cs_engine::EngineApi<'_>, flags: i32| {
            eng.playback_event(flags, None, 1, 0.0, Vec3::ZERO, Vec3::ZERO, 0.5, 0.0, 0, 0, false, false);
        };
        play(eng, FEV_GLOBAL);
        play(eng, FEV_GLOBAL | cs_engine::event_flags::FEV_RELIABLE);
    });
    h.step(&mut [&mut a], 2);
    assert_eq!(h.server.queued_event_count(bot), 0);
    assert_eq!(h.server.client_reliable_backlog(bot), 0);
    let seen = a
        .messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::Event(_) | SvcMessage::EventReliable { .. }))
        .count();
    assert_eq!(seen, 2);
}

#[test]
fn level_change_drops_fake_clients_as_rehlds() {
    let mut h = Harness::with_game(3, game());
    let mut a = joined(&mut h, "Alice", &mut []);
    let bot = spawn_bot(&mut h, "Bot");
    h.step(&mut [&mut a], 10);
    assert_eq!(h.server.fake_client_count(), 1);
    let deactivated = h.server.game().server_deactivate_calls;

    // `SV_InactivateClients`: "Dropping fakeclient on level change"; the real client signs on
    // again.
    h.server.change_map(ServerMap::stub("next"));
    assert_eq!(h.server.client_state(bot), ClientState::Free);
    assert_eq!(h.server.fake_client_count(), 0);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    assert_eq!(h.server.game().server_deactivate_calls, deactivated + 1);
    assert_ne!(h.server.client_state(a.player_slot().unwrap()), ClientState::Free);
    h.step(&mut [&mut a], 300);
    assert_eq!(a.state(), &ProbeState::InGame);
    // Alice came back in her own slot; nobody holds the bot's, and a new bot can take it.
    assert_eq!(h.server.client_count(), 1);
    assert_eq!(spawn_bot(&mut h, "Bot2"), bot);
}

#[test]
fn real_clients_unchanged() {
    let mut h = Harness::with_game(4, game());
    let mut a = joined(&mut h, "Alice", &mut []);
    let bot = spawn_bot(&mut h, "Bot");
    h.step(&mut [&mut a], 30);
    let real = a.player_slot().unwrap();
    assert!(!h.server.is_fake_client(real));
    assert_eq!(h.server.client_state(real), ClientState::Active);
    assert_eq!(h.server.fake_client_count(), 1);
    assert_eq!(h.server.game().client_connect_calls, 1);
    assert_eq!(h.server.game().client_put_in_server_calls, 1);
    // The real client's commands still run, with seeds, and its own origin moves.
    assert!(h.server.game().run_usercmd_calls > 0);
    assert!(a.opcode_log().len() > 10, "the real client still gets updates");
    assert!(a.entities().contains_key(&(u16::from(bot) + 1)));

    // `status` lists the bot as `BOT` and the client as before.
    let status = console(&mut h, "status");
    let lines: Vec<&str> = status.lines().collect();
    assert_eq!(lines.len(), 4, "{status}");
    assert!(lines[2].starts_with("1 ") && lines[2].contains("\"Alice\"") && lines[2].contains("ms "), "{status}");
    assert!(lines[3].starts_with("2 ") && lines[3].ends_with("\"Bot\" active BOT"), "{status}");

    // `kick` finds a bot by name.
    assert!(console(&mut h, "kick Bot gone").starts_with("Kicked \"Bot\""));
    assert_eq!(h.server.client_state(bot), ClientState::Free);
    spawn_bot(&mut h, "Bot");

    // The server info counts it in `bots`, and among the players.
    let mut asker = h.hub.connect();
    let server = PeerAddr::Loopback(0);
    let mut query = vec![0xFF; 4];
    query.push(A2S_INFO);
    query.extend_from_slice(A2S_INFO_PAYLOAD);
    asker.send(server, &query).expect("loopback send");
    h.step(&mut [&mut a], 2);
    let (_, reply) = asker.recv().expect("a challenge");
    let A2sInfoReply::Challenge(challenge) = decode_a2s_info_reply(&reply, "", 0).unwrap() else {
        panic!("expected a challenge");
    };
    asker
        .send(server, &encode_a2s_info_request(Some(challenge)))
        .expect("loopback send");
    h.step(&mut [&mut a], 2);
    let (_, reply) = asker.recv().expect("the server info");
    let A2sInfoReply::Info(info) = decode_a2s_info_reply(&reply, "", 0).unwrap() else {
        panic!("expected the info");
    };
    assert_eq!((info.players, info.bots), (2, 1));
}
