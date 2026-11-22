//! What a player carries and what lies on the ground (`CBasePlayer::AddPlayerItem`,
//! `GiveDefaultItems`, `DropPlayerItem`, `PackDeadPlayerItems`, `CWeaponBox`, `CArmoury`, ReGameDLL_CS,
//! read 2026-11-22): the default items and the messages that tell the client of them, the weapon
//! table of the sign-on, buying weapons and ammo, dropping and picking up, the death's box, the
//! armoury, the selection commands and the commands a hostile client sends. The probe is in
//! slot 0; the stub maps have no collision, so the tests switch gravity off.
//!
//! Weapons are the stubs of their tasks here (the guns' own behaviour is tested with them), so
//! nothing below fires or reloads.

mod common;

use common::{Bench, command, entity, join, stub_spawns};
use cs_engine::consts::FL_ONGROUND;
use cs_engine::{EdictId, GameDll};
use cs_game::inventory::WEAPON_EVENTS;
use cs_game::player::dmg;
use cs_game::weapons::{WeaponId, ammo_index, item_infos};
use cs_proto48::usermsg::CsUserMsg;

/// A stub map with spawn points for both teams (and `extra` entities), the probe in the game
/// and gravity off.
fn bench_with(extra: Vec<cs_assets::bsp::Entity>) -> Bench {
    let mut map = stub_spawns();
    map.extend(extra);
    let mut b = Bench::stub(map);
    b.idle_cmds = true;
    b.step(6);
    b.run(|_, eng| eng.set_cvar("sv_gravity", "0"));
    b
}

/// The probe joins `team` (1 terrorists, 2 counter-terrorists) and settles, so it knows it
/// stands in a buy zone (a stub map has none, so the spawn points count as one).
fn team_bench(team: u8) -> Bench {
    let mut b = bench_with(Vec::new());
    b.probe.clear_messages();
    join(&mut b, 0, team, 1);
    b.step(120);
    b
}

fn carried(b: &Bench) -> Vec<WeaponId> {
    b.server.game().game.carried_weapons(0)
}

fn money(b: &Bench) -> i32 {
    b.server.game().game.state().players[0].as_ref().unwrap().money
}

fn set_money(b: &mut Bench, amount: i32) {
    b.server.game_mut().game.state_mut().players[0].as_mut().unwrap().money = amount;
}

fn active(b: &Bench) -> Option<WeaponId> {
    b.server.game().game.player_weapon_state(0).unwrap().2
}

fn reserve(b: &Bench, ammo: &str) -> i32 {
    b.server.game().game.player_ammo(0, ammo_index(ammo) as usize).unwrap()
}

fn clip(b: &mut Bench, id: WeaponId) -> i32 {
    b.server.game_mut().game.weapon_mut(0, id).expect("carried").clip
}

/// The `weaponbox` edicts of the map.
fn boxes(b: &mut Bench) -> Vec<EdictId> {
    b.run(|_, eng| {
        let mut found = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "weaponbox") {
            found.push(id);
            after = Some(id);
        }
        found
    })
}

