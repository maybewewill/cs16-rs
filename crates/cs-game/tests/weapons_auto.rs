//! The submachine guns, the rifles and the machine gun (Task 10 of phase 2D), ported from
//! ReGameDLL_CS `wpn_shared/wpn_*.cpp` and `weapons.h` (read 2026-11-22): the rows of the
//! source, the accuracy with its integer divisors, the recoil by stance, the Famas burst, the
//! M4A1 silencer, the scopes of the AUG and the SG552, and the arguments of the fire events.
//!
//! The commands run through `post_usercmd` alone (`PlayerPostThink`: `ItemPostFrame`, then the
//! timers count down by one 10 ms frame): the stub map has no floor, so a full command would
//! run the movement and clear `FL_ONGROUND` every time, and the tests set the stance of the
//! player (the flags and the velocity) by hand. The events reach a second client (the
//! `watcher`, slot 1), because the shooter's own client is left out by `FEV_NOTHOST`; the stub
//! world precaches no weapon events yet, so the arena's game does it when the server starts.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{Rig, spawns};
use cs_cvar::CVarRegistry;
use cs_engine::consts::{FL_DUCKING, FL_ONGROUND};
use cs_engine::{EdictId, EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_game::weapons::{WeaponId, ammo_index, item_info, new_weapon};
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::event::EventArgs;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::{CsUserMsg, UserMsgRegistry};
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
use glam::Vec3;

const IN_ATTACK: i32 = 1 << 0;
const IN_ATTACK2: i32 = 1 << 11;
const IN_RELOAD: i32 = 1 << 13;
const EF_MUZZLEFLASH: u32 = 2;

/// The fire events of the twelve weapons (`events/*.sc`).
const EVENTS: [&str; 12] = [
    "events/mac10.sc",
    "events/tmp.sc",
    "events/mp5n.sc",
    "events/ump45.sc",
    "events/p90.sc",
    "events/galil.sc",
    "events/famas.sc",
    "events/ak47.sc",
    "events/m4a1.sc",
    "events/aug.sc",
    "events/sg552.sc",
    "events/m249.sc",
];

/// A [`Rig`] that precaches the weapon events when the server activates.
struct EventRig {
    rig: Rig,
}

impl GameDll for EventRig {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.rig.usermsgs()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        for name in EVENTS {
            assert_ne!(eng.precache_event(name), 0, "{name}");
        }
        self.rig.server_activate(eng);
    }
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.rig.server_deactivate(eng);
    }
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.rig.spawn_entity(eng, id, classname, keys)
    }
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.rig.client_connect(eng, slot)
    }
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.rig.client_put_in_server(eng, slot);
    }
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.rig.client_disconnect(eng, slot);
    }
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.rig.client_command(eng, slot, argv);
    }
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.rig.client_userinfo_changed(eng, slot);
    }
    fn run_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd, seed: u32) {
        self.rig.run_usercmd(eng, slot, cmd, seed);
    }
    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        self.rig.post_usercmd(eng, slot, cmd);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        self.rig.start_frame(eng, frametime);
    }
    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.rig.think(eng, id);
    }
    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        self.rig.touch(eng, toucher, other);
    }
    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        self.rig.blocked(eng, pusher, other);
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.rig.update_client_data(eng, slot)
    }
    fn setup_visibility(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Option<Vec3> {
        self.rig.setup_visibility(eng, slot)
    }
}

/// A stub-map server with two clients that send no commands: the shooter (slot 0) and the
/// watcher (slot 1, the victim of the damage tests).
struct Arena {
    server: Server<EventRig>,
    hub: LoopbackHub,
    shooter: ProbeClient<LoopbackEndpoint>,
    watcher: ProbeClient<LoopbackEndpoint>,
    now: f64,
}

impl Arena {
    fn new() -> Self {
        let map = ServerMap::stub_with_entities("test", spawns(4, 4));
        let cfg = ServerConfig { max_clients: 2, max_edicts: 256, ..ServerConfig::default() };
        let rig = EventRig { rig: Rig { game: CsGame::new(), actions: Vec::new() } };
        let server = Server::new(
            cfg,
            map,
            rig,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        let hub = LoopbackHub::new();
        let probe = |name: &str| {
            let endpoint = hub.connect();
            let cfg = ProbeConfig {
                name: name.to_string(),
                steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
                cdkey_md5_hex: md5_hex(name.as_bytes()),
            };
            let mut p = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
            p.suppress_moves(true);
            p.connect();
            p
        };
        let shooter = probe("shooter");
        let watcher = probe("watcher");
        let mut arena = Self { server, hub, shooter, watcher, now: 0.0 };
        for _ in 0..600 {
            arena.step(1);
            if arena.shooter.state() == &ProbeState::InGame
                && arena.watcher.state() == &ProbeState::InGame
            {
                return arena;
            }
        }
        panic!("the probes did not get into the game");
    }

    fn step(&mut self, frames: usize) {
        for _ in 0..frames {
            self.shooter.pump(self.now);
            self.watcher.pump(self.now);
            self.now = ((self.now / 0.01).round() + 1.0) * 0.01;
            self.server.frame(&mut self.hub, self.now);
        }
    }

    /// Runs `f` as the game at the start of the next frame.
    fn run<T: 'static>(
        &mut self,
        f: impl FnOnce(&mut CsGame, &mut EngineApi<'_>) -> T + 'static,
    ) -> T {
        let out = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&out);
        self.server.game_mut().rig.actions.push(Box::new(move |game, eng| {
            *sink.borrow_mut() = Some(f(game, eng));
        }));
        self.step(1);
        let result = out.borrow_mut().take();
        result.expect("the game ran the action")
    }

    /// The events the watcher has been sent since `clear`, as `(index, args)`.
    fn events(&self) -> Vec<(u16, EventArgs)> {
        self.watcher
            .messages()
            .iter()
            .flat_map(|m| match m {
                SvcMessage::Event(events) => events.clone(),
                _ => Vec::new(),
            })
            .map(|e| (e.index, e.args.unwrap_or_default()))
            .collect()
    }

    /// The texts the shooter's HUD was sent.
    fn texts(&self) -> Vec<String> {
        self.shooter
            .messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::UserMsg { decoded: CsUserMsg::TextMsg { message, .. }, .. } => {
                    Some(message.clone())
                }
                _ => None,
            })
            .collect()
    }
}

