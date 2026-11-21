//! Money: the starting money, the rewards of a round with the loser bonus and its streaks, the
//! reward for a kill and the penalty for a team kill, `mp_tkpunish` and `mp_autokick`. Time is
//! counted in server frames of 10 ms; the probe is in slot 0 (a terrorist), the other slots are
//! players the test drives directly (they receive no messages, so their accounts are read from
//! the game's state).

mod common;

use common::{Bench, command, enter, join, spawns};
use cs_engine::GameDll;
use cs_game::rules::WinStatus;
use cs_probe::ProbeState;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;

/// Frames of 10 ms in `secs` seconds.
fn frames(secs: f64) -> usize {
    (secs * 100.0).round() as usize
}

/// The account of the player of `slot`.
fn money(b: &Bench, slot: usize) -> i32 {
    b.server.game().game.state().players[slot]
        .as_ref()
        .expect("a player in the slot")
        .money
}

fn set_money(b: &mut Bench, slot: usize, amount: i32) {
    b.server.game_mut().game.state_mut().players[slot]
        .as_mut()
        .expect("a player in the slot")
        .money = amount;
}

fn economy<T: 'static>(b: &Bench, f: impl Fn(&cs_game::money::Economy) -> T) -> T {
    f(b.server.game().game.state().rules.economy())
}

fn win_status(b: &Bench) -> WinStatus {
    b.server.game().game.state().rules.win_status()
}

/// A server of `slots` slots on a stub map with plenty of spawn points, the probe at slot 0 and
/// the freeze time of a round at two seconds.
fn bench(slots: u8) -> Bench {
    let mut b = Bench::stub_slots(spawns(8, 8), slots);
    b.idle_cmds = true;
    b.step(6);
    b.probe.clear_messages();
    b.run(|_, eng| {
        eng.set_cvar("mp_freezetime", "2");
        eng.set_cvar("mp_roundtime", "2");
    });
    b
}

/// A match under way: the terrorists of `ts` and the counter-terrorists of `cts` have joined,
/// the game has commenced and the first round has restarted (in its freeze period).
fn match_under_way(slots: u8, ts: &[u8], cts: &[u8]) -> Bench {
    let mut b = bench(slots);
    for (team, group) in [(1, ts), (2, cts)] {
        for &slot in group {
            if slot != 0 {
                enter(&mut b, slot);
            }
            join(&mut b, slot, team, 1);
        }
    }
    b.step(frames(3.3));
    assert!(b.server.game().game.state().rules.game_started());
    assert!(b.server.game().game.state().rules.is_freeze_period());
    b
}

/// Hurts the player of `slot` by `attacker` (a slot; `None` the world) with 500 points.
fn hurt(b: &mut Bench, slot: u8, attacker: Option<u8>) {
    b.run(move |game, eng| game.damage_player(eng, slot, attacker, 500.0, 0));
}

/// Plays a round that the terrorists (slot 0) win when `ts_win`, else the counter-terrorists:
/// the freeze period runs out, the losers die (by the world), the next round has started in its
/// freeze period when this returns.
fn play_round(b: &mut Bench, ts_win: bool, ts: &[u8], cts: &[u8]) {
    b.step(frames(2.3));
    for &slot in if ts_win { cts } else { ts } {
        hurt(b, slot, None);
    }
    b.step(3);
    assert_eq!(
        win_status(b),
        if ts_win {
            WinStatus::Terrorists
        } else {
            WinStatus::Cts
        }
    );
    b.step(frames(5.3));
    assert_eq!(win_status(b), WinStatus::None);
    assert!(b.server.game().game.state().rules.is_freeze_period());
}

fn money_msgs(b: &Bench) -> Vec<(i32, bool)> {
    b.messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::Money { amount, flash } => Some((amount, flash)),
            _ => None,
        })
        .collect()
}

