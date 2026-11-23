//! Temp entity builders, the decal index, sound flags, ambient sounds and the precache limits
//! of the weapon set (phase 2D, Task 4).

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Harness, TestGame};
use cs_engine::{
    ClientState, MAX_DECALS, MAX_MODELS, MAX_SOUNDS, MsgDest, PrecacheTables, ServerMap,
    decal_names_from_wads, res_type,
    wad_lump_names,
};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::messages::{
    SND_FL_CHANGE_PITCH, SND_FL_CHANGE_VOL, SND_FL_STOP, SoundMsg, SvcMessage, SvcParseContext,
    write_svc_centerprint, write_svc_temp_entity, parse_svc_stream,
};
use cs_proto48::tempent::{self, TempEntity, explflag, te};
use cs_transport::LoopbackEndpoint;
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

fn two_active(game: TestGame) -> (Harness, Probe, Probe) {
    let mut h = Harness::with_game(4, game);
    let mut a = h.add_probe("A");
    let mut b = h.add_probe("B");
    h.step(&mut [&mut a, &mut b], 200);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(b.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    (h, a, b)
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

fn temp_entities(p: &Probe) -> Vec<(u8, Vec<u8>)> {
    p.messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::TempEntity { te_type, raw } => Some((*te_type, raw.clone())),
            _ => None,
        })
        .collect()
}

fn ambient_count(p: &Probe) -> usize {
    p.messages()
        .iter()
        .filter(|m| matches!(m, SvcMessage::SpawnStaticSound(_)))
        .count()
}

/// The coordinates `(1.0, -2.5, 100.0)` on the wire: `f * 8` as little-endian `i16`
/// (8, -20, 800), written out by hand.
const POS: Vec3 = Vec3::new(1.0, -2.5, 100.0);
const POS_BYTES: [u8; 6] = [0x08, 0x00, 0xEC, 0xFF, 0x20, 0x03];
/// `(0.5, -1.0, 2.0)`: 4, -8, 16.
const DIR: Vec3 = Vec3::new(0.5, -1.0, 2.0);
const DIR_BYTES: [u8; 6] = [0x04, 0x00, 0xF8, 0xFF, 0x10, 0x00];

fn with_pos(tail: &[u8]) -> Vec<u8> {
    [&POS_BYTES[..], tail].concat()
}

/// Every builder against its layout in `const.h`, byte by byte, and through the message
/// parser, which must take exactly the payload (a sentinel message follows it).
#[test]
fn te_builders_match_the_wire_layout() {
    let cases: Vec<(TempEntity, u8, Vec<u8>)> = vec![
        (tempent::gunshot(POS), te::GUNSHOT, with_pos(&[])),
        (tempent::sparks(POS), te::SPARKS, with_pos(&[])),
        (
            tempent::explosion(POS, 0x0102, 25, 30, explflag::NONE),
            te::EXPLOSION,
            with_pos(&[0x02, 0x01, 25, 30, 0]),
        ),
        (
            tempent::smoke(POS, 0x0304, 40, 5),
            te::SMOKE,
            with_pos(&[0x04, 0x03, 40, 5]),
        ),
        (
            tempent::beam_follow(0x0105, 0x0207, 10, 20, [1, 2, 3], 200),
            te::BEAMFOLLOW,
            vec![0x05, 0x01, 0x07, 0x02, 10, 20, 1, 2, 3, 200],
        ),
        (
            tempent::dlight(POS, 12, [255, 128, 0], 8, 60),
            te::DLIGHT,
            with_pos(&[12, 255, 128, 0, 8, 60]),
        ),
        (
            tempent::streak_splash(POS, DIR, 5, 22, 25, 65),
            te::STREAK_SPLASH,
            [&POS_BYTES[..], &DIR_BYTES[..], &[5, 22, 0, 25, 0, 65, 0]].concat(),
        ),
        (
            tempent::blood_sprite(POS, 0x0102, 0x0304, 247, 9),
            te::BLOODSPRITE,
            with_pos(&[0x02, 0x01, 0x04, 0x03, 247, 9]),
        ),
        (
            tempent::blood_stream(POS, DIR, 70, 255),
            te::BLOODSTREAM,
            [&POS_BYTES[..], &DIR_BYTES[..], &[70, 255]].concat(),
        ),
        (
            tempent::gunshot_decal(POS, 0x0102, 7),
            te::GUNSHOTDECAL,
            with_pos(&[0x02, 0x01, 7]),
        ),
        (
            tempent::decal(POS, 9, 0x0102),
            te::DECAL,
            with_pos(&[9, 0x02, 0x01]),
        ),
        (
            tempent::decal_high(POS, 9, 0x0102),
            te::DECALHIGH,
            with_pos(&[9, 0x02, 0x01]),
        ),
        (
            tempent::world_decal(POS, 200),
            te::WORLDDECAL,
            with_pos(&[200]),
        ),
        (
            tempent::world_decal_high(POS, 3),
            te::WORLDDECALHIGH,
            with_pos(&[3]),
        ),
        (
            tempent::sprite(POS, 0x0102, 151, 150),
            te::SPRITE,
            with_pos(&[0x02, 0x01, 151, 150]),
        ),
        (
            tempent::glow_sprite(POS, 0x0304, 1, 3, 255),
            te::GLOWSPRITE,
            with_pos(&[0x04, 0x03, 1, 3, 255]),
        ),
    ];
    // The numbers of `const.h`.
    let numbers = [2, 9, 3, 5, 22, 27, 25, 115, 101, 109, 104, 118, 116, 117, 17, 23];
    assert_eq!(cases.len(), numbers.len());
    for ((built, want_type, want_payload), number) in cases.iter().zip(numbers) {
        assert_eq!(built.te_type, number);
        assert_eq!(built.te_type, *want_type);
        assert_eq!(&built.payload, want_payload, "TE {number}");

        let mut w = MsgWriter::new();
        write_svc_temp_entity(&mut w, built.te_type, &built.payload);
        write_svc_centerprint(&mut w, "end");
        let parsed = parse_svc_stream(w.as_slice(), &mut SvcParseContext::new())
            .unwrap_or_else(|e| panic!("TE {number} does not parse: {e:?}"));
        assert_eq!(
            parsed,
            vec![
                SvcMessage::TempEntity {
                    te_type: built.te_type,
                    raw: built.payload.clone()
                },
                SvcMessage::CenterPrint("end".to_string()),
            ],
            "TE {number}"
        );
    }
}