/// How the shooter stands (what the guns read: `FL_ONGROUND`, `FL_DUCKING`, `Length2D`).
#[derive(Clone, Copy, Debug)]
enum Stance {
    Air,
    AirMoving,
    Moving,
    Ducking,
    Standing,
}

fn stand(eng: &mut EngineApi<'_>, stance: Stance) {
    let id = eng.client_edict(0);
    let v = eng.ent_mut(id).unwrap();
    v.flags &= !(FL_ONGROUND | FL_DUCKING);
    v.velocity = Vec3::ZERO;
    match stance {
        Stance::Air => {}
        Stance::AirMoving => v.velocity = Vec3::new(200.0, 0.0, 0.0),
        Stance::Moving => {
            v.flags |= FL_ONGROUND;
            v.velocity = Vec3::new(200.0, 0.0, 0.0);
        }
        Stance::Ducking => v.flags |= FL_ONGROUND | FL_DUCKING,
        Stance::Standing => v.flags |= FL_ONGROUND,
    }
}

fn cmd() -> GoldSrcUserCmd {
    GoldSrcUserCmd { msec: 10, ..Default::default() }
}

/// One `PlayerPostThink` with `buttons` down: the weapon's `ItemPostFrame`, then the timers
/// count down by the frame.
fn press(game: &mut CsGame, eng: &mut EngineApi<'_>, buttons: i32) {
    let id = eng.client_edict(0);
    eng.ent_mut(id).unwrap().button = buttons;
    game.post_usercmd(eng, 0, &cmd());
}

fn weapon(game: &mut CsGame, id: WeaponId) -> &mut cs_game::weapons::Weapon {
    game.weapon_mut(0, id).expect("the player carries the weapon")
}

