//! VIP assassination (`as_*`) and terrorist escape (`es_*`) objective modes (Task 9 of phase 2E).
//!
//! Against ReGameDLL_CS (`multiplay_gamerules.cpp`, `player.cpp`, `client.cpp`, `weapontype.cpp`,
//! vanilla `!REGAMEDLL_FIXES` / `!REGAMEDLL_ADD` / `!BUILD_LATEST`):
//! - VIP selection (`MakeVIP`, `PickNextVIP`, `ResetCurrentVIP`, `AddToVIPQueue`, `become_vip`,
//!   `MENU_SLOT_TEAM_VIP`, 3-round rotation, disconnect cleanup, `#Cannot_Switch_From_VIP`,
//!   `info_vip_start` spawn, `"vip"` model, 200 `VestHelm`, USP + 12 `.45acp`, `227.0` max speed,
//!   halved `TakeDamage` armor ratio, `SCORE_STATUS_VIP`, `#Hint_you_are_the_vip`)
//! - VIP buy block (`#VIP_cant_buy`) and assassination weapon restrictions (`CanBuyWeaponByMaptype`,
//!   `#AS_*` buy menus, `#Alias_Not_Avail`)
//! - VIP win conditions (`#VIP_Escaped`, `disappear`, `+2500` self-rescue, `REWARD_VIP_ESCAPED`,
//!   `#VIP_Assassinated`, `REWARD_KILLED_VIP`, `#Hint_reward_for_killing_vip`, `REWARD_VIP_ASSASSINATED`)
//! - Escape (`#Terrorists_Escaped`, `#CTs_PreventEscape`, 3-round `SwapAllPlayers`, T `not_killed`
//!   reset, no loser bonus for T when CTs win on escape maps)
//! - Real `maps/as_oilrig.bsp` smoke test (`CS16_GAMEDIR`, skipped if unavailable)

mod common;

use common::{Bench, command, discover, enter, entity, join, map_named, spawn_at, spawns};
use cs_assets::bsp::Entity;
use cs_engine::consts::{SOLID_NOT, SOLID_TRIGGER};
use cs_engine::{EngineApi, GameDll, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::money::{
    REWARD_CTS_PREVENT_ESCAPE, REWARD_KILLED_VIP, REWARD_LOSER_BONUS_DEFAULT,
    REWARD_TERRORISTS_ESCAPED, REWARD_VIP_ASSASSINATED, REWARD_VIP_ESCAPED,
};
use cs_game::player::{ArmorType, DEAD_NO, Team, dmg};
use cs_game::rules::WinStatus;
use cs_game::weapons::{WeaponId, ammo_index};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

const SCORE_STATUS_VIP: u8 = 1 << 2;
const REWARD_VIP_SELF_RESCUED: i32 = 2500;

fn sized_zone(class: &str, origin: &str) -> Entity {
    entity(&[("classname", class), ("origin", origin)])
}

fn setup_vip_bench(extra: Vec<Entity>, max_clients: u8, vgui: bool) -> Bench {
    let mut entities = spawns(4, 4);
    entities.push(sized_zone("func_vip_safetyzone", "3000 0 0"));
    entities.extend(extra);
    let mut b = Bench::with_config(
        ServerMap::stub_with_entities("test_as", entities),
        ServerConfig {
            max_clients,
            max_edicts: 256,
            ..Default::default()
        },
        &[
            ("cl_lw", "0"),
            ("_vgui_menus", if vgui { "1" } else { "0" }),
            ("_ah", "1"),
        ],
        true,
    );
    b.idle_cmds = true;
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_autoteambalance 0");
    let _ = b.server.console_command("mp_limitteams 0");
    b.run(|_, eng| {
        eng.set_cvar("sv_gravity", "0");
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
                eng.set_origin(id, origin);
            }
        }
    });
    b.step(6);
    b
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

