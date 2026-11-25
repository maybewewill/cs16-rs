//! The client core against a real server over UDP: the original HLDS (build 10210) or our own
//! `cs-server`, headless. Ignored by default; set `CS16_HLDS_ADDR` (`127.0.0.1:27016`) and
//! optionally `CS16_HLDS_MAP` (default `de_dust2`), then:
//!
//! ```text
//! CS16_HLDS_ADDR=127.0.0.1:27016 cargo test -p cs-clientcore --test hlds -- --ignored --nocapture
//! ```
//!
//! Two clients join: the player (scripted walk, then pistol shots) and an observer that stands
//! still, because the server does not send a shooter its own weapon events (the client predicts
//! them; phase 6), so the effects of the shots show on the observer.
//!
//! Both join CT through the server's text menus (`_vgui_menus 0` and `menuselect`). The player
//! walks a scripted 30 s (forward, strafe, jump, duck, turn) with the map's collision loaded for
//! the prediction, then fires the pistol. The report: sign-on, parse errors, frames and
//! entities, the prediction error against the server's `clientdata`, the HUD, and the events,
//! temp entities and sounds the shots made.

use std::collections::BTreeMap;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cs_assets::bsp::BspMap;
use cs_clientcore::{ConnState, Connection, ConnectionConfig, EmitInputs};
use cs_pm::collision::WorldCollision;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_transport::{PeerAddr, UdpTransport};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const IN_ATTACK: u16 = 1;
const IN_JUMP: u16 = 1 << 1;
const IN_DUCK: u16 = 1 << 2;
const IN_FORWARD: u16 = 1 << 3;
const IN_MOVELEFT: u16 = 1 << 9;
const IN_MOVERIGHT: u16 = 1 << 10;

/// One tick of the client: 10 ms commands, as the original client sends at 100 fps.
const TICK: Duration = Duration::from_millis(10);
const WALK_SECONDS: f64 = 30.0;
/// Seconds the clients wait after joining before the walk.
const SETTLE_SECONDS: f64 = 8.0;
/// Seconds of pistol shots after the walk.
const FIRE_SECONDS: f64 = 3.5;

fn cmd(buttons: u16, forward: f32, side: f32, yaw: f32) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec: 10,
        buttons,
        forwardmove: forward,
        sidemove: side,
        viewangles: Vec3::new(0.0, yaw, 0.0),
        ..GoldSrcUserCmd::default()
    }
}

/// The scripted walk at `t` seconds: forward with strafing, jumps, ducks and turns.
fn script(t: f64) -> GoldSrcUserCmd {
    let phase = t % 10.0;
    let yaw = ((t * 45.0) % 360.0) as f32 + 20.0 * (t as f32 * 0.7).sin();
    let mut buttons = IN_FORWARD;
    let mut side = 0.0;
    if (2.0..4.0).contains(&phase) {
        buttons |= IN_MOVELEFT;
        side = -250.0;
    } else if (4.0..6.0).contains(&phase) {
        buttons |= IN_MOVERIGHT;
        side = 250.0;
    }
    if (phase % 1.5) < 0.12 && phase > 6.0 {
        buttons |= IN_JUMP;
    }
    if (8.0..9.0).contains(&phase) {
        buttons |= IN_DUCK;
    }
    cmd(buttons, 250.0, side, yaw)
}

/// What one client saw.
#[derive(Default)]
struct Report {
    /// Prediction errors of the walk (the spawn teleport is not part of it).
    checks: Vec<f32>,
    teleports: usize,
    spawn_teleports: usize,
    max_entities: usize,
    events: usize,
    event_names: BTreeMap<String, usize>,
    peak_tempents: usize,
    sounds: usize,
    sound_names: BTreeMap<String, usize>,
    origin_min: Vec3,
    origin_max: Vec3,
    max_health: u8,
    menus: u32,
    ingame_at: Option<f64>,
}

/// One headless client: a connection and the bookkeeping the report needs.
struct Player {
    conn: Connection<UdpTransport>,
    report: Report,
    last_ack: u32,
    last_menu: Option<String>,
    walk_started: Option<f64>,
    fire_started: Option<f64>,
    last_origin: Option<Vec3>,
    moved: bool,
    walks: bool,
}