/// Both players spawned out of the freeze period, the shooter carrying `id` (`clip` loaded,
/// `reserve` spare), selected, the deploy delay spent, standing still on the ground, looking
/// along +X with no punch.
fn armed(id: WeaponId, clip: i32, reserve: i32) -> Arena {
    let mut a = Arena::new();
    a.run(move |game, eng| {
        eng.set_cvar("sv_gravity", "0");
        assert!(game.spawn_player(eng, 0, Team::Ct, 0));
        assert!(game.spawn_player(eng, 1, Team::Terrorist, 0));
        game.end_freeze_period(eng);
        assert!(game.give_weapon(eng, 0, new_weapon(id).expect("a gun"), clip, reserve));
        assert!(game.select_weapon(eng, 0, id));
        game.set_next_attack(0, 0.0);
        let w = weapon(game, id);
        w.time_weapon_idle = 0.0;
        w.next_primary_attack = 0.0;
        let v = eng.ent_mut(eng.client_edict(0)).unwrap();
        v.v_angle = Vec3::ZERO;
        v.punchangle = Vec3::ZERO;
        stand(eng, Stance::Standing);
    });
    a
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn ft(eng: &EngineApi<'_>) -> f32 {
    eng.frametime()
}

/// The twelve guns.
const GUNS: [WeaponId; 12] = [
    WeaponId::Mac10,
    WeaponId::Tmp,
    WeaponId::Mp5n,
    WeaponId::Ump45,
    WeaponId::P90,
    WeaponId::Galil,
    WeaponId::Famas,
    WeaponId::Ak47,
    WeaponId::M4a1,
    WeaponId::Aug,
    WeaponId::Sg552,
    WeaponId::M249,
];

/// One row of the source: `weapons.h` constants and the numbers of the `*Fire` functions.
struct Row {
    id: WeaponId,
    max_speed: f32,
    draw: i32,
    reload_anim: i32,
    reload_time: f32,
    /// The cycle time of the first shot of a standing player.
    cycle: f32,
    /// What `m_flTimeWeaponIdle` is set to by a shot.
    idle: f32,
    damage: f32,
    range: f32,
    clip: i32,
    ammo: &'static str,
    event: &'static str,
    /// `int(punchangle * k)` of the event.
    punch_scale: f32,
}

#[rustfmt::skip]
const ROWS: [Row; 12] = [
    Row { id: WeaponId::Mac10, max_speed: 250.0, draw: 2, reload_anim: 1, reload_time: 3.15, cycle: 0.07, idle: 2.0, damage: 29.0, range: 0.82, clip: 30, ammo: "45acp", event: "events/mac10.sc", punch_scale: 100.0 },
    Row { id: WeaponId::Tmp, max_speed: 250.0, draw: 2, reload_anim: 1, reload_time: 2.12, cycle: 0.07, idle: 2.0, damage: 20.0, range: 0.85, clip: 30, ammo: "9mm", event: "events/tmp.sc", punch_scale: 100.0 },
    Row { id: WeaponId::Mp5n, max_speed: 250.0, draw: 2, reload_anim: 1, reload_time: 2.63, cycle: 0.075, idle: 2.0, damage: 26.0, range: 0.84, clip: 30, ammo: "9mm", event: "events/mp5n.sc", punch_scale: 100.0 },
    Row { id: WeaponId::Ump45, max_speed: 250.0, draw: 2, reload_anim: 1, reload_time: 3.5, cycle: 0.1, idle: 2.0, damage: 30.0, range: 0.82, clip: 25, ammo: "45acp", event: "events/ump45.sc", punch_scale: 100.0 },
    Row { id: WeaponId::P90, max_speed: 245.0, draw: 2, reload_anim: 1, reload_time: 3.4, cycle: 0.066, idle: 2.0, damage: 21.0, range: 0.885, clip: 50, ammo: "57mm", event: "events/p90.sc", punch_scale: 100.0 },
    Row { id: WeaponId::Galil, max_speed: 240.0, draw: 2, reload_anim: 1, reload_time: 2.45, cycle: 0.0875, idle: 1.28, damage: 30.0, range: 0.98, clip: 35, ammo: "556nato", event: "events/galil.sc", punch_scale: 10_000_000.0 },
    Row { id: WeaponId::Famas, max_speed: 240.0, draw: 2, reload_anim: 1, reload_time: 3.3, cycle: 0.0825, idle: 1.1, damage: 30.0, range: 0.96, clip: 25, ammo: "556nato", event: "events/famas.sc", punch_scale: 10_000_000.0 },
    Row { id: WeaponId::Ak47, max_speed: 221.0, draw: 2, reload_anim: 1, reload_time: 2.45, cycle: 0.0955, idle: 1.9, damage: 36.0, range: 0.98, clip: 30, ammo: "762nato", event: "events/ak47.sc", punch_scale: 100.0 },
    // The M4A1 starts unsilenced: the unsilenced draw and reload animations.
    Row { id: WeaponId::M4a1, max_speed: 230.0, draw: 12, reload_anim: 11, reload_time: 3.05, cycle: 0.0875, idle: 1.5, damage: 32.0, range: 0.97, clip: 30, ammo: "556nato", event: "events/m4a1.sc", punch_scale: 100.0 },
    Row { id: WeaponId::Aug, max_speed: 240.0, draw: 2, reload_anim: 1, reload_time: 3.3, cycle: 0.0825, idle: 1.9, damage: 32.0, range: 0.96, clip: 30, ammo: "556nato", event: "events/aug.sc", punch_scale: 100.0 },
    Row { id: WeaponId::Sg552, max_speed: 235.0, draw: 2, reload_anim: 1, reload_time: 3.0, cycle: 0.0825, idle: 2.0, damage: 33.0, range: 0.955, clip: 30, ammo: "556nato", event: "events/sg552.sc", punch_scale: 100.0 },
    Row { id: WeaponId::M249, max_speed: 220.0, draw: 4, reload_anim: 3, reload_time: 4.7, cycle: 0.1, idle: 1.6, damage: 32.0, range: 0.97, clip: 100, ammo: "556natobox", event: "events/m249.sc", punch_scale: 100.0 },
];

#[test]
fn auto_rows_match_source() {
    assert_eq!(ROWS.map(|r| r.id), GUNS);
    for r in &ROWS {
        let id = r.id;
        // The weapon table: the clip and the ammo it takes.
        let info = item_info(id).unwrap();
        assert_eq!(info.max_clip, r.clip, "{id:?}");
        assert!(info.ammo1.unwrap().eq_ignore_ascii_case(r.ammo), "{id:?}");
        // The speed, as the trait answers it.
        let behavior = new_weapon(id).unwrap();
        assert_eq!(behavior.id(), id);
        assert_eq!(behavior.max_speed(&cs_game::weapons::Weapon::new(id)), r.max_speed, "{id:?}");
        assert!(behavior.use_decrement(), "{id:?}");

        let mut a = armed(id, r.clip, 90);
        a.run(move |game, eng| {
            let f = ft(eng);
            // The draw: the weapon's own animation (a re-deploy: the arena selected it once).
            let (view, other) = (eng.ent(eng.client_edict(0)).unwrap().weaponanim, id);
            assert_eq!(view, r.draw, "{other:?} draw animation");

            // A standing shot with no accuracy loss: the cycle time, the idle time and the
            // damage-free bookkeeping of the source.
            weapon(game, id).accuracy = 0.0;
            press(game, eng, IN_ATTACK);
            let w = weapon(game, id);
            assert_eq!(w.clip, r.clip - 1, "{id:?}");
            assert!(approx(w.next_primary_attack, r.cycle - f), "{id:?}: {}", w.next_primary_attack);
            assert_eq!(w.next_secondary_attack, w.next_primary_attack, "{id:?}");
            assert!(approx(w.time_weapon_idle, r.idle - f), "{id:?}: {}", w.time_weapon_idle);

            // The reload: the delay, the animation, the idle after it (`delay + 0.5`).
            let w = weapon(game, id);
            w.next_primary_attack = -0.5;
            press(game, eng, IN_RELOAD);
            let w = weapon(game, id);
            assert!(w.in_reload, "{id:?}");
            assert!(approx(w.time_weapon_idle, r.reload_time + 0.5 - f), "{id:?}");
            let (next_attack, ..) = game.player_weapon_state(0).unwrap();
            assert!(approx(next_attack, r.reload_time - f), "{id:?}: {next_attack}");
            assert_eq!(eng.ent(eng.client_edict(0)).unwrap().weaponanim, r.reload_anim, "{id:?}");
            // The reload completes: the clip is full and the reserve paid for it.
            game.set_next_attack(0, 0.0);
            press(game, eng, 0);
            assert_eq!(weapon(game, id).clip, r.clip, "{id:?}");
            let reserve = game.player_ammo(0, ammo_index(r.ammo) as usize).unwrap();
            assert_eq!(reserve, 90 - 1, "{id:?}: one round went back into the clip");
        });
    }
}

#[test]
fn damage_and_range_match_source() {
    // The bullet of a standing player with no spread: `int(damage * range ^ (distance / 500))`
    // at a distance of 200 (the near face of the victim, 16 units in front of its centre).
    for r in &ROWS {
        let id = r.id;
        let mut a = armed(id, r.clip, 90);
        let (before, after) = a.run(move |game, eng| {
            let eng_id = eng.client_edict(0);
            eng.set_origin(eng_id, Vec3::ZERO);
            let victim = eng.client_edict(1);
            eng.set_origin(victim, Vec3::new(216.0, 0.0, 0.0));
            // The Famas fires its burst when the mode says so; take the full auto shot.
            weapon(game, id).accuracy = 0.0;
            let before = game.state().players[1].as_ref().unwrap().health;
            press(game, eng, IN_ATTACK);
            let after = game.state().players[1].as_ref().unwrap().health;
            (before, after)
        });
        let expected = (f64::from(r.damage) * f64::from(r.range).powf(200.0 / 500.0)).trunc() as f32;
        assert_eq!(before - after, expected, "{id:?}");
    }
}

/// `m_flAccuracy` after `n` shots, as the source computes it: (kind, divisor, base, cap) with
/// the C types. `cubed`: `n * n * n / DIVISOR`, else `n * n / DIVISOR`; an `int` division unless
/// `double` (the MP5N).
struct Accuracy {
    id: WeaponId,
    cubed: bool,
    divisor: f64,
    double: bool,
    base: f32,
    cap: f32,
}

#[rustfmt::skip]
const ACCURACIES: [Accuracy; 12] = [
    Accuracy { id: WeaponId::Mac10, cubed: true, divisor: 200.0, double: false, base: 0.6, cap: 1.65 },
    Accuracy { id: WeaponId::Tmp, cubed: true, divisor: 200.0, double: false, base: 0.55, cap: 1.4 },
    Accuracy { id: WeaponId::Mp5n, cubed: false, divisor: 220.1, double: true, base: 0.45, cap: 0.75 },
    Accuracy { id: WeaponId::Ump45, cubed: false, divisor: 210.0, double: false, base: 0.5, cap: 1.0 },
    Accuracy { id: WeaponId::P90, cubed: false, divisor: 175.0, double: false, base: 0.45, cap: 1.0 },
    Accuracy { id: WeaponId::Galil, cubed: true, divisor: 200.0, double: false, base: 0.35, cap: 1.25 },
    Accuracy { id: WeaponId::Famas, cubed: true, divisor: 215.0, double: false, base: 0.3, cap: 1.0 },
    Accuracy { id: WeaponId::Ak47, cubed: true, divisor: 200.0, double: false, base: 0.35, cap: 1.25 },
    Accuracy { id: WeaponId::M4a1, cubed: true, divisor: 220.0, double: false, base: 0.3, cap: 1.0 },
    Accuracy { id: WeaponId::Aug, cubed: true, divisor: 215.0, double: false, base: 0.3, cap: 1.0 },
    Accuracy { id: WeaponId::Sg552, cubed: true, divisor: 220.0, double: false, base: 0.3, cap: 1.0 },
    Accuracy { id: WeaponId::M249, cubed: true, divisor: 175.0, double: false, base: 0.4, cap: 0.9 },
];

impl Accuracy {
    fn after(&self, shots: i32) -> f32 {
        let power = if self.cubed { shots * shots * shots } else { shots * shots };
        let value = if self.double {
            (f64::from(power) / self.divisor + f64::from(self.base)) as f32
        } else {
            (power / self.divisor as i32) as f32 + self.base
        };
        if value > self.cap { self.cap } else { value }
    }
}

#[test]
fn accuracy_uses_integer_divisors() {
    for acc in &ACCURACIES {
        let id = acc.id;
        let mut a = armed(id, 100, 90);
        a.run(move |game, eng| {
            weapon(game, id).clip = 100;
            for shots in 1..=12 {
                weapon(game, id).next_primary_attack = 0.0;
                press(game, eng, IN_ATTACK);
                let got = weapon(game, id).accuracy;
                assert_eq!(got, acc.after(shots), "{id:?} after {shots} shots");
                assert_eq!(weapon(game, id).shots_fired, shots, "{id:?}");
            }
        });
    }
    // The truncation by hand: an AK-47 with three shots has 27 / 200 = 0 (an int), so 0.35; the
    // MAC-10 after 6 has 216 / 200 = 1, so 1.6; the MP5N's double divisor has no truncation.
    let mut a = armed(WeaponId::Ak47, 30, 90);
    a.run(|game, eng| {
        for _ in 0..3 {
            weapon(game, WeaponId::Ak47).next_primary_attack = 0.0;
            press(game, eng, IN_ATTACK);
        }
        assert_eq!(weapon(game, WeaponId::Ak47).accuracy, 0.35);
    });
    let mut a = armed(WeaponId::Mac10, 30, 90);
    a.run(|game, eng| {
        for _ in 0..6 {
            weapon(game, WeaponId::Mac10).next_primary_attack = 0.0;
            press(game, eng, IN_ATTACK);
        }
        assert_eq!(weapon(game, WeaponId::Mac10).accuracy, 1.6);
    });
    let mut a = armed(WeaponId::Mp5n, 30, 90);
    a.run(|game, eng| {
        weapon(game, WeaponId::Mp5n).next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK);
        let one = weapon(game, WeaponId::Mp5n).accuracy;
        // 1 / 220.1 + 0.45 (the divisor is a double), not 0 + 0.45.
        assert!(one > 0.45, "{one}");
        assert_eq!(one, (1.0_f64 / 220.1 + f64::from(0.45_f32)) as f32);
    });
}

