//! The objective zones (`func_bomb_target`, `func_hostage_rescue`, `func_escapezone`,
//! `func_vip_safetyzone` and the legacy `info_*` points), `info_map_parameters`, the map flags of
//! `CheckMapConditions` and the zone icons of `HandleSignals`. Time is counted in server frames
//! of 10 ms; the probe is in slot 0. The stub maps have no collision, so the tests switch gravity
//! off to keep the player where it was put and give the zones their box by hand.

mod common;

use std::collections::BTreeSet;

use common::{Bench, command, discover, entity, map_named, stub_spawns};
use cs_assets::bsp::Entity;
use cs_cvar::CVarRegistry;
use cs_engine::consts::{SOLID_NOT, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi, Server, ServerConfig};
use cs_game::buy::{SIGNAL_BOMB, SIGNAL_BUY, SIGNAL_ESCAPE, SIGNAL_RESCUE, SIGNAL_VIPSAFETY};
use cs_game::player::{DEAD_NO, Team};
use cs_game::zones::MapInfo;
use cs_game::CsGame;
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

/// `DHF_IN_TARGET_ZONE`, `DHF_IN_RESCUE_ZONE`, `DHF_IN_ESCAPE_ZONE`, `DHF_IN_VIPSAFETY_ZONE`.
const DHF_IN_TARGET_ZONE: u32 = 1 << 16;
const DHF_IN_RESCUE_ZONE: u32 = 1 << 17;
const DHF_IN_ESCAPE_ZONE: u32 = 1 << 18;
const DHF_IN_VIPSAFETY_ZONE: u32 = 1 << 19;

const STATUSICON_HIDE: u8 = 0;
const STATUSICON_SHOW: u8 = 1;
const STATUSICON_FLASH: u8 = 2;
const GREEN: Option<[u8; 3]> = Some([0, 160, 0]);

/// Where the zones of [`all_zones`] are: far from the spawn points of the stub map (so the
/// legacy checks stay out of it) and from each other.
const BOMB_AT: Vec3 = Vec3::new(3000.0, 0.0, 0.0);
const RESCUE_AT: Vec3 = Vec3::new(3000.0, 3000.0, 0.0);
const ESCAPE_AT: Vec3 = Vec3::new(0.0, 3000.0, 0.0);
const VIP_AT: Vec3 = Vec3::new(-3000.0, 3000.0, 0.0);
const NOWHERE: Vec3 = Vec3::new(0.0, -5000.0, 0.0);

fn zone(class: &str, origin: &str) -> Entity {
    entity(&[("classname", class), ("origin", origin)])
}

/// A stub map with the spawn points and one zone of each class.
fn all_zones() -> Vec<Entity> {
    let mut map = stub_spawns();
    map.push(zone("func_bomb_target", "3000 0 0"));
    map.push(zone("func_hostage_rescue", "3000 3000 0"));
    map.push(zone("func_escapezone", "0 3000 0"));
    map.push(zone("func_vip_safetyzone", "-3000 3000 0"));
    map
}

/// The zones of a stub map have no brush model: give them their box (150 units each way).
fn size_zones(eng: &mut EngineApi<'_>) {
    for id in eng.entity_ids() {
        let Some(v) = eng.ent(id) else { continue };
        if v.solid != SOLID_TRIGGER {
            continue;
        }
        let origin = v.origin;
        eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
        eng.set_origin(id, origin);
    }
}

/// A bench on a stub map of `entities` whose probe is in the game and whose players do not fall.
fn bench(entities: Vec<Entity>) -> Bench {
    let mut b = Bench::stub(entities);
    b.idle_cmds = true;
    b.step(6);
    // The movement takes its gravity from the `sv_gravity` cvar (`movevars`).
    b.run(|_, eng| {
        eng.set_cvar("sv_gravity", "0");
        size_zones(eng);
    });
    b.probe.clear_messages();
    b
}

/// Spawns the probe at `origin` in one frame, so it never touches a zone it is not in.
fn spawn_at(b: &mut Bench, team: Team, origin: Vec3) {
    assert!(b.run(move |game, eng| {
        let spawned = game.spawn_player(eng, 0, team, 0);
        game.place_player(eng, 0, origin) && spawned
    }));
}

fn place(b: &mut Bench, origin: Vec3) {
    assert!(b.run(move |game, eng| game.place_player(eng, 0, origin)));
}

