//! Buy zones and the purchase of equipment: the `buyzone` icon (a `func_buyzone` of the player's
//! team, or the spawn points of a map that has none), `mp_buytime`, the vest, the vest with
//! helmet and the night vision goggles, the menus (`buy`, `buyequip`, `menuselect`,
//! `client_buy_open`) and the refusal of the weapons, which do not exist yet. Time is counted in
//! server frames of 10 ms; the probe is in slot 0. The stub maps have no collision, so the tests
//! switch gravity off to keep the player where it spawned.

mod common;

use common::{Bench, command, discover, dust2, entity, stub_spawns};
use cs_assets::bsp::Entity;
use cs_engine::{EngineApi, ServerMap};
use cs_game::menus::Menu;
use cs_game::player::{ArmorType, DEAD_NO, Team, dmg};
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::{CsUserMsg, encode_cs_usermsg};
use glam::Vec3;

const CENTER: u8 = 4;

/// A stub map with the spawn points of both teams and a buy zone round the first spawn point of
/// each team (`team` keys given by the caller).
fn zone_map(ct_team: &str, t_team: &str) -> Vec<Entity> {
    let mut map = stub_spawns();
    map.push(entity(&[
        ("classname", "func_buyzone"),
        ("origin", "200 0 0"),
        ("team", ct_team),
    ]));
    map.push(entity(&[
        ("classname", "func_buyzone"),
        ("origin", "-200 0 0"),
        ("team", t_team),
    ]));
    map
}

/// The zones of a stub map have no brush model: give them their box (150 units each way, which
/// takes in both spawn points of the team).
fn size_zones(eng: &mut EngineApi<'_>) {
    let mut after = None;
    while let Some(id) = eng.find_by_classname(after, "func_buyzone") {
        after = Some(id);
        let origin = eng.ent(id).unwrap().origin;
        eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
        eng.set_origin(id, origin);
    }
}

/// A bench on `map` (a stub map) whose probe is in the game and whose players do not fall.
fn bench_on(map: ServerMap, userinfo: &[(&str, &str)]) -> Bench {
    let mut b = Bench::with_userinfo(map, 2, userinfo);
    b.idle_cmds = true;
    b.step(6);
    b.server.game_mut().game.state_mut().move_params.gravity = 0.0;
    b.run(|_, eng| size_zones(eng));
    b.probe.clear_messages();
    b
}

fn zones() -> Bench {
    bench_on(
        ServerMap::stub_with_entities("test", zone_map("2", "1")),
        &[],
    )
}

/// A stub map with no `func_buyzone`.
fn no_zones() -> Bench {
    bench_on(ServerMap::stub_with_entities("test", stub_spawns()), &[])
}

fn spawn(b: &mut Bench, team: Team) {
    assert!(b.run(move |game, eng| game.spawn_player(eng, 0, team, 0)));
}

/// Spawns the probe and puts it at `origin` in one frame, so it never touches a zone it is not
/// in.
fn spawn_at_origin(b: &mut Bench, team: Team, origin: Vec3) {
    assert!(b.run(move |game, eng| {
        let spawned = game.spawn_player(eng, 0, team, 0);
        game.place_player(eng, 0, origin) && spawned
    }));
}

fn place(b: &mut Bench, origin: Vec3) {
    assert!(b.run(move |game, eng| game.place_player(eng, 0, origin)));
}

fn set_money(b: &mut Bench, amount: i32) {
    b.server.game_mut().game.state_mut().players[0]
        .as_mut()
        .unwrap()
        .money = amount;
}

fn with_player<T>(b: &Bench, f: impl FnOnce(&cs_game::player::Player) -> T) -> T {
    f(b.server.game().game.state().players[0].as_ref().unwrap())
}

fn money(b: &Bench) -> i32 {
    with_player(b, |p| p.money)
}

fn frames(secs: f64) -> usize {
    (secs * 100.0).round() as usize
}

/// A client in a zone knows it after the half second `HandleSignals` waits (twice that when it
/// has just left one: the last touch may still be pending at the first look).
fn settle(b: &mut Bench) {
    b.step(120);
}