#[test]
fn a_standing_shot_without_accuracy_loss_is_exact() {
    // `0.03 * m_flAccuracy` of the MAC-10 on the ground: with accuracy 0 the bullet is exact,
    // so a shot at a victim 200 units away always hits, and the event carries a zero spread.
    let mut a = armed(WeaponId::Mac10, 30, 90);
    a.watcher.clear_messages();
    a.run(|game, eng| {
        weapon(game, WeaponId::Mac10).accuracy = 0.0;
        press(game, eng, IN_ATTACK);
    });
    a.step(3);
    let events = a.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!((events[0].1.fparam1, events[0].1.fparam2), (0.0, 0.0));
}

/// The `KickBack` arguments of a gun's first shot in each stance: (up base, lateral base).
/// `KickBack(1.3, 0.55, ...)` etc.; in the first shot the kick is the bases, and the direction
/// of the lateral kick is the weapon's `m_iDirection`.
struct Kicks {
    id: WeaponId,
    air: (f32, f32),
    moving: (f32, f32),
    ducking: (f32, f32),
    standing: (f32, f32),
    /// Which of "in the air" and "moving" the source tests first (what an airborne moving
    /// player gets).
    air_first: bool,
}

#[rustfmt::skip]
const KICKS: [Kicks; 12] = [
    Kicks { id: WeaponId::Mac10, air: (1.3, 0.55), moving: (0.9, 0.45), ducking: (0.75, 0.4), standing: (0.775, 0.425), air_first: true },
    Kicks { id: WeaponId::Tmp, air: (1.1, 0.5), moving: (0.8, 0.4), ducking: (0.7, 0.35), standing: (0.725, 0.375), air_first: true },
    Kicks { id: WeaponId::Mp5n, air: (0.9, 0.475), moving: (0.5, 0.275), ducking: (0.225, 0.15), standing: (0.25, 0.175), air_first: true },
    Kicks { id: WeaponId::Ump45, air: (0.125, 0.65), moving: (0.55, 0.3), ducking: (0.25, 0.175), standing: (0.275, 0.2), air_first: true },
    Kicks { id: WeaponId::P90, air: (0.9, 0.45), moving: (0.45, 0.3), ducking: (0.275, 0.2), standing: (0.3, 0.225), air_first: true },
    Kicks { id: WeaponId::Galil, air: (1.2, 0.5), moving: (1.0, 0.45), ducking: (0.6, 0.3), standing: (0.65, 0.35), air_first: false },
    Kicks { id: WeaponId::Famas, air: (1.25, 0.45), moving: (1.0, 0.45), ducking: (0.575, 0.325), standing: (0.625, 0.375), air_first: false },
    Kicks { id: WeaponId::Ak47, air: (2.0, 1.0), moving: (1.5, 0.45), ducking: (0.9, 0.35), standing: (1.0, 0.375), air_first: false },
    Kicks { id: WeaponId::M4a1, air: (1.2, 0.5), moving: (1.0, 0.45), ducking: (0.6, 0.3), standing: (0.65, 0.35), air_first: false },
    Kicks { id: WeaponId::Aug, air: (1.25, 0.45), moving: (1.0, 0.45), ducking: (0.575, 0.325), standing: (0.625, 0.375), air_first: false },
    Kicks { id: WeaponId::Sg552, air: (1.25, 0.45), moving: (1.0, 0.45), ducking: (0.6, 0.35), standing: (0.625, 0.375), air_first: false },
    Kicks { id: WeaponId::M249, air: (1.8, 0.65), moving: (1.1, 0.5), ducking: (0.75, 0.325), standing: (0.8, 0.35), air_first: true },
];