/// A client in a zone knows it after the half second `HandleSignals` waits (twice that when it
/// has just left one: the last touch may still be pending at the first look).
fn settle(b: &mut Bench) {
    b.step(120);
}

fn with_player<T>(b: &Bench, f: impl FnOnce(&cs_game::player::Player) -> T) -> T {
    f(b.server.game().game.state().players[0].as_ref().unwrap())
}

fn set_player(b: &mut Bench, f: impl FnOnce(&mut cs_game::player::Player)) {
    f(b.server.game_mut().game.state_mut().players[0].as_mut().unwrap());
}

fn signals(b: &Bench) -> u8 {
    with_player(b, |p| p.test_zone_signals())
}

/// The `StatusIcon` messages of `sprite` the probe received, as (status, colour).
fn icons(b: &Bench, sprite: &str) -> Vec<(u8, Option<[u8; 3]>)> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::StatusIcon {
                status,
                sprite_name,
                color,
            } if sprite_name == sprite => Some((status, color)),
            _ => None,
        })
        .collect()
}

fn centre_texts(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg {
                dest: 4, message, ..
            } => Some(message),
            _ => None,
        })
        .collect()
}

fn find(b: &mut Bench, class: &'static str) -> EdictId {
    b.run(move |_, eng| eng.find_by_classname(None, class).expect(class))
}

#[test]
fn zones_raise_their_signals_and_icons() {
    let mut b = bench(all_zones());

    // A hostage rescue zone: any player signals it; a counter-terrorist is told what it is, once.
    spawn_at(&mut b, Team::Ct, RESCUE_AT);
    settle(&mut b);
    assert_eq!(signals(&b) & !SIGNAL_BUY, SIGNAL_RESCUE);
    assert_eq!(icons(&b, "rescue"), [(STATUSICON_SHOW, GREEN)]);
    assert_ne!(
        b.server.game().game.display_history(0).unwrap() & DHF_IN_RESCUE_ZONE,
        0
    );
    b.probe.clear_messages();
    place(&mut b, NOWHERE);
    settle(&mut b);
    assert_eq!(signals(&b), 0);
    assert_eq!(icons(&b, "rescue"), [(STATUSICON_HIDE, None)]);

    // A bomb site: a player with no bomb does not signal it.
    place(&mut b, BOMB_AT);
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_BOMB, 0);
    assert_eq!(icons(&b, "c4"), []);
    // The carrier does: the `c4` icon flashes, the carrier is told once, and the site is
    // remembered.
    let site = find(&mut b, "func_bomb_target");
    set_player(&mut b, |p| p.has_c4 = true);
    b.probe.clear_messages();
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_BOMB, SIGNAL_BOMB);
    assert_eq!(icons(&b, "c4"), [(STATUSICON_FLASH, GREEN)]);
    assert_eq!(with_player(&b, |p| p.cur_bomb_target), Some(site));
    assert_ne!(
        b.server.game().game.display_history(0).unwrap() & DHF_IN_TARGET_ZONE,
        0
    );
    // Leaving it shows the icon steadily (`BombTargetFlash_Clear` is `SetBombIcon(FALSE)`).
    b.probe.clear_messages();
    place(&mut b, NOWHERE);
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_BOMB, 0);
    assert_eq!(icons(&b, "c4"), [(STATUSICON_SHOW, GREEN)]);

    // An escape zone: a counter-terrorist signals it and is told what it is.
    b.probe.clear_messages();
    place(&mut b, ESCAPE_AT);
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_ESCAPE, SIGNAL_ESCAPE);
    assert_eq!(icons(&b, "escape"), [(STATUSICON_SHOW, GREEN)]);
    assert_ne!(
        b.server.game().game.display_history(0).unwrap() & DHF_IN_ESCAPE_ZONE,
        0
    );
    assert!(!with_player(&b, |p| p.escaped));
    place(&mut b, NOWHERE);
    settle(&mut b);
    assert_eq!(icons(&b, "escape").last(), Some(&(STATUSICON_HIDE, None)));

    // A VIP safety zone: a player that is not the VIP only gets the icon.
    b.probe.clear_messages();
    place(&mut b, VIP_AT);
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_VIPSAFETY, SIGNAL_VIPSAFETY);
    assert_eq!(icons(&b, "vipsafety"), [(STATUSICON_SHOW, GREEN)]);
    assert_ne!(
        b.server.game().game.display_history(0).unwrap() & DHF_IN_VIPSAFETY_ZONE,
        0
    );
    assert!(!with_player(&b, |p| p.escaped));
    let id = b.run(|_, eng| eng.client_edict(0));
    assert_eq!(b.run(move |_, eng| eng.ent(id).unwrap().solid), cs_engine::consts::SOLID_SLIDEBOX);
}

