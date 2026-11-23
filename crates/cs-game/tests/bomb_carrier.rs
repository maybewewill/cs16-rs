//! Carrying, dropping, and giving the C4, plus the defuser (`item_thighpack`) (Task 5 of phase 2E).
//!
//! Against ReGameDLL_CS (`player.cpp`, `weapons.cpp`, `items.cpp`, `multiplay_gamerules.cpp`,
//! `client.cpp`, vanilla `!REGAMEDLL_FIXES` / `!REGAMEDLL_ADD` / `!BUILD_LATEST`):
//! - `ScoreAttrib` bomb bit (`SCORE_STATUS_BOMB`), `StatusIcon("c4")` green steady / flashing in zone
//! - Dropping C4 (`DropPlayerItem`, `#Game_bomb_drop`, `BombDrop(DROPPED)`, `CWeaponBox::BombThink`)
//! - Dying carrier drops C4 (`Killed`); disconnecting carrier drops C4 (`ClientDisconnected`)
//! - Picking up dropped C4 (`CWeaponBox::Touch`: T-only, `BombPickup`, `#Game_bomb_pickup`, hints)
//! - Buying defuser (`$200` CT on bomb map, `ItemStatus(2)`, `StatusIcon("defuser")`)
//! - Dropping `item_thighpack` on CT death and CT pickup
//! - `InitHUD` sends `BombDrop(DROPPED)` to late joiners when the bomb lies dropped

mod common;

use common::{Bench, command, commands_of, entity, spawns};
use cs_assets::bsp::Entity;
use cs_engine::consts::{FL_ONGROUND, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi, GameDll, ServerConfig, ServerMap};
use cs_game::player::{Team, dmg};
use cs_game::weapons::{WeaponId, ammo_index};
use cs_game::CsGame;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const SCORE_STATUS_DEAD: u8 = 1 << 0;
const SCORE_STATUS_BOMB: u8 = 1 << 1;

const STATUSICON_HIDE: u8 = 0;
const STATUSICON_SHOW: u8 = 1;
const STATUSICON_FLASH: u8 = 2;
const GREEN: Option<[u8; 3]> = Some([0, 160, 0]);

const CHAN_VOICE: u8 = 2;
const CHAN_ITEM: u8 = 3;

const ZONE_AT: Vec3 = Vec3::new(1000.0, 0.0, 0.0);
const T_SPAWN: Vec3 = Vec3::new(-100.0, 0.0, 0.0);
const CT_SPAWN: Vec3 = Vec3::new(100.0, 0.0, 0.0);

fn bomb_arena(with_buyzones: bool, extra: Vec<Entity>) -> Bench {
    let mut entities = spawns(4, 4);
    entities.push(entity(&[
        ("classname", "func_bomb_target"),
        ("origin", "1000 0 0"),
    ]));
    if with_buyzones {
        entities.push(entity(&[
            ("classname", "func_buyzone"),
            ("origin", "100 0 0"),
            ("team", "2"),
        ]));
        entities.push(entity(&[
            ("classname", "func_buyzone"),
            ("origin", "-100 0 0"),
            ("team", "1"),
        ]));
    }
    entities.extend(extra);
    let mut b = Bench::with_config(
        ServerMap::stub_with_entities("test", entities),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        false,
    );
    b.run(|_game, eng| {
        eng.set_cvar("sv_gravity", "0");
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
                eng.set_origin(id, origin);
            }
        }
    });
    join_pair(&mut b, Team::Terrorist, Team::Ct);
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, T_SPAWN);
        game.place_player(eng, 1, CT_SPAWN);
    });
    b.step(12);
    b.probe.clear_messages();
    b
}

