//! Events: `precache_event` and `playback_event`, what the clients' updates carry
//! (`svc_event`) and what their reliable streams do (`svc_event_reliable`).

mod common;

use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::BspMap;
use cs_engine::event_flags::*;
use cs_engine::{ClientState, EngineApi, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::event::{EventArgs, WireEvent};
use cs_proto48::messages::SvcMessage;
use cs_proto48::messages::ResourceEntry;
use cs_transport::LoopbackEndpoint;
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

const AK47: &str = "events/ak47.sc";
const GLOCK: &str = "events/glock18.sc";
/// `FL_DUCKING` and `EF_NODRAW`.
const FL_DUCKING: u32 = 1 << 14;
const EF_NODRAW: u32 = 128;

fn game() -> TestGame {
    TestGame {
        precache_events: vec![AK47.to_string(), GLOCK.to_string()],
        ..TestGame::default()
    }
}

/// Two probes in game on a stub map (slots 0 and 1, entities 1 and 2).
fn two_active() -> (Harness, Probe, Probe) {
    two_active_with(&[], &[])
}

/// [`two_active`] with userinfo keys set for A and B before they connect (both have
/// `cl_lw 1` otherwise, as a real client).
fn two_active_with(a_info: &[(&str, &str)], b_info: &[(&str, &str)]) -> (Harness, Probe, Probe) {
    let mut h = Harness::with_game(4, game());
    let mut a = h.add_probe("A");
    let mut b = h.add_probe("B");
    // The connect packet is built when the challenge arrives, after these.
    for (key, value) in a_info {
        a.set_userinfo(key, value);
    }
    for (key, value) in b_info {
        b.set_userinfo(key, value);
    }
    h.step(&mut [&mut a, &mut b], 200);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(b.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    (h, a, b)
}

fn events_of(p: &Probe) -> Vec<WireEvent> {
    p.messages()
        .iter()
        .flat_map(|m| match m {
            SvcMessage::Event(events) => events.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn reliable_events_of(p: &Probe) -> Vec<(u16, EventArgs, Option<u16>)> {
    p.messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::EventReliable {
                index,
                args,
                fire_time,
            } => Some((*index, *args, *fire_time)),
            _ => None,
        })
        .collect()
}

/// A `playback_event` of event `index` by slot `slot`'s player with nothing but the flags set.
fn play(eng: &mut EngineApi<'_>, flags: i32, slot: u8, index: u16) {
    let invoker = eng.client_edict(slot);
    eng.playback_event(
        flags,
        Some(invoker),
        index,
        0.0,
        Vec3::ZERO,
        Vec3::ZERO,
        0.5,
        0.0,
        0,
        0,
        false,
        false,
    );
}

#[test]
fn precached_events_are_in_the_resource_list() {
    let (_h, a, _b) = two_active();
    let list: Vec<ResourceEntry> = a
        .messages()
        .iter()
        .find_map(|m| match m {
            SvcMessage::ResourceList { resources, .. } => Some(resources.clone()),
            _ => None,
        })
        .expect("the client was sent the resource list");
    let events: Vec<(&str, u16)> = list
        .iter()
        .filter(|r| r.res_type == 5)
        .map(|r| (r.filename.as_str(), r.index))
        .collect();
    assert_eq!(events, vec![(AK47, 1), (GLOCK, 2)]);
    // They come after every other kind, as `SV_CreateResourceList` puts them last.
    let first_event = list.iter().position(|r| r.res_type == 5).unwrap();
    assert!(list[first_event..].iter().all(|r| r.res_type == 5));
}

#[test]
fn precache_event_indexes_from_one_and_refuses_new_names_late() {
    let (mut h, _a, _b) = two_active();
    let results = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let r = results.clone();
    h.server.game_mut().act(move |eng| {
        let mut r = r.borrow_mut();
        r.push(eng.precache_event(AK47));
        r.push(eng.precache_event("EVENTS\\Glock18.SC"));
        r.push(eng.precache_event("events/new.sc"));
        r.push(eng.precache_event(""));
    });
    let (mut a, mut b) = (h.add_probe("late-a"), h.add_probe("late-b"));
    h.step(&mut [&mut a, &mut b], 2);
    // Already precached names resolve (case and slash style do not matter); a new name after
    // `server_activate` and an empty one are refused with 0.
    assert_eq!(*results.borrow(), vec![1, 2, 0, 0]);
}

#[test]
fn playback_reaches_pas_clients_not_the_shooter_with_lw() {
    // A predicts weapons (`cl_lw 1`, the default of a real client); B does not.
    let (mut h, mut a, mut b) = two_active_with(&[], &[("cl_lw", "0")]);
    h.server.game_mut().act(|eng| {
        assert!(eng.client_lw(0));
        assert!(!eng.client_lw(1));
        // Weapons fire like this: from the shooter, not to the shooter's own client.
        play(eng, FEV_NOTHOST, 0, 1);
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert!(events_of(&a).is_empty(), "the shooter predicts it");
    let heard = events_of(&b);
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].index, 1);

    // A shooter without `cl_lw` is sent its own event.
    h.server.game_mut().act(|eng| play(eng, FEV_NOTHOST, 1, 2));
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(events_of(&a).len(), 1);
    assert_eq!(events_of(&b).len(), 2, "B hears its own shot: it has no cl_lw");
}

#[test]
fn event_args_round_trip_on_the_wire() {
    let (mut h, mut a, mut b) = two_active();
    h.server.game_mut().act(|eng| {
        let shooter = eng.client_edict(0);
        let vars = eng.ent_mut(shooter).unwrap();
        vars.flags |= FL_DUCKING;
        vars.angles = Vec3::new(10.0, 90.0, 0.0);
        // Given origin and angles are sent; the other numbers are the game's.
        eng.playback_event(
            0,
            Some(shooter),
            2,
            0.0,
            Vec3::new(100.5, -20.25, 30.125),
            Vec3::new(1.5, -2.5, 3.5),
            0.5,
            -0.25,
            -1234,
            567,
            true,
            false,
        );
    });
    h.step(&mut [&mut a, &mut b], 30);
    let ev = events_of(&b)[0];
    assert_eq!(ev.index, 2);
    // A is in B's packet entities (slot 0 first): the client takes the entity from there.
    assert_eq!(ev.packet_index, Some(0));
    assert_eq!(ev.fire_time, None);
    let args = ev.args.expect("the arguments differ from zero");
    assert_eq!(args.ducking, 0, "an invoker in the packet gives its ducking by itself");
    assert_eq!(args.origin, Vec3::new(100.5, -20.25, 30.125));
    assert_eq!(args.angles, Vec3::new(1.5, -2.5, 3.5));
    assert_eq!((args.fparam1, args.fparam2), (0.5, -0.25));
    assert_eq!((args.iparam1, args.iparam2), (-1234, 567));
    assert_eq!((args.bparam1, args.bparam2), (1, 0));
}

#[test]
fn an_invoker_outside_the_packet_is_sent_by_number_with_its_own_origin() {
    let (mut h, mut a, mut b) = two_active();
    h.server.game_mut().act(|eng| {
        let shooter = eng.client_edict(0);
        let vars = eng.ent_mut(shooter).unwrap();
        vars.flags |= FL_DUCKING;
        vars.effects |= EF_NODRAW;
        vars.origin = Vec3::new(8.0, 16.0, 24.0);
        vars.angles = Vec3::new(0.0, 45.0, 0.0);
        play(eng, 0, 0, 1);
    });
    h.step(&mut [&mut a, &mut b], 30);
    let ev = events_of(&b)[0];
    let args = ev.args.unwrap();
    // B is sent its own entity only: the invoker is past the end of the list.
    assert_eq!(ev.packet_index, Some(1));
    assert_eq!(args.entindex, 1);
    assert_eq!(args.ducking, 1);
    assert_eq!(args.origin, Vec3::new(8.0, 16.0, 24.0));
    assert_eq!(args.angles, Vec3::new(0.0, 45.0, 0.0));
}

#[test]
fn reliable_events_use_svc_event_reliable() {
    let (mut h, mut a, mut b) = two_active();
    h.server.game_mut().act(|eng| {
        let shooter = eng.client_edict(0);
        eng.playback_event(
            FEV_RELIABLE,
            Some(shooter),
            1,
            0.5,
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::ZERO,
            0.0,
            0.0,
            7,
            0,
            false,
            true,
        );
        // Without an invoker the entity number is 0.
        eng.playback_event(
            FEV_RELIABLE | FEV_GLOBAL,
            None,
            2,
            0.0,
            Vec3::new(4.0, 5.0, 6.0),
            Vec3::ZERO,
            0.0,
            0.0,
            0,
            0,
            false,
            false,
        );
    });
    h.step(&mut [&mut a, &mut b], 30);
    for p in [&a, &b] {
        let got = reliable_events_of(p);
        assert_eq!(got.len(), 2, "{got:?}");
        let (index, args, fire_time) = got[0];
        assert_eq!((index, fire_time), (1, Some(50)));
        assert_eq!((args.entindex, args.iparam1, args.bparam2), (1, 7, 1));
        assert_eq!(args.origin, Vec3::new(1.0, 2.0, 3.0));
        // The invoker's own angles (zero here) go as given by the entity.
        let (index, args, fire_time) = got[1];
        assert_eq!((index, fire_time, args.entindex), (2, None, 0));
        assert_eq!(args.origin, Vec3::new(4.0, 5.0, 6.0));
        // Reliable events do not travel in an update.
        assert!(events_of(p).is_empty());
    }
}

#[test]
fn delayed_events_carry_their_delay_and_queued_ones_ride_one_update() {
    let (mut h, mut a, mut b) = two_active();
    h.server.game_mut().act(|eng| {
        let shooter = eng.client_edict(0);
        for (index, delay) in [(1, 0.25), (2, 0.0)] {
            eng.playback_event(
                0,
                Some(shooter),
                index,
                delay,
                Vec3::ZERO,
                Vec3::ZERO,
                1.0,
                0.0,
                0,
                0,
                false,
                false,
            );
        }
    });
    h.step(&mut [&mut a, &mut b], 30);
    let got = events_of(&b);
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].index, got[0].fire_time), (1, Some(25)));
    assert_eq!((got[1].index, got[1].fire_time), (2, None));
    // Sent once.
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(events_of(&b).len(), 2);
}