#[test]
fn kickback_by_state() {
    for k in &KICKS {
        let id = k.id;
        let mut a = armed(id, 100, 90);
        a.run(move |game, eng| {
            let edict = eng.client_edict(0);
            let shot = |game: &mut CsGame, eng: &mut EngineApi<'_>, stance: Stance| {
                stand(eng, stance);
                eng.ent_mut(edict).unwrap().punchangle = Vec3::ZERO;
                let w = weapon(game, id);
                w.next_primary_attack = 0.0;
                // The first shot of a burst of fire, the lateral kick to the right.
                w.shots_fired = 0;
                w.direction = 1;
                press(game, eng, IN_ATTACK);
                let punch = eng.ent(edict).unwrap().punchangle;
                (-punch.x, punch.y)
            };
            // `KickBack` with a first shot: the bases (the bounds are far above them).
            assert_eq!(shot(game, eng, Stance::Air), k.air, "{id:?} in the air");
            assert_eq!(shot(game, eng, Stance::Moving), k.moving, "{id:?} moving");
            assert_eq!(shot(game, eng, Stance::Ducking), k.ducking, "{id:?} ducking");
            assert_eq!(shot(game, eng, Stance::Standing), k.standing, "{id:?} standing");
            // Airborne and moving at once: the test the source makes first wins.
            let expected = if k.air_first { k.air } else { k.moving };
            assert_eq!(shot(game, eng, Stance::AirMoving), expected, "{id:?} in the air, moving");
        });
    }
}

#[test]
fn empty_clip_clicks_and_still_counts_the_shot() {
    for id in GUNS {
        let mut a = armed(id, 30, 90);
        a.run(move |game, eng| {
            let w = weapon(game, id);
            w.clip = 0;
            w.fire_on_empty = true;
            w.next_primary_attack = 0.0;
            press(game, eng, IN_ATTACK);
            let w = weapon(game, id);
            assert_eq!(w.clip, 0);
            assert_eq!(w.shots_fired, 1, "{id:?}: the shot count goes up for a click");
            assert!(w.delay_fire, "{id:?}");
            assert!(approx(w.next_primary_attack, 0.2 - ft(eng)), "{id:?}: {}", w.next_primary_attack);
            assert_eq!(eng.ent(eng.client_edict(0)).unwrap().effects & EF_MUZZLEFLASH, 0, "{id:?}");
        });
    }
}

#[test]
fn famas_burst() {
    let mut a = armed(WeaponId::Famas, 25, 90);
    a.shooter.clear_messages();
    let famas = WeaponId::Famas;
    a.run(move |game, eng| {
        // The secondary attack switches to burst fire and says so, 0.3 s before the next switch.
        press(game, eng, IN_ATTACK2);
        let w = weapon(game, famas);
        assert_eq!(w.weapon_state & (1 << 4), 1 << 4);
        assert!(approx(w.next_secondary_attack, 0.3 - ft(eng)));
        // Full auto cycle time, the spread widened by 0.01: a burst does not widen it.
        w.next_primary_attack = 0.0;
        w.accuracy = 0.0;
        press(game, eng, IN_ATTACK);
        let w = weapon(game, famas);
        // The burst: the cycle time is 0.55, the shot count of the burst 1, the next shot of it
        // is armed 0.05 s after now (absolute server time), with the spread of the first.
        assert_eq!(w.clip, 24);
        assert!(approx(w.next_primary_attack, 0.55 - ft(eng)), "{}", w.next_primary_attack);
        assert_eq!(w.famas_shots_fired, 1);
        assert_eq!(w.burst_spread, 0.0, "a burst's spread is the plain one");
        assert!(approx(w.famas_shoot, eng.time() as f32 + 0.05));
    });
    a.step(1);
    // The other two shots of the burst come out of `FireRemaining`, 0.05 s apart, and end it.
    let mut frames = 0;
    loop {
        let done = a.run(move |game, eng| {
            press(game, eng, 0);
            let w = weapon(game, famas);
            w.famas_shots_fired == 3 && w.famas_shoot == 0.0
        });
        frames += 1;
        assert!(frames < 40, "the burst never ended");
        if done {
            break;
        }
    }
    a.run(move |game, _| {
        let w = weapon(game, famas);
        assert_eq!(w.clip, 22, "three rounds in all");
        assert_eq!(w.famas_shots_fired, 3);
    });
    // A burst's first shot does 34, full auto 30.
    let burst_damage = (34.0_f64 * f64::from(0.96_f32).powf(200.0 / 500.0)).trunc() as f32;
    let auto_damage = (30.0_f64 * f64::from(0.96_f32).powf(200.0 / 500.0)).trunc() as f32;
    for (burst, expected) in [(true, burst_damage), (false, auto_damage)] {
        let mut a = armed(famas, 25, 90);
        let lost = a.run(move |game, eng| {
            let edict = eng.client_edict(0);
            eng.set_origin(edict, Vec3::ZERO);
            eng.set_origin(eng.client_edict(1), Vec3::new(216.0, 0.0, 0.0));
            let w = weapon(game, famas);
            w.accuracy = 0.0;
            if burst {
                w.weapon_state |= 1 << 4;
            }
            let before = game.state().players[1].as_ref().unwrap().health;
            press(game, eng, IN_ATTACK);
            before - game.state().players[1].as_ref().unwrap().health
        });
        assert_eq!(lost, expected, "burst: {burst}");
    }
    // The burst mode went on with the message, and goes off again with the other.
    let mut a = armed(famas, 25, 90);
    a.shooter.clear_messages();
    a.run(move |game, eng| {
        press(game, eng, IN_ATTACK2);
        weapon(game, famas).next_secondary_attack = 0.0;
        press(game, eng, 0);
        press(game, eng, IN_ATTACK2);
        assert_eq!(weapon(game, famas).weapon_state & (1 << 4), 0);
    });
    a.step(3);
    assert_eq!(a.texts(), ["#Switch_To_BurstFire", "#Switch_To_FullAuto"]);
    // QUIRK: a reload with the FOV not at 90 flips the burst mode again (the Famas has no
    // scope, but the source asks).
    a.run(move |game, eng| {
        game.set_fov(0, 55);
        let w = weapon(game, famas);
        w.clip = 10;
        w.next_primary_attack = -0.5;
        press(game, eng, IN_RELOAD);
        assert!(weapon(game, famas).in_reload);
        assert_eq!(weapon(game, famas).weapon_state & (1 << 4), 1 << 4);
    });
}