fn join_pair(b: &mut Bench, slot0_team: Team, slot1_team: Team) {
    b.run(move |game, eng| {
        game.client_put_in_server(eng, 1);
        eng.test_set_client_active(1, true);
        commands_of(game, eng, 0, 2);
        commands_of(game, eng, 1, 2);
        let team_cmd = |t: Team| match t {
            Team::Terrorist => "1".to_string(),
            _ => "2".to_string(),
        };
        game.client_command(eng, 0, &["jointeam".to_string(), team_cmd(slot0_team)]);
        game.client_command(eng, 0, &["joinclass".to_string(), "1".to_string()]);
        game.client_command(eng, 1, &["jointeam".to_string(), team_cmd(slot1_team)]);
        game.client_command(eng, 1, &["joinclass".to_string(), "1".to_string()]);
        commands_of(game, eng, 0, 1);
        commands_of(game, eng, 1, 1);
    });
}

fn cmd_slot(game: &mut CsGame, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let viewangles = eng.ent(id).unwrap().v_angle;
    let cmd = GoldSrcUserCmd {
        msec: 10,
        viewangles,
        ..Default::default()
    };
    game.run_usercmd(eng, slot, &cmd, 0);
    if let Some((pmin, pmax)) = eng.ent(id).map(|v| (v.absmin, v.absmax)) {
        for other in eng.entity_ids() {
            if other == id {
                continue;
            }
            if let Some(o) = eng.ent(other)
                && o.solid == SOLID_TRIGGER
                && o.absmin.x <= pmax.x
                && o.absmax.x >= pmin.x
                && o.absmin.y <= pmax.y
                && o.absmax.y >= pmin.y
                && o.absmin.z <= pmax.z
                && o.absmax.z >= pmin.z
            {
                game.touch(eng, other, id);
            }
        }
    }
    game.post_usercmd(eng, slot, &cmd);
}

fn step_cmd(b: &mut Bench, slot: u8, frames: usize) {
    for _ in 0..frames {
        b.run(move |game, eng| cmd_slot(game, eng, slot));
    }
}

fn probe_msgs_from(b: &Bench) -> Vec<CsUserMsg> {
    b.messages()
}

fn find_class(b: &mut Bench, class: &'static str) -> Vec<EdictId> {
    b.run(move |_, eng| {
        let mut out = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, class) {
            out.push(id);
            after = Some(id);
        }
        out
    })
}

fn sound_id(b: &mut Bench, sample: &str) -> u16 {
    let sample = sample.to_string();
    b.run(move |_, eng| eng.precache_sound(&sample).expect("precached sound"))
}

fn sound_indices(b: &Bench) -> Vec<(u16, u8)> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Sound(s) => Some((s.sound_index, s.channel)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_carrier_has_the_bomb_attrib_and_icon() {
    let mut b = bomb_arena(false, vec![]);
    // `restart_round_now` already invokes `GiveC4 -> MakeBomber`; verify a fresh restart emits
    // `StatusIcon("c4")` and `ScoreAttrib(BOMB)` and that giving a second C4 fails.
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, T_SPAWN);
        game.place_player(eng, 1, CT_SPAWN);
        assert!(!game.give_c4(eng, 0));
    });
    b.step(12);

    b.run(|game, eng| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.has_c4);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().body, 1);
        assert!(game.carried_weapons(0).contains(&WeaponId::C4));
    });

    let msgs = probe_msgs_from(&b);
    assert!(msgs.contains(&CsUserMsg::StatusIcon {
        status: STATUSICON_SHOW,
        sprite_name: "c4".to_string(),
        color: GREEN,
    }));
    assert!(msgs.contains(&CsUserMsg::ScoreAttrib {
        player_id: 1,
        flags: SCORE_STATUS_BOMB,
    }));

    // Entering `func_bomb_target` flips the icon to flashing after HandleSignals (0.5 s).
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.place_player(eng, 0, ZONE_AT);
    });
    step_cmd(&mut b, 0, 70);
    b.step(12);
    assert!(probe_msgs_from(&b).contains(&CsUserMsg::StatusIcon {
        status: STATUSICON_FLASH,
        sprite_name: "c4".to_string(),
        color: GREEN,
    }));

    // Leaving returns it to steady green (wait up to two 0.5 s HandleSignals cycles to clear).
    b.probe.clear_messages();
    b.run(|game, eng| {
        game.place_player(eng, 0, T_SPAWN);
    });
    step_cmd(&mut b, 0, 120);
    b.step(12);
    assert!(probe_msgs_from(&b).contains(&CsUserMsg::StatusIcon {
        status: STATUSICON_SHOW,
        sprite_name: "c4".to_string(),
        color: GREEN,
    }));
}