impl Player {
    fn new(name: &str, peer: std::net::SocketAddr, world: Arc<WorldCollision>, walks: bool) -> Self {
        let transport = UdpTransport::bind("0.0.0.0:0").expect("socket");
        let steam_id = 76_561_197_960_265_728 + u64::from(std::process::id()) + u64::from(!walks);
        let cfg = ConnectionConfig::new(name, steam_id, &md5_hex(name.as_bytes()));
        let mut conn = Connection::new(transport, PeerAddr::Udp(peer), cfg);
        conn.set_userinfo("_vgui_menus", "0");
        if let Ok(ur) = std::env::var("CS16_HLDS_UPDATERATE") {
            conn.set_cvar("cl_updaterate", &ur);
            conn.set_userinfo("cl_updaterate", &ur);
        }
        conn.set_world_collision(Some(world));
        conn.connect();
        Self {
            conn,
            report: Report::default(),
            last_ack: 0,
            last_menu: None,
            walk_started: None,
            fire_started: None,
            last_origin: None,
            moved: false,
            walks,
        }
    }

    /// Whether the scripted run of this player is over.
    fn finished(&self, now: f64) -> bool {
        self.fire_started.is_some_and(|f| now - f > FIRE_SECONDS)
    }

    /// One 10 ms tick at client time `now`.
    fn tick(&mut self, now: f64, go: bool) {
        let conn = &mut self.conn;
        conn.pump(now);
        if conn.state() != &ConnState::InGame {
            return;
        }
        let r = &mut self.report;
        r.ingame_at.get_or_insert(now);

        // Frames, effects and the entities the renderer would draw.
        conn.frames_mut().advance(0.01);
        let weapon_state = {
            let p = conn.predictor().prediction();
            p.weapon
                .weapons
                .get(&(p.weapon.weapon_id.clamp(0, 255) as u8))
                .map_or(0, |w| w.m_i_weapon_state)
        };
        conn.set_fx_local(weapon_state, false, true);
        let fired = conn.update_effects(now);
        r.events += fired.len();
        for f in &fired {
            let name = f.name.clone().unwrap_or_else(|| format!("#{}", f.index));
            *r.event_names.entry(name).or_default() += 1;
        }
        r.peak_tempents = r.peak_tempents.max(conn.fx().tempents.active().len());
        for s in conn.fx_mut().sounds.drain() {
            r.sounds += 1;
            let name: String = format!("{s:?}").chars().take(70).collect();
            *r.sound_names.entry(name).or_default() += 1;
        }
        let prediction = conn.predictor().prediction().clone();
        let cd = conn.client_data().cloned();
        let emitted = conn.frames_mut().emit(&EmitInputs {
            simorg: prediction
                .predicted
                .then_some(prediction.simorg)
                .or_else(|| cd.as_ref().map(|c| c.origin)),
            standing_on: prediction.standing_on(),
        });
        r.max_entities = r.max_entities.max(emitted.entities.len());

        // The prediction checks since the last tick: those before the walk are the spawn.
        let seen = self.last_ack;
        let walking = self.walk_started.is_some();
        for c in conn.predictor().checks().iter().filter(|c| c.ack > seen) {
            if walking {
                if c.error > 0.1 {
                    let at = now - self.walk_started.unwrap_or(now);
                    println!("  check ack {} at walk +{at:.2} s: error {:.3} teleport {}", c.ack, c.error, c.teleport);
                }
                r.checks.push(c.error);
                r.teleports += usize::from(c.teleport);
            } else {
                r.spawn_teleports += usize::from(c.teleport);
            }
            self.last_ack = self.last_ack.max(c.ack);
        }
        r.max_health = r.max_health.max(conn.fx().hud.health);

        // Joining: answer the server's text menus (team, then class).
        let menu = conn.fx().hud.menu.as_ref().map(|m| m.text.clone());
        if let Some(text) = menu
            && self.last_menu.as_deref() != Some(text.as_str())
        {
            r.menus += 1;
            // CT for both (the observer must be near the player's shots).
            let choice = if text.to_lowercase().contains("team") { "2" } else { "1" };
            println!("menu {}: {:?} -> menuselect {choice}", r.menus, text.lines().next());
            self.last_menu = Some(text);
            conn.send_stringcmd(&format!("menuselect {choice}"));
            conn.fx_mut().hud.menu = None;
        }

        let mut c = cmd(0, 0.0, 0.0, 0.0);
        let alive = conn.fx().hud.health > 0
            && cd.as_ref().is_some_and(|d| d.deadflag == 0 && d.health > 0.0);
        if alive && self.walks && go {
            let ws = *self.walk_started.get_or_insert(now);
            let t = now - ws;
            if t < WALK_SECONDS {
                c = script(t);
            } else {
                // Stand still and fire the pistol: a click every 300 ms.
                let ft = now - *self.fire_started.get_or_insert(now);
                c = cmd(if (ft % 0.3) < 0.1 { IN_ATTACK } else { 0 }, 0.0, 0.0, 0.0);
            }
            let o = prediction.simorg;
            if r.checks.is_empty() {
                r.origin_min = o;
                r.origin_max = o;
            }
            r.origin_min = r.origin_min.min(o);
            r.origin_max = r.origin_max.max(o);
            if self.last_origin.is_some_and(|l| (o - l).length() > 0.5) {
                self.moved = true;
            }
            self.last_origin = Some(o);
        }
        conn.queue_cmd(c);
    }