#[test]
fn m4a1_silencer() {
    let m4 = WeaponId::M4a1;
    let mut a = armed(m4, 30, 90);
    a.run(move |game, eng| {
        let f = ft(eng);
        let edict = eng.client_edict(0);
        // Unsilenced first: the muzzle flash is on, the counter of the 5.56 ammo goes down.
        weapon(game, m4).accuracy = 0.0;
        let counter = game.tabulated_ammo(0).unwrap()[2];
        press(game, eng, IN_ATTACK);
        assert_ne!(eng.ent(edict).unwrap().effects & EF_MUZZLEFLASH, 0);
        assert_eq!(game.tabulated_ammo(0).unwrap()[2], counter - 1, "`ammo_556nato--`");

        // The secondary attack puts the silencer on: the attach animation, 2 s of nothing.
        eng.ent_mut(edict).unwrap().effects = 0;
        let w = weapon(game, m4);
        w.next_primary_attack = 0.0;
        w.next_secondary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        let w = weapon(game, m4);
        assert_eq!(w.weapon_state & 4, 4);
        assert_eq!(eng.ent(edict).unwrap().weaponanim, 6, "M4A1_ATTACH_SILENCER");
        assert!(approx(w.next_secondary_attack, 2.0 - f));
        assert!(approx(w.time_weapon_idle, 2.0 - f));
        assert!(approx(w.next_primary_attack, 2.0 - f));

        // Silenced: 33 damage at 0.95, no flash, and the draw, idle and reload animations of
        // the silenced model.
        let w = weapon(game, m4);
        w.next_primary_attack = 0.0;
        w.accuracy = 0.0;
        press(game, eng, IN_ATTACK);
        assert_eq!(eng.ent(edict).unwrap().effects & EF_MUZZLEFLASH, 0, "silenced: no flash");
        let w = weapon(game, m4);
        w.next_primary_attack = -0.5;
        w.clip = 10;
        press(game, eng, IN_RELOAD);
        assert_eq!(eng.ent(edict).unwrap().weaponanim, 4, "M4A1_RELOAD");
        game.set_next_attack(0, 0.0);
        press(game, eng, 0);
        weapon(game, m4).time_weapon_idle = 0.0;
        weapon(game, m4).next_primary_attack = 1.0;
        press(game, eng, 0);
        assert_eq!(eng.ent(edict).unwrap().weaponanim, 0, "M4A1_IDLE");

        // Taking it off again: the detach animation.
        let w = weapon(game, m4);
        w.next_secondary_attack = 0.0;
        w.next_primary_attack = 0.0;
        press(game, eng, IN_ATTACK2);
        assert_eq!(weapon(game, m4).weapon_state & 4, 0);
        assert_eq!(eng.ent(edict).unwrap().weaponanim, 13, "M4A1_DETACH_SILENCER");
    });
    // The damage of the two states.
    for (silenced, base, range) in [(true, 33.0_f32, 0.95_f32), (false, 32.0, 0.97)] {
        let mut a = armed(m4, 30, 90);
        let lost = a.run(move |game, eng| {
            eng.set_origin(eng.client_edict(0), Vec3::ZERO);
            eng.set_origin(eng.client_edict(1), Vec3::new(216.0, 0.0, 0.0));
            let w = weapon(game, m4);
            w.accuracy = 0.0;
            if silenced {
                w.weapon_state |= 4;
            }
            let before = game.state().players[1].as_ref().unwrap().health;
            press(game, eng, IN_ATTACK);
            before - game.state().players[1].as_ref().unwrap().health
        });
        let expected = (f64::from(base) * f64::from(range).powf(200.0 / 500.0)).trunc() as f32;
        assert_eq!(lost, expected, "silenced: {silenced}");
    }
}

#[test]
fn aug_sg552_zoom() {
    for (id, speed, zoom_speed) in [(WeaponId::Aug, 240.0, 240.0), (WeaponId::Sg552, 235.0, 200.0)] {
        let mut a = armed(id, 30, 90);
        a.run(move |game, eng| {
            let edict = eng.client_edict(0);
            let f = ft(eng);
            let behavior = new_weapon(id).unwrap();
            // The scope: 90 to 55, on the player and on the edict, then 0.3 s before the next.
            let w = weapon(game, id);
            w.next_secondary_attack = 0.0;
            press(game, eng, IN_ATTACK2);
            assert_eq!(game.player_weapon_state(0).unwrap().3, 55, "{id:?}");
            assert_eq!(eng.ent(edict).unwrap().fov, 55.0, "{id:?}");
            assert!(approx(weapon(game, id).next_secondary_attack, 0.3 - f), "{id:?}");
            // Vanilla does not reset the speed at the zoom; the SG552's `GetMaxSpeed` slows
            // down while the FOV is not 90.
            assert_eq!(eng.ent(edict).unwrap().max_speed, speed, "{id:?}: not reset yet");
            assert_eq!(behavior.max_speed(weapon(game, id)), zoom_speed, "{id:?} scoped");
            game.reset_max_speed(eng, 0);
            assert_eq!(eng.ent(edict).unwrap().max_speed, zoom_speed, "{id:?}");

            // Scoped, the cycle time is 0.135 s.
            let w = weapon(game, id);
            w.next_primary_attack = 0.0;
            w.accuracy = 0.0;
            press(game, eng, IN_ATTACK);
            assert!(approx(weapon(game, id).next_primary_attack, 0.135 - f), "{id:?}");

            // A reload leaves the scope.
            let w = weapon(game, id);
            w.next_primary_attack = -0.5;
            press(game, eng, IN_RELOAD);
            assert!(weapon(game, id).in_reload, "{id:?}");
            assert_eq!(game.player_weapon_state(0).unwrap().3, 90, "{id:?} left the scope");
            assert_eq!(eng.ent(edict).unwrap().fov, 90.0, "{id:?}");
            assert_eq!(behavior.max_speed(weapon(game, id)), speed, "{id:?} unscoped");

            // The toggle goes back and forth.
            game.set_next_attack(0, 0.0);
            let w = weapon(game, id);
            w.in_reload = false;
            w.next_secondary_attack = 0.0;
            press(game, eng, IN_ATTACK2);
            assert_eq!(game.player_weapon_state(0).unwrap().3, 55);
            weapon(game, id).next_secondary_attack = 0.0;
            press(game, eng, IN_ATTACK2);
            assert_eq!(game.player_weapon_state(0).unwrap().3, 90);
            // Unscoped, 0.0825 s.
            let w = weapon(game, id);
            w.next_primary_attack = 0.0;
            w.accuracy = 0.0;
            press(game, eng, IN_ATTACK);
            assert!(approx(weapon(game, id).next_primary_attack, 0.0825 - f), "{id:?}");
        });
    }
}