/// The `buyzone` icon messages the probe received, as (status, colour).
fn icons(b: &Bench) -> Vec<(u8, Option<[u8; 3]>)> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::StatusIcon {
                status,
                sprite_name,
                color,
            } if sprite_name == "buyzone" => Some((status, color)),
            _ => None,
        })
        .collect()
}

/// What the HUD was told about money, armor and purchases, in order.
fn purchase_log(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::ItemPickup(item) => Some(format!("ItemPickup {item}")),
            CsUserMsg::ArmorType(t) => Some(format!("ArmorType {t}")),
            CsUserMsg::Money { amount, flash } => Some(format!("Money {amount} {flash}")),
            CsUserMsg::Battery(a) => Some(format!("Battery {a}")),
            CsUserMsg::BlinkAcct(n) => Some(format!("BlinkAcct {n}")),
            CsUserMsg::ItemStatus(s) => Some(format!("ItemStatus {s}")),
            CsUserMsg::NvgToggle(on) => Some(format!("NVGToggle {on}")),
            CsUserMsg::TextMsg {
                dest,
                message,
                params,
            } => Some(format!("TextMsg {dest} {message} {params:?}")),
            _ => None,
        })
        .collect()
}

/// Runs `line` as a command of the probe and returns what the HUD was told since, then clears
/// the probe's messages.
fn buy(b: &mut Bench, line: &str) -> Vec<String> {
    b.probe.clear_messages();
    command(b, 0, line);
    b.step(5);
    let log = purchase_log(b);
    b.probe.clear_messages();
    log
}

/// The sounds the probe heard, as (sound index, channel).
fn sounds(b: &Bench) -> Vec<(u16, u8)> {
    b.probe
        .messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Sound(s) => Some((s.sound_index, s.channel)),
            _ => None,
        })
        .collect()
}

fn sound_index(b: &mut Bench, name: &'static str) -> u16 {
    b.run(move |_, eng| eng.sound_index(name))
        .expect("a precached sound")
}

/// `log` without the `Battery` lines: the HUD hears of armor at the next client data update, which
/// may come before or after the rest of what a command did.
fn without_battery(log: Vec<String>) -> Vec<String> {
    log.into_iter()
        .filter(|l| !l.starts_with("Battery"))
        .collect()
}

fn text(message: &str, params: &[&str]) -> String {
    format!("TextMsg {CENTER} {message} {params:?}")
}

