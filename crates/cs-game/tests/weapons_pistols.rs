//! The pistols (Task 8 of phase 2D): the Glock 18, USP, P228, Desert Eagle, Five-seven and the
//! dual Berettas, ReGameDLL_CS `wpn_shared/wpn_*.cpp` read 2026-11-22, vanilla build. A shooter
//! (slot 0, the probe, counter-terrorist) and a victim (slot 1, terrorist) on a stub map; the
//! shooter has no ground under it (gravity 0), so the shots take the "in the air" spread.
//!
//! The server of these tests precaches the weapons' events at map activation (the world's own
//! weapon precache is Task 12's), so the events of the shots reach the probe.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{run_cmd, spawns};
use cs_cvar::CVarRegistry;
use cs_engine::{EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_game::weapons::{WeaponId, ammo_index, new_weapon};
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::event::WireEvent;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
use glam::Vec3;

const IN_ATTACK: u16 = 1 << 0;
const IN_ATTACK2: u16 = 1 << 11;
const WPNSTATE_USP_SILENCED: i32 = 1 << 0;
const WPNSTATE_GLOCK18_BURST_MODE: i32 = 1 << 1;
const WPNSTATE_ELITE_LEFT: i32 = 1 << 3;
const EF_MUZZLEFLASH: u32 = 2;

type Action = Box<dyn FnOnce(&mut CsGame, &mut EngineApi<'_>)>;

/// `CsGame` that precaches the weapons at the activation of the map and runs the closures the
/// test queued at the start of the next frame.
struct PistolRig {
    game: CsGame,
    actions: Vec<Action>,
}

impl GameDll for PistolRig {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.game.usermsgs()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        cs_game::weapons::precache_all(eng);
        self.game.server_activate(eng);
    }
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_deactivate(eng);
    }
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: cs_engine::EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.game.spawn_entity(eng, id, classname, keys)
    }
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.game.client_connect(eng, slot)
    }
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_put_in_server(eng, slot);
    }
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_disconnect(eng, slot);
    }
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.game.client_command(eng, slot, argv);
    }
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_userinfo_changed(eng, slot);
    }
    fn run_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd, seed: u32) {
        self.game.run_usercmd(eng, slot, cmd, seed);
    }
    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        self.game.post_usercmd(eng, slot, cmd);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        for action in std::mem::take(&mut self.actions) {
            action(&mut self.game, eng);
        }
        self.game.start_frame(eng, frametime);
    }
    fn think(&mut self, eng: &mut EngineApi<'_>, id: cs_engine::EdictId) {
        self.game.think(eng, id);
    }
    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: cs_engine::EdictId, other: cs_engine::EdictId) {
        self.game.touch(eng, toucher, other);
    }
    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: cs_engine::EdictId, other: cs_engine::EdictId) {
        self.game.blocked(eng, pusher, other);
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.game.update_client_data(eng, slot)
    }
    fn setup_visibility(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Option<Vec3> {
        self.game.setup_visibility(eng, slot)
    }
}

/// A server of three slots on a stub map running a [`PistolRig`], with the probe in slot 0.
struct Arena {
    server: Server<PistolRig>,
    hub: LoopbackHub,
    probe: ProbeClient<LoopbackEndpoint>,
    now: f64,
}