#[test]
fn dropping_the_bomb_sends_bomb_drop() {
    let mut b = bomb_arena(false, vec![]);
    assert!(b.run(|game, _| game.state().players[0].as_ref().unwrap().has_c4));

    b.run(|game, eng| {
        game.client_command(eng, 0, &["drop".to_string(), "weapon_c4".to_string()]);
        let p = game.state().players[0].as_ref().unwrap();
        assert!(!p.has_c4);
        assert!(game.state().rules.bomb_dropped);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().body, 0);
        assert!(!game.carried_weapons(0).contains(&WeaponId::C4));
    });
    b.step(12);

    let msgs = probe_msgs_from(&b);
    assert!(msgs.contains(&CsUserMsg::StatusIcon {
        status: STATUSICON_HIDE,
        sprite_name: "c4".to_string(),
        color: None,
    }));
    assert!(msgs.contains(&CsUserMsg::ScoreAttrib {
        player_id: 1,
        flags: 0,
    }));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::BombDrop { origin, planted: false } if (*origin - T_SPAWN).length() < 5.0
    )));
    assert!(msgs.iter().any(|m| matches!(
        m,
        CsUserMsg::TextMsg { dest: 4, message, .. } if message == "#Game_bomb_drop"
    )));

    // `CWeaponBox::BombThink` repeats `BombDrop(DROPPED)` every 1.0 s instead of expiring.
    let boxes = find_class(&mut b, "weaponbox");
    assert_eq!(boxes.len(), 1);
    let box_id = boxes[0];
    b.run(move |_, eng| {
        let model = eng.model_index("models/w_backpack.mdl").unwrap();
        assert_eq!(eng.ent(box_id).unwrap().modelindex, model);
    });
    b.probe.clear_messages();
    b.step(220);
    let periodic_drops = probe_msgs_from(&b)
        .into_iter()
        .filter(|m| matches!(m, CsUserMsg::BombDrop { planted: false, .. }))
        .count();
    assert!(periodic_drops >= 2, "expected >= 2 BombThink ticks, got {periodic_drops}");
    assert_eq!(find_class(&mut b, "weaponbox").len(), 1);
}

#[test]
fn a_dead_carrier_drops_the_backpack() {
    let mut b = bomb_arena(false, vec![]);
    // Join a second Terrorist in slot 2 so killing carrier 0 does not end the round
    // (`DropPlayerItem` only sets `m_bBombDropped` while `m_flRestartRoundTime == 0`).
    b.run(|game, eng| {
        game.client_put_in_server(eng, 2);
        eng.test_set_client_active(2, true);
        commands_of(game, eng, 2, 2);
        game.client_command(eng, 2, &["jointeam".to_string(), "1".to_string()]);
        game.client_command(eng, 2, &["joinclass".to_string(), "1".to_string()]);
        commands_of(game, eng, 2, 1);
        game.restart_round_now(eng);
        // First restart after adding T slot 2 rotates C4 to slot 2; restart once more to rotate back to slot 0.
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, T_SPAWN);
        game.place_player(eng, 1, CT_SPAWN);
        game.place_player(eng, 2, Vec3::new(-300.0, 0.0, 0.0));
    });
    b.run(|game, _| {
        assert!(game.state().players[0].as_ref().unwrap().has_c4);
        assert!(!game.state().players[2].as_ref().unwrap().has_c4);
    });
    b.step(12);
    b.probe.clear_messages();

    b.run(|game, eng| {
        assert!(!game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET));
        let p = game.state().players[0].as_ref().unwrap();
        assert!(!p.has_c4);
        assert!(game.state().rules.bomb_dropped);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().body, 0);
    });
    b.step(12);

    let boxes = find_class(&mut b, "weaponbox");
    assert_eq!(boxes.len(), 1);
    let msgs = probe_msgs_from(&b);
    assert!(msgs.contains(&CsUserMsg::ScoreAttrib {
        player_id: 1,
        flags: SCORE_STATUS_DEAD,
    }));

    // Disconnecting carrier also drops C4.
    let mut b2 = bomb_arena(false, vec![]);
    b2.run(|game, _| {
        assert!(game.state().players[0].as_ref().unwrap().has_c4);
    });
    b2.run(|game, eng| {
        game.client_disconnect(eng, 0);
        assert!(game.state().rules.bomb_dropped);
    });
    assert_eq!(find_class(&mut b2, "weaponbox").len(), 1);
}