fn centre_texts(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::TextMsg { dest: 4, message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

fn hints(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::HudTextArgs { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

fn audio_codes(b: &Bench) -> Vec<String> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::SendAudio { audio_code, .. } => Some(audio_code),
            _ => None,
        })
        .collect()
}

#[test]
fn a_vip_is_picked_and_equipped() {
    let mut b = setup_vip_bench(vec![spawn_at("info_vip_start", "500 0 0")], 4, false);
    enter(&mut b, 1);
    enter(&mut b, 2);
    join(&mut b, 0, 2, 1); // CT 0 -> becomes VIP immediately
    join(&mut b, 1, 2, 1); // CT 1
    join(&mut b, 2, 1, 1); // T 2
    b.step(6);
    b.probe.clear_messages();

    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
    });
    b.step(12);

    b.run(|game, eng| {
        assert_eq!(game.state().rules.vip_slot(), Some(0));
        let p0 = game.state().players[0].as_ref().unwrap();
        assert!(p0.is_vip);
        assert_eq!(p0.kevlar, ArmorType::VestHelm);
        assert_eq!(p0.armor, 200.0);
        assert_eq!(eng.client_key(0, "model"), Some("vip"));
        let edict0 = eng.client_edict(0);
        let v0 = eng.ent(edict0).unwrap();
        assert_eq!(v0.body, 0);
        assert!((v0.origin - Vec3::new(500.0, 0.0, 1.0)).length() < 1.0);
        assert_eq!(v0.max_speed, 227.0);

        let weapons = game.carried_weapons(0);
        assert!(weapons.contains(&WeaponId::Knife));
        assert!(weapons.contains(&WeaponId::Usp));
        let acp_idx = usize::try_from(ammo_index("45acp")).unwrap();
        assert_eq!(game.player_ammo(0, acp_idx), Some(12));

        // Regular CT starts with 24 .45acp and 0 armor.
        let p1 = game.state().players[1].as_ref().unwrap();
        assert!(!p1.is_vip);
        assert_eq!(game.player_ammo(1, acp_idx), Some(24));
    });

    assert!(b.messages().contains(&CsUserMsg::ScoreAttrib {
        player_id: 1,
        flags: SCORE_STATUS_VIP,
    }));
    assert!(hints(&b).contains(&"#Hint_you_are_the_vip".to_string()));

    // VIP armor absorbs twice as much ratio (`flRatio *= 0.5`): 40 bullet dmg hits health for
    // `40 * (0.5 * 0.5) = 10` instead of `40 * 0.5 = 20`.
    b.run(|game, eng| {
        game.damage_player(eng, 0, Some(2), 40.0, dmg::BULLET);
        let hp = eng.ent(eng.client_edict(0)).unwrap().health;
        assert_eq!(hp, 90.0);
    });

    // VIP cannot drop carried weapons.
    b.run(|game, eng| {
        game.client_command(eng, 0, &["drop".to_string(), "weapon_usp".to_string()]);
        assert!(game.carried_weapons(0).contains(&WeaponId::Usp));
    });

    // Living VIP cannot switch teams (`#Cannot_Switch_From_VIP`).
    b.probe.clear_messages();
    command(&mut b, 0, "jointeam 1");
    b.step(6);
    assert!(centre_texts(&b).contains(&"#Cannot_Switch_From_VIP".to_string()));
    assert!(b.run(|game, _| game.state().players[0].as_ref().unwrap().is_vip));

    // CT1 queues with `become_vip` (`jointeam 3` also queues), then next round CT1 takes VIP.
    b.run(|game, eng| {
        game.client_command(eng, 1, &["become_vip".to_string()]);
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(1));
        let p0 = game.state().players[0].as_ref().unwrap();
        let p1 = game.state().players[1].as_ref().unwrap();
        assert!(!p0.is_vip);
        assert!(p1.is_vip);
        assert_ne!(eng.client_key(0, "model"), Some("vip"));
    });

    // Empty queue keeps CT1 for 3 rounds total, then rotates back to CT0 on the 4th restart.
    b.run(|game, eng| {
        // Round 2 as CT1 (consecutive = 2)
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(1));
        // Round 3 as CT1 (consecutive = 3)
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(1));
        // Next restart (`consecutive_vip >= 3`) rotates away from CT1 to CT0.
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(0));
        assert_eq!(eng.client_key(0, "model"), Some("vip"));
    });
}

