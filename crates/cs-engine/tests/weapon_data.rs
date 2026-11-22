//! `weapon_data_t` in `svc_clientdata`, `svc_weaponanim` and the `random_seed` of a command:
//! what the server sends to which `cs-probe` client, and the wire format of the weapon list.

mod common;

use common::Harness;
use cs_engine::{ClientState, select_cmds_with_seeds};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{
    DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd, GoldSrcWeaponData, parse_delta_lst,
};
use cs_proto48::messages::{
    SvcMessage, SvcParseContext, WeaponDelta, WeaponRecords, parse_svc_stream, weapon_records,
    write_svc_clientdata_with_weapons,
};
use cs_transport::LoopbackEndpoint;
use cs_vfs::{Vfs, VfsConfig};

type Probe = ProbeClient<LoopbackEndpoint>;

/// A weapon whose every field on the wire is a value the field's width and scale carry exactly.
fn rifle() -> GoldSrcWeaponData {
    GoldSrcWeaponData {
        m_i_id: 28,
        m_i_clip: 30,
        m_fl_next_primary_attack: 1.5,
        m_fl_next_secondary_attack: 0.25,
        m_fl_time_weapon_idle: 2.125,
        m_f_in_reload: 1,
        m_f_in_special_reload: 2,
        m_fl_next_reload: 3.5,
        m_fl_pump_time: 0.75,
        m_f_reload_time: 2.5,
        m_f_aimed_damage: 12.0,
        m_f_next_aim_bonus: -0.5,
        m_f_in_zoom: 40,
        m_i_weapon_state: 3,
        iuser1: 5,
        fuser1: 4.5,
        fuser2: 6.25,
        fuser3: -7.5,
        ..GoldSrcWeaponData::default()
    }
}

fn pistol() -> GoldSrcWeaponData {
    GoldSrcWeaponData {
        m_i_id: 17,
        m_i_clip: 12,
        m_fl_next_primary_attack: 0.5,
        ..GoldSrcWeaponData::default()
    }
}

/// Two probes in game: slot 0 with `cl_lw 1` (the default), slot 1 with `cl_lw 0`.
fn lw_and_no_lw(game: common::TestGame) -> (Harness, Probe, Probe) {
    let mut h = Harness::with_game(4, game);
    let mut with_lw = h.add_probe("Lw");
    let mut without_lw = h.add_probe("NoLw");
    without_lw.set_userinfo("cl_lw", "0");
    h.step(&mut [&mut with_lw, &mut without_lw], 200);
    assert_eq!(with_lw.state(), &ProbeState::InGame);
    assert_eq!(without_lw.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    (h, with_lw, without_lw)
}

#[test]
fn weapon_data_is_sent_only_to_lw_clients() {
    let game = common::TestGame {
        weapon_list: Some(vec![(28, rifle()), (17, pistol())]),
        ..Default::default()
    };
    let (mut h, mut lw, mut no_lw) = lw_and_no_lw(game);
    h.step(&mut [&mut lw, &mut no_lw], 50);

    let got = lw.weapon_data();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[&28], rifle());
    assert_eq!(got[&17], pistol());
    assert!(no_lw.weapon_data().is_empty(), "{:?}", no_lw.weapon_data());

    // The game is not even asked for the client without `cl_lw`.
    let slots = &h.server.game().weapon_data_slots;
    assert!(slots.contains(&0));
    assert!(!slots.contains(&1), "{slots:?}");
}

#[test]
fn no_weapon_data_when_the_game_has_none() {
    // `pfnGetWeaponData` returning 0 sends no list.
    let (mut h, mut lw, mut no_lw) = lw_and_no_lw(common::TestGame::default());
    h.step(&mut [&mut lw, &mut no_lw], 50);
    assert!(lw.weapon_data().is_empty());
    assert!(h.server.game().weapon_data_slots.contains(&0));
}