#[test]
fn update_replaces_the_queued_event_and_hostonly_keeps_to_the_invoker() {
    let (mut h, mut a, mut b) = two_active();
    h.server.game_mut().act(|eng| {
        let shooter = eng.client_edict(0);
        for fparam in [1.0, 2.0] {
            eng.playback_event(
                FEV_UPDATE,
                Some(shooter),
                1,
                0.0,
                Vec3::ZERO,
                Vec3::ZERO,
                fparam,
                0.0,
                0,
                0,
                false,
                false,
            );
        }
        play(eng, FEV_HOSTONLY, 0, 2);
    });
    h.step(&mut [&mut a, &mut b], 30);
    let at_b = events_of(&b);
    assert_eq!(at_b.len(), 1, "one update event, and the host-only one is not B's");
    assert_eq!(at_b[0].args.unwrap().fparam1, 2.0);
    let at_a = events_of(&a);
    assert_eq!(at_a.iter().map(|e| e.index).collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn bad_event_index_is_refused_quietly() {
    let (mut h, mut a, mut b) = two_active();
    h.server.game_mut().act(|eng| {
        let shooter = eng.client_edict(0);
        let stale = {
            let e = eng.create_entity("test_gone").expect("free edict");
            eng.remove_entity(e);
            e
        };
        // Index 0, one past the table, one that was never precached, the largest.
        for index in [0u16, 3, 255, 256, 1023, u16::MAX] {
            play(eng, 0, 0, index);
            play(eng, FEV_RELIABLE, 0, index);
        }
        // A stale invoker, non-finite numbers, and a client-only event.
        eng.playback_event(
            0, Some(stale), 1, 0.0, Vec3::ZERO, Vec3::ZERO, 0.0, 0.0, 0, 0, false, false,
        );
        for (origin, delay, f1) in [
            (Vec3::splat(f32::NAN), 0.0, 0.0),
            (Vec3::ZERO, f32::INFINITY, 0.0),
            (Vec3::ZERO, 0.0, f32::NAN),
        ] {
            eng.playback_event(
                0, Some(shooter), 1, delay, origin, Vec3::ZERO, f1, 0.0, 0, 0, false, false,
            );
        }
        play(eng, FEV_CLIENT, 0, 1);
    });
    h.step(&mut [&mut a, &mut b], 30);
    for p in [&a, &b] {
        assert!(events_of(p).is_empty());
        assert!(reliable_events_of(p).is_empty());
    }
    assert_eq!(h.server.client_state(0), ClientState::Active);
}

/// de_dust2 with a T probe at the T spawn (slot 0) and a CT probe at the CT spawn (slot 1),
/// events precached; `None` without the installed game.
fn dust2_two_players() -> Option<(Harness, Probe, Probe)> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
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
    let (t_spawn, ct_spawn) = (
        origin_of("info_player_deathmatch"),
        origin_of("info_player_start"),
    );
    let game = TestGame {
        spawn_origins: vec![t_spawn, ct_spawn],
        ..game()
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
    Some((h, t, ct))
}

#[test]
fn events_follow_the_pas_unless_global() {
    let Some((mut h, mut t, mut ct)) = dust2_two_players() else {
        return;
    };
    // The spawns are apart: the T is outside the PAS of a shot at the CT spawn (the sound
    // tests of `messages.rs` check the same pair).
    h.server.game_mut().act(|eng| play(eng, 0, 1, 1));
    h.step(&mut [&mut t, &mut ct], 30);
    assert_eq!(events_of(&ct).len(), 1);
    assert!(events_of(&t).is_empty(), "outside the PAS of the shooter");

    h.server
        .game_mut()
        .act(|eng| play(eng, FEV_GLOBAL, 1, 2));
    h.step(&mut [&mut t, &mut ct], 30);
    assert_eq!(events_of(&ct).len(), 2);
    assert_eq!(events_of(&t).len(), 1, "a global event reaches everyone");
    assert_eq!(events_of(&t)[0].index, 2);
}