#[test]
fn the_vip_cannot_buy() {
    // Part 1: slot 0 is VIP -> buying is blocked with `#VIP_cant_buy`.
    let mut b = setup_vip_bench(vec![sized_zone("func_buyzone", "0 0 0")], 4, false);
    b.run(|_, eng| {
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.classname == "func_buyzone") {
                eng.set_size(id, Vec3::splat(-2000.0), Vec3::splat(2000.0));
                eng.set_origin(id, Vec3::ZERO);
            }
        }
    });
    enter(&mut b, 1);
    join(&mut b, 0, 2, 1);
    join(&mut b, 1, 1, 1);
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    b.step(120);
    b.run(|game, _| {
        let p = game.state().players[0].as_ref().unwrap();
        assert_eq!(p.test_zone_signals() & 1, 1, "expected SIGNAL_BUY for VIP");
    });
    b.probe.clear_messages();

    command(&mut b, 0, "buy");
    command(&mut b, 0, "vest");
    command(&mut b, 0, "m4a1");
    b.step(12);

    assert_eq!(
        b.run(|game, _| game.state().players[0].as_ref().unwrap().money),
        16000
    );
    let vip_texts: Vec<_> = centre_texts(&b);
    assert_eq!(
        vip_texts.iter().filter(|t| *t == "#VIP_cant_buy").count(),
        2,
        "expected #VIP_cant_buy for vest & m4a1, got {vip_texts:?}"
    );

    // Part 2: slot 0 is a non-VIP CT (slot 1 joined CT first and holds VIP) -> `as_` CT buy rules.
    let mut b_ct = setup_vip_bench(vec![sized_zone("func_buyzone", "0 0 0")], 4, false);
    b_ct.run(|_, eng| {
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.classname == "func_buyzone") {
                eng.set_size(id, Vec3::splat(-2000.0), Vec3::splat(2000.0));
                eng.set_origin(id, Vec3::ZERO);
            }
        }
    });
    enter(&mut b_ct, 1);
    enter(&mut b_ct, 2);
    join(&mut b_ct, 1, 2, 1); // CT 1 becomes VIP
    join(&mut b_ct, 0, 2, 1); // CT 0 is regular CT
    join(&mut b_ct, 2, 1, 1); // T 2
    b_ct.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    b_ct.step(120);
    b_ct.probe.clear_messages();

    command(&mut b_ct, 0, "buy");
    command(&mut b_ct, 0, "menuselect 4");
    b_ct.step(6);
    assert!(b_ct.messages().contains(&CsUserMsg::ShowMenu {
        keys_bitmask: 0x200 | 0x001 | 0x004 | 0x008 | 0x010,
        display_time: -1,
        multipart: false,
        text: "#AS_CT_BuyRifle".to_string(),
    }));
    command(&mut b_ct, 0, "awp");
    command(&mut b_ct, 0, "scout");
    command(&mut b_ct, 0, "m4a1");
    b_ct.step(6);
    assert_eq!(
        centre_texts(&b_ct)
            .into_iter()
            .filter(|t| t == "#Alias_Not_Avail")
            .count(),
        2
    );
    assert!(b_ct.run(|game, _| game.carried_weapons(0).contains(&WeaponId::M4a1)));

    // Part 3: slot 0 is a Terrorist on `as_` -> cannot buy `m3`/`sg552`, can buy `ak47`/`awp`.
    let mut b_t = setup_vip_bench(vec![sized_zone("func_buyzone", "0 0 0")], 4, false);
    b_t.run(|_, eng| {
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.classname == "func_buyzone") {
                eng.set_size(id, Vec3::splat(-2000.0), Vec3::splat(2000.0));
                eng.set_origin(id, Vec3::ZERO);
            }
        }
    });
    enter(&mut b_t, 1);
    join(&mut b_t, 1, 2, 1); // CT 1 VIP
    join(&mut b_t, 0, 1, 1); // T 0
    b_t.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        game.state_mut().players[0].as_mut().unwrap().money = 16000;
    });
    b_t.step(120);
    b_t.probe.clear_messages();

    command(&mut b_t, 0, "buy");
    command(&mut b_t, 0, "menuselect 4");
    b_t.step(6);
    assert!(b_t.messages().contains(&CsUserMsg::ShowMenu {
        keys_bitmask: 0x200 | 0x001 | 0x002 | 0x010,
        display_time: -1,
        multipart: false,
        text: "#AS_T_BuyRifle".to_string(),
    }));
    command(&mut b_t, 0, "m3");
    command(&mut b_t, 0, "sg552");
    command(&mut b_t, 0, "ak47");
    command(&mut b_t, 0, "awp");
    b_t.step(6);
    assert_eq!(
        centre_texts(&b_t)
            .into_iter()
            .filter(|t| t == "#Alias_Not_Avail")
            .count(),
        2
    );
    assert!(b_t.run(|game, _| game.carried_weapons(0).contains(&WeaponId::Awp)));
}