    fn print(&self, label: &str) {
        let r = &self.report;
        let stats = self.conn.stats();
        let n = r.checks.len().max(1);
        let worst = r.checks.iter().copied().fold(0.0f32, f32::max);
        let mean = r.checks.iter().sum::<f32>() / n as f32;
        let over = r.checks.iter().filter(|&&e| e > 0.1).count();
        let hud = &self.conn.fx().hud;
        println!("---- {label} ----");
        println!("state: {:?}", self.conn.state());
        println!("signon: in game at {:?} s, menus answered {}", r.ingame_at, r.menus);
        println!("parse_errors: {}", stats.parse_errors);
        println!(
            "datagrams: {} in, full updates {}, delta updates {}, updates in the last second {}, \
             max entities visible {}",
            stats.datagrams_in,
            stats.full_updates,
            stats.delta_updates,
            stats.frames_in_last_second,
            r.max_entities
        );
        println!(
            "prediction (walk only): {} checks, worst {worst:.4}, mean {mean:.4}, over 0.1: {over}, \
             teleports {} (spawn teleports before the walk: {})",
            r.checks.len(),
            r.teleports,
            r.spawn_teleports
        );
        println!(
            "movement: x {:.0}..{:.0} y {:.0}..{:.0} z {:.0}..{:.0}, moved {}",
            r.origin_min.x,
            r.origin_max.x,
            r.origin_min.y,
            r.origin_max.y,
            r.origin_min.z,
            r.origin_max.z,
            self.moved
        );
        println!(
            "hud: health {} (max seen {}), money {}, armor {}, weapon {:?}",
            hud.health, r.max_health, hud.money, hud.armor, hud.current_weapon
        );
        let ammo: BTreeMap<_, _> = hud.ammo.iter().filter(|(_, n)| **n > 0).collect();
        println!("ammo {ammo:?}, players on the scoreboard {}", hud.players.len());
        println!(
            "effects: events {} {:?}, peak tempents {}, sounds {} {:?}",
            r.events, r.event_names, r.peak_tempents, r.sounds, r.sound_names
        );
        println!("server: {:?}", self.conn.server_info().map(|i| (&i.map_path, i.max_clients)));
    }
}