#[test]
fn buy_zone_icon_on_spawn() {
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    // Not at once: the zones are looked at every half second.
    b.step(20);
    settle(&mut b);
    let shown = icons(&b);
    // The green icon, once.
    assert_eq!(shown, [(1, Some([0, 160, 0]))]);
    let (name, raw) = encode_cs_usermsg(&CsUserMsg::StatusIcon {
        status: 1,
        sprite_name: "buyzone".to_string(),
        color: shown[0].1,
    });
    assert_eq!(name, "StatusIcon");
    assert_eq!(raw, b"\x01buyzone\0\x00\xa0\x00");

    // Leaving the zone hides it (and nothing closes: no menu is open).
    b.probe.clear_messages();
    place(&mut b, Vec3::new(5000.0, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(icons(&b), [(0, None)]);
    let (_, raw) = encode_cs_usermsg(&CsUserMsg::StatusIcon {
        status: 0,
        sprite_name: "buyzone".to_string(),
        color: None,
    });
    assert_eq!(raw, b"\x00buyzone\0");
    assert!(
        !b.probe
            .messages()
            .iter()
            .any(|m| matches!(m, SvcMessage::StuffText(_)))
    );

    // The zone of the other team is no zone of this one.
    b.probe.clear_messages();
    place(&mut b, Vec3::new(-100.0, 0.0, 0.0));
    settle(&mut b);
    assert!(icons(&b).is_empty());
    // A terrorist is in it.
    spawn(&mut b, Team::Terrorist);
    settle(&mut b);
    assert_eq!(icons(&b), [(1, Some([0, 160, 0]))]);
}

#[test]
fn a_zone_of_no_team_or_a_bad_team_is_for_everyone() {
    for key in ["0", "7", "-1", "x"] {
        let mut map = stub_spawns();
        map.push(entity(&[
            ("classname", "func_buyzone"),
            ("origin", "200 0 0"),
            ("team", key),
        ]));
        let mut b = bench_on(ServerMap::stub_with_entities("test", map), &[]);
        spawn(&mut b, Team::Ct);
        settle(&mut b);
        assert_eq!(icons(&b), [(1, Some([0, 160, 0]))], "team {key}");
        place(&mut b, Vec3::new(-100.0, 0.0, 0.0));
        spawn(&mut b, Team::Terrorist);
        place(&mut b, Vec3::new(100.0, 0.0, 0.0));
        settle(&mut b);
        assert!(
            with_player(&b, |p| p.test_zone_signals() & 1 != 0),
            "team {key}"
        );
    }
}

#[test]
fn a_map_without_buy_zones_buys_around_the_spawn_points() {
    let mut b = no_zones();
    spawn(&mut b, Team::Ct);
    settle(&mut b);
    assert!(!b.server.game().game.state().rules.map_has_buy_zone());
    assert_eq!(icons(&b), [(1, Some([0, 160, 0]))]);
    // The counter-terrorist spawn points are at x 100 and 300: within 200 of one is in.
    b.probe.clear_messages();
    place(&mut b, Vec3::new(499.0, 0.0, 0.0));
    settle(&mut b);
    assert!(icons(&b).is_empty(), "still in the zone");
    place(&mut b, Vec3::new(500.5, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(icons(&b), [(0, None)]);
    // The terrorists' spawn points are another class.
    spawn(&mut b, Team::Terrorist);
    b.probe.clear_messages();
    place(&mut b, Vec3::new(-290.0, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(icons(&b), [(1, Some([0, 160, 0]))]);
}

#[test]
fn buy_armor_as_regamedll() {
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    settle(&mut b);
    assert_eq!(money(&b), 800);
    let ammo_pickup = sound_index(&mut b, "items/ammopickup2.wav");
    b.probe.clear_messages();

    // The vest: 650 for 100 armor, the pickup, the account (flashing) and the HUD's armor.
    b.probe.clear_messages();
    command(&mut b, 0, "vest");
    b.step(5);
    assert_eq!(
        purchase_log(&b),
        [
            "ItemPickup item_kevlar",
            "ArmorType 0",
            "Money 150 true",
            "Battery 100"
        ]
    );
    // The sound of the item is on the item channel.
    assert!(sounds(&b).contains(&(ammo_pickup, 3)), "{:?}", sounds(&b));
    with_player(&b, |p| {
        assert_eq!((p.armor, p.kevlar, p.money), (100.0, ArmorType::Vest, 150));
    });
    let wire = |m: CsUserMsg| encode_cs_usermsg(&m).1;
    assert_eq!(
        wire(CsUserMsg::ItemPickup("item_kevlar".into())),
        b"item_kevlar\0"
    );
    assert_eq!(wire(CsUserMsg::ArmorType(0)), [0]);
    assert_eq!(
        wire(CsUserMsg::Money {
            amount: 150,
            flash: true
        }),
        [150, 0, 0, 0, 1]
    );
    assert_eq!(wire(CsUserMsg::Battery(100)), [100, 0]);
    b.probe.clear_messages();
    // Full armor already: said, nothing taken.
    set_money(&mut b, 800);
    assert_eq!(buy(&mut b, "vest"), [text("#Already_Have_Kevlar", &[])]);
    assert_eq!(money(&b), 800);

    // The helmet is the 350 that is missing.
    assert_eq!(
        buy(&mut b, "vesthelm"),
        [
            text("#Already_Have_Kevlar_Bought_Helmet", &[]),
            "ItemPickup item_assaultsuit".to_string(),
            "ArmorType 1".to_string(),
            "Money 450 true".to_string(),
        ]
    );
    with_player(&b, |p| {
        assert_eq!((p.kevlar, p.armor), (ArmorType::VestHelm, 100.0))
    });
    assert_eq!(
        buy(&mut b, "vesthelm"),
        [text("#Already_Have_Kevlar_Helmet", &[])]
    );
    assert_eq!(money(&b), 450);

    // Armor worn down: a vest for a player with a helmet repairs the armor but, as in the
    // original, tells the client it is a plain vest and keeps the helmet.
    b.run(|game, eng| game.damage_player(eng, 0, None, 40.0, dmg::GENERIC));
    with_player(&b, |p| assert!(p.armor < 100.0 && p.armor > 0.0));
    set_money(&mut b, 700);
    assert_eq!(
        buy(&mut b, "vest"),
        [
            // The worn armor, which the HUD is told of first.
            "Battery 90".to_string(),
            text("#Already_Have_Helmet_Bought_Kevlar", &[]),
            "ItemPickup item_kevlar".to_string(),
            "ArmorType 0".to_string(),
            "Money 50 true".to_string(),
            "Battery 100".to_string(),
        ]
    );
    with_player(&b, |p| {
        assert_eq!((p.kevlar, p.armor), (ArmorType::VestHelm, 100.0))
    });

    // A whole suit from nothing costs 1000.
    b.run(|game, _| {
        let p = game.state_mut().players[0].as_mut().unwrap();
        p.armor = 0.0;
        p.kevlar = ArmorType::None;
    });
    set_money(&mut b, 999);
    assert_eq!(
        without_battery(buy(&mut b, "vesthelm")),
        [text("#Not_Enough_Money", &[]), "BlinkAcct 2".to_string()]
    );
    set_money(&mut b, 1000);
    assert_eq!(
        buy(&mut b, "vesthelm"),
        [
            "ItemPickup item_assaultsuit",
            "ArmorType 1",
            "Money 0 true",
            "Battery 100"
        ]
    );
    // And with a helmet but no armor left it is the 650 of a vest, which the suit item gives.
    b.run(|game, _| game.state_mut().players[0].as_mut().unwrap().armor = 0.0);
    set_money(&mut b, 649);
    assert_eq!(
        without_battery(buy(&mut b, "vesthelm")),
        [text("#Not_Enough_Money", &[]), "BlinkAcct 2".to_string()]
    );
    set_money(&mut b, 650);
    assert_eq!(
        buy(&mut b, "vesthelm"),
        [
            text("#Already_Have_Helmet_Bought_Kevlar", &[]),
            "ItemPickup item_assaultsuit".to_string(),
            "ArmorType 1".to_string(),
            "Money 0 true".to_string(),
            "Battery 100".to_string(),
        ]
    );
}

#[test]
fn buy_night_vision_goggles() {
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    settle(&mut b);
    let equip = sound_index(&mut b, "items/equip_nvg.wav");
    set_money(&mut b, 1300);
    b.probe.clear_messages();
    command(&mut b, 0, "nvgs");
    assert!(sounds(&b).contains(&(equip, 3)), "{:?}", sounds(&b));
    // The account first, then the item status: the goggles are bit 0.
    assert_eq!(purchase_log(&b), ["Money 50 true", "ItemStatus 1"]);
    assert_eq!(encode_cs_usermsg(&CsUserMsg::ItemStatus(1)).1, [1]);
    with_player(&b, |p| {
        assert!(p.test_has_night_vision() && !p.test_night_vision_on())
    });
    // One pair is enough.
    set_money(&mut b, 2000);
    assert_eq!(buy(&mut b, "nvgs"), [text("#Already_Have_One", &[])]);
    assert_eq!(money(&b), 2000);

    // The goggles survive the round a player lives through, and go with a death: they are off
    // at once, taken away at the next spawn.
    b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::GENERIC));
    with_player(&b, |p| {
        assert!(p.test_has_night_vision() && !p.test_night_vision_on() && !p.alive)
    });
    b.probe.clear_messages();
    b.run(|game, eng| game.respawn_player(eng, 0));
    b.step(5);
    let log = purchase_log(&b);
    assert!(
        log.windows(2)
            .any(|w| w == ["ItemStatus 0", "NVGToggle false"]),
        "{log:?}"
    );
    with_player(&b, |p| {
        assert!(!p.test_has_night_vision() && !p.test_night_vision_on())
    });
}

#[test]
fn buy_time_expires() {
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    // The freeze period of the map runs out, and the round clock starts.
    b.step(900);
    assert!(!b.server.game().game.state().rules.is_freeze_period());
    let start = b.server.game().game.state().rules.round_start_time();
    set_money(&mut b, 3000);
    // `mp_buytime` is at least 15 seconds, and the cvar is corrected.
    b.run(|_, eng| eng.set_cvar("mp_buytime", "0.1"));
    let to = |b: &Bench, secs: f64| frames(start + secs - b.now);
    let n = to(&b, 14.0);
    b.step(n);
    assert_eq!(buy(&mut b, "nvgs"), ["Money 1750 true", "ItemStatus 1"]);
    assert_eq!(b.run(|_, eng| eng.cvar_f32("mp_buytime")), 0.25);

    // Past it: told how long it was, and nothing is sold.
    let n = to(&b, 15.5);
    b.step(n);
    assert_eq!(buy(&mut b, "vest"), [text("#Cant_buy", &["15"])]);
    assert_eq!(money(&b), 1750);
    with_player(&b, |p| assert_eq!(p.armor, 0.0));
    // Menus open only for a player who can buy, and a text menu client's key is no better.
    assert_eq!(buy(&mut b, "buyammo1"), [text("#Cant_buy", &["15"])]);

    // The cvar is the length of the window, in minutes: a minute and a half is the default.
    b.run(|_, eng| eng.set_cvar("mp_buytime", "1.5"));
    assert_eq!(
        buy(&mut b, "vest"),
        [
            "ItemPickup item_kevlar",
            "ArmorType 0",
            "Money 1100 true",
            "Battery 100"
        ]
    );
}

#[test]
fn cannot_buy_outside_the_zone_or_without_money() {
    let mut b = zones();
    // Never in a zone: nothing is sold and nothing is said.
    spawn_at_origin(&mut b, Team::Ct, Vec3::new(5000.0, 0.0, 0.0));
    settle(&mut b);
    assert!(icons(&b).is_empty());
    for line in ["vest", "vesthelm", "nvgs", "buy", "buyequip"] {
        assert_eq!(buy(&mut b, line), Vec::<String>::new(), "{line}");
    }
    assert_eq!(money(&b), 800);
    with_player(&b, |p| assert_eq!((p.menu, p.armor), (Menu::Off, 0.0)));

    // In the zone with too little money: the message and the blinking account.
    place(&mut b, Vec3::new(100.0, 0.0, 0.0));
    settle(&mut b);
    let broke = [text("#Not_Enough_Money", &[]), "BlinkAcct 2".to_string()];
    for (line, price) in [("vest", 650), ("vesthelm", 1000), ("nvgs", 1250)] {
        set_money(&mut b, price - 1);
        assert_eq!(buy(&mut b, line), broke, "{line}");
        assert_eq!(money(&b), price - 1);
    }
    with_player(&b, |p| {
        assert_eq!((p.armor, p.test_has_night_vision()), (0.0, false))
    });
    assert_eq!(encode_cs_usermsg(&CsUserMsg::BlinkAcct(2)).1, [2]);
    // Just enough is enough.
    set_money(&mut b, 650);
    assert_eq!(buy(&mut b, "vest")[2], "Money 0 true");

    // A dead player is out of the zone on the spot and buys nothing.
    set_money(&mut b, 5000);
    b.probe.clear_messages();
    b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::GENERIC));
    b.step(5);
    assert_eq!(icons(&b), [(0, None)], "the death hides the icon");
    b.probe.clear_messages();
    for line in ["vest", "nvgs", "buy"] {
        assert_eq!(buy(&mut b, line), Vec::<String>::new(), "{line}");
    }
    assert_eq!(money(&b), 5000);
    assert!(b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().deadflag) != DEAD_NO);

    // Arguments of a hostile client do not matter.
    for line in [
        "buy 1 2 3",
        "buyequip x",
        "vest\u{1}",
        "primammo",
        "secammo",
        "buyammo1",
        "buyammo2",
    ] {
        buy(&mut b, line);
    }
}

#[test]
fn weapon_purchase_refused_until_weapons() {
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    settle(&mut b);
    set_money(&mut b, 16000);
    // A weapon the team may buy: refused, nothing taken.
    for line in ["m4a1", "usp", "AK47", "flash", "hegren", "sgren", "shield"] {
        let log = buy(&mut b, line);
        let expected = if line == "AK47" {
            // The counter-terrorists cannot buy it: the alias says so, as the original does.
            vec![text("#Alias_Not_Avail", &["#AK47"])]
        } else {
            vec![text("#Cannot_Buy_This", &[])]
        };
        assert_eq!(log, expected, "{line}");
        assert_eq!(money(&b), 16000, "{line}");
    }
    // Ammo is bought for the weapons a player has: it has none.
    for line in ["primammo", "secammo", "buyammo1", "buyammo2"] {
        assert_eq!(buy(&mut b, line), Vec::<String>::new(), "{line}");
    }
    // The defuser is for a bomb map (2E); on this one the command is no use, and a terrorist's
    // is not available.
    assert_eq!(buy(&mut b, "defuser"), Vec::<String>::new());
    assert_eq!(money(&b), 16000);
    spawn(&mut b, Team::Terrorist);
    place(&mut b, Vec3::new(-100.0, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(
        buy(&mut b, "defuser"),
        [text("#Alias_Not_Avail", &["#Bomb_Defusal_Kit"])]
    );
    assert_eq!(
        buy(&mut b, "shield"),
        [text("#Alias_Not_Avail", &["#TactShield_Desc"])]
    );
    assert_eq!(buy(&mut b, "m4a1"), [text("#Alias_Not_Avail", &["#M4A1"])]);
    assert_eq!(buy(&mut b, "ak47"), [text("#Cannot_Buy_This", &[])]);
    assert_eq!(money(&b), 16000);
    with_player(&b, |p| {
        assert_eq!((p.armor, p.test_has_night_vision()), (0.0, false))
    });
}

#[test]
fn the_buy_menus_follow_the_clients_preference() {
    // A VGUI client gets the panels and answers with commands.
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    settle(&mut b);
    b.probe.clear_messages();
    command(&mut b, 0, "buy");
    assert!(b.messages().contains(&CsUserMsg::VguiMenu {
        menu_id: 28,
        keys_bitmask: 0x2ff,
        time: -1,
        multipart: false,
        name: " ".to_string(),
    }));
    with_player(&b, |p| assert_eq!(p.menu, Menu::Buy));
    b.probe.clear_messages();
    command(&mut b, 0, "buyequip");
    // A counter-terrorist on a map with no bomb target: no defuser, but the shield.
    assert!(b.messages().contains(&CsUserMsg::VguiMenu {
        menu_id: 34,
        keys_bitmask: 0x2bf,
        time: -1,
        multipart: false,
        name: " ".to_string(),
    }));
    with_player(&b, |p| assert_eq!(p.menu, Menu::BuyItem));
    // The keys of a VGUI client are no menu answers.
    set_money(&mut b, 5000);
    assert_eq!(buy(&mut b, "menuselect 1"), Vec::<String>::new());
    assert_eq!(money(&b), 5000);

    // A text menu client gets `ShowMenu` and answers with `menuselect`.
    let mut t = bench_on(
        ServerMap::stub_with_entities("test", zone_map("2", "1")),
        &[("_vgui_menus", "0")],
    );
    spawn(&mut t, Team::Ct);
    settle(&mut t);
    t.probe.clear_messages();
    command(&mut t, 0, "buy");
    assert!(t.messages().contains(&CsUserMsg::ShowMenu {
        keys_bitmask: 0x2ff,
        display_time: -1,
        multipart: false,
        text: "#Buy".to_string(),
    }));
    // Key 4 is the rifles, which no weapon here is for sale; key 8 the equipment.
    t.probe.clear_messages();
    command(&mut t, 0, "menuselect 4");
    assert!(t.messages().contains(&CsUserMsg::ShowMenu {
        keys_bitmask: 0x23f,
        display_time: -1,
        multipart: false,
        text: "#CT_BuyRifle".to_string(),
    }));
    with_player(&t, |p| assert_eq!(p.menu, Menu::BuyRifle));
    set_money(&mut t, 5000);
    assert_eq!(buy(&mut t, "menuselect 2"), [text("#Cannot_Buy_This", &[])]);
    assert_eq!(
        buy(&mut t, "menuselect 7"),
        Vec::<String>::new(),
        "no seventh rifle"
    );
    assert_eq!(money(&t), 5000);
    command(&mut t, 0, "buy");
    command(&mut t, 0, "menuselect 8");
    with_player(&t, |p| assert_eq!(p.menu, Menu::BuyItem));
    // Key 1 of the equipment menu is the vest.
    assert_eq!(
        buy(&mut t, "menuselect 1"),
        [
            "ItemPickup item_kevlar",
            "ArmorType 0",
            "Money 4350 true",
            "Battery 100"
        ]
    );
    // Key 6 the goggles; key 0 closes.
    assert_eq!(
        buy(&mut t, "menuselect 6"),
        ["Money 3100 true", "ItemStatus 1"]
    );
    command(&mut t, 0, "menuselect 10");
    with_player(&t, |p| assert_eq!(p.menu, Menu::Off));
    assert_eq!(buy(&mut t, "menuselect 6"), Vec::<String>::new());
}

#[test]
fn leaving_the_zone_closes_the_buy_menu() {
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    settle(&mut b);
    command(&mut b, 0, "buy");
    b.probe.clear_messages();
    place(&mut b, Vec3::new(5000.0, 0.0, 0.0));
    settle(&mut b);
    assert_eq!(icons(&b), [(0, None)]);
    // The client closes its menu with `slot10`.
    assert!(
        b.probe
            .messages()
            .iter()
            .any(|m| *m == SvcMessage::StuffText("slot10\n".to_string()))
    );

    // Key 0 closed the menu (a menu is only replaced by `client_buy_open` when none is open).
    command(&mut b, 0, "menuselect 10");
    with_player(&b, |p| assert_eq!(p.menu, Menu::Off));
    // The client's own menu: opened, and closed with `BuyClose` when the zone is left; outside
    // a zone it is shut at once.
    place(&mut b, Vec3::new(100.0, 0.0, 0.0));
    settle(&mut b);
    command(&mut b, 0, "client_buy_open");
    with_player(&b, |p| assert_eq!(p.menu, Menu::ClientBuy));
    assert!(!b.messages().contains(&CsUserMsg::BuyClose));
    b.probe.clear_messages();
    place(&mut b, Vec3::new(5000.0, 0.0, 0.0));
    settle(&mut b);
    assert!(b.messages().contains(&CsUserMsg::BuyClose));
    b.probe.clear_messages();
    command(&mut b, 0, "client_buy_close");
    with_player(&b, |p| assert_eq!(p.menu, Menu::Off));
    command(&mut b, 0, "client_buy_open");
    assert!(b.messages().contains(&CsUserMsg::BuyClose), "not in a zone");
    with_player(&b, |p| assert_eq!(p.menu, Menu::ClientBuy));
}

#[test]
fn buy_zones_of_a_real_map() {
    let Some(vfs) = discover() else { return };
    let mut b = Bench::on(dust2(&vfs));
    b.idle_cmds = true;
    b.step(6);
    assert!(b.server.game().game.state().rules.map_has_buy_zone());
    for team in [Team::Ct, Team::Terrorist] {
        b.probe.clear_messages();
        spawn(&mut b, team);
        b.step(200);
        assert_eq!(icons(&b).last(), Some(&(1, Some([0, 160, 0]))), "{team:?}");
    }
    set_money(&mut b, 3000);
    // The terrorist buys a vest and the goggles where it stands.
    assert_eq!(
        buy(&mut b, "vest"),
        [
            "ItemPickup item_kevlar",
            "ArmorType 0",
            "Money 2350 true",
            "Battery 100"
        ]
    );
    assert_eq!(buy(&mut b, "nvgs"), ["Money 1100 true", "ItemStatus 1"]);
}

/// F26: the dead buy nothing and are told nothing. The finding asked for `#Alias_Not_Avail` to a
/// dead player; the source says otherwise: `ClientCommand_` (`client.cpp`, read 2026-11-22)
/// runs `HandleBuyAliasCommands` only inside the `deadflag == DEAD_NO` branch, so a dead
/// player's alias (wrong team or not) reaches nothing, as here.
#[test]
fn a_dead_players_buy_alias_says_nothing() {
    let mut b = zones();
    spawn(&mut b, Team::Ct);
    settle(&mut b);
    set_money(&mut b, 16000);
    assert_eq!(buy(&mut b, "ak47"), [text("#Alias_Not_Avail", &["#AK47"])]);
    b.run(|game, eng| game.damage_player(eng, 0, None, 500.0, dmg::GENERIC));
    b.step(5);
    assert!(b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().deadflag) != DEAD_NO);
    for line in ["ak47", "m4a1", "defuser", "vest"] {
        assert_eq!(buy(&mut b, line), Vec::<String>::new(), "{line}");
    }
    assert_eq!(money(&b), 16000);
}