fn said(b: &Bench, dest: u8, text: &str) -> bool {
    b.messages().iter().any(
        |m| matches!(m, CsUserMsg::TextMsg { dest: d, message, .. } if *d == dest && message == text),
    )
}

#[test]
fn start_money_from_cvar() {
    let mut b = bench(3);
    // `CheckStartMoney` keeps `mp_startmoney` in 800..=16000 and writes the value back.
    for (set, want) in [("5000", 5000), ("100", 800), ("99999", 16000), ("-5", 800)] {
        b.run(move |_, eng| eng.set_cvar("mp_startmoney", set));
        enter(&mut b, 1);
        assert_eq!(money(&b, 1), want, "mp_startmoney {set}");
        let cvar = b.run(|_, eng| eng.cvar_f32("mp_startmoney"));
        assert_eq!(cvar as i32, want);
        b.run(|game, eng| game.client_disconnect(eng, 1));
    }

    // A new match gives everybody the starting money, told as a change the HUD flashes, after
    // the empty account `Reset` sends.
    b.run(|_, eng| eng.set_cvar("mp_startmoney", "3000"));
    b.probe.clear_messages();
    for (slot, team) in [(0u8, 1u8), (1, 2)] {
        if slot != 0 {
            enter(&mut b, slot);
        }
        join(&mut b, slot, team, 1);
    }
    b.step(frames(3.3));
    assert_eq!(money(&b, 0), 3000);
    assert_eq!(money(&b, 1), 3000);
    let sent = money_msgs(&b);
    assert!(
        sent.windows(2).any(|w| w == [(0, false), (3000, true)]),
        "{sent:?}"
    );
    // (The HUD's own update after a respawn repeats the account without the flash.)
    assert!(sent.contains(&(3000, true)));
}

#[test]
fn round_rewards_as_regamedll() {
    let mut b = match_under_way(3, &[0], &[1, 2]);
    // The first round pays nothing but the starting money.
    assert_eq!((money(&b, 0), money(&b, 1), money(&b, 2)), (800, 800, 800));
    assert_eq!(economy(&b, |e| e.loser_bonus()), 1400);
    assert_eq!(economy(&b, |e| (e.account_ct(), e.account_terrorist())), (0, 0));

    // The terrorists win: they are paid 3000, the counter-terrorists the loser bonus of 1400,
    // when the next round starts.
    b.step(frames(2.3));
    hurt(&mut b, 1, None);
    hurt(&mut b, 2, None);
    b.step(3);
    assert_eq!(win_status(&b), WinStatus::Terrorists);
    assert_eq!(
        economy(&b, |e| (e.account_ct(), e.account_terrorist())),
        (0, 3000)
    );
    assert_eq!((money(&b, 0), money(&b, 1)), (800, 800));
    b.probe.clear_messages();
    b.step(frames(5.3));
    assert_eq!((money(&b, 0), money(&b, 1), money(&b, 2)), (3800, 2200, 2200));
    assert_eq!(economy(&b, |e| (e.account_ct(), e.account_terrorist())), (0, 0));
    // The terrorist probe was told, flashing the change.
    assert!(money_msgs(&b).contains(&(3800, true)));
    // One round lost is no streak: the bonus stays.
    assert_eq!(economy(&b, |e| e.consecutive_ct_loses()), 1);
    assert_eq!(economy(&b, |e| e.loser_bonus()), 1400);

    // A player marked as not receiving money is skipped (and is paid again once it spawns).
    b.server.game_mut().game.state_mut().players[1]
        .as_mut()
        .unwrap()
        .receives_no_money_next_round = true;
    play_round(&mut b, true, &[0], &[1, 2]);
    // Slot 1 got nothing, slot 2 the grown bonus of 1900; the terrorist 3000 again.
    assert_eq!((money(&b, 0), money(&b, 1), money(&b, 2)), (6800, 2200, 4100));
    assert!(
        !b.server.game().game.state().players[1]
            .as_ref()
            .unwrap()
            .receives_no_money_next_round
    );

    // The counter-terrorists win, ending their streak: a streak that grew the bonus falls back
    // to the minimum, which the new loser is paid; the winners get 3000.
    play_round(&mut b, false, &[0], &[1, 2]);
    assert_eq!(economy(&b, |e| e.loser_bonus()), 1500);
    assert_eq!(economy(&b, |e| e.consecutive_ct_loses()), 0);
    assert_eq!(economy(&b, |e| e.consecutive_terrorist_loses()), 1);
    assert_eq!((money(&b, 0), money(&b, 1), money(&b, 2)), (8300, 5200, 7100));

    // A complete reset (`sv_restartround`) forgets all of it: starting money, default bonus.
    b.run(|_, eng| eng.set_cvar("sv_restartround", "1"));
    b.step(frames(3.5));
    assert_eq!((money(&b, 0), money(&b, 1), money(&b, 2)), (800, 800, 800));
    assert_eq!(economy(&b, |e| e.loser_bonus()), 1400);
    assert_eq!(economy(&b, |e| e.consecutive_terrorist_loses()), 0);
}