#[test]
fn only_terrorists_pick_up_the_bomb() {
    let mut b = bomb_arena(false, vec![]);
    assert!(b.run(|game, _| game.state().players[0].as_ref().unwrap().has_c4));
    b.run(|game, eng| {
        game.client_command(eng, 0, &["drop".to_string(), "weapon_c4".to_string()]);
    });
    // Let the dropped box settle and run think so its bbox widens (while on ground).
    let box_id = find_class(&mut b, "weaponbox")[0];
    b.run(move |_, eng| {
        let world = eng.world();
        let v = eng.ent_mut(box_id).unwrap();
        v.flags |= FL_ONGROUND;
        v.groundentity = Some(world);
        v.velocity = Vec3::ZERO;
    });
    b.step(15);

    // CT touches the box: rejected!
    b.run(move |game, eng| {
        let box_pos = eng.ent(box_id).unwrap().origin;
        game.place_player(eng, 1, box_pos);
        game.touch(eng, box_id, eng.client_edict(1));
        assert!(eng.ent(box_id).is_some(), "CT must not pick up C4");
        assert!(!game.state().players[1].as_ref().unwrap().has_c4);
        assert!(game.state().rules.bomb_dropped);
    });

    // T touches the box: picks it up, removes the box, sends BombPickup.
    b.probe.clear_messages();
    b.run(move |game, eng| {
        let box_pos = eng.ent(box_id).unwrap().origin;
        game.place_player(eng, 0, box_pos);
        game.touch(eng, box_id, eng.client_edict(0));
        assert!(eng.ent(box_id).is_none(), "weaponbox must be removed on C4 pickup");
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.has_c4);
        assert!(!game.state().rules.bomb_dropped);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().body, 1);
        assert!(game.carried_weapons(0).contains(&WeaponId::C4));
        let idx = usize::try_from(ammo_index("C4")).unwrap();
        assert_eq!(game.player_ammo(0, idx), Some(1));
    });
    b.step(12);

    let msgs = probe_msgs_from(&b);
    assert!(msgs.contains(&CsUserMsg::BombPickup));
    assert!(msgs.contains(&CsUserMsg::StatusIcon {
        status: STATUSICON_SHOW,
        sprite_name: "c4".to_string(),
        color: GREEN,
    }));
    assert!(msgs.contains(&CsUserMsg::ScoreAttrib {
        player_id: 1,
        flags: SCORE_STATUS_BOMB,
    }));
}