#[test]
fn vip_escape_ends_the_round() {
    let mut b = setup_vip_bench(vec![spawn_at("info_vip_start", "100 0 0")], 4, false);
    enter(&mut b, 1);
    join(&mut b, 0, 2, 1); // CT 0 VIP
    join(&mut b, 1, 1, 1); // T 1
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
    });
    b.step(10);
    let vip_money_before = b.run(|game, _| game.state().players[0].as_ref().unwrap().money);
    b.probe.clear_messages();

    b.run(|game, eng| {
        game.place_player(eng, 0, Vec3::new(3000.0, 0.0, 0.0));
        cmd_slot(game, eng, 0);
    });
    b.step(12);

    b.run(move |game, eng| {
        let p0 = game.state().players[0].as_ref().unwrap();
        assert!(p0.escaped);
        assert!(!p0.alive);
        assert_eq!(p0.money, vip_money_before + REWARD_VIP_SELF_RESCUED);
        let v0 = eng.ent(eng.client_edict(0)).unwrap();
        assert_ne!(v0.deadflag, DEAD_NO);
        assert_eq!(v0.solid, SOLID_NOT);
        assert_eq!(game.state().rules.win_status(), WinStatus::Cts);
        assert_eq!(game.state().num_ct_wins, 1);
        assert!(game.state().rules.restart_round_time() > 0.0);
    });
    assert!(centre_texts(&b).contains(&"#VIP_Escaped".to_string()));
    assert!(audio_codes(&b).contains(&"%!MRAD_ctwin".to_string()));

    let ct_before_restart = b.run(|game, _| game.state().players[0].as_ref().unwrap().money);
    b.run(|game, eng| game.restart_round_now(eng));
    assert_eq!(
        b.run(|game, _| game.state().players[0].as_ref().unwrap().money),
        ct_before_restart + REWARD_VIP_ESCAPED
    );
}

#[test]
fn vip_killed_rewards_the_killer() {
    // Distant `func_buyzone` keeps `#Hint_press_buy_to_purchase` out of the killer's hint queue.
    let mut b = setup_vip_bench(vec![sized_zone("func_buyzone", "5000 5000 0")], 4, false);
    enter(&mut b, 1);
    join(&mut b, 0, 1, 1); // T 0 (probe)
    join(&mut b, 1, 2, 1); // CT 1 (VIP)
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
    });
    // Wait out any initial `#Hint_spotted_an_enemy` (6 s duration) so `#Hint_reward_for_killing_vip`
    // flushes immediately when the VIP is killed.
    b.step(650);
    let killer_before = b.run(|game, _| game.state().players[0].as_ref().unwrap().money);
    b.probe.clear_messages();

    b.run(|game, eng| {
        assert!(!game.damage_player(eng, 1, Some(0), 500.0, dmg::BULLET));
    });
    b.step(20);

    b.run(move |game, _| {
        assert_eq!(
            game.state().players[0].as_ref().unwrap().money,
            killer_before + REWARD_KILLED_VIP
        );
        assert_eq!(game.state().rules.win_status(), WinStatus::Terrorists);
        assert_eq!(game.state().num_terrorist_wins, 1);
    });
    assert!(hints(&b).contains(&"#Hint_reward_for_killing_vip".to_string()));
    assert!(centre_texts(&b).contains(&"#VIP_Assassinated".to_string()));
    assert!(audio_codes(&b).contains(&"%!MRAD_terwin".to_string()));

    let t_before_restart = b.run(|game, _| game.state().players[0].as_ref().unwrap().money);
    b.run(|game, eng| game.restart_round_now(eng));
    assert_eq!(
        b.run(|game, _| game.state().players[0].as_ref().unwrap().money),
        t_before_restart + REWARD_VIP_ASSASSINATED
    );
}