#[test]
fn weapon_data_follows_changes_over_acked_deltas() {
    let game = common::TestGame {
        weapon_list: Some(vec![(28, rifle()), (17, pistol())]),
        ..Default::default()
    };
    let (mut h, mut lw, mut no_lw) = lw_and_no_lw(game);
    h.step(&mut [&mut lw, &mut no_lw], 50);
    assert_eq!(lw.weapon_data().len(), 2);

    // Only the pistol's clip changes; the rifle is not resent but stays in the client's list.
    let mut shot = pistol();
    shot.m_i_clip = 11;
    shot.m_fl_next_primary_attack = 0.75;
    h.server.game_mut().weapon_list = Some(vec![(28, rifle()), (17, shot)]);
    h.step(&mut [&mut lw, &mut no_lw], 50);
    let got = lw.weapon_data();
    assert_eq!(got[&17], shot);
    assert_eq!(got[&28], rifle());

    // A weapon the game drops goes back to zero (an all-zero record is a missing index).
    h.server.game_mut().weapon_list = Some(vec![(28, rifle())]);
    h.step(&mut [&mut lw, &mut no_lw], 50);
    let got = lw.weapon_data();
    assert_eq!(got.get(&17).copied().unwrap_or_default(), GoldSrcWeaponData::default());
    assert_eq!(got[&28], rifle());
}

/// The weapon list of one `svc_clientdata` as `write_svc_clientdata_with_weapons` writes it,
/// parsed back with `from`'s lists known to the client under `ack`.
fn clientdata_round_trip(
    ctx: &mut SvcParseContext,
    from: Option<(u8, &cs_proto48::delta::DeltaRecord)>,
    weapons: Option<WeaponDelta<'_>>,
) -> Vec<(u8, GoldSrcWeaponData)> {
    let schema = ctx.deltas.get("clientdata_t").unwrap().clone();
    let wschema = ctx.deltas.get("weapon_data_t").unwrap().clone();
    let mut w = MsgWriter::new();
    write_svc_clientdata_with_weapons(&mut w, &schema, from, &GoldSrcClientData::default(), weapons, 1.0);
    let messages = parse_svc_stream(w.as_slice(), ctx).expect("parse");
    let [SvcMessage::ClientData { weapon_deltas, .. }] = &messages[..] else {
        panic!("one clientdata, got {messages:?}");
    };
    weapon_deltas
        .iter()
        .map(|(i, rec)| (*i, GoldSrcWeaponData::from_delta(rec, &wschema)))
        .collect()
}

#[test]
fn weapon_data_round_trip_on_the_wire() {
    let mut ctx = SvcParseContext::new();
    let wschema = ctx.deltas.get("weapon_data_t").unwrap().clone();
    let mut list = [GoldSrcWeaponData::default(); 64];
    list[28] = rifle();
    list[63] = pistol();
    // Fields the schema has no place for are not sent.
    list[63].iuser2 = 9;
    list[63].fuser4 = 1.0;
    let to = weapon_records(&wschema, &list);
    assert_eq!(to.len(), 2);

    let baseline = WeaponRecords::new();
    let got = clientdata_round_trip(
        &mut ctx,
        None,
        Some(WeaponDelta { schema: &wschema, from: &baseline, to: &to }),
    );
    let mut expected_last = pistol();
    expected_last.iuser2 = 0;
    expected_last.fuser4 = 0.0;
    assert_eq!(got, vec![(28, rifle()), (63, expected_last)]);

    // Without a list the message ends with the 0 bit alone.
    let mut ctx = SvcParseContext::new();
    assert!(clientdata_round_trip(&mut ctx, None, None).is_empty());
}