#[test]
fn the_vip_that_reaches_a_safety_zone_disappears_and_is_paid() {
    let mut b = bench(all_zones());
    spawn_at(&mut b, Team::Ct, NOWHERE);
    set_player(&mut b, |p| {
        p.is_vip = true;
        p.money = 1000;
    });
    b.probe.clear_messages();
    place(&mut b, VIP_AT);
    b.step(10);
    assert!(with_player(&b, |p| p.escaped));
    assert_eq!(with_player(&b, |p| p.money), 3500);
    let id = b.run(|_, eng| eng.client_edict(0));
    let (dead, solid) = b.run(move |_, eng| {
        let v = eng.ent(id).unwrap();
        (v.deadflag, v.solid)
    });
    // `DEAD_DYING` at first (`PlayerDeathThink` makes it `DEAD_DEAD` within a few frames).
    assert_ne!(dead, DEAD_NO);
    assert_eq!(solid, SOLID_NOT);
}

#[test]
fn a_terrorist_escapes_once_and_the_team_is_told() {
    let mut b = bench(all_zones());
    spawn_at(&mut b, Team::Terrorist, NOWHERE);
    b.probe.clear_messages();
    place(&mut b, ESCAPE_AT);
    b.step(5);
    assert!(with_player(&b, |p| p.escaped));
    // `CEscapeZone::EscapeTouch` calls `CheckWinConditions` (`#Terrorists_Escaped`, `MsgDest::All` ->
    // server reliable datagram) before sending `#Terrorist_Escaped` (`MsgDest::One(slot)` -> per-client
    // reliable buffer); per-client reliable messages precede server-wide reliable messages in the packet.
    assert_eq!(
        centre_texts(&b),
        ["#Terrorist_Escaped", "#Terrorists_Escaped"]
    );
    // It does not escape twice, and a terrorist never signals the zone.
    b.probe.clear_messages();
    b.step(5);
    assert_eq!(centre_texts(&b), [] as [String; 0]);
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_ESCAPE, 0);
    assert_eq!(icons(&b, "escape"), []);
}

fn flags(b: &Bench) -> [bool; 5] {
    let r = &b.server.game().game.state().rules;
    [
        r.map_has_bomb_target(),
        r.map_has_bomb_zone(),
        r.map_has_rescue_zone(),
        r.map_has_escape_zone(),
        r.map_has_vip_safety_zone(),
    ]
}

fn flags_of(extra: &[Entity]) -> [bool; 5] {
    let mut map = stub_spawns();
    map.extend_from_slice(extra);
    flags(&Bench::stub(map))
}

#[test]
fn map_flags_as_check_map_conditions() {
    // [bomb target, bomb zone, rescue zone, escape zone, VIP safety zone]
    assert_eq!(flags_of(&[]), [false; 5]);
    // A `func_bomb_target` is both; the legacy `info_bomb_target` is the target only.
    assert_eq!(
        flags_of(&[zone("func_bomb_target", "0 0 0")]),
        [true, true, false, false, false]
    );
    assert_eq!(
        flags_of(&[zone("info_bomb_target", "0 0 0")]),
        [true, false, false, false, false]
    );
    assert_eq!(
        flags_of(&[zone("info_bomb_target", "0 0 0"), zone("func_bomb_target", "0 0 0")]),
        [true, true, false, false, false]
    );
    assert_eq!(
        flags_of(&[zone("func_hostage_rescue", "0 0 0")]),
        [false, false, true, false, false]
    );
    // The legacy `info_hostage_rescue` does not make a rescue zone.
    assert_eq!(flags_of(&[zone("info_hostage_rescue", "0 0 0")]), [false; 5]);
    assert_eq!(
        flags_of(&[zone("func_escapezone", "0 0 0")]),
        [false, false, false, true, false]
    );
    assert_eq!(
        flags_of(&[zone("func_vip_safetyzone", "0 0 0")]),
        [false, false, false, false, true]
    );
    assert_eq!(flags_of(&all_zones()[5..]), [true, true, true, true, true]);
}