impl Arena {
    /// The shooter (slot 0, counter-terrorist) and the victim (slot 1, terrorist) spawned,
    /// gravity off, the freeze period over.
    fn new() -> Self {
        let map = ServerMap::stub_with_entities("test", spawns(4, 4));
        let cfg = ServerConfig { max_clients: 3, max_edicts: 256, ..ServerConfig::default() };
        let rig = PistolRig { game: CsGame::new(), actions: Vec::new() };
        let mut server = Server::new(
            cfg,
            map,
            rig,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        let mut hub = LoopbackHub::new();
        let endpoint = hub.connect();
        let cfg = ProbeConfig {
            name: "p".to_string(),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(b"probe"),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        // A probe with `cl_lw 1` (the default) is left out of the events of its own shots
        // (`FEV_NOTHOST`); this one plays them, so it can read them.
        probe.set_userinfo("cl_lw", "0");
        probe.suppress_moves(true);
        probe.connect();
        let mut now = 0.0;
        for _ in 0..300 {
            probe.pump(now);
            now = ((now / 0.01).round() + 1.0) * 0.01;
            server.frame(&mut hub, now);
            if probe.state() == &ProbeState::InGame {
                break;
            }
        }
        assert_eq!(probe.state(), &ProbeState::InGame);
        let mut a = Self { server, hub, probe, now };
        a.run(|game, eng| {
            eng.set_cvar("sv_gravity", "0");
            game.client_put_in_server(eng, 1);
            assert!(game.spawn_player(eng, 0, Team::Ct, 0));
            assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
            game.end_freeze_period(eng);
        });
        a
    }

    /// Runs `frames` server frames (10 ms each).
    fn step(&mut self, frames: usize) {
        for _ in 0..frames {
            self.probe.pump(self.now);
            self.now = ((self.now / 0.01).round() + 1.0) * 0.01;
            self.server.frame(&mut self.hub, self.now);
        }
    }

    /// Runs `f` as the game at the start of the next frame.
    fn run<T: 'static>(&mut self, f: impl FnOnce(&mut CsGame, &mut EngineApi<'_>) -> T + 'static) -> T {
        let out = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&out);
        self.server.game_mut().actions.push(Box::new(move |game, eng| {
            *sink.borrow_mut() = Some(f(game, eng));
        }));
        self.step(1);
        let result = out.borrow_mut().take();
        result.expect("the game ran the action")
    }

    /// The events the probe has received since the messages were last cleared.
    fn events(&mut self) -> Vec<WireEvent> {
        self.step(3);
        self.probe
            .messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::Event(events) => Some(events.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

/// One command with `buttons`, as the engine runs it: `v.button` is set before `CmdStart`.
fn press(game: &mut CsGame, eng: &mut EngineApi<'_>, buttons: u16) {
    let id = eng.client_edict(0);
    eng.ent_mut(id).unwrap().button = i32::from(buttons);
    run_cmd(game, eng, 0, &GoldSrcUserCmd { buttons, msec: 10, ..Default::default() }, 0);
}

fn weapon(game: &mut CsGame, id: WeaponId) -> &mut cs_game::weapons::Weapon {
    game.weapon_mut(0, id).expect("the player carries the weapon")
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn health(game: &CsGame, slot: usize) -> f32 {
    game.state().players[slot].as_ref().unwrap().health
}

/// Gives the shooter pistol `id` with `clip` rounds loaded and 100 in reserve, selects it and
/// spends the deploy delay; stands the shooter at the origin and the victim `gap` units away
/// along +X (the near face of its box is at `gap`).
fn arm(a: &mut Arena, id: WeaponId, clip: i32, gap: f32) {
    a.run(move |game, eng| {
        let behavior = new_weapon(id).expect("a pistol");
        assert!(game.give_weapon(eng, 0, behavior, clip, 100));
        assert!(game.select_weapon(eng, 0, id));
        game.set_next_attack(0, 0.0);
        weapon(game, id).time_weapon_idle = 0.0;
        let me = eng.client_edict(0);
        eng.set_origin(me, Vec3::ZERO);
        let victim = eng.client_edict(1);
        eng.set_origin(victim, Vec3::new(gap + 16.0, 0.0, 0.0));
    });
}

/// What a row of the table says about a pistol.
struct Row {
    id: WeaponId,
    /// The event script the pistol plays.
    event: &'static str,
    /// `iMaxClip`.
    max_clip: i32,
    /// How far the victim stands, and the health the hit takes: `int(damage * range_modifier ^
    /// (distance / 500))` at that distance.
    gap: f32,
    damage: i32,
    /// `m_flAccuracy` after `Deploy`.
    deploy_accuracy: f32,
    /// The cycle time after the pistol's own deduction (what the first shot waits).
    cycle: f32,
    /// `m_flTimeWeaponIdle` after a shot.
    idle: f32,
    /// The recoil of the shot (`punchangle.x -= recoil`).
    recoil: f32,
    /// The accuracy loss of the next shot: `acc -= (base - dt) * k`, kept within `[low, high]`.
    base: f32,
    k: f32,
    high: f32,
    low: f32,
}

const ROWS: [Row; 6] = [
    Row {
        id: WeaponId::Glock18,
        event: "events/glock18.sc",
        max_clip: 20,
        gap: 200.0,
        damage: 22,
        deploy_accuracy: 0.9,
        cycle: 0.2 - 0.05,
        idle: 2.5,
        recoil: 0.0,
        base: 0.325,
        k: 0.275,
        high: 0.9,
        low: 0.6,
    },
    Row {
        id: WeaponId::Usp,
        event: "events/usp.sc",
        max_clip: 12,
        gap: 200.0,
        damage: 30,
        deploy_accuracy: 0.92,
        cycle: 0.225 - 0.075,
        idle: 2.0,
        recoil: 2.0,
        base: 0.3,
        k: 0.275,
        high: 0.92,
        low: 0.6,
    },
    Row {
        id: WeaponId::P228,
        event: "events/p228.sc",
        max_clip: 13,
        gap: 200.0,
        damage: 29,
        deploy_accuracy: 0.9,
        cycle: 0.2 - 0.05,
        idle: 2.0,
        recoil: 2.0,
        base: 0.325,
        k: 0.3,
        high: 0.9,
        low: 0.6,
    },
    Row {
        id: WeaponId::Deagle,
        event: "events/deagle.sc",
        max_clip: 7,
        gap: 200.0,
        damage: 49,
        deploy_accuracy: 0.9,
        cycle: 0.3 - 0.075,
        idle: 1.8,
        recoil: 2.0,
        base: 0.4,
        k: 0.35,
        high: 0.9,
        low: 0.55,
    },
    Row {
        id: WeaponId::FiveSeven,
        event: "events/fiveseven.sc",
        max_clip: 20,
        gap: 200.0,
        damage: 19,
        deploy_accuracy: 0.92,
        cycle: 0.2 - 0.05,
        idle: 2.0,
        recoil: 2.0,
        base: 0.275,
        k: 0.25,
        high: 0.92,
        low: 0.725,
    },
    Row {
        id: WeaponId::Elite,
        event: "events/elite_left.sc",
        max_clip: 30,
        // Close, because the guns shoot 5 units to the side with the widest spread.
        gap: 60.0,
        damage: 34,
        deploy_accuracy: 0.88,
        cycle: 0.2 - 0.125,
        idle: 2.0,
        recoil: 2.0,
        base: 0.325,
        k: 0.275,
        high: 0.88,
        low: 0.55,
    },
];

#[test]
fn pistol_rows_match_source() {
    for row in &ROWS {
        let mut a = Arena::new();
        let id = row.id;
        arm(&mut a, id, row.max_clip, row.gap);
        let (row_id, max_clip, deploy_accuracy) = (row.id, row.max_clip, row.deploy_accuracy);
        a.run(move |game, eng| {
            // The row of `GetItemInfo`, the speed and the accuracy of `Deploy`.
            let info = cs_game::weapons::item_info(row_id).unwrap();
            assert_eq!(info.max_clip, max_clip, "{row_id:?}");
            let v = eng.ent(eng.client_edict(0)).unwrap();
            assert_eq!(v.max_speed, 250.0, "{row_id:?}");
            let w = weapon(game, row_id);
            assert_eq!(w.accuracy, deploy_accuracy, "{row_id:?}");
            assert_eq!(w.clip, max_clip);
        });

        // The first shot: no decay of the accuracy (`m_flLastFire` is 0), the damage of the row,
        // the cycle time, the idle and the recoil.
        let before = a.run(|game, _| health(game, 1));
        let (ft, flash) = a.run(move |game, eng| {
            let ft = eng.frametime();
            press(game, eng, IN_ATTACK);
            // The flash lives for the frame: the engine clears it at the end (`SV_CleanupEnts`).
            let flash = eng.ent(eng.client_edict(0)).unwrap().effects & EF_MUZZLEFLASH;
            (ft, flash)
        });
        assert_eq!(flash, EF_MUZZLEFLASH, "{:?}", row.id);
        let (damage, cycle, idle, recoil, accuracy) =
            (row.damage, row.cycle, row.idle, row.recoil, row.deploy_accuracy);
        a.run(move |game, eng| {
            assert_eq!(before - health(game, 1), damage as f32, "{row_id:?} damage");
            let w = weapon(game, row_id);
            assert_eq!(w.clip, max_clip - 1, "{row_id:?}");
            assert_eq!(w.accuracy, accuracy, "{row_id:?} first shot keeps the accuracy");
            assert!(w.last_fire > 0.0);
            assert!(approx(w.next_primary_attack, cycle - ft), "{row_id:?} cycle {}", w.next_primary_attack);
            assert!(approx(w.next_secondary_attack, cycle - ft), "{row_id:?}");
            assert!(approx(w.time_weapon_idle, idle - ft), "{row_id:?} idle {}", w.time_weapon_idle);
            assert_eq!(w.shots_fired, 1);
            let v = eng.ent(eng.client_edict(0)).unwrap();
            assert!(approx(v.punchangle.x, -recoil), "{row_id:?} recoil {}", v.punchangle.x);
        });

        // The next shot, after the button is released and a little time has passed: the accuracy
        // falls by `(base - dt) * k`, within its bounds.
        a.run(|game, eng| press(game, eng, 0));
        a.step(5);
        let (base, k, high, low) = (row.base, row.k, row.high, row.low);
        a.run(move |game, eng| {
            let now = eng.time() as f32;
            let (last, acc) = {
                let w = weapon(game, row_id);
                w.next_primary_attack = 0.0;
                assert_eq!(w.shots_fired, 0, "released");
                (w.last_fire, w.accuracy)
            };
            press(game, eng, IN_ATTACK);
            let mut expected = acc - (base - (now - last)) * k;
            if expected > high {
                expected = high;
            } else if expected < low {
                expected = low;
            }
            let w = weapon(game, row_id);
            assert_eq!(w.clip, max_clip - 2, "{row_id:?}");
            assert!(approx(w.accuracy, expected), "{row_id:?} accuracy {} vs {expected}", w.accuracy);
            assert!(w.accuracy < acc, "{row_id:?} a quick tap loses accuracy");
        });
    }
}

#[test]
fn semi_auto_needs_release() {
    for row in &ROWS {
        let mut a = Arena::new();
        let id = row.id;
        arm(&mut a, id, row.max_clip, 500.0);
        let max_clip = row.max_clip;
        a.run(move |game, eng| {
            // Held: the timer is reset every command, so only `++m_iShotsFired > 1` stops it.
            for _ in 0..8 {
                weapon(game, id).next_primary_attack = 0.0;
                press(game, eng, IN_ATTACK);
            }
            let w = weapon(game, id);
            assert_eq!(w.clip, max_clip - 1, "{id:?} fired once");
            assert_eq!(w.shots_fired, 8, "{id:?} every attempt counts");
            // Released for a command: the count is back to 0 and the next press fires.
            press(game, eng, 0);
            assert_eq!(weapon(game, id).shots_fired, 0);
            weapon(game, id).next_primary_attack = 0.0;
            press(game, eng, IN_ATTACK);
            let w = weapon(game, id);
            assert_eq!(w.clip, max_clip - 2, "{id:?} fired again after the release");
            assert_eq!(w.shots_fired, 1);
        });
    }
}

#[test]
fn usp_silencer_toggle_timing() {
    let mut a = Arena::new();
    arm(&mut a, WeaponId::Usp, 12, 200.0);
    a.run(|game, eng| {
        let ft = eng.frametime();
        let id = eng.client_edict(0);
        // Unsilenced: the draw is the unsilenced one, the shot makes a muzzle flash and does 34.
        assert_eq!(weapon(game, WeaponId::Usp).weapon_state & WPNSTATE_USP_SILENCED, 0);
        weapon(game, WeaponId::Usp).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        let w = weapon(game, WeaponId::Usp);
        assert_ne!(w.weapon_state & WPNSTATE_USP_SILENCED, 0, "the silencer is on");
        // `m_flNextSecondaryAttack = m_flTimeWeaponIdle = 3.0`, the primary through
        // `GetNextAttackDelay(3.0)`; the command's `PostThink` takes one frame off all three.
        assert!(approx(w.next_secondary_attack, 3.0 - ft), "{}", w.next_secondary_attack);
        assert!(approx(w.time_weapon_idle, 3.0 - ft), "{}", w.time_weapon_idle);
        assert!(approx(w.next_primary_attack, 3.0 - ft), "{}", w.next_primary_attack);
        let v = eng.ent(id).unwrap();
        assert_eq!(v.weaponanim, 7, "USP_ATTACH_SILENCER");
        assert_eq!(v.button & i32::from(IN_ATTACK2), 0, "the button is consumed");

        // Nothing fires until the timer runs out (a shot needs `m_flNextPrimaryAttack <= 0`).
        for _ in 0..10 {
            press(game, eng, IN_ATTACK);
        }
        assert_eq!(weapon(game, WeaponId::Usp).clip, 12);

        // Silencer off again: the other animation, the same 3 s.
        let w = weapon(game, WeaponId::Usp);
        w.next_secondary_attack = 0.0;
        w.next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        let w = weapon(game, WeaponId::Usp);
        assert_eq!(w.weapon_state & WPNSTATE_USP_SILENCED, 0, "the silencer is off");
        assert!(approx(w.next_primary_attack, 3.0 - ft));
        assert_eq!(eng.ent(id).unwrap().weaponanim, 15, "USP_DETACH_SILENCER");
    });

    // A silenced shot does 30 and shows no muzzle flash; an unsilenced one does 34 and does.
    for silenced in [true, false] {
        let mut a = Arena::new();
        arm(&mut a, WeaponId::Usp, 12, 100.0);
        let lost = a.run(move |game, eng| {
            let before = health(game, 1);
            let id = eng.client_edict(0);
            if silenced {
                weapon(game, WeaponId::Usp).weapon_state |= WPNSTATE_USP_SILENCED;
            }
            eng.ent_mut(id).unwrap().effects = 0;
            press(game, eng, IN_ATTACK);
            let flash = eng.ent(id).unwrap().effects & EF_MUZZLEFLASH != 0;
            assert_eq!(flash, !silenced);
            before - health(game, 1)
        });
        // `int(damage * 0.79 ^ (100 / 500))`: 30 -> 28.6, 34 -> 32.4.
        assert_eq!(lost, if silenced { 28.0 } else { 32.0 }, "silenced {silenced}");
    }
}

#[test]
fn glock_burst_three_shots() {
    let mut a = Arena::new();
    arm(&mut a, WeaponId::Glock18, 20, 200.0);
    a.probe.clear_messages();
    a.run(|game, eng| {
        let ft = eng.frametime();
        let glock = WeaponId::Glock18;
        // The secondary attack switches to the burst mode (and back).
        assert_eq!(weapon(game, glock).weapon_state & WPNSTATE_GLOCK18_BURST_MODE, 0);
        weapon(game, glock).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        let w = weapon(game, glock);
        assert_ne!(w.weapon_state & WPNSTATE_GLOCK18_BURST_MODE, 0);
        assert!(approx(w.next_secondary_attack, 0.3 - ft), "{}", w.next_secondary_attack);
        w.next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        assert_eq!(weapon(game, glock).weapon_state & WPNSTATE_GLOCK18_BURST_MODE, 0);
        weapon(game, glock).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        assert_ne!(weapon(game, glock).weapon_state & WPNSTATE_GLOCK18_BURST_MODE, 0);

        // One press: the first shot at once, the burst armed (`m_flGlock18Shoot`), and the cycle
        // is the burst's 0.5 s (no deduction in the burst mode).
        weapon(game, glock).next_primary_attack = 0.0;
        weapon(game, glock).next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        let w = weapon(game, glock);
        assert_eq!(w.clip, 19);
        assert_eq!(w.glock18_shots_fired, 1);
        assert!(w.glock18_shoot > 0.0, "the burst is armed");
        assert!(approx(w.next_primary_attack, 0.5 - ft), "{}", w.next_primary_attack);
        assert_eq!(w.shots_fired, 0, "the burst does not count as semi-automatic shots");
        assert!(approx(w.time_weapon_idle, 2.5 - ft), "idle {}", w.time_weapon_idle);
        // The two shots that follow are `FireRemaining`'s, one per command (vanilla has no time
        // test for the Glock), and then the burst is over.
        press(game, eng, 0);
        let w = weapon(game, glock);
        assert_eq!((w.clip, w.glock18_shots_fired), (18, 2));
        assert!(w.glock18_shoot > 0.0);
        press(game, eng, 0);
        let w = weapon(game, glock);
        assert_eq!((w.clip, w.glock18_shots_fired), (17, 3));
        assert_eq!(w.glock18_shoot, 0.0, "the third shot ends the burst");
        press(game, eng, 0);
        assert_eq!(weapon(game, glock).clip, 17);
    });

    // The three shots played the Glock's event. The first one is the burst-mode shot in the air
    // (spread 1.2 * (1 - 0.9) = 0.12); `FireRemaining`'s are 0.05 wide and draw the same random
    // numbers (the same command seed), so their spread vector is the first one scaled by
    // 0.05 / 0.12. QUIRK: FireRemaining's integers are the punch times 10000, the first's times
    // 100; the last bool is the clip being empty.
    let index = a.run(|_, eng| eng.precache_event("events/glock18.sc"));
    let events = a.events();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|e| e.index == index));
    let args: Vec<_> = events.iter().map(|e| e.args.expect("arguments")).collect();
    let (first, rest) = (args[0], &args[1..]);
    let (f, g) = if first.fparam1.abs() > first.fparam2.abs() {
        (first.fparam1, [rest[0].fparam1, rest[1].fparam1])
    } else {
        (first.fparam2, [rest[0].fparam2, rest[1].fparam2])
    };
    assert!(f.abs() > 1e-3 && f.abs() <= 0.24, "burst-mode spread {f}");
    for g in g {
        // The wire keeps two decimals of a float argument, so both sides carry up to 0.005 of
        // rounding.
        assert!((g - f * (0.05 / 0.12)).abs() < 0.0125, "FireRemaining spread {g} vs {}", f * (0.05 / 0.12));
    }
    assert!(args.iter().all(|a| a.bparam1 == 0 && a.bparam2 == 0));

    // A burst on a nearly empty clip ends at the empty clip with the count at 3.
    let mut a = Arena::new();
    arm(&mut a, WeaponId::Glock18, 2, 200.0);
    a.run(|game, eng| {
        let glock = WeaponId::Glock18;
        weapon(game, glock).weapon_state |= WPNSTATE_GLOCK18_BURST_MODE;
        press(game, eng, IN_ATTACK);
        press(game, eng, 0);
        press(game, eng, 0);
        let w = weapon(game, glock);
        assert_eq!(w.clip, 0);
        assert_eq!(w.glock18_shoot, 0.0);
        assert_eq!(w.glock18_shots_fired, 3);
    });

    // The semi-automatic mode: 0.2 - 0.05 s, and a shot is one bullet.
    let mut a = Arena::new();
    arm(&mut a, WeaponId::Glock18, 20, 200.0);
    a.run(|game, eng| {
        let ft = eng.frametime();
        press(game, eng, IN_ATTACK);
        let w = weapon(game, WeaponId::Glock18);
        assert_eq!(w.clip, 19);
        assert_eq!(w.glock18_shoot, 0.0);
        assert!(approx(w.next_primary_attack, 0.15 - ft));
    });
}

#[test]
fn elite_alternates_hands() {
    let mut a = Arena::new();
    arm(&mut a, WeaponId::Elite, 30, 200.0);
    let (left, right) = a.run(|game, eng| {
        let elite = WeaponId::Elite;
        // An even clip starts with the left gun.
        assert_ne!(weapon(game, elite).weapon_state & WPNSTATE_ELITE_LEFT, 0);
        (eng.precache_event("events/elite_left.sc"), eng.precache_event("events/elite_right.sc"))
    });
    assert!(left != 0 && right != 0 && left != right);
    a.probe.clear_messages();
    for shot in 0..4 {
        a.run(move |game, eng| {
            let elite = WeaponId::Elite;
            let hand_was_left = weapon(game, elite).weapon_state & WPNSTATE_ELITE_LEFT != 0;
            assert_eq!(hand_was_left, shot % 2 == 0, "shot {shot}");
            press(game, eng, 0);
            weapon(game, elite).next_primary_attack = 0.0;
            press(game, eng, IN_ATTACK);
            let w = weapon(game, elite);
            assert_eq!(w.clip, 29 - shot);
            assert_eq!(w.weapon_state & WPNSTATE_ELITE_LEFT != 0, !hand_was_left);
        });
        a.step(3);
    }
    let events = a.events();
    let indexes: Vec<u16> = events.iter().map(|e| e.index).collect();
    assert_eq!(indexes, vec![left, right, left, right], "left, right, left, right");
    // Both hands carry the clip left in the second integer.
    for (n, e) in events.iter().enumerate() {
        let args = e.args.expect("the arguments are not all zero");
        assert_eq!(args.iparam2, 29 - n as i32, "event {n}");
    }

    // An odd clip leaves the hand as it was at the deploy.
    let mut a = Arena::new();
    arm(&mut a, WeaponId::Elite, 29, 200.0);
    a.run(|game, _| {
        assert_eq!(weapon(game, WeaponId::Elite).weapon_state & WPNSTATE_ELITE_LEFT, 0);
    });
}

#[test]
fn pistol_events_carry_source_args() {
    for row in &ROWS {
        for clip in [3, 1] {
            let mut a = Arena::new();
            let id = row.id;
            arm(&mut a, id, clip, 200.0);
            // The Elite's hand follows the clip at the deploy: an even clip starts with the left
            // gun, an odd one with the right.
            let name = if id == WeaponId::Elite && clip % 2 != 0 {
                "events/elite_right.sc"
            } else {
                row.event
            };
            let index = a.run(move |_, eng| eng.precache_event(name));
            assert_ne!(index, 0, "{name} is precached");
            a.probe.clear_messages();
            let (time, punch) = a.run(move |game, eng| {
                // The event reports the punch before the recoil of the shot (the view's punch
                // decays on its own before the shot, so it is read back after it).
                let me = eng.client_edict(0);
                eng.ent_mut(me).unwrap().punchangle = Vec3::new(-4.0, 3.0, 0.0);
                let time = eng.time() as f32;
                press(game, eng, IN_ATTACK);
                (time, eng.ent(me).unwrap().punchangle)
            });
            let (before_x, before_y) = (punch.x + row.recoil, punch.y);
            let near = |got: i32, want: f32| (got - (want * 100.0) as i32).abs() <= 1;
            let events = a.events();
            assert_eq!(events.len(), 1, "{id:?}: one event");
            let e = events[0];
            assert_eq!(e.index, index, "{id:?}");
            let args = e.args.expect("arguments");
            let empty = clip == 1;
            match id {
                WeaponId::Elite => {
                    // flTimeDiff (the server time: `m_flLastFire` was 0), vecDir.x, int(vecDir.y
                    // * 100), the clip left; both bools false.
                    assert!((args.fparam1 - time).abs() < 1e-2, "elite fparam1 {} vs {time}", args.fparam1);
                    assert!(args.fparam2.abs() <= 1.0);
                    assert!(args.iparam1.abs() <= 100);
                    assert_eq!(args.iparam2, clip - 1);
                    assert_eq!((args.bparam1, args.bparam2), (0, 0));
                }
                WeaponId::Usp => {
                    // int(punch.x * 100), 0 in place of the lateral punch, the clip-empty flag
                    // and the silencer.
                    assert!(near(args.iparam1, before_x), "usp {} vs {before_x}", args.iparam1);
                    assert_eq!(args.iparam2, 0, "usp: the second integer is always 0");
                    assert_eq!(args.bparam1, i32::from(empty));
                    assert_eq!(args.bparam2, 0, "not silenced");
                }
                _ => {
                    assert!(near(args.iparam1, before_x), "{id:?} {} vs {before_x}", args.iparam1);
                    assert!(near(args.iparam2, before_y), "{id:?} {} vs {before_y}", args.iparam2);
                    assert_eq!(args.bparam1, i32::from(empty), "{id:?}");
                    assert_eq!(args.bparam2, 0);
                }
            }
            if id != WeaponId::Elite {
                assert!(args.fparam1.abs() <= 1.0 && args.fparam2.abs() <= 1.0, "{id:?} spread vector");
            }
        }
    }

    // The silenced USP sets the last bool.
    let mut a = Arena::new();
    arm(&mut a, WeaponId::Usp, 12, 200.0);
    a.run(|game, _| weapon(game, WeaponId::Usp).weapon_state |= WPNSTATE_USP_SILENCED);
    a.probe.clear_messages();
    a.run(|game, eng| press(game, eng, IN_ATTACK));
    let events = a.events();
    assert_eq!(events[0].args.unwrap().bparam2, 1);
}

#[test]
fn reload_quirks_and_idle() {
    // `Reload` returns at once when the legacy counter of the weapon's ammo is 0; with ammo it
    // reloads for the row's time and resets the accuracy.
    for (id, reload_time, ammo) in [
        (WeaponId::Glock18, 2.2_f32, "9mm"),
        (WeaponId::Usp, 2.7, "45ACP"),
        (WeaponId::P228, 2.7, "357SIG"),
        (WeaponId::Deagle, 2.2, "50AE"),
        (WeaponId::FiveSeven, 2.7, "57mm"),
        (WeaponId::Elite, 4.5, "9mm"),
    ] {
        let mut a = Arena::new();
        arm(&mut a, id, 3, 200.0);
        a.run(move |game, eng| {
            let ft = eng.frametime();
            let index = ammo_index(ammo) as usize;
            // No reserve: nothing happens.
            game.set_player_ammo(0, index, 0);
            let w = weapon(game, id);
            w.next_primary_attack = -1.0;
            w.accuracy = 0.5;
            press(game, eng, 1 << 13);
            assert!(!weapon(game, id).in_reload, "{id:?} without reserve");
            // With reserve it reloads: `m_flNextAttack` is the delay (a countdown) and the
            // accuracy is back to the deploy value.
            game.set_player_ammo(0, index, 50);
            game.set_next_attack(0, 0.0);
            weapon(game, id).next_primary_attack = -1.0;
            press(game, eng, 1 << 13);
            let w = weapon(game, id);
            assert!(w.in_reload, "{id:?}");
            assert!(w.accuracy > 0.5, "{id:?} accuracy reset");
            let (next_attack, ..) = game.player_weapon_state(0).unwrap();
            assert!(approx(next_attack, reload_time - ft), "{id:?}: {next_attack}");
        });
    }

    // The idle: the Deagle's is the shield branch whatever the player holds (QUIRK), the others
    // play their own animation and wait their own time.
    for (id, idle, anim) in [
        (WeaponId::P228, 3.0625_f32, Some(0)),
        (WeaponId::FiveSeven, 3.0625, Some(0)),
        (WeaponId::Elite, 60.0, Some(0)),
        (WeaponId::Usp, 60.0, Some(8)),
        (WeaponId::Deagle, 20.0, None),
    ] {
        let mut a = Arena::new();
        arm(&mut a, id, 5, 200.0);
        a.run(move |game, eng| {
            let me = eng.client_edict(0);
            eng.ent_mut(me).unwrap().weaponanim = 99;
            press(game, eng, 0);
            let w = weapon(game, id);
            assert!(w.play_empty_sound, "{id:?}: ResetEmptySound");
            let ft = eng.frametime();
            assert!(approx(w.time_weapon_idle, idle - ft), "{id:?}: idle {}", w.time_weapon_idle);
            let sent = eng.ent(me).unwrap().weaponanim;
            match anim {
                Some(anim) => assert_eq!(sent, anim, "{id:?}"),
                None => assert_eq!(sent, 99, "{id:?}: no idle animation"),
            }
        });
    }
}

#[test]
fn elite_shoots_from_five_units_to_the_side() {
    // With the accuracy at 1 the spread is 0, so the bullet goes straight along +X from the eye
    // moved 5 units along `v_right` (which points to -Y for a player facing +X): the left gun
    // (an even clip starts with it) shoots from y = +5, the right gun from y = -5. A victim
    // whose box (+-16 wide) covers one of those and not the other is hit by that gun only.
    for (clip, left_gun) in [(30, true), (29, false)] {
        for victim_y in [-20.0_f32, 20.0] {
            let mut a = Arena::new();
            arm(&mut a, WeaponId::Elite, clip, 100.0);
            let lost = a.run(move |game, eng| {
                let victim = eng.client_edict(1);
                eng.set_origin(victim, Vec3::new(116.0, victim_y, 0.0));
                weapon(game, WeaponId::Elite).accuracy = 1.0;
                let before = health(game, 1);
                press(game, eng, IN_ATTACK);
                before - health(game, 1)
            });
            // The gun at y = +5 hits the victim at y = +20 (box 4..36), the gun at y = -5 the
            // victim at y = -20 (box -36..-4).
            let gun_y = if left_gun { 5.0 } else { -5.0 };
            let hit = (victim_y > 0.0) == (gun_y > 0.0);
            assert_eq!(lost > 0.0, hit, "left gun {left_gun}, victim at y {victim_y}: lost {lost}");
        }
    }
}