#[test]
#[ignore = "needs a server: set CS16_HLDS_ADDR"]
fn headless_client_against_a_real_server() {
    let Ok(addr) = std::env::var("CS16_HLDS_ADDR") else {
        eprintln!("SKIPPED: CS16_HLDS_ADDR not set");
        return;
    };
    let map_name = std::env::var("CS16_HLDS_MAP").unwrap_or_else(|_| "de_dust2".to_string());
    let vfs = Vfs::discover(None, VfsConfig::default()).expect("game directory (set CS16_GAMEDIR)");
    let bsp = BspMap::load(&vfs, &map_name).expect("the map parses");
    let world = Arc::new(WorldCollision::from_bsp(&bsp));
    let peer = addr
        .to_socket_addrs()
        .expect("address")
        .find(std::net::SocketAddr::is_ipv4)
        .expect("an IPv4 address");

    let mut player = Player::new("clientcore", peer, world.clone(), true);
    let mut observer = Player::new("observer", peer, world, false);

    let started = Instant::now();
    let mut next = started;
    // The walk starts once both clients joined and the server's round restart has passed
    // (it respawns everyone: a teleport the walk must not mistake for a prediction error).
    let mut go_at: Option<f64> = None;
    loop {
        next += TICK;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
        let now = started.elapsed().as_secs_f64();
        if go_at.is_none() && player.report.menus >= 2 && observer.report.menus >= 2 {
            go_at = Some(now + SETTLE_SECONDS);
        }
        let go = go_at.is_some_and(|g| now >= g);
        player.tick(now, go);
        observer.tick(now, go);
        if player.conn.state().is_terminal() || now > 150.0 || player.finished(now) {
            break;
        }
    }
    // Let the last events reach the observer.
    for _ in 0..50 {
        std::thread::sleep(TICK);
        let now = started.elapsed().as_secs_f64();
        player.tick(now, true);
        observer.tick(now, true);
    }

    println!("==== {addr}, {map_name} ====");
    player.print("player (walk, then pistol shots)");
    observer.print("observer (stands still, sees the player's shots)");
    for m in player.conn.console_log().iter().rev().take(3) {
        println!("console: {m}");
    }

    let report = &player.report;
    let state = player.conn.state().clone();
    let last_message = player.conn.messages().back().cloned();
    player.conn.execute_local("disconnect");
    observer.conn.execute_local("disconnect");
    assert_eq!(state, ConnState::InGame, "{last_message:?}");
    assert_eq!(player.conn.stats().parse_errors, 0, "every datagram parsed");
    assert_eq!(observer.conn.stats().parse_errors, 0, "every observer datagram parsed");
    assert!(report.max_entities > 0, "entities arrive");
    assert_eq!(report.max_health, 100, "joined and alive with 100 health");
    assert_eq!(report.teleports, 0, "no teleports during the walk");
    let worst = report.checks.iter().copied().fold(0.0f32, f32::max);
    let mean = report.checks.iter().sum::<f32>() / report.checks.len().max(1) as f32;
    let over_1 = report.checks.iter().filter(|&&e| e > 1.0).count();
    let max_over_1 = (report.checks.len() / 100).max(5);
    // At 22 Hz updates stock HLDS stays below 0.5 units; at 60 Hz (`+sv_maxupdaterate 102` on stock
    // HLDS build 10210 and default on `cs-server`) `clientdata_t.origin` 1/128 truncation toward
    // zero lands negative-facing axial positions (`-1600.000`, `-1528.000`) onto `d == 0.0` where
    // `PM_HullPointContents` returns `CONTENTS_SOLID`, triggering `PM_CheckStuck` `-0.125` brush
    // nudges (~2.8-6.8 u ledge spikes) or the 50 ms `PM_CHECKSTUCK_MINTIME` cooldown against player
    // bounding boxes (~8.3 u) on both servers while keeping mean < 0.05 and 0 teleports.
    let limit: f32 = std::env::var("CS16_HLDS_MAX_ERROR").ok().and_then(|v| v.parse().ok()).unwrap_or(10.0);
    assert!(worst < limit, "prediction error worst {worst} (limit {limit})");
    assert!(mean < 0.05, "prediction error mean {mean} (limit 0.05)");
    assert!(over_1 <= max_over_1, "spikes over 1.0 unit: {over_1} (limit {max_over_1})");
    assert!(observer.report.events > 0, "the observer saw the shots");
    assert!(report.checks.len() > 200, "the server acked the walk: {}", report.checks.len());
}