#[test]
fn defuser_purchase_as_regamedll() {
    // Bomb map with slot 0 = CT, slot 1 = T.
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        common::spawn_at("info_player_start", "100 0 0"),
        common::spawn_at("info_player_deathmatch", "-100 0 0"),
        entity(&[("classname", "func_bomb_target"), ("origin", "1000 0 0")]),
        entity(&[("classname", "func_buyzone"), ("origin", "100 0 0"), ("team", "2")]),
        entity(&[("classname", "func_buyzone"), ("origin", "-100 0 0"), ("team", "1")]),
    ];
    let mut b = Bench::with_config(
        ServerMap::stub_with_entities("test", entities),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        false,
    );
    b.run(|_game, eng| {
        eng.set_cvar("sv_gravity", "0");
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
                eng.set_origin(id, origin);
            }
        }
    });
    join_pair(&mut b, Team::Ct, Team::Terrorist);
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, CT_SPAWN);
        game.place_player(eng, 1, T_SPAWN);
        game.state_mut().players[0].as_mut().unwrap().money = 2000;
    });
    step_cmd(&mut b, 0, 60);
    b.step(12);
    b.probe.clear_messages();

    // CT buys defuser via equipment menu (buyequip -> menuselect 7).
    command(&mut b, 0, "buyequip");
    command(&mut b, 0, "menuselect 7");
    b.step(12);

    b.run(|game, eng| {
        let p = game.state().players[0].as_ref().unwrap();
        assert!(p.has_defuser);
        assert_eq!(p.money, 1800);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().body, 1);
    });

    let kevlar_snd = sound_id(&mut b, "items/kevlar.wav");
    assert!(sound_indices(&b).contains(&(kevlar_snd, CHAN_ITEM)));
    let msgs = probe_msgs_from(&b);
    assert!(msgs.contains(&CsUserMsg::ItemStatus(2)));
    assert!(msgs.contains(&CsUserMsg::StatusIcon {
        status: STATUSICON_SHOW,
        sprite_name: "defuser".to_string(),
        color: GREEN,
    }));

    // Buying NVG alongside defuser combines bits (1 | 2 = 3).
    b.probe.clear_messages();
    command(&mut b, 0, "nvgs");
    b.step(12);
    assert!(probe_msgs_from(&b).contains(&CsUserMsg::ItemStatus(3)));

    // Second defuser purchase fails with #Already_Have_One.
    b.probe.clear_messages();
    command(&mut b, 0, "defuser");
    b.step(12);
    assert!(probe_msgs_from(&b).iter().any(|m| matches!(
        m,
        CsUserMsg::TextMsg { dest: 4, message, .. } if message == "#Already_Have_One"
    )));
}

#[test]
fn defuser_drops_at_death() {
    let entities = vec![
        entity(&[("classname", "worldspawn")]),
        common::spawn_at("info_player_start", "100 0 0"),
        common::spawn_at("info_player_start", "300 0 0"),
        common::spawn_at("info_player_deathmatch", "-100 0 0"),
        entity(&[("classname", "func_bomb_target"), ("origin", "1000 0 0")]),
        entity(&[("classname", "func_buyzone"), ("origin", "100 0 0"), ("team", "2")]),
    ];
    let mut b = Bench::with_config(
        ServerMap::stub_with_entities("test", entities),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        false,
    );
    b.run(|_game, eng| {
        eng.set_cvar("sv_gravity", "0");
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
                eng.set_origin(id, origin);
            }
        }
    });
    join_pair(&mut b, Team::Ct, Team::Terrorist);
    // Also join slot 2 as CT so a teammate can pick up the dropped defuser.
    b.run(|game, eng| {
        game.client_put_in_server(eng, 2);
        eng.test_set_client_active(2, true);
        commands_of(game, eng, 2, 2);
        game.client_command(eng, 2, &["jointeam".to_string(), "2".to_string()]);
        game.client_command(eng, 2, &["joinclass".to_string(), "1".to_string()]);
        commands_of(game, eng, 2, 1);
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.place_player(eng, 0, CT_SPAWN);
        game.place_player(eng, 1, T_SPAWN);
        game.place_player(eng, 2, Vec3::new(300.0, 0.0, 0.0));
        game.state_mut().players[0].as_mut().unwrap().money = 1000;
    });
    step_cmd(&mut b, 0, 60);
    command(&mut b, 0, "defuser");
    b.step(12);
    b.probe.clear_messages();

    // Kill CT0: defuser removed, icon hidden, `item_thighpack` dropped at corpse origin.
    b.run(|game, eng| {
        assert!(!game.damage_player(eng, 0, Some(1), 500.0, dmg::BULLET));
        let p = game.state().players[0].as_ref().unwrap();
        assert!(!p.has_defuser);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().body, 0);
    });
    b.step(12);

    let msgs = probe_msgs_from(&b);
    assert!(msgs.contains(&CsUserMsg::ItemStatus(0)));
    assert!(msgs.contains(&CsUserMsg::StatusIcon {
        status: STATUSICON_HIDE,
        sprite_name: "defuser".to_string(),
        color: None,
    }));

    let kits = find_class(&mut b, "item_thighpack");
    assert_eq!(kits.len(), 1);
    let kit_id = kits[0];

    // Terrorist cannot pick up `item_thighpack`.
    b.run(move |game, eng| {
        let pos = eng.ent(kit_id).unwrap().origin;
        game.place_player(eng, 1, pos);
        game.touch(eng, kit_id, eng.client_edict(1));
        assert!(eng.ent(kit_id).is_some());
        assert!(!game.state().players[1].as_ref().unwrap().has_defuser);
    });

    // CT2 picks up `item_thighpack` and removes it.
    let kit_snd = sound_id(&mut b, "items/kevlar.wav");
    b.probe.clear_messages();
    b.run(move |game, eng| {
        let pos = eng.ent(kit_id).unwrap().origin;
        game.place_player(eng, 2, pos);
        game.touch(eng, kit_id, eng.client_edict(2));
        assert!(eng.ent(kit_id).is_none());
        let p2 = game.state().players[2].as_ref().unwrap();
        assert!(p2.has_defuser);
        assert_eq!(eng.ent(eng.client_edict(2)).unwrap().body, 1);
    });
    b.step(12);
    // Sound plays from CT2's edict on CHAN_VOICE (`items/kevlar.wav`).
    assert!(sound_indices(&b).contains(&(kit_snd, CHAN_VOICE)));

    // Surviving CT2 keeps defuser across round restart (`body = 1`); CT0 spawns without it.
    b.run(|game, eng| {
        game.restart_round_now(eng);
        assert!(!game.state().players[0].as_ref().unwrap().has_defuser);
        assert_eq!(eng.ent(eng.client_edict(0)).unwrap().body, 0);
        assert!(game.state().players[2].as_ref().unwrap().has_defuser);
        assert_eq!(eng.ent(eng.client_edict(2)).unwrap().body, 1);
    });
}