#[test]
fn weapon_data_deltas_against_the_acked_frame() {
    let mut ctx = SvcParseContext::new();
    let schema = ctx.deltas.get("clientdata_t").unwrap().clone();
    let wschema = ctx.deltas.get("weapon_data_t").unwrap().clone();

    let mut list = [GoldSrcWeaponData::default(); 64];
    list[28] = rifle();
    list[17] = pistol();
    let first = weapon_records(&wschema, &list);
    ctx.last_incoming_seq = 1;
    let baseline = WeaponRecords::new();
    let got = clientdata_round_trip(
        &mut ctx,
        None,
        Some(WeaponDelta { schema: &wschema, from: &baseline, to: &first }),
    );
    assert_eq!(got.len(), 2);

    // The next update is a delta against the first: one weapon changes, one field of it.
    list[17].m_i_clip = 11;
    let second = weapon_records(&wschema, &list);
    ctx.last_incoming_seq = 2;
    let from_rec = GoldSrcClientData::default().to_delta(&schema);
    let got = clientdata_round_trip(
        &mut ctx,
        Some((1, &from_rec)),
        Some(WeaponDelta { schema: &wschema, from: &first, to: &second }),
    );
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0], (17, list[17]));

    // Written against the same frame, an unchanged list is just the end bit.
    let mut w = MsgWriter::new();
    write_svc_clientdata_with_weapons(
        &mut w,
        &schema,
        Some((1, &from_rec)),
        &GoldSrcClientData::default(),
        Some(WeaponDelta { schema: &wschema, from: &second, to: &second }),
        1.0,
    );
    let mut bare = MsgWriter::new();
    write_svc_clientdata_with_weapons(
        &mut bare,
        &schema,
        Some((1, &from_rec)),
        &GoldSrcClientData::default(),
        None,
        1.0,
    );
    assert_eq!(w.as_slice(), bare.as_slice());
}

/// The `weapon_data_t` the built-in schema has is the one of the installed game's
/// `cstrike/delta.lst` (checked 2026-11-22: same 18 fields, types, widths and scales; the file
/// only lacks the commas after `m_iWeaponState` and `m_iId`, which the parser tolerates).
#[test]
fn weapon_data_schema_matches_the_installed_delta_lst() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let Ok(text) = vfs.read_string_lossy("delta.lst") else {
        return;
    };
    let installed = parse_delta_lst(&text).expect("the installed delta.lst parses");
    let installed = installed
        .iter()
        .find(|s| s.name == "weapon_data_t")
        .expect("weapon_data_t in the installed delta.lst");
    let builtin = DeltaRegistry::cs16_default();
    let builtin = builtin.get("weapon_data_t").unwrap();
    assert_eq!(installed.fields.len(), 18);
    assert_eq!(format!("{:?}", installed.fields), format!("{:?}", builtin.fields));
    // The clientdata ammo fields too.
    let installed_cd = parse_delta_lst(&text).unwrap();
    let installed_cd = installed_cd.iter().find(|s| s.name == "clientdata_t").unwrap();
    for name in ["ammo_nails", "ammo_shells", "ammo_cells", "ammo_rockets"] {
        assert!(installed_cd.field_index(name).is_some(), "{name}");
    }
}

#[test]
fn clientdata_ammo_fields_round_trip() {
    let mut ctx = SvcParseContext::new();
    let schema = ctx.deltas.get("clientdata_t").unwrap().clone();
    let data = GoldSrcClientData {
        ammo_nails: 5,
        ammo_shells: -3,
        ammo_cells: 100,
        ammo_rockets: 511,
        ..Default::default()
    };
    assert_eq!(GoldSrcClientData::from_delta(&data.to_delta(&schema), &schema), data);
    let mut w = MsgWriter::new();
    write_svc_clientdata_with_weapons(&mut w, &schema, None, &data, None, 1.0);
    let messages = parse_svc_stream(w.as_slice(), &mut ctx).expect("parse");
    let [SvcMessage::ClientData { parsed, .. }] = &messages[..] else {
        panic!("one clientdata");
    };
    assert_eq!(*parsed, data);
}