/// The text message names the probe got since it was last cleared.
fn texts(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

/// A command of the probe, then the messages it caused.
fn say(b: &mut Bench, line: &str) -> Vec<String> {
    b.probe.clear_messages();
    command(b, 0, line);
    b.step(5);
    texts(b)
}

#[test]
fn default_items_by_team() {
    // A counter-terrorist: the knife, the USP with its clip and two more clips of .45 ACP.
    let mut b = team_bench(2);
    assert_eq!(carried(&b), [WeaponId::Usp, WeaponId::Knife]);
    // The knife has no clip (`CKnife::Spawn`: `m_iClip = WEAPON_NOCLIP`).
    assert_eq!(clip(&mut b, WeaponId::Knife), -1);
    assert_eq!(clip(&mut b, WeaponId::Usp), 12);
    assert_eq!(reserve(&b, "45acp"), 24);
    // The USP weighs more than the knife, so it was taken out when it was picked up.
    assert_eq!(active(&b), Some(WeaponId::Usp));
    let mask = b.server.game().game.weapons_mask(0).unwrap();
    assert_eq!(mask, ((1 << 31) | WeaponId::Usp.bit() | WeaponId::Knife.bit(), false));

    // The HUD was told in the order of `Spawn`: the HUD starts over in `RemoveAllItems`
    // (`UpdateClientData`), which then sends the empty `CurWeapon`; the two weapons come with
    // `WeapPickup`, the ammo with `AmmoPickup`; the first client data update sends the ammo
    // count and, per weapon in slot order, `CurWeapon` (1 for the active one; the knife's clip is
    // the byte 255).
    let log: Vec<String> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::ResetHud => Some("ResetHUD".to_string()),
            CsUserMsg::CurWeapon { is_active, weapon_id, clip_ammo } => {
                Some(format!("CurWeapon {} {weapon_id} {clip_ammo}", u8::from(is_active)))
            }
            CsUserMsg::WeapPickup(id) => Some(format!("WeapPickup {id}")),
            CsUserMsg::AmmoPickup { ammo_id, amount } => Some(format!("AmmoPickup {ammo_id} {amount}")),
            CsUserMsg::AmmoX { ammo_id, amount } => Some(format!("AmmoX {ammo_id} {amount}")),
            _ => None,
        })
        .collect();
    let ammo = ammo_index("45acp");
    assert_eq!(
        log,
        [
            "ResetHUD".to_string(),
            "CurWeapon 0 0 0".to_string(),
            "WeapPickup 29".to_string(),
            "WeapPickup 16".to_string(),
            format!("AmmoPickup {ammo} 24"),
            format!("AmmoX {ammo} 24"),
            "CurWeapon 1 16 12".to_string(),
            "CurWeapon 0 29 -1".to_string(),
        ]
    );

    // A terrorist: the knife, the Glock 18 and 40 rounds of 9 mm.
    let mut b = team_bench(1);
    assert_eq!(carried(&b), [WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!(clip(&mut b, WeaponId::Glock18), 20);
    assert_eq!(clip(&mut b, WeaponId::Knife), -1);
    assert_eq!(reserve(&b, "9mm"), 40);
    assert_eq!(active(&b), Some(WeaponId::Glock18));
}

#[test]
fn weapon_list_at_join_as_regamedll() {
    let b = bench_with(Vec::new());
    let lists: Vec<CsUserMsg> = b
        .messages()
        .into_iter()
        .filter(|m| matches!(m, CsUserMsg::WeaponList { .. }))
        .collect();
    // One per weapon of the item table, in the order of the weapon ids.
    assert_eq!(lists.len(), item_infos().len());
    let ids: Vec<u8> = lists
        .iter()
        .map(|m| match m {
            CsUserMsg::WeaponList { weapon_id, .. } => *weapon_id,
            _ => unreachable!(),
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    let find = |name: &str| {
        lists
            .iter()
            .find(|m| matches!(m, CsUserMsg::WeaponList { name: n, .. } if n == name))
            .cloned()
            .unwrap_or_else(|| panic!("{name} is not in the list"))
    };
    // The AK47: 7.62 mm up to 90, no second ammo (a byte of -1), slot 0 position 1, id 28.
    assert_eq!(
        find("weapon_ak47"),
        CsUserMsg::WeaponList {
            name: "weapon_ak47".to_string(),
            ammo1_type: ammo_index("762Nato") as u8,
            ammo1_max: 90,
            ammo2_type: 255,
            ammo2_max: 255,
            slot: 0,
            position: 1,
            weapon_id: 28,
            flags: 0,
        }
    );
    // The knife uses no ammo.
    assert_eq!(
        find("weapon_knife"),
        CsUserMsg::WeaponList {
            name: "weapon_knife".to_string(),
            ammo1_type: 255,
            ammo1_max: 255,
            ammo2_type: 255,
            ammo2_max: 255,
            slot: 2,
            position: 1,
            weapon_id: 29,
            flags: 0,
        }
    );
    // The M3 is `NOFIREUNDERWATER`, a flag the client is not told of; a grenade keeps its flags
    // (`LIMITINWORLD | EXHAUSTIBLE`).
    let flags = |name: &str| match find(name) {
        CsUserMsg::WeaponList { flags, .. } => flags,
        _ => unreachable!(),
    };
    assert_eq!(flags("weapon_m3"), 0);
    assert_eq!(flags("weapon_hegrenade"), 8 | 16);
}

#[test]
fn every_weapon_event_resolves_after_activation() {
    let mut b = bench_with(Vec::new());
    let indices = b.run(|_, eng| {
        WEAPON_EVENTS.iter().map(|name| eng.precache_event(name)).collect::<Vec<u16>>()
    });
    assert!(indices.iter().all(|&i| i != 0), "{indices:?}");
    // In the order of `WeaponsPrecache`, one index each, counting from 1.
    assert_eq!(indices, (1..=WEAPON_EVENTS.len() as u16).collect::<Vec<_>>());
}

#[test]
fn buy_drop_pickup_as_regamedll() {
    let mut b = team_bench(1);
    set_money(&mut b, 16000);
    // The AK47 costs 2500; it is heavier than the Glock, so the player takes it out. Bought, it
    // has its clip and no reserve (`AK47_DEFAULT_GIVE`, and `AddPrimaryAmmo` fills the clip first).
    assert_eq!(say(&mut b, "ak47"), Vec::<String>::new());
    assert_eq!(money(&b), 13500);
    assert_eq!(carried(&b), [WeaponId::Ak47, WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!(active(&b), Some(WeaponId::Ak47));
    assert_eq!(clip(&mut b, WeaponId::Ak47), 30);
    assert_eq!(reserve(&b, "762Nato"), 0);
    assert!(b.server.game().game.weapons_mask(0).unwrap().1);
    // Already own it: said, nothing taken.
    assert_eq!(say(&mut b, "ak47"), ["#Cstrike_Already_Own_Weapon"]);
    assert_eq!(money(&b), 13500);

    // `drop`: the active weapon leaves as a box 10 units in front of the player, thrown
    // forward; the player takes the next best weapon and loses the weapon's bit.
    command(&mut b, 0, "drop");
    assert_eq!(carried(&b), [WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!(active(&b), Some(WeaponId::Glock18));
    let mask = b.server.game().game.weapons_mask(0).unwrap();
    assert_eq!(mask.0 & WeaponId::Ak47.bit(), 0);
    assert!(!mask.1, "m_bHasPrimary goes with the primary weapon");
    let found = boxes(&mut b);
    assert_eq!(found.len(), 1);
    let weapon_box = found[0];
    let (model, owner, velocity) = b.run(move |_, eng| {
        let v = eng.ent(weapon_box).unwrap();
        (v.model.clone(), v.owner, v.velocity)
    });
    assert_eq!(model, "models/w_ak47.mdl");
    assert_eq!(owner, Some(b.run(|_, eng| eng.client_edict(0))));
    assert!(velocity.length() > 300.0, "{velocity:?}");

    // A box that has not landed is not picked up.
    b.run(move |game, eng| {
        let me = eng.client_edict(0);
        game.touch(eng, weapon_box, me);
    });
    assert_eq!(carried(&b), [WeaponId::Glock18, WeaponId::Knife]);
    // On the ground it is: the AK47 comes back with the clip it was dropped with, and the
    // box is gone.
    b.run(move |game, eng| {
        eng.ent_mut(weapon_box).unwrap().flags |= FL_ONGROUND;
        let me = eng.client_edict(0);
        game.touch(eng, weapon_box, me);
    });
    assert_eq!(carried(&b), [WeaponId::Ak47, WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!(clip(&mut b, WeaponId::Ak47), 30);
    assert!(boxes(&mut b).is_empty());
    assert_eq!(active(&b), Some(WeaponId::Ak47));
}

#[test]
fn buying_a_primary_drops_the_old_one() {
    let mut b = team_bench(1);
    set_money(&mut b, 16000);
    say(&mut b, "ak47");
    say(&mut b, "sg552");
    assert_eq!(money(&b), 16000 - 2500 - 3500);
    // The new weapon is in, the old one lies in a box.
    assert_eq!(carried(&b), [WeaponId::Sg552, WeaponId::Glock18, WeaponId::Knife]);
    let found = boxes(&mut b);
    assert_eq!(found.len(), 1);
    let model = b.run(move |_, eng| eng.ent(found[0]).unwrap().model.clone());
    assert_eq!(model, "models/w_ak47.mdl");
    // A pistol replaces the pistol the same way.
    say(&mut b, "deagle");
    assert_eq!(carried(&b), [WeaponId::Sg552, WeaponId::Deagle, WeaponId::Knife]);
    assert_eq!(boxes(&mut b).len(), 2);
    // A weapon of the other team, one it cannot afford.
    assert_eq!(say(&mut b, "m4a1"), ["#Alias_Not_Avail"]);
    set_money(&mut b, 100);
    assert_eq!(say(&mut b, "awp"), ["#Not_Enough_Money"]);
    assert_eq!(money(&b), 100);
    assert_eq!(carried(&b), [WeaponId::Sg552, WeaponId::Deagle, WeaponId::Knife]);
}

#[test]
fn ammo_buy_and_caps() {
    let mut b = team_bench(1);
    set_money(&mut b, 16000);
    say(&mut b, "ak47");
    assert_eq!(reserve(&b, "762Nato"), 0);
    // `buyammo1`: one clip (30 rounds for 80).
    say(&mut b, "buyammo1");
    assert_eq!((reserve(&b, "762Nato"), money(&b)), (30, 16000 - 2500 - 80));
    // `primammo`: as many as fit, up to the 90 the AK47 carries.
    say(&mut b, "primammo");
    assert_eq!((reserve(&b, "762Nato"), money(&b)), (90, 16000 - 2500 - 3 * 80));
    // Full: nothing more is bought or taken.
    say(&mut b, "buyammo1");
    assert_eq!((reserve(&b, "762Nato"), money(&b)), (90, 16000 - 2500 - 3 * 80));
    // The pistol's ammo is its own slot: the Glock's 9 mm (20 for 30 rounds, up to 120).
    assert_eq!(reserve(&b, "9mm"), 40);
    say(&mut b, "secammo");
    assert_eq!(reserve(&b, "9mm"), 120);
    // Not enough money for the first clip: said, and the account blinks.
    set_money(&mut b, 10);
    let mut b2 = team_bench(1);
    set_money(&mut b2, 10);
    assert_eq!(say(&mut b2, "buyammo2"), ["#Not_Enough_Money"]);
    assert_eq!((reserve(&b2, "9mm"), money(&b2)), (40, 10));

    // Grenades: two flashbangs fit, the third is refused.
    set_money(&mut b, 1000);
    say(&mut b, "flash");
    say(&mut b, "flash");
    assert_eq!(money(&b), 600);
    assert_eq!(reserve(&b, "Flashbang"), 2);
    assert_eq!(say(&mut b, "flash"), ["#Cannot_Carry_Anymore"]);
    assert_eq!(money(&b), 600);
    // One of each of the others.
    say(&mut b, "hegren");
    assert_eq!(say(&mut b, "hegren"), ["#Cannot_Carry_Anymore"]);
    assert_eq!(money(&b), 300);
    assert!(carried(&b).contains(&WeaponId::HeGrenade) && carried(&b).contains(&WeaponId::Flashbang));
}

#[test]
fn death_packs_a_weaponbox() {
    let mut b = team_bench(1);
    set_money(&mut b, 16000);
    say(&mut b, "ak47");
    // A grenade is not dropped by the vanilla rules.
    say(&mut b, "hegren");
    assert!(boxes(&mut b).is_empty());
    b.run(|game, eng| {
        game.damage_player(eng, 0, None, 500.0, dmg::GENERIC);
    });
    // The player's next death thinks drop the best weapon (the heaviest of the primary and the
    // pistol slots), then everything is taken away, the suit too.
    b.step(30);
    let found = boxes(&mut b);
    assert_eq!(found.len(), 1);
    let model = b.run(move |_, eng| eng.ent(found[0]).unwrap().model.clone());
    assert_eq!(model, "models/w_ak47.mdl");
    assert!(carried(&b).is_empty());
    assert_eq!(b.server.game().game.weapons_mask(0).unwrap(), (0, false));
    assert_eq!(reserve(&b, "762Nato"), 0);
    assert_eq!(active(&b), None);
}

#[test]
fn armoury_gives_items() {
    let mut b = bench_with(vec![
        entity(&[("classname", "armoury_entity"), ("origin", "400 0 0"), ("item", "4"), ("count", "2")]),
        entity(&[("classname", "armoury_entity"), ("origin", "500 0 0"), ("item", "16"), ("count", "1")]),
    ]);
    join(&mut b, 0, 1, 1);
    b.step(10);
    let racks: Vec<EdictId> = b.run(|_, eng| {
        let mut found = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "armoury_entity") {
            found.push(id);
            after = Some(id);
        }
        found
    });
    assert_eq!(racks.len(), 2);
    let touch = |b: &mut Bench, rack: EdictId| {
        b.run(move |game, eng| {
            let me = eng.client_edict(0);
            game.touch(eng, rack, me);
        });
    };
    // An AK47 rack, two guns: the first player takes one, with 60 rounds (and the clip).
    touch(&mut b, racks[0]);
    assert_eq!(carried(&b), [WeaponId::Ak47, WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!((clip(&mut b, WeaponId::Ak47), reserve(&b, "762Nato")), (30, 60));
    // A player with a primary weapon gets no second gun, and the rack keeps its count.
    touch(&mut b, racks[0]);
    assert_eq!(carried(&b), [WeaponId::Ak47, WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!(reserve(&b, "762Nato"), 60);
    // The vest rack: 100 armor, and then the rack is empty and hidden (`EF_NODRAW`).
    touch(&mut b, racks[1]);
    let armor = b.server.game().game.state().players[0].as_ref().unwrap().armor;
    assert_eq!(armor, 100.0);
    let rack = racks[1];
    let hidden = b.run(move |_, eng| eng.ent(rack).unwrap().effects & 128 != 0);
    assert!(hidden);
    // The round starts over: the racks are full and drawn again, the weapon boxes are gone.
    command(&mut b, 0, "drop");
    assert_eq!(boxes(&mut b).len(), 1);
    b.run(|game, eng| game.restart_round_now(eng));
    assert!(boxes(&mut b).is_empty(), "round_restart_clears_weapons_on_the_ground");
    let rack = racks[1];
    let hidden = b.run(move |_, eng| eng.ent(rack).unwrap().effects & 128 != 0);
    assert!(!hidden);
}

#[test]
fn round_restart_clears_weapons_on_the_ground() {
    let mut b = team_bench(1);
    set_money(&mut b, 16000);
    say(&mut b, "ak47");
    say(&mut b, "sg552");
    assert_eq!(boxes(&mut b).len(), 1);
    b.run(|game, eng| game.restart_round_now(eng));
    b.step(5);
    assert!(boxes(&mut b).is_empty());
    // The survivor keeps what it carried (`m_bNotKilled`).
    assert!(carried(&b).contains(&WeaponId::Sg552));
}

#[test]
fn weapon_switch_commands() {
    let mut b = team_bench(2);
    assert_eq!(active(&b), Some(WeaponId::Usp));
    command(&mut b, 0, "weapon_knife");
    assert_eq!(active(&b), Some(WeaponId::Knife));
    command(&mut b, 0, "lastinv");
    assert_eq!(active(&b), Some(WeaponId::Usp));
    command(&mut b, 0, "use weapon_knife");
    assert_eq!(active(&b), Some(WeaponId::Knife));
    command(&mut b, 0, "weapon_usp");
    assert_eq!(active(&b), Some(WeaponId::Usp));
    // A weapon the player does not carry, or the active one: nothing changes.
    command(&mut b, 0, "weapon_awp");
    command(&mut b, 0, "weapon_usp");
    assert_eq!(active(&b), Some(WeaponId::Usp));
    // Deploying takes 0.75 s of the player's attack time (a stub weapon does not do it: the state
    // that changed is the active item and the last one).
    let state = b.server.game().game.player_weapon_state(0).unwrap();
    assert_eq!(state.2, Some(WeaponId::Usp));
}

#[test]
fn hostile_weapon_commands_are_ignored() {
    let mut b = team_bench(2);
    let before = (carried(&b), active(&b), money(&b));
    let long = "x".repeat(20_000);
    let lines = [
        "weapon_".to_string(),
        "weapon_\u{1F4A3}".to_string(),
        format!("weapon_{long}"),
        format!("use {long}"),
        "use".to_string(),
        "use weapon_".to_string(),
        "use weapon_ak47".to_string(),
        "use  ".to_string(),
        format!("drop {long}"),
        "drop weapon_ak47".to_string(),
        "drop weapon_hegrenade".to_string(),
        "drop nonsense".to_string(),
        "lastinv extra args".to_string(),
        "WEAPON_USP".to_string(),
        "weapon_usp weapon_knife".to_string(),
    ];
    for line in &lines {
        command(&mut b, 0, line);
    }
    // The knife and the pistol are droppable and nothing above named them; nothing moved.
    assert_eq!((carried(&b), active(&b), money(&b)), before);
    assert!(boxes(&mut b).is_empty());
    // A dead player's weapon commands do nothing at all.
    b.run(|game, eng| {
        game.damage_player(eng, 0, None, 500.0, dmg::GENERIC);
    });
    b.step(30);
    for line in ["weapon_knife", "lastinv", "drop", "use weapon_usp", "ak47", "buyammo1"] {
        command(&mut b, 0, line);
    }
    assert!(carried(&b).is_empty());
    // The commands of a slot with no player are ignored too.
    b.run(|game, eng| {
        let argv = vec!["drop".to_string()];
        game.client_command(eng, 1, &argv);
        let argv = vec!["weapon_usp".to_string()];
        game.client_command(eng, 1, &argv);
    });
}

/// The edicts of class `class`.
fn of_class(b: &mut Bench, class: &'static str) -> Vec<EdictId> {
    b.run(move |_, eng| {
        let mut found = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, class) {
            found.push(id);
            after = Some(id);
        }
        found
    })
}

/// Touches `thing` with the probe, the way the engine would when the player stands on it.
fn touch(b: &mut Bench, thing: EdictId) {
    b.run(move |game, eng| {
        eng.ent_mut(thing).unwrap().flags |= FL_ONGROUND;
        let me = eng.client_edict(0);
        game.touch(eng, thing, me);
    });
}

#[test]
fn a_weapon_box_removes_itself_at_the_end_of_its_life() {
    let mut b = team_bench(1);
    // A box that lives 1.5 s (`DropPlayerItem` gives 300 s; the hook shortens it).
    let weapon_box = b
        .run(|game, eng| game.drop_weapon_with_life(eng, 0, WeaponId::Glock18, 1.5))
        .expect("a box");
    b.step(100);
    assert_eq!(boxes(&mut b), [weapon_box], "still there after a second");
    b.step(70);
    assert!(boxes(&mut b).is_empty(), "gone after its life");
}

#[test]
fn a_dead_players_ammo_is_in_the_box_and_comes_back_with_it() {
    let mut b = team_bench(1);
    set_money(&mut b, 16000);
    say(&mut b, "ak47");
    say(&mut b, "buyammo1");
    assert_eq!(reserve(&b, "762Nato"), 30);
    b.run(|game, eng| {
        game.damage_player(eng, 0, None, 500.0, dmg::GENERIC);
    });
    b.step(30);
    assert_eq!(reserve(&b, "762Nato"), 0, "the player has nothing left");
    let found = boxes(&mut b);
    assert_eq!(found.len(), 1);
    // The player lives again with the default items and picks the box up: the rifle with the
    // clip it had, and, the box being empty then, its 30 rounds.
    b.run(|game, eng| {
        assert!(game.respawn_player(eng, 0));
    });
    b.step(5);
    assert_eq!(carried(&b), [WeaponId::Glock18, WeaponId::Knife]);
    touch(&mut b, found[0]);
    assert_eq!(carried(&b), [WeaponId::Ak47, WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!(clip(&mut b, WeaponId::Ak47), 30);
    assert_eq!(reserve(&b, "762Nato"), 30);
    assert!(boxes(&mut b).is_empty());
}

#[test]
fn a_second_weapon_of_a_class_gives_its_ammo_to_the_first() {
    // A rifle lying in the world (a map's `weapon_ak47`) touched by a player who has one.
    let mut b = bench_with(vec![entity(&[("classname", "weapon_ak47"), ("origin", "900 0 0")])]);
    join(&mut b, 0, 1, 1);
    b.step(5);
    set_money(&mut b, 16000);
    say(&mut b, "ak47");
    let world = of_class(&mut b, "weapon_ak47");
    assert_eq!(world.len(), 1);
    let rifle = world[0];
    b.run(move |_, eng| eng.ent_mut(rifle).unwrap().solid = cs_engine::consts::SOLID_TRIGGER);
    assert_eq!(reserve(&b, "762Nato"), 0);
    touch(&mut b, rifle);
    // `AddDuplicate`: the default ammo of 30 goes to the rifle the player has; the world's is
    // spent.
    assert_eq!(carried(&b), [WeaponId::Ak47, WeaponId::Glock18, WeaponId::Knife]);
    assert_eq!(reserve(&b, "762Nato"), 30);
    assert!(of_class(&mut b, "weapon_ak47").is_empty());
}

#[test]
fn a_map_knife_cannot_be_picked_up() {
    let mut b = bench_with(vec![entity(&[("classname", "weapon_knife"), ("origin", "900 0 0")])]);
    join(&mut b, 0, 2, 1);
    b.step(5);
    let world = of_class(&mut b, "weapon_knife");
    assert_eq!(world.len(), 1);
    let knife = world[0];
    // On the ground it becomes a trigger without a touch function (`Materialize`: `CanDrop` is
    // false), so touching it does nothing and it is removed a second later.
    let land = |b: &mut Bench, frames: usize| {
        for _ in 0..frames {
            b.run(move |_, eng| {
                if let Some(v) = eng.ent_mut(knife) {
                    v.flags |= FL_ONGROUND;
                }
            });
        }
    };
    land(&mut b, 30);
    touch(&mut b, knife);
    assert_eq!(carried(&b), [WeaponId::Usp, WeaponId::Knife]);
    assert_eq!(of_class(&mut b, "weapon_knife").len(), 1, "a touch does not use it up");
    land(&mut b, 130);
    assert!(of_class(&mut b, "weapon_knife").is_empty());
}

#[test]
fn lastinv_without_a_last_item_takes_the_first_other_weapon() {
    let mut b = team_bench(2);
    assert_eq!(active(&b), Some(WeaponId::Usp));
    b.server.game_mut().game.set_last_item(0, None);
    command(&mut b, 0, "lastinv");
    // `SelectLastItem`: the first weapon of the primary, pistol and knife slots that is not
    // active is the last item, and the two swap.
    assert_eq!(active(&b), Some(WeaponId::Knife));
    command(&mut b, 0, "lastinv");
    assert_eq!(active(&b), Some(WeaponId::Usp));
}

#[test]
fn armoury_racks_restart_by_the_share_of_the_terrorists() {
    // Two gun racks of two guns each: 4 guns in all. The first restart fills each rack with its
    // own count; the next ones give `int((2 / 4) * terrorists * 0.85)` (the division is of ints:
    // 0), and at least 1.
    let mut b = bench_with(vec![
        entity(&[("classname", "armoury_entity"), ("origin", "400 0 0"), ("item", "4"), ("count", "2")]),
        entity(&[("classname", "armoury_entity"), ("origin", "500 0 0"), ("item", "6"), ("count", "2")]),
    ]);
    join(&mut b, 0, 1, 1);
    b.step(5);
    let racks = of_class(&mut b, "armoury_entity");
    assert_eq!(racks.len(), 2);
    let rack = racks[0];
    let hidden = |b: &mut Bench| b.run(move |_, eng| eng.ent(rack).unwrap().effects & 128 != 0);
    b.run(|game, eng| game.restart_round_now(eng));
    b.step(5);
    // The first restart: the count is the map's 2, so one gun does not empty the rack.
    command(&mut b, 0, "drop weapon_ak47");
    touch(&mut b, rack);
    assert!(carried(&b).contains(&WeaponId::Ak47));
    assert!(!hidden(&mut b));
    command(&mut b, 0, "drop weapon_ak47");
    // The second restart: 1 gun, so the next gun taken empties the rack.
    b.run(|game, eng| game.restart_round_now(eng));
    b.step(5);
    command(&mut b, 0, "drop weapon_ak47");
    assert!(!carried(&b).contains(&WeaponId::Ak47));
    touch(&mut b, rack);
    assert!(carried(&b).contains(&WeaponId::Ak47));
    assert!(hidden(&mut b), "the count after the second restart was 1");
}