#[test]
fn init_hud_shows_a_dropped_bomb() {
    let mut b = bomb_arena(false, vec![]);
    assert!(b.run(|game, _| game.state().players[0].as_ref().unwrap().has_c4));
    // `InitHUD` sends `pWeaponC4->pev->origin`: where the weapon was when it was packed (the
    // dropper's origin), not where the weapon box has flown since (`multiplay_gamerules.cpp`
    // 3544): the box is moved away here and the late joiner must still be told the drop spot.
    let expected_origin = b.run(|game, eng| {
        game.place_player(eng, 0, Vec3::new(250.0, -150.0, 0.0));
        let dropper_origin = eng.ent(eng.client_edict(0)).unwrap().origin;
        game.client_command(eng, 0, &["drop".to_string(), "weapon_c4".to_string()]);
        let box_id = eng.find_by_classname(None, "weaponbox").unwrap();
        let v = eng.ent_mut(box_id).unwrap();
        v.velocity = Vec3::ZERO;
        v.flags |= FL_ONGROUND;
        v.origin += Vec3::new(120.0, 80.0, 0.0);
        dropper_origin
    });
    b.step(5);

    // A late-joining player connects and receives `InitHUD`: it gets `BombDrop(DROPPED)`.
    let mut late = b.add_probe("late");
    for _ in 0..40 {
        late.queue_cmd(GoldSrcUserCmd {
            msec: 10,
            ..Default::default()
        });
        b.step_both(&mut late, 1);
    }

    let late_msgs: Vec<CsUserMsg> = late
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::UserMsg { decoded, .. } => Some(decoded.clone()),
            _ => None,
        })
        .collect();
    assert!(
        late_msgs.iter().any(|m| matches!(
            m,
            CsUserMsg::BombDrop { origin, planted: false }
                if (*origin - expected_origin).length() < 1.0
        )),
        "late joiner did not receive BombDrop(DROPPED) at {expected_origin:?}: {late_msgs:?}"
    );
}