#[test]
fn weapon_anim_reaches_only_its_client() {
    let (mut h, mut a, mut b) = lw_and_no_lw(common::TestGame::default());
    a.clear_messages();
    b.clear_messages();
    h.server.game_mut().act(|eng| {
        eng.weapon_anim(1, 5, 2);
        // A slot out of range is refused quietly.
        eng.weapon_anim(99, 1, 1);
    });
    h.step(&mut [&mut a, &mut b], 5);
    let anims = |p: &Probe| -> Vec<(u8, u8)> {
        p.messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::WeaponAnim { anim, body } => Some((*anim, *body)),
                _ => None,
            })
            .collect()
    };
    assert_eq!(anims(&b), vec![(5, 2)]);
    assert!(anims(&a).is_empty());
}

#[test]
fn client_lw_follows_the_userinfo() {
    let (mut h, mut lw, mut no_lw) = lw_and_no_lw(common::TestGame::default());
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = std::rc::Rc::clone(&seen);
    h.server.game_mut().act(move |eng| {
        log.borrow_mut()
            .extend([eng.client_lw(0), eng.client_lw(1), eng.client_lw(99)]);
    });
    h.step(&mut [&mut lw, &mut no_lw], 5);
    assert_eq!(*seen.borrow(), vec![true, false, false]);
}

fn cmd(msec: u8) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        ..Default::default()
    }
}

#[test]
fn random_seed_follows_rehlds_and_halves_share_it() {
    // `SV_ParseMove`: the commands are numbered from the newest (0); a new command runs with
    // `incoming_sequence - index`.
    let cmds = [cmd(10), cmd(11), cmd(12)];
    let run = select_cmds_with_seeds(&cmds, 0, 0, &cmd(99), 1000);
    let seeds: Vec<u32> = run.iter().map(|(_, s)| *s).collect();
    assert_eq!(seeds, [998, 999, 1000]);
    assert_eq!(run[0].0.msec, 12);

    // One new command and two backups; three packets lost. `SV_RunDroppedCmds`: the last command
    // for the loss the backups do not cover, with seed 0; then the backups as
    // `incoming_sequence - cmdnum` (cmdnum 2, then 1), oldest first; then the new command.
    let cmds = [cmd(10), cmd(11), cmd(12)];
    let run = select_cmds_with_seeds(&cmds, 2, 3, &cmd(99), 1000);
    let seeds: Vec<u32> = run.iter().map(|(_, s)| *s).collect();
    assert_eq!(seeds, [0, 998, 999, 1000]);
    let msecs: Vec<u8> = run.iter().map(|(c, _)| c.msec).collect();
    assert_eq!(msecs, [99, 12, 11, 10]);

    // The seed wraps like the C `int` -> `unsigned` it ends as.
    let run = select_cmds_with_seeds(&cmds[..1], 0, 0, &cmd(99), 0);
    assert_eq!(run[0].1, 0);
    let run = select_cmds_with_seeds(&cmds, 0, 0, &cmd(99), 1);
    assert_eq!(run.iter().map(|(_, s)| *s).collect::<Vec<_>>(), [u32::MAX, 0, 1]);
}

#[test]
fn split_commands_run_both_halves_with_the_same_seed() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    let before = h.server.game().random_seeds.len();
    probe.queue_cmd(GoldSrcUserCmd {
        msec: 100,
        forwardmove: 250.0,
        ..Default::default()
    });
    h.step(&mut [&mut probe], 1);
    h.step(&mut [&mut probe], 10);
    let game = h.server.game();
    let split: Vec<u32> = game.usercmd_msecs[before..]
        .iter()
        .zip(&game.random_seeds[before..])
        .filter(|(ms, _)| **ms != 0)
        .map(|(_, seed)| *seed)
        .collect();
    assert_eq!(split.len(), 2, "{split:?}");
    assert_eq!(split[0], split[1]);
}