#[test]
fn escape_ratio_ends_the_round() {
    let mut entities = spawns(4, 4);
    entities.push(sized_zone("func_escapezone", "1000 0 0"));
    let mut b = Bench::with_config(
        ServerMap::stub_with_entities("test_es", entities),
        ServerConfig {
            max_clients: 4,
            max_edicts: 256,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        true,
    );
    b.idle_cmds = true;
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_autoteambalance 0");
    let _ = b.server.console_command("mp_limitteams 0");
    b.run(|_, eng| {
        eng.set_cvar("sv_gravity", "0");
        for id in eng.entity_ids() {
            if eng.ent(id).is_some_and(|v| v.solid == SOLID_TRIGGER) {
                let origin = eng.ent(id).unwrap().origin;
                eng.set_size(id, Vec3::splat(-150.0), Vec3::splat(150.0));
                eng.set_origin(id, origin);
            }
        }
    });
    b.step(6);

    enter(&mut b, 1);
    enter(&mut b, 2);
    join(&mut b, 0, 1, 1); // T 0
    join(&mut b, 1, 1, 1); // T 1
    join(&mut b, 2, 2, 1); // CT 2
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        assert_eq!(game.state().rules.num_escapers(), 2);
        assert_eq!(game.state().rules.have_escaped(), 0);
        // Give T0 armor to verify `not_killed = false` resets T gear on escape round restart.
        game.state_mut().players[0].as_mut().unwrap().armor = 100.0;
    });
    b.step(6);
    b.probe.clear_messages();

    // 1 of 2 Ts escapes -> 0.5 >= 0.5 -> `#Terrorists_Escaped`.
    b.run(|game, eng| {
        game.place_player(eng, 0, Vec3::new(1000.0, 0.0, 0.0));
        cmd_slot(game, eng, 0);
        assert_eq!(game.state().rules.have_escaped(), 1);
        assert_eq!(game.state().rules.win_status(), WinStatus::Terrorists);
        assert_eq!(game.state().num_terrorist_wins, 1);
    });
    b.step(12);
    assert!(centre_texts(&b).contains(&"#Terrorists_Escaped".to_string()));
    assert!(audio_codes(&b).contains(&"%!MRAD_terwin".to_string()));

    // Round 2: Ts get REWARD_TERRORISTS_ESCAPED (3150), CT gets default loser bonus (1400), T armor reset.
    let (t0, ct0) = b.run(|game, _| {
        (
            game.state().players[0].as_ref().unwrap().money,
            game.state().players[2].as_ref().unwrap().money,
        )
    });
    b.run(move |game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
        assert_eq!(game.state().players[0].as_ref().unwrap().armor, 0.0);
        assert_eq!(
            game.state().players[0].as_ref().unwrap().money,
            t0 + REWARD_TERRORISTS_ESCAPED
        );
        assert_eq!(
            game.state().players[2].as_ref().unwrap().money,
            ct0 + REWARD_LOSER_BONUS_DEFAULT
        );
    });

    // Kill both Ts with 0 escaped -> `#CTs_PreventEscape`, CT reward `REWARD_CTS_PREVENT_ESCAPE` (3500).
    b.probe.clear_messages();
    b.run(|game, eng| {
        assert!(!game.damage_player(eng, 0, Some(2), 500.0, dmg::BULLET));
        assert!(!game.damage_player(eng, 1, Some(2), 500.0, dmg::BULLET));
        assert_eq!(game.state().rules.win_status(), WinStatus::Cts);
        assert_eq!(game.state().num_ct_wins, 1);
    });
    b.step(12);
    assert!(centre_texts(&b).contains(&"#CTs_PreventEscape".to_string()));
    assert!(audio_codes(&b).contains(&"%!MRAD_ctwin".to_string()));

    // Round 3: CT earns 3500, Ts earn NO loser bonus on escape maps when CTs win.
    let (t1, ct1) = b.run(|game, _| {
        (
            game.state().players[0].as_ref().unwrap().money,
            game.state().players[2].as_ref().unwrap().money,
        )
    });
    b.run(move |game, eng| {
        game.restart_round_now(eng);
        assert_eq!(game.state().players[0].as_ref().unwrap().money, t1);
        assert_eq!(
            game.state().players[2].as_ref().unwrap().money,
            ct1 + REWARD_CTS_PREVENT_ESCAPE
        );
        // Set distinctive win counts before round 4 restart (which triggers 3-round SwapAllPlayers).
        game.state_mut().num_terrorist_wins = 4;
        game.state_mut().num_ct_wins = 2;
        game.restart_round_now(eng);
        assert_eq!(game.state().players[0].as_ref().unwrap().team, Team::Ct);
        assert_eq!(game.state().players[1].as_ref().unwrap().team, Team::Ct);
        assert_eq!(
            game.state().players[2].as_ref().unwrap().team,
            Team::Terrorist
        );
        assert_eq!(game.state().num_terrorist_wins, 2);
        assert_eq!(game.state().num_ct_wins, 4);
    });
}