#[test]
fn map_flags_are_looked_at_again_at_every_round_restart() {
    let mut b = bench(stub_spawns());
    assert_eq!(flags(&b), [false; 5]);
    // The map grows a zone (a `UTIL_RestartOther` recreation would): the next round sees it.
    b.run(|_, eng| {
        eng.create_entity("func_escapezone").unwrap();
    });
    assert!(!flags(&b)[3]);
    b.run(|game, eng| game.restart_round_now(eng));
    assert!(flags(&b)[3]);
}

fn rules_of(entities: &[Entity]) -> (f32, bool, bool) {
    let mut map = stub_spawns();
    map.extend_from_slice(entities);
    let b = Bench::stub(map);
    let r = &b.server.game().game.state().rules;
    (r.bomb_radius(), r.ct_cant_buy(), r.t_cant_buy())
}

fn params(pairs: &[(&str, &str)]) -> Entity {
    let mut all = vec![("classname", "info_map_parameters")];
    all.extend_from_slice(pairs);
    entity(&all)
}

#[test]
fn map_parameters_buying_and_bombradius() {
    // No entity: everyone buys, the radius is 500.
    assert_eq!(rules_of(&[]), (500.0, false, false));
    // The entity's own defaults.
    assert_eq!(rules_of(&[params(&[])]), (500.0, false, false));
    // `buying`: 0 everyone, 1 only CTs, 2 only Ts, 3 no one, anything else everyone.
    assert_eq!(rules_of(&[params(&[("buying", "0")])]), (500.0, false, false));
    assert_eq!(rules_of(&[params(&[("buying", "1")])]), (500.0, false, true));
    assert_eq!(rules_of(&[params(&[("buying", "2")])]), (500.0, true, false));
    assert_eq!(rules_of(&[params(&[("buying", "3")])]), (500.0, true, true));
    assert_eq!(rules_of(&[params(&[("buying", "4")])]), (500.0, false, false));
    assert_eq!(rules_of(&[params(&[("buying", "-1")])]), (500.0, false, false));
    // `bombradius` is read as an integer and is at most 2048.
    assert_eq!(rules_of(&[params(&[("bombradius", "1500")])]).0, 1500.0);
    assert_eq!(rules_of(&[params(&[("bombradius", "2048")])]).0, 2048.0);
    assert_eq!(rules_of(&[params(&[("bombradius", "9999")])]).0, 2048.0);
    assert_eq!(rules_of(&[params(&[("bombradius", "12.9")])]).0, 12.0);
    // With two entities the last one created is the map's.
    assert_eq!(
        rules_of(&[params(&[("buying", "3")]), params(&[("buying", "1")])]),
        (500.0, false, true)
    );
}

#[test]
fn map_parameters_refuse_purchases_of_the_team_kept_out() {
    // A buy zone for each team round their spawn points; only the terrorists may buy.
    let mut map = stub_spawns();
    map.push(params(&[("buying", "2")]));
    map.push(entity(&[("classname", "func_buyzone"), ("origin", "200 0 0")]));
    map.push(entity(&[("classname", "func_buyzone"), ("origin", "-200 0 0")]));
    let mut b = bench(map);
    for (team, vest_origin, cant_buy) in [
        (Team::Ct, Vec3::new(200.0, 0.0, 0.0), true),
        (Team::Terrorist, Vec3::new(-200.0, 0.0, 0.0), false),
    ] {
        spawn_at(&mut b, team, vest_origin);
        set_player(&mut b, |p| p.money = 16000);
        settle(&mut b);
        assert_eq!(signals(&b) & SIGNAL_BUY, SIGNAL_BUY);
        b.probe.clear_messages();
        command(&mut b, 0, "vest");
        b.step(5);
        let bought = with_player(&b, |p| p.money) < 16000;
        let told = centre_texts(&b);
        if cant_buy {
            assert!(!bought);
            assert_eq!(told, ["#CT_cant_buy"]);
        } else {
            assert!(bought, "{told:?}");
        }
    }
    // Every restart takes the parameters again: with no one buying, nobody does.
    b.run(|game, eng| {
        game.state_mut().map_info = Some(MapInfo {
            buying: 3,
            bomb_radius: 700.0,
        });
        game.restart_round_now(eng);
    });
    let r = &b.server.game().game.state().rules;
    assert_eq!((r.bomb_radius(), r.ct_cant_buy(), r.t_cant_buy()), (700.0, true, true));
}