#[test]
fn auto_events_carry_source_args() {
    for r in &ROWS {
        let id = r.id;
        let mut a = armed(id, r.clip, 90);
        let (event, punch) = a.run(move |game, eng| {
            let edict = eng.client_edict(0);
            let event = eng.precache_event(r.event);
            assert_ne!(event, 0, "{}", r.event);
            let punch = Vec3::new(0.5, -0.25, 0.0);
            eng.ent_mut(edict).unwrap().punchangle = punch;
            weapon(game, id).accuracy = 0.0;
            press(game, eng, IN_ATTACK);
            (event, punch)
        });
        a.step(3);
        let events = a.events();
        assert_eq!(events.len(), 1, "{id:?}: {events:?}");
        let (index, args) = events[0];
        assert_eq!(index, event, "{id:?}");
        // `int(punchangle * 100)` (the Galil and the Famas: `* 10000000`), taken before the
        // kick. `iparam1` and `iparam2` are 18 bits signed on the wire, so the Galil's and the
        // Famas's huge ints reach the client held to +-131071 (vanilla quirk, kept).
        let wire = |v: f32| ((v * r.punch_scale) as i32).clamp(-131_071, 131_071);
        assert_eq!(args.iparam1, wire(punch.x), "{id:?}");
        assert_eq!(args.iparam2, wire(punch.y), "{id:?}");
        // The spread of the standing shot of accuracy 0 is none.
        assert_eq!((args.fparam1, args.fparam2), (0.0, 0.0), "{id:?}");
        // The TMP, the P90 and the SG552 send the int 5 as the first bool (one bit on the
        // wire); the M4A1 says whether it is silenced (not, here); the rest send 0.
        let expect_b1 = i32::from(matches!(id, WeaponId::Tmp | WeaponId::P90 | WeaponId::Sg552));
        assert_eq!(args.bparam1, expect_b1, "{id:?}");
        assert_eq!(args.bparam2, 0, "{id:?}");
        a.watcher.clear_messages();
    }
    // The silenced M4A1 says so.
    let mut a = armed(WeaponId::M4a1, 30, 90);
    a.run(|game, eng| {
        weapon(game, WeaponId::M4a1).weapon_state |= 4;
        press(game, eng, IN_ATTACK);
    });
    a.step(3);
    let events = a.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1.bparam1, 1);
}

// ------------------------------------------------------------------ the review's additions

/// The seed whose four shared draws give the largest spread factors (the least quantised wire
/// values), and the factors `(x, y)` of the source: `x = SharedRandomFloat(seed, -0.5, 0.5) +
/// SharedRandomFloat(seed + 1, -0.5, 0.5)`, `y` the same from `seed + 2` and `seed + 3`.
fn spread_seed() -> (u32, f32, f32) {
    use cs_game::weapons::shared_random_float as sr;
    let factors = |seed: u32| {
        (
            sr(seed, -0.5, 0.5) + sr(seed + 1, -0.5, 0.5),
            sr(seed + 2, -0.5, 0.5) + sr(seed + 3, -0.5, 0.5),
        )
    };
    let seed = (0..256_u32)
        .max_by(|&a, &b| {
            let m = |s| {
                let (x, y) = factors(s);
                x.abs().min(y.abs())
            };
            m(a).total_cmp(&m(b))
        })
        .unwrap();
    let (x, y) = factors(seed);
    (seed, x, y)
}

/// The `flSpread` the source computes for a gun (`*::PrimaryAttack`), as a C `double`
/// expression of the accuracy cut to the `float` parameter.
fn source_spread(id: WeaponId, stance: Stance, acc: f32, silenced: bool, burst: bool) -> f32 {
    let a = f64::from(acc);
    let air = matches!(stance, Stance::Air);
    let moving = matches!(stance, Stance::Moving);
    let s: f64 = match id {
        WeaponId::Mac10 => if air { 0.375 * a } else { 0.03 * a },
        WeaponId::Tmp => if air { 0.25 * a } else { 0.03 * a },
        WeaponId::Mp5n => if air { 0.2 * a } else { 0.04 * a },
        WeaponId::Ump45 => if air { 0.24 * a } else { 0.04 * a },
        WeaponId::P90 => if air { 0.3 * a } else if moving { 0.115 * a } else { 0.045 * a },
        WeaponId::Galil => if air { 0.04 + 0.3 * a } else if moving { 0.04 + 0.07 * a } else { 0.0375 * a },
        WeaponId::Famas => if air { 0.030 + 0.3 * a } else if moving { 0.030 + 0.07 * a } else { 0.02 * a },
        WeaponId::Ak47 => if air { 0.04 + 0.4 * a } else if moving { 0.04 + 0.07 * a } else { 0.0275 * a },
        WeaponId::M4a1 => {
            if air { 0.035 + 0.4 * a } else if moving { 0.035 + 0.07 * a } else if silenced { 0.025 * a } else { 0.02 * a }
        }
        WeaponId::Aug => if air { 0.035 + 0.4 * a } else if moving { 0.035 + 0.07 * a } else { 0.02 * a },
        WeaponId::Sg552 => if air { 0.035 + 0.45 * a } else if moving { 0.035 + 0.075 * a } else { 0.02 * a },
        WeaponId::M249 => if air { 0.045 + 0.5 * a } else if moving { 0.045 + 0.095 * a } else { 0.03 * a },
        _ => unreachable!(),
    };
    let mut spread = s as f32;
    // `FamasFire`: full auto adds 0.01f, a burst does not.
    if id == WeaponId::Famas && !burst {
        spread += 0.01;
    }
    spread
}