#[test]
fn temp_entities_reach_their_destinations() {
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    h.server.game_mut().act(|eng| {
        eng.send_temp_entity(MsgDest::One(0), &tempent::gunshot_decal(POS, 0, 1));
        eng.send_temp_entity(MsgDest::Pvs(POS), &tempent::sparks(POS));
        eng.send_temp_entity(MsgDest::Pas(POS), &tempent::explosion(POS, 1, 25, 30, 0));
        eng.send_temp_entity(MsgDest::All, &tempent::world_decal(POS, 2));
    });
    h.step(&mut [&mut a, &mut b], 30);
    // The stub map has no visibility data: PVS and PAS reach everyone.
    // `One` and `All` ride the reliable stream, the others the unreliable one: the order of
    // arrival between the two is not fixed.
    let types = |p: &Probe| {
        let mut t: Vec<u8> = temp_entities(p).iter().map(|t| t.0).collect();
        t.sort_unstable();
        t
    };
    assert_eq!(types(&a), vec![te::EXPLOSION, te::SPARKS, te::GUNSHOTDECAL, te::WORLDDECAL]);
    assert_eq!(types(&b), vec![te::EXPLOSION, te::SPARKS, te::WORLDDECAL]);
}

/// A WAD3 holding only a directory of lumps with these names.
fn wad_with(names: &[&str]) -> Vec<u8> {
    let mut data = b"WAD3".to_vec();
    data.extend_from_slice(&(names.len() as u32).to_le_bytes());
    data.extend_from_slice(&12u32.to_le_bytes());
    for (i, name) in names.iter().enumerate() {
        data.extend_from_slice(&(i as u32).to_le_bytes()); // filepos
        data.extend_from_slice(&0u32.to_le_bytes()); // disksize
        data.extend_from_slice(&0u32.to_le_bytes()); // size
        data.extend_from_slice(&[0x40, 0, 0, 0]); // type, compression, pad
        let mut raw = [0u8; 16];
        let len = name.len().min(16);
        raw[..len].copy_from_slice(&name.as_bytes()[..len]);
        data.extend_from_slice(&raw);
    }
    data
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `PF_DecalIndex` answers the position in the list `Decal_Init` builds from `decals.wad`
/// (`Decal_MergeInDecals` reverses it when a second wad is merged in).
#[test]
fn decal_index_follows_decals_wad() {
    // Lump names are lowercased and cut at 16 bytes as `W_CleanupName` does.
    let wad = wad_with(&["{Shot1", "{SHOT2", "{blood1", "0123456789ABCDEFGH"]);
    let names = wad_lump_names(&wad).expect("a WAD3");
    assert_eq!(names, strings(&["{shot1", "{shot2", "{blood1", "0123456789abcdef"]));
    assert_eq!(wad_lump_names(b"WAD2\0\0\0\0\0\0\0\0"), None);
    assert_eq!(wad_lump_names(&wad[..wad.len() - 1]), None, "directory cut short");

    // One wad: the file's order.
    assert_eq!(decal_names_from_wads(std::slice::from_ref(&names)), names);
    // Two wads: the first reversed, a replaced lump keeps its place, a new lump is in front.
    let first = strings(&["a", "b", "c"]);
    let second = strings(&["B", "d"]);
    assert_eq!(decal_names_from_wads(&[first.clone(), second]), strings(&["d", "c", "B", "a"]));
    // The same file twice (what the stock valve and cstrike directories give): reversed.
    assert_eq!(decal_names_from_wads(&[first.clone(), first]), strings(&["c", "b", "a"]));
    assert!(decal_names_from_wads(&[]).is_empty());

    // The engine call, and the list surviving a map change.
    let (mut h, mut a, mut b) = two_active(TestGame::default());
    h.server.set_decals(strings(&["{shot1", "{shot2", "{Blood1"]));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    h.server.game_mut().act(move |eng| {
        sink.borrow_mut().extend([
            eng.decal_index("{shot1"),
            eng.decal_index("{SHOT2"),
            eng.decal_index("{blood1"),
            eng.decal_index("{nothing"),
            eng.decal_index(""),
        ]);
    });
    h.step(&mut [&mut a, &mut b], 5);
    assert_eq!(*seen.borrow(), vec![0, 1, 2, -1, -1]);
    h.server.change_map(ServerMap::stub("second"));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    h.server.game_mut().act(move |eng| sink.borrow_mut().push(eng.decal_index("{shot2")));
    h.step(&mut [&mut a, &mut b], 5);
    assert_eq!(*seen.borrow(), vec![1]);

    // The installed game: valve and cstrike `decals.wad`, merged as the engine does.
    let Ok(dir) = std::env::var("CS16_GAMEDIR") else {
        return;
    };
    let read = |sub: &str| {
        std::fs::read(std::path::Path::new(&dir).join(sub).join("decals.wad"))
            .ok()
            .and_then(|bytes| wad_lump_names(&bytes))
    };
    let (Some(valve), Some(cstrike)) = (read("valve"), read("cstrike")) else {
        return;
    };
    let merged = decal_names_from_wads(&[valve.clone(), cstrike.clone()]);
    // cstrike's file holds every lump of valve's and a few more.
    assert!(cstrike.len() >= valve.len());
    assert_eq!(merged.len(), cstrike.len());
    let mut sorted = merged.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), merged.len(), "every name once");
    // Each name the weapons use is there, at the index the list gives it.
    let mut tables = PrecacheTables::new("maps/x.bsp", 1);
    tables.set_decals(merged.clone());
    for name in ["{shot1", "{shot5", "{bigshot1", "{break1", "{blood1", "{yblood1", "{smscorch1"] {
        let index = tables.decal_index(name);
        assert!(index >= 0, "{name} is a decal");
        assert_eq!(merged[index as usize], name);
    }
    // The reversal of the merge: `reflect1`, the first lump of the file, comes last.
    assert_eq!(tables.decal_index("reflect1"), merged.len() as i32 - 1);
}

