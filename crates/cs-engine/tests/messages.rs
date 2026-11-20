//! Message destinations, sounds, prints and light styles: what the game's `EngineApi::send` and
//! its helpers deliver to which `cs-probe` clients.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame, print_bytes};
use cs_assets::bsp::{BspMap, Entity};
use cs_engine::{
    ClientState, MAX_INIT_BUFFER, MAX_MESSAGE_TEXT, MAX_RELIABLE_BACKLOG, MsgDest, ServerConfig,
    ServerMap, SignonStage,
};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::messages::{SoundMsg, SvcMessage, svc};
use cs_proto48::netchan::MAX_MSGLEN;
use cs_proto48::usermsg::CsUserMsg;
use cs_transport::{LoopbackEndpoint, MAX_UDP_PACKET};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

/// A harness with two probes in game (slots 0 and 1).
fn two_active(game: TestGame) -> (Harness, Probe, Probe) {
    two_active_within(game, 200)
}

/// [`two_active`] for a game whose sign-on needs up to `frames` frames.
fn two_active_within(game: TestGame, frames: usize) -> (Harness, Probe, Probe) {
    let mut h = Harness::with_game(4, game);
    let mut a = h.add_probe("A");
    let mut b = h.add_probe("B");
    h.step(&mut [&mut a, &mut b], frames);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(b.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    (h, a, b)
}

/// Texts of the `svc_print` messages the probe has received.
fn prints(p: &Probe) -> Vec<String> {
    p.messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Print(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// How many `svc_print` messages with exactly `text` the probe has received.
fn count_print(p: &Probe, text: &str) -> usize {
    prints(p).iter().filter(|t| *t == text).count()
}

/// The last pattern the probe was sent for light style `index`.
fn light_style_of(p: &Probe, index: u8) -> Option<String> {
    p.messages().iter().rev().find_map(|m| match m {
        SvcMessage::LightStyle { index: i, pattern } if *i == index => Some(pattern.clone()),
        _ => None,
    })
}

fn sounds(p: &Probe) -> Vec<SoundMsg> {
    p.messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Sound(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn spawn_entity(classname: &str) -> Entity {
    Entity {
        properties: HashMap::from([("classname".to_string(), classname.to_string())]),
    }
}

#[test]
fn each_destination_reaches_exactly_its_audience() {
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    // C stops after `new`: past that stage, not yet in game.
    let mut c = h.add_probe("C");
    for _ in 0..200 {
        if h.server.client_state(2) == ClientState::Signon(SignonStage::AwaitSendRes) {
            break;
        }
        h.step(&mut [&mut a, &mut b, &mut c], 1);
    }
    assert_eq!(
        h.server.client_state(2),
        ClientState::Signon(SignonStage::AwaitSendRes)
    );

    h.server.game_mut().act(|eng| {
        eng.send(MsgDest::One(0), &print_bytes("one-a"));
        eng.send(MsgDest::OneUnreliable(1), &print_bytes("unrel-b"));
        eng.send(MsgDest::All, &print_bytes("all"));
        eng.send(MsgDest::Broadcast, &print_bytes("broadcast"));
        // C is not in game: nothing unreliable reaches it, but its reliable stream is open.
        eng.send(MsgDest::OneUnreliable(2), &print_bytes("unrel-c"));
        eng.send(MsgDest::One(2), &print_bytes("one-c"));
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(count_print(&a, "one-a"), 1);
    assert_eq!(count_print(&b, "one-a"), 0);
    assert_eq!(count_print(&a, "unrel-b"), 0);
    assert_eq!(count_print(&b, "unrel-b"), 1);
    assert_eq!(count_print(&a, "all"), 1);
    assert_eq!(count_print(&b, "all"), 1);
    assert_eq!(count_print(&a, "broadcast"), 1);
    assert_eq!(count_print(&b, "broadcast"), 1);
    // C was not pumped: it has seen none of it yet.
    assert_eq!(count_print(&c, "all"), 0);

    h.step(&mut [&mut a, &mut b, &mut c], 250);
    assert_eq!(c.state(), &ProbeState::InGame);
    assert_eq!(count_print(&c, "all"), 1);
    assert_eq!(count_print(&c, "one-c"), 1);
    assert_eq!(count_print(&c, "one-a"), 0);
    assert_eq!(count_print(&c, "broadcast"), 0);
    assert_eq!(count_print(&c, "unrel-c"), 0);
    // Nothing was delivered twice.
    assert_eq!(count_print(&a, "all"), 1);
    assert_eq!(count_print(&b, "broadcast"), 1);
}

#[test]
fn pvs_message_respects_visibility() {
    // A stub map has no visibility data: everything is visible.
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    h.server.game_mut().act(|eng| {
        eng.send(MsgDest::Pvs(Vec3::new(5000.0, 5000.0, 5000.0)), &print_bytes("pvs"));
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(count_print(&a, "pvs"), 1);
    assert_eq!(count_print(&b, "pvs"), 1);

    // Without the installed game the real-map half cannot run.
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
    let map = ServerMap::from_bsp("de_dust2", &raw, bsp).expect("valid BSP header");
    let origin_of = |classname: &str| {
        map.entities
            .iter()
            .filter(|e| e.classname() == classname)
            .find_map(|e| e.get_vec3("origin"))
            .unwrap_or_else(|| panic!("de_dust2 has a {classname}"))
    };
    let t_spawn = origin_of("info_player_deathmatch");
    let ct_spawn = origin_of("info_player_start");
    let view = Vec3::new(0.0, 0.0, 17.0);
    assert!(
        !map.can_see(ct_spawn + view, t_spawn - Vec3::ONE, t_spawn + Vec3::ONE),
        "the CT spawn must not see the T spawn"
    );
    assert!(map.can_see(t_spawn + view, t_spawn - Vec3::ONE, t_spawn + Vec3::ONE));

    let game = TestGame {
        spawn_origins: vec![t_spawn, ct_spawn],
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 4,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_map(cfg, map, game);
    let mut t = h.add_probe("T");
    let mut ct = h.add_probe("CT");
    h.step(&mut [&mut t, &mut ct], 300);
    assert_eq!(t.state(), &ProbeState::InGame);
    assert_eq!(ct.state(), &ProbeState::InGame);
    h.server.game_mut().act(move |eng| {
        eng.send(MsgDest::Pvs(t_spawn), &print_bytes("pvs-t-spawn"));
    });
    h.step(&mut [&mut t, &mut ct], 30);
    assert_eq!(count_print(&t, "pvs-t-spawn"), 1);
    assert_eq!(count_print(&ct, "pvs-t-spawn"), 0);
}

#[test]
fn usermsg_roundtrips_to_the_probe() {
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    let results = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&results);
    h.server.game_mut().act(move |eng| {
        let money = CsUserMsg::Money {
            amount: 1234,
            flash: true,
        };
        sink.borrow_mut().push(eng.send_usermsg(MsgDest::One(0), &money));
        let unknown = CsUserMsg::Unknown {
            name: "NotRegistered".to_string(),
            payload: vec![1, 2],
        };
        sink.borrow_mut().push(eng.send_usermsg(MsgDest::All, &unknown));
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(*results.borrow(), vec![true, false]);
    let money: Vec<_> = a
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::UserMsg { name, decoded, .. } if name == "Money" => Some(decoded.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        money,
        vec![CsUserMsg::Money {
            amount: 1234,
            flash: true
        }]
    );
    assert!(
        !b.messages()
            .iter()
            .any(|m| matches!(m, SvcMessage::UserMsg { name, .. } if name == "Money"))
    );
}

#[test]
fn prints_and_stufftext_arrive() {
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    let long = "€".repeat(400);
    h.server.game_mut().act(move |eng| {
        eng.client_print(0, "hello\n");
        eng.print_all("to everyone\n");
        eng.center_print(1, "centre");
        eng.stufftext(0, "echo from-server\n");
        // Cut at the first NUL.
        eng.client_print(1, "before\0after");
        // Cut to MAX_MESSAGE_TEXT bytes on a character boundary (3-byte characters).
        eng.client_print(1, &long);
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(count_print(&a, "hello\n"), 1);
    assert_eq!(count_print(&b, "hello\n"), 0);
    assert_eq!(count_print(&a, "to everyone\n"), 1);
    assert_eq!(count_print(&b, "to everyone\n"), 1);
    assert!(b.messages().contains(&SvcMessage::CenterPrint("centre".to_string())));
    assert!(!a.messages().iter().any(|m| matches!(m, SvcMessage::CenterPrint(_))));
    assert!(
        a.messages()
            .contains(&SvcMessage::StuffText("echo from-server\n".to_string()))
    );
    assert_eq!(count_print(&b, "before"), 1);
    let cut = "€".repeat(MAX_MESSAGE_TEXT / 3);
    assert_eq!(cut.len(), MAX_MESSAGE_TEXT - 1);
    assert_eq!(count_print(&b, &cut), 1);
}

#[test]
fn sound_requires_precache_and_reaches_listeners() {
    let game = TestGame {
        precache_sounds: vec!["weapons/ak47-1.wav".to_string()],
        spawn_origins: vec![Vec3::new(100.0, -50.0, 24.0), Vec3::new(64.0, 0.0, 0.0)],
        ..TestGame::default()
    };
    let (mut h, mut a, mut b) = two_active(game);
    let results = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&results);
    h.server.game_mut().act(move |eng| {
        let ent = eng.client_edict(0);
        let mut emit = |sample: &str, channel: u8, volume: f32, attenuation: f32, pitch: u8| {
            sink.borrow_mut()
                .push(eng.emit_sound(ent, channel, sample, volume, attenuation, pitch));
        };
        emit("weapons/ak47-1.wav", 1, 0.5, 0.8125, 90);
        // Out of range values are clamped.
        emit("WEAPONS\\AK47-1.WAV", 7, 7.0, 9.0, 100);
        // Refused: not precached, empty, channel, non-finite.
        emit("weapons/awp1.wav", 1, 1.0, 1.0, 100);
        emit("", 1, 1.0, 1.0, 100);
        emit("weapons/ak47-1.wav", 8, 1.0, 1.0, 100);
        emit("weapons/ak47-1.wav", 1, f32::NAN, 1.0, 100);
        emit("weapons/ak47-1.wav", 1, 1.0, f32::INFINITY, 100);
        // A stale entity.
        let gone = eng.create_entity("test_gone").expect("free edict");
        eng.remove_entity(gone);
        sink.borrow_mut()
            .push(eng.emit_sound(gone, 1, "weapons/ak47-1.wav", 1.0, 1.0, 100));
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(
        *results.borrow(),
        vec![true, true, false, false, false, false, false, false]
    );
    for probe in [&a, &b] {
        let heard = sounds(probe);
        assert_eq!(heard.len(), 2, "{heard:?}");
        assert_eq!(heard[0].sound_index, 1);
        assert_eq!(heard[0].entity, 1);
        assert_eq!((heard[0].channel, heard[0].pitch), (1, 90));
        assert_eq!(heard[0].volume, 128);
        assert_eq!(heard[0].attenuation, 0.8125);
        assert!((heard[0].origin - Vec3::new(100.0, -50.0, 24.0)).abs().max_element() <= 0.125);
        // Volume 7.0 is 1.0, attenuation 9.0 is 3.98 (254 / 64 on the wire).
        assert_eq!((heard[1].volume, heard[1].channel, heard[1].pitch), (255, 7, 100));
        assert_eq!(heard[1].attenuation, 254.0 / 64.0);
    }
}

#[test]
fn init_messages_are_replayed_to_late_joiners() {
    let game = TestGame {
        activate_sends: vec![
            (MsgDest::Init, print_bytes("init-1")),
            (MsgDest::Init, print_bytes("init-2")),
        ],
        ..TestGame::default()
    };
    let (mut h, mut a, mut b) = two_active(game);
    // Everyone entering gets the stored messages, in order, after the light styles.
    for probe in [&a, &b] {
        assert_eq!(count_print(probe, "init-1"), 1);
        assert_eq!(count_print(probe, "init-2"), 1);
        let texts = prints(probe);
        let first = texts.iter().position(|t| t == "init-1").unwrap();
        assert_eq!(texts[first + 1], "init-2");
    }
    let styles = a
        .messages()
        .iter()
        .rposition(|m| matches!(m, SvcMessage::LightStyle { .. }))
        .unwrap();
    let init = a
        .messages()
        .iter()
        .position(|m| *m == SvcMessage::Print("init-1".to_string()))
        .unwrap();
    let signon_num = a
        .messages()
        .iter()
        .position(|m| *m == SvcMessage::SignonNum(1))
        .unwrap();
    assert!(styles < init && init < signon_num);

    let mut c = h.add_probe("C");
    h.step(&mut [&mut a, &mut b, &mut c], 250);
    assert_eq!(c.state(), &ProbeState::InGame);
    assert_eq!(count_print(&c, "init-1"), 1);
    assert_eq!(count_print(&c, "init-2"), 1);
    // Nobody got them twice.
    assert_eq!(count_print(&a, "init-1"), 1);

    // After a map change the old messages are gone, even for clients that were there.
    h.server.game_mut().activate_sends.clear();
    h.server.change_map(ServerMap::stub("second"));
    assert_eq!(h.server.init_buffer_len(), 0);
    let mut d = h.add_probe("D");
    h.step(&mut [&mut a, &mut b, &mut c, &mut d], 400);
    for probe in [&a, &b, &c, &d] {
        assert_eq!(probe.state(), &ProbeState::InGame);
    }
    assert_eq!(count_print(&d, "init-1"), 0);
    assert_eq!(count_print(&a, "init-1"), 1);
    assert_eq!(count_print(&c, "init-2"), 1);
}

#[test]
fn init_messages_come_from_spawn_entity_too_and_only_during_activation() {
    let game = TestGame {
        spawn_sends: vec![(MsgDest::Init, print_bytes("from-spawn"))],
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 4,
        ..ServerConfig::default()
    };
    let map = ServerMap::stub_with_entities("test", vec![spawn_entity("test_initmsg")]);
    let mut h = Harness::with_map(cfg, map, game);
    let mut a = h.add_probe("A");
    h.step(&mut [&mut a], 200);
    assert_eq!(count_print(&a, "from-spawn"), 1);

    // Outside activation `Init` is `All`: delivered now, never stored.
    h.server.game_mut().act(|eng| eng.send(MsgDest::Init, &print_bytes("late-init")));
    h.step(&mut [&mut a], 30);
    assert_eq!(count_print(&a, "late-init"), 1);
    let mut b = h.add_probe("B");
    h.step(&mut [&mut a, &mut b], 250);
    assert_eq!(b.state(), &ProbeState::InGame);
    assert_eq!(count_print(&b, "from-spawn"), 1);
    assert_eq!(count_print(&b, "late-init"), 0);
}

#[test]
fn a_full_init_buffer_still_reaches_a_late_joiner() {
    // 65 messages of 1000 bytes and one of 536 fill the buffer exactly; one more byte spills
    // over and is sent as `All` (to nobody).
    let big = "x".repeat(998);
    let mut sends: Vec<(MsgDest, Vec<u8>)> = (0..65)
        .map(|i| (MsgDest::Init, print_bytes(&format!("{i:03}{}", &big[3..]))))
        .collect();
    sends.push((MsgDest::Init, print_bytes(&"y".repeat(534))));
    assert_eq!(
        sends.iter().map(|(_, b)| b.len()).sum::<usize>(),
        MAX_INIT_BUFFER
    );
    sends.push((MsgDest::Init, vec![svc::NOP]));
    let game = TestGame {
        activate_sends: sends,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    assert_eq!(h.server.init_buffer_len(), MAX_INIT_BUFFER);
    let mut a = h.add_probe("A");
    let mut worst_backlog = 0;
    for _ in 0..1500 {
        h.step(&mut [&mut a], 1);
        worst_backlog = worst_backlog.max(h.server.client_reliable_backlog(0));
        if a.state() == &ProbeState::InGame {
            break;
        }
    }
    assert_eq!(a.state(), &ProbeState::InGame, "{:?}", h.server.net_stats());
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(
        prints(&a).iter().filter(|t| t.starts_with("0") || t.starts_with("y")).count(),
        66
    );
    assert!(worst_backlog < MAX_RELIABLE_BACKLOG, "{worst_backlog}");
    eprintln!("worst reliable backlog during a full init replay: {worst_backlog}");
}

#[test]
fn light_styles_are_stored_and_updated() {
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    // Defaults: style 0 is "m", the others are empty.
    assert_eq!(light_style_of(&a, 0), Some("m".to_string()));
    assert_eq!(light_style_of(&a, 5), Some(String::new()));
    assert_eq!(light_style_of(&a, 63), Some(String::new()));
    h.server.game_mut().act(|eng| {
        eng.light_style(5, "abcba");
        eng.light_style(63, "z");
        // Ignored: out of range, too long, NUL inside.
        eng.light_style(64, "nope");
        eng.light_style(200, "nope");
        eng.light_style(6, &"a".repeat(65));
        eng.light_style(7, "a\0b");
        // The longest pattern is allowed; an empty one is a valid pattern.
        eng.light_style(8, &"b".repeat(64));
        eng.light_style(0, "");
    });
    h.step(&mut [&mut a, &mut b], 30);
    for probe in [&a, &b] {
        assert_eq!(light_style_of(probe, 5), Some("abcba".to_string()));
        assert_eq!(light_style_of(probe, 63), Some("z".to_string()));
        assert_eq!(light_style_of(probe, 6), Some(String::new()));
        assert_eq!(light_style_of(probe, 7), Some(String::new()));
        assert_eq!(light_style_of(probe, 8), Some("b".repeat(64)));
        assert_eq!(light_style_of(probe, 0), Some(String::new()));
    }
    // A client entering later is sent the stored styles in its spawn stage.
    let mut c = h.add_probe("C");
    h.step(&mut [&mut a, &mut b, &mut c], 250);
    assert_eq!(c.state(), &ProbeState::InGame);
    assert_eq!(light_style_of(&c, 5), Some("abcba".to_string()));
    assert_eq!(light_style_of(&c, 63), Some("z".to_string()));
    assert_eq!(light_style_of(&c, 7), Some(String::new()));
    assert_eq!(light_style_of(&c, 0), Some(String::new()));

    // A map change resets them.
    h.server.change_map(ServerMap::stub("second"));
    let mut d = h.add_probe("D");
    h.step(&mut [&mut a, &mut b, &mut c, &mut d], 400);
    assert_eq!(d.state(), &ProbeState::InGame);
    assert_eq!(light_style_of(&d, 5), Some(String::new()));
    assert_eq!(light_style_of(&d, 0), Some("m".to_string()));
    assert_eq!(light_style_of(&a, 5), Some(String::new()));
}

#[test]
fn temp_entities_and_raw_sends_follow_the_limits() {
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    // TE_EXPLOSION (3): 11 payload bytes.
    let payload: Vec<u8> = (40..51).collect();
    let expected = payload.clone();
    h.server.game_mut().act(move |eng| {
        eng.temp_entity(MsgDest::Broadcast, 3, &payload);
        // Over the per-message limit: dropped.
        eng.temp_entity(MsgDest::All, 3, &vec![0; MAX_MSGLEN]);
        eng.send(MsgDest::All, &vec![svc::NOP; MAX_MSGLEN + 1]);
        eng.send(MsgDest::All, &[]);
    });
    h.step(&mut [&mut a, &mut b], 30);
    // Exactly at the limit: delivered, as a message of its own.
    h.server.game_mut().act(|eng| {
        eng.send(MsgDest::One(0), &print_bytes(&"p".repeat(MAX_MSGLEN - 2)));
    });
    h.step(&mut [&mut a, &mut b], 60);
    for probe in [&a, &b] {
        let temp: Vec<_> = probe
            .messages()
            .iter()
            .filter(|m| matches!(m, SvcMessage::TempEntity { .. }))
            .collect();
        assert_eq!(
            temp,
            vec![&SvcMessage::TempEntity {
                te_type: 3,
                raw: expected.clone()
            }]
        );
        assert_eq!(probe.state(), &ProbeState::InGame);
    }
    assert_eq!(count_print(&a, &"p".repeat(MAX_MSGLEN - 2)), 1);
}

#[test]
fn out_of_range_slots_and_stale_entities_are_ignored() {
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    let results = Rc::new(Cell::new(true));
    let sink = Rc::clone(&results);
    h.server.game_mut().act(move |eng| {
        for slot in [4u8, 31, 200, 255] {
            eng.send(MsgDest::One(slot), &print_bytes("nobody"));
            eng.send(MsgDest::OneUnreliable(slot), &print_bytes("nobody"));
            eng.client_print(slot, "nobody");
            eng.center_print(slot, "nobody");
            eng.stufftext(slot, "echo nobody\n");
        }
        // Free slots take nothing either.
        eng.send(MsgDest::One(3), &print_bytes("nobody"));
        eng.send(MsgDest::OneUnreliable(3), &print_bytes("nobody"));
        for point in [Vec3::NAN, Vec3::splat(f32::INFINITY), Vec3::splat(1.0e30)] {
            eng.send(MsgDest::Pvs(point), &print_bytes("nobody"));
        }
        let stale = eng.create_entity("test_gone").expect("free edict");
        eng.remove_entity(stale);
        let never = cs_engine::EdictId {
            index: u16::MAX,
            serial: 7,
        };
        sink.set(
            !eng.emit_sound(stale, 0, "x.wav", 1.0, 1.0, 100)
                && !eng.emit_sound(never, 0, "x.wav", 1.0, 1.0, 100),
        );
        eng.temp_entity(MsgDest::One(200), 3, &[0; 11]);
        eng.light_style(255, "a");
        eng.print_all("");
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert!(results.get());
    for probe in [&a, &b] {
        assert!(
            !prints(probe).iter().any(|t| t == "nobody"),
            "{:?}",
            prints(probe)
        );
        assert!(
            !probe
                .messages()
                .iter()
                .any(|m| matches!(m, SvcMessage::CenterPrint(_) | SvcMessage::Sound(_)))
        );
        assert_eq!(probe.state(), &ProbeState::InGame);
    }
    assert!(
        !a.messages()
            .iter()
            .any(|m| matches!(m, SvcMessage::StuffText(t) if t.contains("nobody")))
    );
}

#[test]
fn per_frame_message_flood_respects_client_limits() {
    // The game also fills the init buffer to the brim and past it during activation.
    let game = TestGame {
        activate_sends: (0..70)
            .map(|_| (MsgDest::Init, vec![svc::NOP; 1000]))
            .collect(),
        ..TestGame::default()
    };
    // Each client is replayed 65 KB of init messages first.
    let (mut h, mut a, mut b) = two_active_within(game, 1000);
    assert!(h.server.init_buffer_len() <= MAX_INIT_BUFFER);
    let before = h.server.net_stats();
    let updates_before = a.stats().full_updates + a.stats().delta_updates;
    h.server.game_mut().message_flood = true;
    for _ in 0..300 {
        let tap = h.step_tapped(&mut [&mut a, &mut b]);
        assert!(tap.sent.iter().all(|(_, len)| *len <= MAX_UDP_PACKET));
    }
    let after = h.server.net_stats();
    assert!(
        after.unreliable_dropped > before.unreliable_dropped,
        "{after:?}"
    );
    assert_eq!(after.send_failures, 0);
    assert!(h.server.init_buffer_len() <= MAX_INIT_BUFFER);
    assert!(a.stats().largest_datagram <= MAX_UDP_PACKET);
    assert!(b.stats().largest_datagram <= MAX_UDP_PACKET);
    for probe in [&a, &b] {
        assert_eq!(probe.state(), &ProbeState::InGame);
    }
    let updates_after = a.stats().full_updates + a.stats().delta_updates;
    assert!(
        updates_after >= updates_before + 30,
        "{updates_before} -> {updates_after}"
    );
}