/// A probe of `team` in a buy zone of a stub map with `extra` entities, that tries `vest`: the
/// money it has left and the centre-print texts it got.
fn try_vest(extra: Vec<Entity>, team: Team, vip: bool) -> (i32, Vec<String>) {
    let mut map = stub_spawns();
    map.extend(extra);
    map.push(entity(&[("classname", "func_buyzone"), ("origin", "200 0 0")]));
    let mut b = bench(map);
    spawn_at(&mut b, team, Vec3::new(200.0, 0.0, 0.0));
    set_player(&mut b, |p| {
        p.money = 16000;
        p.is_vip = vip;
    });
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_BUY, SIGNAL_BUY);
    b.probe.clear_messages();
    command(&mut b, 0, "vest");
    b.step(5);
    (with_player(&b, |p| p.money), centre_texts(&b))
}

#[test]
fn buying_3_refuses_both_teams() {
    for (team, text) in [(Team::Ct, "#CT_cant_buy"), (Team::Terrorist, "#Terrorist_cant_buy")] {
        let (money, told) = try_vest(vec![params(&[("buying", "3")])], team, false);
        assert_eq!(money, 16000, "{team:?} bought");
        assert_eq!(told, [text]);
    }
}

#[test]
fn buying_1_refuses_the_terrorists_only() {
    let (money, told) = try_vest(vec![params(&[("buying", "1")])], Team::Terrorist, false);
    assert_eq!((money, told), (16000, vec!["#Terrorist_cant_buy".to_string()]));
    let (money, told) = try_vest(vec![params(&[("buying", "1")])], Team::Ct, false);
    assert!(money < 16000, "{told:?}");
}

#[test]
fn the_vip_cannot_buy() {
    let (money, told) = try_vest(vec![], Team::Ct, true);
    assert_eq!((money, told), (16000, vec!["#VIP_cant_buy".to_string()]));
}

#[test]
fn a_terrorist_in_a_vip_safety_zone_is_told_once_what_it_is() {
    let mut b = bench(all_zones());
    spawn_at(&mut b, Team::Terrorist, VIP_AT);
    settle(&mut b);
    let hints = |b: &Bench| -> Vec<Vec<u8>> {
        b.messages()
            .iter()
            .filter_map(|m| match m {
                CsUserMsg::Unknown { name, payload } if name == "HudTextArgs" => {
                    Some(payload.clone())
                }
                _ => None,
            })
            .collect()
    };
    let texts: Vec<String> = hints(&b)
        .iter()
        .map(|p| String::from_utf8_lossy(&p[..p.iter().position(|&c| c == 0).unwrap()]).into_owned())
        .collect();
    assert!(texts.contains(&"#Hint_terrorist_vip_zone".to_string()), "{texts:?}");
    // Leaving and coming back does not repeat it (`DHF_IN_VIPSAFETY_ZONE`).
    b.probe.clear_messages();
    place(&mut b, NOWHERE);
    settle(&mut b);
    place(&mut b, VIP_AT);
    settle(&mut b);
    assert!(hints(&b).is_empty());
}

#[test]
fn bomb_target_use_fires_targets() {
    let mut map = stub_spawns();
    map.push(entity(&[
        ("classname", "func_bomb_target"),
        ("origin", "3000 0 0"),
        ("target", "boom"),
    ]));
    map.push(entity(&[
        ("classname", "test_use_counter"),
        ("origin", "0 500 0"),
        ("targetname", "boom"),
    ]));
    map.push(entity(&[
        ("classname", "test_use_counter"),
        ("origin", "0 600 0"),
        ("targetname", "other"),
    ]));
    let mut b = bench(map);
    let site = find(&mut b, "func_bomb_target");
    let uses = |b: &mut Bench| -> Vec<f32> {
        b.run(|_, eng| {
            eng.entity_ids()
                .into_iter()
                .filter(|id| eng.ent(*id).is_some_and(|v| v.classname == "test_use_counter"))
                .map(|id| eng.ent(id).unwrap().health)
                .collect()
        })
    };
    // The entities with the zone's `target` as their name are used, the others are not.
    b.run(move |game, eng| game.test_use_entity(eng, site));
    let mut after = uses(&mut b);
    after.sort_by(f32::total_cmp);
    assert_eq!(after, [0.0, 1.0]);
    b.run(move |game, eng| game.test_use_entity(eng, site));
    let mut after = uses(&mut b);
    after.sort_by(f32::total_cmp);
    assert_eq!(after, [0.0, 2.0]);
}