/// `UTIL_DecalTrace`: an index of 256 and up is the HIGH message with `index - 256`.
#[test]
fn decal_helpers_choose_the_high_messages() {
    assert_eq!(tempent::decal_for(POS, 255, 7), Some(tempent::decal(POS, 255, 7)));
    assert_eq!(tempent::decal_for(POS, 256, 7), Some(tempent::decal_high(POS, 0, 7)));
    assert_eq!(tempent::decal_for(POS, 511, 7), Some(tempent::decal_high(POS, 255, 7)));
    assert_eq!(tempent::decal_for(POS, 512, 7), None);
    assert_eq!(tempent::decal_for(POS, -1, 7), None);
    assert_eq!(tempent::world_decal_for(POS, 0), Some(tempent::world_decal(POS, 0)));
    assert_eq!(tempent::world_decal_for(POS, 300), Some(tempent::world_decal_high(POS, 44)));
    assert_eq!(tempent::world_decal_for(POS, 1000), None);
    assert_eq!(tempent::world_decal_for(POS, -5), None);
}

/// Decal names keep 15 characters, a wad is cut at the table size before the merge, and the
/// decals are in the resource list after the models.
#[test]
fn decal_list_limits_and_resources() {
    let mut t = PrecacheTables::new("maps/x.bsp", 1);
    t.set_decals(strings(&["0123456789abcdefghij", "{shot1"]));
    assert_eq!(t.decals(), &strings(&["0123456789abcde", "{shot1"])[..]);
    assert_eq!(t.decal_index("0123456789abcde"), 0);
    assert_eq!(t.decal_index("0123456789abcdefghij"), -1);

    let many: Vec<String> = (0..100_000).map(|i| format!("d{i}")).collect();
    let merged = decal_names_from_wads(&[many.clone(), many]);
    assert_eq!(merged.len(), MAX_DECALS);
    let mut wad = wad_with(&["a"]);
    wad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(wad_lump_names(&wad), None, "a directory that does not fit");
    let mut big = wad_with(&vec!["x"; 600]);
    big[4..8].copy_from_slice(&600u32.to_le_bytes());
    assert_eq!(wad_lump_names(&big).map(|n| n.len()), Some(MAX_DECALS));

    t.precache_model("models/a.mdl").unwrap();
    t.precache_generic("hud.txt").unwrap();
    let list = t.resource_list(&|_| 0);
    let types: Vec<(u8, u16, &str)> = list
        .iter()
        .map(|r| (r.res_type, r.index, r.filename.as_str()))
        .collect();
    assert_eq!(
        types,
        vec![
            (res_type::GENERIC, 1, "hud.txt"),
            (res_type::MODEL, 1, "maps/x.bsp"),
            (res_type::MODEL, 2, "models/a.mdl"),
            (res_type::DECAL, 0, "0123456789abcde"),
            (res_type::DECAL, 1, "{shot1"),
        ]
    );
}