/// What `fparam1`/`fparam2` become on the wire: `(v * 100)` cut to an int, over 100.
fn wire_float(v: f32) -> f32 {
    ((f64::from(v) * 100.0) as i32) as f32 / 100.0
}

#[test]
fn spread_coefficients_by_stance() {
    let (seed, fx, fy) = spread_seed();
    assert!(fx.abs() > 0.5 && fy.abs() > 0.5, "{fx} {fy}");
    let acc = 0.8_f32;
    // (gun, silenced, burst) variants.
    let mut variants: Vec<(WeaponId, bool, bool)> = GUNS.iter().map(|&g| (g, false, false)).collect();
    variants.push((WeaponId::M4a1, true, false));
    variants.push((WeaponId::Famas, false, true));
    for (id, silenced, burst) in variants {
        let mut a = armed(id, 100, 90);
        for stance in [Stance::Air, Stance::Moving, Stance::Ducking, Stance::Standing] {
            a.watcher.clear_messages();
            a.run(move |game, eng| {
                // `CmdStart` takes the command's random seed (the movement it runs does not
                // matter: the stance is set after it).
                game.run_usercmd(eng, 0, &cmd(), seed);
                stand(eng, stance);
                let w = weapon(game, id);
                w.accuracy = acc;
                w.next_primary_attack = 0.0;
                w.weapon_state = 0;
                if silenced {
                    w.weapon_state |= 4;
                }
                if burst {
                    w.weapon_state |= 1 << 4;
                }
                press(game, eng, IN_ATTACK);
            });
            a.step(3);
            let events = a.events();
            assert_eq!(events.len(), 1, "{id:?} {stance:?}: {events:?}");
            let spread = source_spread(id, stance, acc, silenced, burst);
            let args = events[0].1;
            let label = format!("{id:?} {stance:?} silenced {silenced} burst {burst}");
            assert!((args.fparam1 - wire_float(fx * spread)).abs() < 1e-5, "{label}: x {}", args.fparam1);
            assert!((args.fparam2 - wire_float(fy * spread)).abs() < 1e-5, "{label}: y {}", args.fparam2);
        }
    }
}

/// `KickBack` of the source, done by hand: the punch after one more shot.
fn source_kick(
    punch: Vec3,
    shots: i32,
    direction: i32,
    (up_base, lat_base, up_mod, lat_mod, up_max, lat_max): (f32, f32, f32, f32, f32, f32),
) -> Vec3 {
    let (up, lateral) = if shots == 1 {
        (up_base, lat_base)
    } else {
        (shots as f32 * up_mod + up_base, shots as f32 * lat_mod + lat_base)
    };
    let mut p = punch;
    p.x -= up;
    if p.x < -up_max {
        p.x = -up_max;
    }
    if direction == 1 {
        p.y += lateral;
        if p.y > lat_max {
            p.y = lat_max;
        }
    } else {
        p.y -= lateral;
        if p.y < -lat_max {
            p.y = -lat_max;
        }
    }
    p
}

#[test]
fn kickback_progression_follows_the_source_math() {
    // A standing player's `KickBack` arguments: the modifiers grow the kick with the shots, the
    // maxima stop the punch, and the direction flips by chance (read back from the weapon
    // before each shot, so the expectation follows it).
    let cases = [
        (WeaponId::Ak47, (1.0, 0.375, 0.175, 0.0375, 5.75, 1.75)),
        (WeaponId::Mac10, (0.775, 0.425, 0.2, 0.03, 3.0, 2.75)),
    ];
    for (id, args) in cases {
        let mut a = armed(id, 100, 90);
        a.run(move |game, eng| {
            let edict = eng.client_edict(0);
            let (mut hit_up_max, mut hit_lat_max) = (false, false);
            for shot in 1..=40 {
                let w = weapon(game, id);
                w.next_primary_attack = 0.0;
                w.accuracy = 0.0;
                let direction = w.direction;
                let punch = eng.ent(edict).unwrap().punchangle;
                press(game, eng, IN_ATTACK);
                let shots = weapon(game, id).shots_fired;
                assert_eq!(shots, shot, "{id:?}");
                let expected = source_kick(punch, shots, direction, args);
                let got = eng.ent(edict).unwrap().punchangle;
                assert!(
                    (got - expected).abs().max_element() < 1e-4,
                    "{id:?} shot {shot}: {got} != {expected}"
                );
                hit_up_max |= got.x == -args.4;
                hit_lat_max |= got.y.abs() == args.5;
            }
            assert!(hit_up_max && hit_lat_max, "{id:?}: the maxima were never reached");
        });
    }
}

#[test]
fn tmp_has_no_muzzle_flash_on_a_real_shot() {
    let mut a = armed(WeaponId::Tmp, 30, 90);
    a.run(|game, eng| {
        let edict = eng.client_edict(0);
        eng.ent_mut(edict).unwrap().effects = 0;
        press(game, eng, IN_ATTACK);
        assert_eq!(weapon(game, WeaponId::Tmp).clip, 29, "the shot was fired");
        assert_eq!(eng.ent(edict).unwrap().effects & EF_MUZZLEFLASH, 0);
    });
    // The MAC-10 next to it does flash.
    let mut a = armed(WeaponId::Mac10, 30, 90);
    a.run(|game, eng| {
        let edict = eng.client_edict(0);
        eng.ent_mut(edict).unwrap().effects = 0;
        press(game, eng, IN_ATTACK);
        assert_ne!(eng.ent(edict).unwrap().effects & EF_MUZZLEFLASH, 0);
    });
}

#[test]
fn accuracy_reaches_every_cap() {
    // Far enough for the UMP45 (15 shots: 225 / 210 = 1) and the P90 (14: 196 / 175 = 1) to hit
    // their caps of 1.0, and every other gun its own.
    for acc in &ACCURACIES {
        let id = acc.id;
        let mut a = armed(id, 100, 90);
        a.run(move |game, eng| {
            let mut capped_at = None;
            for shots in 1..=20 {
                weapon(game, id).next_primary_attack = 0.0;
                press(game, eng, IN_ATTACK);
                let got = weapon(game, id).accuracy;
                assert_eq!(got, acc.after(shots), "{id:?} after {shots} shots");
                if got == acc.cap && capped_at.is_none() {
                    capped_at = Some(shots);
                }
            }
            let at = capped_at.unwrap_or_else(|| panic!("{id:?} never reached its cap"));
            match id {
                WeaponId::Ump45 => assert_eq!(at, 15),
                WeaponId::P90 => assert_eq!(at, 14),
                _ => {}
            }
        });
    }
}