#[test]
fn as_oilrig_loads_and_picks_a_vip() {
    let Some(vfs) = discover() else {
        eprintln!("SKIPPED: Half-Life game directory not found (set CS16_GAMEDIR)");
        return;
    };
    if !vfs.exists("maps/as_oilrig.bsp") {
        eprintln!("SKIPPED: maps/as_oilrig.bsp not found");
        return;
    }

    let mut b = Bench::with_config(
        map_named(&vfs, "as_oilrig"),
        ServerConfig {
            max_clients: 4,
            max_edicts: 1024,
            ..Default::default()
        },
        &[("cl_lw", "0"), ("_vgui_menus", "0")],
        true,
    );
    b.idle_cmds = true;
    let _ = b.server.console_command("mp_freezetime 0");
    let _ = b.server.console_command("mp_autoteambalance 0");
    let _ = b.server.console_command("mp_limitteams 0");
    b.step(6);

    enter(&mut b, 1);
    join(&mut b, 0, 2, 1); // CT 0
    join(&mut b, 1, 1, 1); // T 1
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
    });

    b.run(|game, eng| {
        assert!(game.state().rules.map_has_vip_safety_zone());
        assert_eq!(game.state().rules.vip_slot(), Some(0));
        let p0 = game.state().players[0].as_ref().unwrap();
        assert!(p0.is_vip);
        assert_eq!(eng.client_key(0, "model"), Some("vip"));
        assert_eq!(p0.kevlar, ArmorType::VestHelm);
        assert_eq!(p0.armor, 200.0);
        let weapons = game.carried_weapons(0);
        assert!(weapons.contains(&WeaponId::Usp));
        assert_eq!(weapons.len(), 2); // knife + usp
        let acp_idx = usize::try_from(ammo_index("45acp")).unwrap();
        assert_eq!(game.player_ammo(0, acp_idx), Some(12));

        let v0 = eng.ent(eng.client_edict(0)).unwrap();
        assert_eq!(v0.max_speed, 227.0);
        let vip_spot = eng
            .find_by_classname(None, "info_vip_start")
            .and_then(|id| eng.ent(id))
            .expect("as_oilrig has info_vip_start");
        assert!(
            (v0.origin - (vip_spot.origin + Vec3::new(0.0, 0.0, 1.0))).length() < 1.0,
            "expected VIP at info_vip_start, got {:?} vs {:?}",
            v0.origin,
            vip_spot.origin
        );
    });
}

/// Three CTs and a T on an `as_` stub map; the first CT is the VIP after the first restart.
fn three_cts_bench() -> Bench {
    let mut b = setup_vip_bench(vec![spawn_at("info_vip_start", "500 0 0")], 5, false);
    for slot in 1..4 {
        enter(&mut b, slot);
    }
    join(&mut b, 0, 2, 1);
    join(&mut b, 1, 2, 1);
    join(&mut b, 2, 2, 1);
    join(&mut b, 3, 1, 1);
    b.run(|game, eng| {
        game.restart_round_now(eng);
        game.end_freeze_period(eng);
    });
    b.step(6);
    b
}