#[test]
fn kill_reward_and_teamkill_penalty() {
    let mut b = match_under_way(4, &[0, 2], &[1, 3]);
    b.run(|_, eng| eng.set_cvar("mp_friendlyfire", "1"));
    b.step(frames(2.3));
    set_money(&mut b, 0, 5000);

    // An enemy killed: 300, flashing.
    b.probe.clear_messages();
    hurt(&mut b, 1, Some(0));
    b.step(2);
    assert_eq!(money(&b, 0), 5300);
    assert!(money_msgs(&b).contains(&(5300, true)));
    assert_eq!(
        b.server.game().game.state().players[0].as_ref().unwrap().frags,
        1
    );

    // A teammate killed: -3300, the kill counted, the warning on the killer's screen.
    b.probe.clear_messages();
    hurt(&mut b, 2, Some(0));
    b.step(2);
    assert_eq!(money(&b, 0), 2000);
    assert!(money_msgs(&b).contains(&(2000, true)));
    assert!(said(&b, 4, "#Killed_Teammate"));
    assert!(b.messages().iter().any(|m| matches!(
        m,
        CsUserMsg::TextMsg { dest: 2, message, params }
            if message == "#Game_teammate_kills" && params == &["1".to_string()]
    )));
    let p = b.server.game().game.state().players[0].clone().unwrap();
    assert_eq!(p.team_kills, 1);
    assert!(p.just_killed_teammate);
    assert_eq!(p.frags, 0);

    // The world and a suicide pay nothing.
    b.run(|game, eng| game.respawn_player(eng, 2));
    hurt(&mut b, 2, None);
    hurt(&mut b, 0, Some(0));
    assert_eq!(money(&b, 0), 2000);
    assert_eq!(money(&b, 2), 800);
}

#[test]
fn money_is_clamped_and_loss_bonus_grows() {
    // The account stays within 0..=16000.
    let mut b = match_under_way(4, &[0, 2], &[1, 3]);
    b.run(|_, eng| eng.set_cvar("mp_friendlyfire", "1"));
    b.step(frames(2.3));
    set_money(&mut b, 0, 15_900);
    hurt(&mut b, 1, Some(0));
    assert_eq!(money(&b, 0), 16_000);
    set_money(&mut b, 0, 1000);
    hurt(&mut b, 2, Some(0));
    assert_eq!(money(&b, 0), 0);

    // The loser bonus of the counter-terrorists, round after round lost: the first loss pays
    // the default 1400, then 500 more for each further one, and the original keeps adding while
    // the bonus is below 3000 (2900 grows to 3400, then it stays).
    let mut b = match_under_way(3, &[0], &[1, 2]);
    let mut paid = Vec::new();
    for _ in 0..7 {
        let before = money(&b, 1);
        play_round(&mut b, true, &[0], &[1, 2]);
        paid.push(money(&b, 1) - before);
    }
    // The sixth is cut short by the limit of the account, the seventh paid nothing.
    assert_eq!(paid, [1400, 1900, 2400, 2900, 3400, 3200, 0]);
    assert_eq!(economy(&b, |e| e.loser_bonus()), 3400);
    assert_eq!(economy(&b, |e| e.consecutive_ct_loses()), 7);
    assert_eq!(money(&b, 1), 16_000);
}