#[test]
fn old_zone_fallbacks_without_entities() {
    // No `func_bomb_target`, no `func_hostage_rescue`: the points of the legacy maps count, for
    // any player, within 256 units (`<=`).
    let mut map = stub_spawns();
    map.push(zone("info_bomb_target", "3000 0 0"));
    map.push(zone("info_hostage_rescue", "0 3000 0"));
    let mut b = bench(map);
    spawn_at(&mut b, Team::Terrorist, NOWHERE);
    b.probe.clear_messages();
    place(&mut b, BOMB_AT + Vec3::new(256.0, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_BOMB, SIGNAL_BOMB);
    // The player has no bomb: the icon `BombTargetFlash_Set` sends is a hide (`SetBombIcon`),
    // never a show.
    let c4 = icons(&b, "c4");
    assert!(!c4.is_empty() && c4.iter().all(|i| *i == (STATUSICON_HIDE, None)), "{c4:?}");
    place(&mut b, BOMB_AT + Vec3::new(257.0, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_BOMB, 0);

    b.probe.clear_messages();
    place(&mut b, ESCAPE_AT + Vec3::new(0.0, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_RESCUE, SIGNAL_RESCUE);
    assert_eq!(icons(&b, "rescue"), [(STATUSICON_SHOW, GREEN)]);
    place(&mut b, ESCAPE_AT + Vec3::new(0.0, 300.0, 0.0));
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_RESCUE, 0);

    // A map with the entity of the new kind ignores the legacy point.
    let mut map = stub_spawns();
    map.push(zone("info_bomb_target", "3000 0 0"));
    map.push(zone("func_bomb_target", "-3000 -3000 0"));
    let mut b = bench(map);
    spawn_at(&mut b, Team::Ct, BOMB_AT);
    settle(&mut b);
    assert_eq!(signals(&b) & SIGNAL_BOMB, 0);
}

/// The edict numbers that got a baseline on `cs_office` in the original HLDS (build 10210; the
/// controller's capture of 2026-11-23, 218 baselines), as ranges:
///
/// ```text
/// 0-8, 10-62, 64-71, 81-94, 105-144, 149-163, 168, 171, 173-209, 211-242, 244-251
/// ```
const HLDS_RANGES: [(u16, u16); 11] = [
    (0, 8),
    (10, 62),
    (64, 71),
    (81, 94),
    (105, 144),
    (149, 163),
    (168, 168),
    (171, 171),
    (173, 209),
    (211, 242),
    (244, 251),
];

/// What differs from the HLDS list today, because two classes of `cs_office` are not ported yet:
/// `ambient_generic` (3 of them) and `infodecal` (2), both later phases (all 4 `hostage_entity`
/// edicts now match HLDS). A class that is not ported frees its edict, so everything after it
/// moves down. Checked 2026-11-23.
const ONLY_HLDS: [u16; 5] = [247, 248, 249, 250, 251];
const ONLY_OURS: [u16; 5] = [169, 170, 172, 210, 243];

#[test]
fn cs_office_edict_numbers_as_hlds() {
    let Some(vfs) = discover() else {
        return;
    };
    let map = map_named(&vfs, "cs_office");
    let cfg = ServerConfig {
        max_clients: 8,
        ..ServerConfig::default()
    };
    let server = Server::new(
        cfg,
        map,
        common::Rig {
            game: CsGame::new(),
            actions: Vec::new(),
            precache_events: Vec::new(),
        },
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let ours: BTreeSet<u16> = server.baseline_numbers().into_iter().collect();
    let hlds: BTreeSet<u16> = HLDS_RANGES.iter().flat_map(|&(a, b)| a..=b).collect();
    assert_eq!(hlds.len(), 218);
    let only_hlds: Vec<u16> = hlds.difference(&ours).copied().collect();
    let only_ours: Vec<u16> = ours.difference(&hlds).copied().collect();
    // Task 6 (hostages) and the phases of `ambient_generic` and `infodecal` empty these two.
    assert_eq!(only_hlds, ONLY_HLDS);
    assert_eq!(only_ours, ONLY_OURS);
}