/// `CBasePlayer::Kill` (`player.cpp` 10979): `if (m_pVIP == this) m_iConsecutiveVIP = 10`, so the
/// next `PickNextVIP` takes the rotation branch (the `m_iLastPick`-th CT: the second one first).
#[test]
fn a_vip_suicide_forces_the_next_pick() {
    let mut b = three_cts_bench();
    assert_eq!(b.run(|game, _| game.state().rules.vip_slot()), Some(0));
    command(&mut b, 0, "kill");
    assert_eq!(b.run(|game, _| game.state().rules.consecutive_vip()), 10);
    b.run(|game, eng| {
        game.restart_round_now(eng);
        // `m_iLastPick` 1 -> 2: the second CT in entity order.
        assert_eq!(game.state().rules.vip_slot(), Some(1));
        assert!(!game.state().players[0].as_ref().unwrap().is_vip);
        assert!(game.state().players[1].as_ref().unwrap().is_vip);
    });
}

/// `PickNextVIP` rotation: `m_iLastPick` counts CTs in entity order, wraps past `m_iNumCT` to 1,
/// and a pick that is the current VIP is swapped for `pLastPlayer` (the previous non-spectator).
#[test]
fn the_rotation_counts_cts_and_swaps_a_pick_that_is_the_vip() {
    let mut b = three_cts_bench();
    // Rotation 1: last pick 2 -> CT slot 1. Rotation 2: 3 -> slot 2.
    for expected in [1u8, 2] {
        let vip = b.run(|game, _| game.state().rules.vip_slot().unwrap());
        b.step(110); // `ClientKill` refuses a second suicide within a second.
        command(&mut b, vip, "kill");
        b.run(move |game, eng| {
            game.restart_round_now(eng);
            assert_eq!(game.state().rules.vip_slot(), Some(expected));
        });
    }
    // Rotation 3: last pick 4 wraps to 1 -> slot 0.
    b.step(110);
    command(&mut b, 2, "kill");
    b.run(|game, eng| {
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(0));
    });
    // Rotation 4: last pick 2 -> slot 1 again.
    b.step(110);
    command(&mut b, 0, "kill");
    b.run(|game, eng| {
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(1));
    });
    // Slot 2 queues for the VIP and becomes it; when it kills itself, last pick 3 is slot 2, the
    // current VIP, so the previous non-spectator player (slot 1) takes over.
    b.run(|game, eng| {
        game.client_command(eng, 2, &["become_vip".to_string()]);
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(2));
    });
    b.step(110);
    command(&mut b, 2, "kill");
    b.run(|game, eng| {
        game.restart_round_now(eng);
        assert_eq!(game.state().rules.vip_slot(), Some(1));
        assert!(game.state().players[1].as_ref().unwrap().is_vip);
        assert!(!game.state().players[2].as_ref().unwrap().is_vip);
    });
}

/// `ClientDisconnected` (`multiplay_gamerules.cpp` 3604-3607): only `m_pVIP = nullptr`;
/// `m_iConsecutiveVIP` is left as it was.
#[test]
fn a_vip_leaving_only_clears_the_vip_pointer() {
    let mut b = three_cts_bench();
    let before = b.run(|game, _| game.state().rules.consecutive_vip());
    assert!(before > 0);
    b.run(|game, eng| game.client_disconnect(eng, 0));
    b.run(move |game, _| {
        assert_eq!(game.state().rules.vip_slot(), None);
        assert_eq!(game.state().rules.consecutive_vip(), before);
    });
}

/// A team change keeps `m_bIsVIP`, `m_pVIP` and `m_iConsecutiveVIP` in vanilla: a dead VIP may
/// change team (only while someone waits in the queue) and stays the VIP.
#[test]
fn a_dead_vip_changing_team_stays_the_vip() {
    let mut b = three_cts_bench();
    b.run(|game, eng| {
        game.client_command(eng, 1, &["become_vip".to_string()]);
    });
    command(&mut b, 0, "kill");
    command(&mut b, 0, "jointeam 1");
    b.run(|game, _| {
        let p0 = game.state().players[0].as_ref().unwrap();
        assert_eq!(p0.team, Team::Terrorist);
        assert!(p0.is_vip);
        assert_eq!(game.state().rules.vip_slot(), Some(0));
        assert_eq!(game.state().rules.consecutive_vip(), 10);
    });
}