#[test]
fn ambient_sound_from_a_point() {
    let game = TestGame {
        precache_sounds: vec!["ambience/fan.wav".to_string()],
        ..TestGame::default()
    };
    let (mut h, mut a, mut b) = two_active(game);
    let results = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&results);
    h.server.game_mut().act(move |eng| {
        let ent = eng.client_edict(0);
        let mut emit = |sample: &str, origin: Vec3, volume: f32, flags: i32| {
            sink.borrow_mut()
                .push(eng.emit_ambient_sound(ent, origin, sample, volume, 0.8125, flags, 100));
        };
        emit("ambience/fan.wav", Vec3::new(10.0, 20.0, 30.0), 0.5, 0);
        emit("AMBIENCE\\FAN.WAV", Vec3::ZERO, 1.0, 0);
        // A sentence number below the 1536 sentences.
        emit("!1535", Vec3::ZERO, 1.0, 0);
        // Spawning: into the init buffer while activating, else to everyone.
        emit("ambience/fan.wav", Vec3::ZERO, 1.0, 1 << 8);
        // Refused.
        emit("!1536", Vec3::ZERO, 1.0, 0);
        emit("ambience/none.wav", Vec3::ZERO, 1.0, 0);
        emit("", Vec3::ZERO, 1.0, 0);
        emit("ambience/fan.wav", Vec3::new(f32::NAN, 0.0, 0.0), 1.0, 0);
        emit("ambience/fan.wav", Vec3::ZERO, f32::NAN, 0);
        let gone = eng.create_entity("test_gone").expect("free edict");
        eng.remove_entity(gone);
        sink.borrow_mut().push(eng.emit_ambient_sound(
            gone,
            Vec3::ZERO,
            "ambience/fan.wav",
            1.0,
            1.0,
            0,
            100,
        ));
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(
        *results.borrow(),
        vec![true, true, true, true, false, false, false, false, false, false]
    );
    // It is a static sound, not an entity sound: no `svc_sound`, and both clients get it.
    for probe in [&a, &b] {
        assert_eq!(ambient_count(probe), 4);
        assert!(sounds(probe).is_empty());
    }
}

/// `PF_sound_I`: the entity's own client hears it, and the `SND_*` flags reach the wire.
#[test]
fn sound_flags_reach_the_wire() {
    let game = TestGame {
        precache_sounds: vec!["weapons/ak47-1.wav".to_string()],
        ..TestGame::default()
    };
    let (mut h, mut a, mut b) = two_active(game);
    let results = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&results);
    h.server.game_mut().act(move |eng| {
        let ent = eng.client_edict(0);
        let mut emit = |channel: i32, volume: f32, flags: i32, pitch: i32| {
            sink.borrow_mut().push(eng.emit_sound_flags(
                ent,
                channel,
                "weapons/ak47-1.wav",
                volume,
                0.8,
                flags,
                pitch,
            ));
        };
        emit(1, 1.0, 0, 100);
        emit(1, 0.5, SND_FL_CHANGE_VOL as i32, 100);
        emit(2, 1.0, SND_FL_CHANGE_PITCH as i32, 120);
        emit(1, 1.0, SND_FL_STOP as i32, 100);
        // Cut to the wire as `SV_BuildSoundMsg` does: channel -3 is 0, 9 is 4, pitch to 255.
        emit(-3, 1.0, 0, 100);
        emit(9, 1.0, 0, 300);
        emit(1, f32::NAN, 0, 100);
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(*results.borrow(), vec![true, true, true, true, true, true, false]);
    for probe in [&a, &b] {
        let heard = sounds(probe);
        assert_eq!(heard.len(), 6, "the shooter hears its own sound: {heard:?}");
        // 0x02 is the attenuation of 0.8, 0x01 the volume and 0x08 the pitch; the bits asked
        // for are ORed in. A stop goes on the reliable stream, so the order is not fixed.
        let mut flags: Vec<u16> = heard.iter().map(|s| s.flags).collect();
        flags.sort_unstable();
        let mut want = vec![0x02, 0x02 | 0x01 | 0x40, 0x02 | 0x08 | 0x80, 0x02 | 0x20, 0x02, 0x02 | 0x08];
        want.sort_unstable();
        assert_eq!(flags, want);
        let channels: Vec<(u8, u8)> = heard.iter().map(|s| (s.channel, s.pitch)).collect();
        assert!(channels.contains(&(0, 100)), "channel -3 is 0: {channels:?}");
        assert!(channels.contains(&(4, 255)), "channel 9 is 4, pitch 300 is 255: {channels:?}");
    }
}

/// `MAX_MODELS` and `MAX_SOUNDS` are the HLDS 512 (`qlimits.h`) and the whole weapon, grenade,
/// player and client precache of ReGameDLL (every literal `PRECACHE_*` of `weapons.cpp`,
/// `wpn_*.cpp`, `ggrenade.cpp`, `client.cpp` and `player.cpp`, read 2026-11-22) fits next to a
/// map with a few hundred brush models.
#[test]
fn full_weapon_precache_fits() {
    assert_eq!((MAX_MODELS, MAX_SOUNDS), (512, 512));
    // Slot 0 is the empty name and slot 1 the map; submodels `*1..*N` follow.
    let mut t = PrecacheTables::new("maps/cs_big.bsp", 201);
    for m in MODELS {
        t.precache_model(m).unwrap_or_else(|e| panic!("{m}: {e:?}"));
    }
    for s in SOUNDS {
        t.precache_sound(s).unwrap_or_else(|e| panic!("{s}: {e:?}"));
    }
    // What is left is the room for the rest of the game.
    let models_used = 2 + 200 + MODELS.len();
    assert!(models_used <= MAX_MODELS, "{models_used} models");
    assert!(SOUNDS.len() < MAX_SOUNDS, "{} sounds", SOUNDS.len());
    assert!(t.model_index(MODELS[MODELS.len() - 1]).is_some());
    assert!(t.sound_index(SOUNDS[SOUNDS.len() - 1]).is_some());
}

const MODELS: &[&str] = &[
    "models/grenade.mdl",
    "models/p_ak47.mdl",
    "models/p_aug.mdl",
    "models/p_awp.mdl",
    "models/p_c4.mdl",
    "models/p_deagle.mdl",
    "models/p_elite.mdl",
    "models/p_famas.mdl",
    "models/p_fiveseven.mdl",
    "models/p_flashbang.mdl",
    "models/p_g3sg1.mdl",
    "models/p_galil.mdl",
    "models/p_glock18.mdl",
    "models/p_hegrenade.mdl",
    "models/p_knife.mdl",
    "models/p_m249.mdl",
    "models/p_m3.mdl",
    "models/p_m4a1.mdl",
    "models/p_mac10.mdl",
    "models/p_mp5.mdl",
    "models/p_p228.mdl",
    "models/p_p90.mdl",
    "models/p_scout.mdl",
    "models/p_sg550.mdl",
    "models/p_sg552.mdl",
    "models/p_shield.mdl",
    "models/p_smokegrenade.mdl",
    "models/p_tmp.mdl",
    "models/p_ump45.mdl",
    "models/p_usp.mdl",
    "models/p_xm1014.mdl",
    "models/player.mdl",
    "models/pshell.mdl",
    "models/rshell.mdl",
    "models/rshell_big.mdl",
    "models/shield/p_shield_deagle.mdl",
    "models/shield/p_shield_fiveseven.mdl",
    "models/shield/p_shield_flashbang.mdl",
    "models/shield/p_shield_glock18.mdl",
    "models/shield/p_shield_hegrenade.mdl",
    "models/shield/p_shield_knife.mdl",
    "models/shield/p_shield_p228.mdl",
    "models/shield/p_shield_smokegrenade.mdl",
    "models/shield/p_shield_usp.mdl",
    "models/shield/v_shield_deagle.mdl",
    "models/shield/v_shield_fiveseven.mdl",
    "models/shield/v_shield_flashbang.mdl",
    "models/shield/v_shield_glock18.mdl",
    "models/shield/v_shield_hegrenade.mdl",
    "models/shield/v_shield_knife.mdl",
    "models/shield/v_shield_p228.mdl",
    "models/shield/v_shield_smokegrenade.mdl",
    "models/shield/v_shield_usp.mdl",
    "models/shotgunshell.mdl",
    "models/v_ak47.mdl",
    "models/v_aug.mdl",
    "models/v_awp.mdl",
    "models/v_c4.mdl",
    "models/v_deagle.mdl",
    "models/v_elite.mdl",
    "models/v_famas.mdl",
    "models/v_fiveseven.mdl",
    "models/v_flashbang.mdl",
    "models/v_g3sg1.mdl",
    "models/v_galil.mdl",
    "models/v_glock18.mdl",
    "models/v_hegrenade.mdl",
    "models/v_knife.mdl",
    "models/v_m249.mdl",
    "models/v_m3.mdl",
    "models/v_m4a1.mdl",
    "models/v_mac10.mdl",
    "models/v_mp5.mdl",
    "models/v_p228.mdl",
    "models/v_p90.mdl",
    "models/v_scout.mdl",
    "models/v_sg550.mdl",
    "models/v_sg552.mdl",
    "models/v_smokegrenade.mdl",
    "models/v_tmp.mdl",
    "models/v_ump45.mdl",
    "models/v_usp.mdl",
    "models/v_xm1014.mdl",
    "models/w_ak47.mdl",
    "models/w_aug.mdl",
    "models/w_awp.mdl",
    "models/w_backpack.mdl",
    "models/w_c4.mdl",
    "models/w_deagle.mdl",
    "models/w_elite.mdl",
    "models/w_famas.mdl",
    "models/w_fiveseven.mdl",
    "models/w_flashbang.mdl",
    "models/w_g3sg1.mdl",
    "models/w_galil.mdl",
    "models/w_glock18.mdl",
    "models/w_hegrenade.mdl",
    "models/w_knife.mdl",
    "models/w_m249.mdl",
    "models/w_m3.mdl",
    "models/w_m4a1.mdl",
    "models/w_mac10.mdl",
    "models/w_mp5.mdl",
    "models/w_p228.mdl",
    "models/w_p90.mdl",
    "models/w_scout.mdl",
    "models/w_sg550.mdl",
    "models/w_sg552.mdl",
    "models/w_shield.mdl",
    "models/w_smokegrenade.mdl",
    "models/w_tmp.mdl",
    "models/w_ump45.mdl",
    "models/w_usp.mdl",
    "models/w_weaponbox.mdl",
    "models/w_xm1014.mdl",
    "sprites/b-tele1.spr",
    "sprites/black_smoke1.spr",
    "sprites/black_smoke2.spr",
    "sprites/black_smoke3.spr",
    "sprites/black_smoke4.spr",
    "sprites/blood.spr",
    "sprites/bloodspray.spr",
    "sprites/bubble.spr",
    "sprites/c-tele1.spr",
    "sprites/eexplo.spr",
    "sprites/explode1.spr",
    "sprites/fast_wallpuff1.spr",
    "sprites/fexplo.spr",
    "sprites/fexplo1.spr",
    "sprites/gas_puff_01.spr",
    "sprites/laserbeam.spr",
    "sprites/laserdot.spr",
    "sprites/ledglow.spr",
    "sprites/pistol_smoke1.spr",
    "sprites/pistol_smoke2.spr",
    "sprites/radio.spr",
    "sprites/rifle_smoke1.spr",
    "sprites/rifle_smoke2.spr",
    "sprites/rifle_smoke3.spr",
    "sprites/shadow_circle.spr",
    "sprites/smokepuff.spr",
    "sprites/steam1.spr",
    "sprites/wall_puff1.spr",
    "sprites/wall_puff2.spr",
    "sprites/wall_puff3.spr",
    "sprites/wall_puff4.spr",
    "sprites/wxplo1.spr",
    "sprites/zerogxplode.spr",
];

const SOUNDS: &[&str] = &[
    "buttons/spark5.wav",
    "buttons/spark6.wav",
    "common/bodysplat.wav",
    "common/npc_step1.wav",
    "common/npc_step2.wav",
    "common/npc_step3.wav",
    "common/npc_step4.wav",
    "common/wpn_denyselect.wav",
    "common/wpn_hudoff.wav",
    "common/wpn_hudon.wav",
    "common/wpn_moveselect.wav",
    "common/wpn_select.wav",
    "debris/glass1.wav",
    "debris/glass2.wav",
    "debris/glass3.wav",
    "debris/wood1.wav",
    "debris/wood2.wav",
    "debris/wood3.wav",
    "items/ammopickup2.wav",
    "items/equip_nvg.wav",
    "items/kevlar.wav",
    "items/nvg_off.wav",
    "items/nvg_on.wav",
    "items/weapondrop1.wav",
    "plats/train_use1.wav",
    "plats/vehicle_ignition.wav",
    "player/bhit_flesh-1.wav",
    "player/bhit_flesh-2.wav",
    "player/bhit_flesh-3.wav",
    "player/bhit_helmet-1.wav",
    "player/bhit_kevlar-1.wav",
    "player/death6.wav",
    "player/die1.wav",
    "player/die2.wav",
    "player/die3.wav",
    "player/geiger1.wav",
    "player/geiger2.wav",
    "player/geiger3.wav",
    "player/geiger4.wav",
    "player/geiger5.wav",
    "player/geiger6.wav",
    "player/headshot1.wav",
    "player/headshot2.wav",
    "player/headshot3.wav",
    "player/pl_die1.wav",
    "player/pl_dirt1.wav",
    "player/pl_dirt2.wav",
    "player/pl_dirt3.wav",
    "player/pl_dirt4.wav",
    "player/pl_duct1.wav",
    "player/pl_duct2.wav",
    "player/pl_duct3.wav",
    "player/pl_duct4.wav",
    "player/pl_fallpain2.wav",
    "player/pl_fallpain3.wav",
    "player/pl_grate1.wav",
    "player/pl_grate2.wav",
    "player/pl_grate3.wav",
    "player/pl_grate4.wav",
    "player/pl_ladder1.wav",
    "player/pl_ladder2.wav",
    "player/pl_ladder3.wav",
    "player/pl_ladder4.wav",
    "player/pl_metal1.wav",
    "player/pl_metal2.wav",
    "player/pl_metal3.wav",
    "player/pl_metal4.wav",
    "player/pl_pain2.wav",
    "player/pl_pain4.wav",
    "player/pl_pain5.wav",
    "player/pl_pain6.wav",
    "player/pl_pain7.wav",
    "player/pl_shot1.wav",
    "player/pl_slosh1.wav",
    "player/pl_slosh2.wav",
    "player/pl_slosh3.wav",
    "player/pl_slosh4.wav",
    "player/pl_snow1.wav",
    "player/pl_snow2.wav",
    "player/pl_snow3.wav",
    "player/pl_snow4.wav",
    "player/pl_snow5.wav",
    "player/pl_snow6.wav",
    "player/pl_step1.wav",
    "player/pl_step2.wav",
    "player/pl_step3.wav",
    "player/pl_step4.wav",
    "player/pl_swim1.wav",
    "player/pl_swim2.wav",
    "player/pl_swim3.wav",
    "player/pl_swim4.wav",
    "player/pl_tile1.wav",
    "player/pl_tile2.wav",
    "player/pl_tile3.wav",
    "player/pl_tile4.wav",
    "player/pl_tile5.wav",
    "player/pl_wade1.wav",
    "player/pl_wade2.wav",
    "player/pl_wade3.wav",
    "player/pl_wade4.wav",
    "player/sprayer.wav",
    "radio/com_go.wav",
    "radio/letsgo.wav",
    "radio/locknload.wav",
    "radio/moveout.wav",
    "radio/rescued.wav",
    "radio/rounddraw.wav",
    "weapons/357_cock1.wav",
    "weapons/ak47-1.wav",
    "weapons/ak47-2.wav",
    "weapons/ak47_boltpull.wav",
    "weapons/ak47_clipin.wav",
    "weapons/ak47_clipout.wav",
    "weapons/aug-1.wav",
    "weapons/aug_boltpull.wav",
    "weapons/aug_boltslap.wav",
    "weapons/aug_clipin.wav",
    "weapons/aug_clipout.wav",
    "weapons/aug_forearm.wav",
    "weapons/awp1.wav",
    "weapons/awp_clipin.wav",
    "weapons/awp_clipout.wav",
    "weapons/awp_deploy.wav",
    "weapons/boltdown.wav",
    "weapons/boltpull1.wav",
    "weapons/boltup.wav",
    "weapons/bullet_hit1.wav",
    "weapons/bullet_hit2.wav",
    "weapons/c4_beep1.wav",
    "weapons/c4_beep2.wav",
    "weapons/c4_beep3.wav",
    "weapons/c4_beep4.wav",
    "weapons/c4_beep5.wav",
    "weapons/c4_click.wav",
    "weapons/c4_disarm.wav",
    "weapons/c4_disarmed.wav",
    "weapons/c4_explode1.wav",
    "weapons/c4_plant.wav",
    "weapons/clipin1.wav",
    "weapons/clipout1.wav",
    "weapons/de_clipin.wav",
    "weapons/de_clipout.wav",
    "weapons/de_deploy.wav",
    "weapons/deagle-1.wav",
    "weapons/deagle-2.wav",
    "weapons/debris1.wav",
    "weapons/debris2.wav",
    "weapons/debris3.wav",
    "weapons/dryfire_pistol.wav",
    "weapons/dryfire_rifle.wav",
    "weapons/elite_clipout.wav",
    "weapons/elite_deploy.wav",
    "weapons/elite_fire.wav",
    "weapons/elite_leftclipin.wav",
    "weapons/elite_reloadstart.wav",
    "weapons/elite_rightclipin.wav",
    "weapons/elite_sliderelease.wav",
    "weapons/explode3.wav",
    "weapons/explode4.wav",
    "weapons/explode5.wav",
    "weapons/famas-1.wav",
    "weapons/famas-2.wav",
    "weapons/famas-burst.wav",
    "weapons/famas_boltpull.wav",
    "weapons/famas_boltslap.wav",
    "weapons/famas_clipin.wav",
    "weapons/famas_clipout.wav",
    "weapons/famas_forearm.wav",
    "weapons/fiveseven-1.wav",
    "weapons/fiveseven_clipin.wav",
    "weapons/fiveseven_clipout.wav",
    "weapons/fiveseven_slidepull.wav",
    "weapons/fiveseven_sliderelease.wav",
    "weapons/flashbang-1.wav",
    "weapons/flashbang-2.wav",
    "weapons/g3sg1-1.wav",
    "weapons/g3sg1_clipin.wav",
    "weapons/g3sg1_clipout.wav",
    "weapons/g3sg1_slide.wav",
    "weapons/galil-1.wav",
    "weapons/galil-2.wav",
    "weapons/galil_boltpull.wav",
    "weapons/galil_clipin.wav",
    "weapons/galil_clipout.wav",
    "weapons/generic_reload.wav",
    "weapons/glock18-1.wav",
    "weapons/glock18-2.wav",
    "weapons/grenade_hit1.wav",
    "weapons/grenade_hit2.wav",
    "weapons/grenade_hit3.wav",
    "weapons/he_bounce-1.wav",
    "weapons/hegrenade-1.wav",
    "weapons/hegrenade-2.wav",
    "weapons/knife_deploy1.wav",
    "weapons/knife_hit1.wav",
    "weapons/knife_hit2.wav",
    "weapons/knife_hit3.wav",
    "weapons/knife_hit4.wav",
    "weapons/knife_hitwall1.wav",
    "weapons/knife_slash1.wav",
    "weapons/knife_slash2.wav",
    "weapons/knife_stab.wav",
    "weapons/m249-1.wav",
    "weapons/m249-2.wav",
    "weapons/m249_boxin.wav",
    "weapons/m249_boxout.wav",
    "weapons/m249_chain.wav",
    "weapons/m249_coverdown.wav",
    "weapons/m249_coverup.wav",
    "weapons/m3-1.wav",
    "weapons/m3_insertshell.wav",
    "weapons/m3_pump.wav",
    "weapons/m4a1-1.wav",
    "weapons/m4a1_boltpull.wav",
    "weapons/m4a1_clipin.wav",
    "weapons/m4a1_clipout.wav",
    "weapons/m4a1_deploy.wav",
    "weapons/m4a1_silencer_off.wav",
    "weapons/m4a1_silencer_on.wav",
    "weapons/m4a1_unsil-1.wav",
    "weapons/m4a1_unsil-2.wav",
    "weapons/mac10-1.wav",
    "weapons/mac10_boltpull.wav",
    "weapons/mac10_clipin.wav",
    "weapons/mac10_clipout.wav",
    "weapons/mp5-1.wav",
    "weapons/mp5-2.wav",
    "weapons/mp5_clipin.wav",
    "weapons/mp5_clipout.wav",
    "weapons/mp5_slideback.wav",
    "weapons/p228-1.wav",
    "weapons/p228_clipin.wav",
    "weapons/p228_clipout.wav",
    "weapons/p228_slidepull.wav",
    "weapons/p228_sliderelease.wav",
    "weapons/p90-1.wav",
    "weapons/p90_boltpull.wav",
    "weapons/p90_clipin.wav",
    "weapons/p90_clipout.wav",
    "weapons/p90_cliprelease.wav",
    "weapons/pinpull.wav",
    "weapons/reload1.wav",
    "weapons/reload3.wav",
    "weapons/scout_bolt.wav",
    "weapons/scout_clipin.wav",
    "weapons/scout_clipout.wav",
    "weapons/scout_fire-1.wav",
    "weapons/sg550-1.wav",
    "weapons/sg550_boltpull.wav",
    "weapons/sg550_clipin.wav",
    "weapons/sg550_clipout.wav",
    "weapons/sg552-1.wav",
    "weapons/sg552-2.wav",
    "weapons/sg552_boltpull.wav",
    "weapons/sg552_clipin.wav",
    "weapons/sg552_clipout.wav",
    "weapons/sg_explode.wav",
    "weapons/slideback1.wav",
    "weapons/sliderelease1.wav",
    "weapons/tmp-1.wav",
    "weapons/tmp-2.wav",
    "weapons/ump45-1.wav",
    "weapons/ump45_boltslap.wav",
    "weapons/ump45_clipin.wav",
    "weapons/ump45_clipout.wav",
    "weapons/usp1.wav",
    "weapons/usp2.wav",
    "weapons/usp_clipin.wav",
    "weapons/usp_clipout.wav",
    "weapons/usp_silencer_off.wav",
    "weapons/usp_silencer_on.wav",
    "weapons/usp_slideback.wav",
    "weapons/usp_sliderelease.wav",
    "weapons/usp_unsil-1.wav",
    "weapons/xm1014-1.wav",
    "weapons/zoom.wav",
];