#[test]
fn team_kill_is_punished_at_the_next_round() {
    let mut b = match_under_way(4, &[0, 2], &[1, 3]);
    b.run(|_, eng| {
        eng.set_cvar("mp_friendlyfire", "1");
        eng.set_cvar("mp_tkpunish", "1");
    });
    b.step(frames(2.3));
    hurt(&mut b, 2, Some(0));
    assert!(
        b.server.game().game.state().players[0]
            .as_ref()
            .unwrap()
            .just_killed_teammate
    );
    // The round ends; the punished killer is told to kill itself as the next round starts.
    b.probe.clear_messages();
    hurt(&mut b, 1, None);
    hurt(&mut b, 3, None);
    b.step(frames(5.3));
    let told = b
        .probe
        .messages()
        .iter()
        .any(|m| matches!(m, SvcMessage::StuffText(t) if t == "kill\n"));
    assert!(told);
    let p = b.server.game().game.state().players[0].clone().unwrap();
    assert!(p.punished_for_tk && !p.just_killed_teammate);
    assert!(p.alive);
    // It does as told, and the punishment is over with its death.
    command(&mut b, 0, "kill");
    let p = b.server.game().game.state().players[0].clone().unwrap();
    assert!(!p.alive && !p.punished_for_tk);

    // With `mp_tkpunish` off nobody is told.
    let mut b = match_under_way(4, &[0, 2], &[1, 3]);
    b.run(|_, eng| eng.set_cvar("mp_friendlyfire", "1"));
    b.step(frames(2.3));
    hurt(&mut b, 2, Some(0));
    b.probe.clear_messages();
    hurt(&mut b, 1, None);
    hurt(&mut b, 3, None);
    b.step(frames(5.3));
    assert!(
        !b.probe
            .messages()
            .iter()
            .any(|m| matches!(m, SvcMessage::StuffText(t) if t == "kill\n"))
    );
    assert!(!b.server.game().game.state().players[0].as_ref().unwrap().punished_for_tk);
}

#[test]
fn the_third_team_kill_is_kicked() {
    let mut b = match_under_way(3, &[0, 2], &[1]);
    b.run(|_, eng| eng.set_cvar("mp_friendlyfire", "1"));
    b.step(frames(2.3));
    for n in 1..=3 {
        assert_eq!(b.probe.state(), &ProbeState::InGame, "kill {n}");
        hurt(&mut b, 2, Some(0));
        b.run(|game, eng| game.respawn_player(eng, 2));
        b.step(3);
    }
    assert!(matches!(b.probe.state(), ProbeState::Disconnected(_)));

    // Not with `mp_autokick` off.
    let mut b = match_under_way(3, &[0, 2], &[1]);
    b.run(|_, eng| {
        eng.set_cvar("mp_friendlyfire", "1");
        eng.set_cvar("mp_autokick", "0");
    });
    b.step(frames(2.3));
    for _ in 0..4 {
        hurt(&mut b, 2, Some(0));
        b.run(|game, eng| game.respawn_player(eng, 2));
        b.step(3);
    }
    assert_eq!(b.probe.state(), &ProbeState::InGame);
    assert_eq!(
        b.server.game().game.state().players[0].as_ref().unwrap().team_kills,
        4
    );
}

#[test]
fn blink_account_sends_the_count() {
    let mut b = bench(2);
    b.probe.clear_messages();
    b.run(|_, eng| cs_game::money::blink_account(eng, 0, 3));
    b.step(2);
    assert!(b.messages().contains(&CsUserMsg::BlinkAcct(3)));
}
