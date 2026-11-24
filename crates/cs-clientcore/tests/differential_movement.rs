//! 65-second deterministic differential movement traces comparing `cs-clientcore` prediction
//! against our own in-process `Server<CsGame>` and (when `CS16_HLDS_ADDR` is set) live `hlds.exe`.

mod predict_support;

use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cs_assets::bsp::BspMap;
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_engine::ServerMap;
use cs_pm::collision::WorldCollision;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_transport::{PeerAddr, UdpTransport};
use cs_vfs::{Vfs, VfsConfig};
use predict_support::{IN_DUCK, IN_FORWARD, IN_JUMP, IN_MOVELEFT, Loop, map_entities, server_map, walk};

const IN_BACK: u16 = 1 << 4;
const IN_MOVERIGHT: u16 = 1 << 10;

/// 65 seconds at 100 Hz = 6500 usercmds (`>= 60 s` requirement).
const TRACE_TICKS: usize = 6500;
const TICK: Duration = Duration::from_millis(10);
/// Protocol coordinate grid is 1/8 unit; tolerance is 1/32 unit (`0.03125` u).
const MAX_ORIGIN_TOLERANCE: f32 = 0.03125;
/// Velocity comparison tolerance (`0.1` u/s).
const MAX_VELOCITY_TOLERANCE: f32 = 0.1;

/// Multi-maneuver deterministic script over 65 seconds:
/// walking, running, 90/180-degree turns, crouch-walk, jump + air-strafe, crouch-jump,
/// friction stops, and backward/diagonal strafe.
fn maneuver_cmd(tick: usize) -> GoldSrcUserCmd {
    let phase = (tick / 250) % 10;
    let within = tick % 250;
    match phase {
        // 0: straight sprint forward
        0 => walk(10, IN_FORWARD, 250.0, 0.0, 0.0),
        // 1: 90-degree turn + diagonal sprint
        1 => walk(10, IN_FORWARD | IN_MOVERIGHT, 250.0, 250.0, 90.0),
        // 2: jump + air-strafe arc
        2 => {
            let mut buttons = IN_FORWARD;
            let side = if within < 125 {
                buttons |= IN_MOVELEFT;
                -250.0
            } else {
                buttons |= IN_MOVERIGHT;
                250.0
            };
            if within % 65 < 5 {
                buttons |= IN_JUMP;
            }
            let yaw = 90.0 + (within as f32) * 0.4;
            walk(10, buttons, 250.0, side, yaw)
        }
        // 3: crouch-walk + 180-degree turn
        3 => walk(10, IN_FORWARD | IN_DUCK, 250.0, 0.0, 180.0),
        // 4: friction decelerate to stop, then stand
        4 => {
            if within < 100 {
                walk(10, IN_FORWARD, 250.0, 0.0, 180.0)
            } else {
                walk(10, 0, 0.0, 0.0, 180.0)
            }
        }
        // 5: crouch-jump bursts
        5 => {
            let mut buttons = IN_FORWARD | IN_DUCK;
            if within % 80 < 5 {
                buttons |= IN_JUMP;
            }
            walk(10, buttons, 250.0, 0.0, 225.0)
        }
        // 6: backward walk + strafe left
        6 => walk(10, IN_BACK | IN_MOVELEFT, -250.0, -250.0, 270.0),
        // 7: continuous figure-8 yaw turn with strafe switching
        7 => {
            let yaw = 270.0 + 45.0 * ((within as f32) * 0.05).sin();
            let side = if within % 80 < 40 { 250.0 } else { -250.0 };
            walk(10, IN_FORWARD | IN_MOVELEFT, 250.0, side, yaw)
        }
        // 8: mixed jump + duck + micro-stops
        8 => {
            let mut buttons = 0;
            let mut fwd = 0.0;
            if within % 50 < 35 {
                buttons |= IN_FORWARD;
                fwd = 250.0;
            }
            if within % 90 == 10 {
                buttons |= IN_JUMP;
            }
            if (100..150).contains(&within) {
                buttons |= IN_DUCK;
            }
            walk(10, buttons, fwd, 0.0, 315.0)
        }
        // 9: return turn back toward 0 yaw
        _ => walk(10, IN_FORWARD | IN_MOVERIGHT, 250.0, 250.0, 360.0),
    }
}

fn load_dust2_or_stub() -> (ServerMap, Arc<WorldCollision>) {
    if let Ok(vfs) = Vfs::discover(None, VfsConfig::default())
        && let (Ok(bsp), Ok(raw)) = (BspMap::load(&vfs, "de_dust2"), vfs.read_bytes("maps/de_dust2.bsp"))
    {
        let bsp = Arc::new(bsp);
        let world = Arc::new(WorldCollision::from_bsp(&bsp));
        if let Some(map) = ServerMap::from_bsp("de_dust2", &raw, bsp) {
            return (map, world);
        }
    }
    server_map(map_entities(None))
}

#[test]
fn differential_60s_movement_trace_matches_server_within_protocol_tolerance() {
    let (map, world) = load_dust2_or_stub();
    let mut l = Loop::new(map, world);
    // Clear freeze period so client moves at full weapon speed immediately.
    l.server.game_mut().actions.push(Box::new(|game, eng| {
        eng.set_cvar("mp_freezetime", "0");
        game.restart_round_now(eng);
    }));
    for _ in 0..40 {
        l.step(Some(walk(10, 0, 0.0, 0.0, 0.0)));
    }

    let mut last_ack = l.conn.predictor().checks().back().map_or(0, |c| c.ack);
    let mut samples: Vec<(usize, f32, f32, bool)> = Vec::new();

    for i in 0..TRACE_TICKS {
        l.step(Some(maneuver_cmd(i)));
        let seen = last_ack;
        for c in l.conn.predictor().checks().iter().filter(|c| c.ack > seen) {
            samples.push((i, c.error, c.velocity_error, c.teleport));
            last_ack = last_ack.max(c.ack);
        }
    }
    for _ in 0..10 {
        l.step(None);
        let seen = last_ack;
        for c in l.conn.predictor().checks().iter().filter(|c| c.ack > seen) {
            samples.push((TRACE_TICKS, c.error, c.velocity_error, c.teleport));
            last_ack = last_ack.max(c.ack);
        }
    }

    assert!(
        samples.len() >= 1000,
        "expected >= 1000 acked movement checks over 65s, got {}",
        samples.len()
    );
    let mut origins: Vec<f32> = samples.iter().map(|s| s.1).collect();
    let mut vels: Vec<f32> = samples.iter().map(|s| s.2).collect();
    origins.sort_by(f32::total_cmp);
    vels.sort_by(f32::total_cmp);

    let teleports = samples.iter().filter(|s| s.3).count();
    let worst_origin = origins.last().copied().unwrap_or(0.0);
    let p99_origin = origins
        .get((origins.len() * 99) / 100)
        .copied()
        .unwrap_or(0.0);
    let mean_origin = origins.iter().sum::<f32>() / origins.len().max(1) as f32;
    let p99_vel = vels.get((vels.len() * 99) / 100).copied().unwrap_or(0.0);

    let first_10s: Vec<f32> = samples
        .iter()
        .filter(|s| s.0 < 1000)
        .map(|s| s.1)
        .collect();
    let final_10s: Vec<f32> = samples
        .iter()
        .filter(|s| s.0 >= TRACE_TICKS - 1000)
        .map(|s| s.1)
        .collect();
    let first_mean = first_10s.iter().sum::<f32>() / first_10s.len().max(1) as f32;
    let final_mean = final_10s.iter().sum::<f32>() / final_10s.len().max(1) as f32;

    assert_eq!(teleports, 0, "65s movement trace had {teleports} teleports");
    assert!(
        mean_origin <= MAX_ORIGIN_TOLERANCE,
        "65s mean origin error {mean_origin:.5} > {MAX_ORIGIN_TOLERANCE}"
    );
    assert!(
        p99_origin <= MAX_ORIGIN_TOLERANCE,
        "65s p99 origin error {p99_origin:.5} > {MAX_ORIGIN_TOLERANCE}"
    );
    assert!(
        worst_origin <= 0.0625,
        "65s worst origin error {worst_origin:.5} > 0.0625"
    );
    assert!(
        p99_vel <= 0.25,
        "65s p99 velocity error {p99_vel:.5} > 0.25"
    );
    assert!(
        final_mean <= first_mean + 0.01,
        "drift grew over 65s: first 10s mean {first_mean:.5}, final 10s mean {final_mean:.5}"
    );
}

fn quantize_cd_vel(v: glam::Vec3) -> glam::Vec3 {
    glam::Vec3::new(
        (v.x * 8.0).trunc() / 8.0,
        (v.y * 8.0).trunc() / 8.0,
        (v.z * 8.0).trunc() / 8.0,
    )
}

#[test]
fn differential_60s_pmove_direct_matches_server_zero_drift() {
    let (map, world) = server_map(map_entities(None));
    let mut l = Loop::new(map, world);
    l.conn.set_cvar("cl_updaterate", "100");
    l.server.game_mut().actions.push(Box::new(|game, eng| {
        eng.set_cvar("mp_freezetime", "0");
        game.restart_round_now(eng);
    }));
    for _ in 0..40 {
        l.step(Some(walk(10, 0, 0.0, 0.0, 0.0)));
    }

    let mut last_ack = l.conn.predictor().checks().back().map_or(0, |c| c.ack);
    let mut samples: Vec<(usize, f32, bool)> = Vec::new();
    let mut worst_checkpoint_vel = 0.0_f32;
    let mut worst_checkpoint_origin = 0.0_f32;

    for i in 0..TRACE_TICKS {
        l.step(Some(maneuver_cmd(i)));
        let seen = last_ack;
        for c in l.conn.predictor().checks().iter().filter(|c| c.ack > seen) {
            samples.push((i, c.error, c.teleport));
            last_ack = last_ack.max(c.ack);
        }
        if (i + 1) % 650 == 0 {
            for _ in 0..5 {
                l.step(None);
            }
            let out = std::rc::Rc::new(std::cell::Cell::new((
                glam::Vec3::ZERO,
                glam::Vec3::ZERO,
            )));
            let sink = std::rc::Rc::clone(&out);
            l.server.game_mut().actions.push(Box::new(move |_, eng| {
                let e = eng.ent(eng.client_edict(0)).unwrap();
                sink.set((e.origin, e.velocity));
            }));
            l.step(None);
            let (sv_org, sv_vel) = out.get();
            let pred = l.conn.predictor().prediction().clone();
            worst_checkpoint_origin = worst_checkpoint_origin.max((sv_org - pred.simorg).length());
            worst_checkpoint_vel =
                worst_checkpoint_vel.max((quantize_cd_vel(sv_vel) - pred.simvel).length());
        }
    }

    assert!(samples.len() >= 1000, "got {} samples", samples.len());
    let teleports = samples.iter().filter(|s| s.2).count();
    let worst_origin = samples.iter().map(|s| s.1).fold(0.0_f32, f32::max);
    let first_mean = {
        let s: Vec<f32> = samples.iter().filter(|x| x.0 < 1000).map(|x| x.1).collect();
        s.iter().sum::<f32>() / s.len().max(1) as f32
    };
    let final_mean = {
        let s: Vec<f32> = samples
            .iter()
            .filter(|x| x.0 >= TRACE_TICKS - 1000)
            .map(|x| x.1)
            .collect();
        s.iter().sum::<f32>() / s.len().max(1) as f32
    };

    assert_eq!(teleports, 0, "65s direct pmove trace had {teleports} teleports");
    assert!(
        worst_origin <= MAX_ORIGIN_TOLERANCE,
        "65s direct worst origin error {worst_origin:.5} > {MAX_ORIGIN_TOLERANCE}"
    );
    assert!(
        worst_checkpoint_origin <= MAX_ORIGIN_TOLERANCE,
        "65s checkpoint origin error {worst_checkpoint_origin:.5} > {MAX_ORIGIN_TOLERANCE}"
    );
    assert!(
        worst_checkpoint_vel <= MAX_VELOCITY_TOLERANCE,
        "65s checkpoint velocity error {worst_checkpoint_vel:.5} > {MAX_VELOCITY_TOLERANCE}"
    );
    assert!(
        final_mean <= first_mean + 0.01,
        "drift grew over 65s: first 10s mean {first_mean:.5}, final 10s mean {final_mean:.5}"
    );
}

#[test]
#[ignore = "needs a live HLDS server: set CS16_HLDS_ADDR"]
fn differential_60s_movement_trace_against_hlds() {
    let Ok(addr) = std::env::var("CS16_HLDS_ADDR") else {
        eprintln!("SKIPPED: CS16_HLDS_ADDR not set");
        return;
    };
    let map_name = std::env::var("CS16_HLDS_MAP").unwrap_or_else(|_| "de_dust2".to_string());
    let vfs = Vfs::discover(None, VfsConfig::default()).expect("CS16_GAMEDIR required");
    let bsp = Arc::new(BspMap::load(&vfs, &map_name).expect("load bsp"));
    let world = Arc::new(WorldCollision::from_bsp(&bsp));
    let peer = addr
        .to_socket_addrs()
        .expect("resolve address")
        .find(std::net::SocketAddr::is_ipv4)
        .expect("IPv4 address");

    let transport = UdpTransport::bind("0.0.0.0:0").expect("bind UDP");
    let cfg = ConnectionConfig::new(
        "diff65s",
        76_561_197_960_266_601 + u64::from(std::process::id()),
        &md5_hex(b"diff65s"),
    );
    let mut conn = Connection::new(transport, PeerAddr::Udp(peer), cfg);
    conn.set_userinfo("_vgui_menus", "0");
    conn.set_world_collision(Some(world));
    conn.connect();

    let started = Instant::now();
    let mut next = started;
    let mut menus = 0u32;
    let mut last_menu: Option<String> = None;
    let mut go_at: Option<f64> = None;
    let mut walk_tick = 0usize;
    let mut last_ack = 0u32;
    let mut checks: Vec<(usize, f32, f32, bool)> = Vec::new();
    let mut spawn_teleports = 0usize;

    loop {
        next += TICK;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
        let now = started.elapsed().as_secs_f64();
        conn.pump(now);

        if conn.state() == &ConnState::InGame {
            conn.frames_mut().advance(0.01);
            let _ = conn.update_effects(now);
            if let Some(text) = conn.fx().hud.menu.as_ref().map(|m| m.text.clone())
                && last_menu.as_deref() != Some(text.as_str())
            {
                menus += 1;
                let choice = if text.to_lowercase().contains("team") { "2" } else { "1" };
                last_menu = Some(text);
                conn.send_stringcmd(&format!("menuselect {choice}"));
                conn.fx_mut().hud.menu = None;
            }
            if go_at.is_none() && menus >= 2 {
                go_at = Some(now + 4.0);
            }
            let alive = conn.fx().hud.health > 0
                && conn
                    .client_data()
                    .is_some_and(|d| d.deadflag == 0 && d.health > 0.0);
            let walking = alive && go_at.is_some_and(|g| now >= g);
            let seen = last_ack;
            for c in conn.predictor().checks().iter().filter(|c| c.ack > seen) {
                if walking {
                    checks.push((walk_tick, c.error, c.velocity_error, c.teleport));
                } else {
                    spawn_teleports += usize::from(c.teleport);
                }
                last_ack = last_ack.max(c.ack);
            }
            let cmd = if walking {
                let c = maneuver_cmd(walk_tick);
                walk_tick += 1;
                c
            } else {
                walk(10, 0, 0.0, 0.0, 0.0)
            };
            conn.queue_cmd(cmd);
        }

        if conn.state().is_terminal() || walk_tick >= TRACE_TICKS || now > 160.0 {
            break;
        }
    }

    for _ in 0..30 {
        std::thread::sleep(TICK);
        let now = started.elapsed().as_secs_f64();
        conn.pump(now);
        let seen = last_ack;
        for c in conn.predictor().checks().iter().filter(|c| c.ack > seen) {
            checks.push((walk_tick, c.error, c.velocity_error, c.teleport));
            last_ack = last_ack.max(c.ack);
        }
        conn.queue_cmd(walk(10, 0, 0.0, 0.0, 0.0));
    }

    conn.execute_local("disconnect");

    let mut origins: Vec<f32> = checks.iter().map(|c| c.1).collect();
    let mut vels: Vec<f32> = checks.iter().map(|c| c.2).collect();
    origins.sort_by(f32::total_cmp);
    vels.sort_by(f32::total_cmp);

    let worst_origin = origins.last().copied().unwrap_or(0.0);
    let p99_origin = origins
        .get((origins.len() * 99) / 100)
        .copied()
        .unwrap_or(0.0);
    let mean_origin = origins.iter().sum::<f32>() / origins.len().max(1) as f32;
    let worst_vel = vels.last().copied().unwrap_or(0.0);
    let p99_vel = vels.get((vels.len() * 99) / 100).copied().unwrap_or(0.0);
    let mean_vel = vels.iter().sum::<f32>() / vels.len().max(1) as f32;
    let teleports = checks.iter().filter(|c| c.3).count();

    let first_10s: Vec<f32> = checks.iter().filter(|c| c.0 < 1000).map(|c| c.1).collect();
    let last_10s: Vec<f32> = checks
        .iter()
        .filter(|c| c.0 >= TRACE_TICKS.saturating_sub(1000))
        .map(|c| c.1)
        .collect();
    let first_mean = first_10s.iter().sum::<f32>() / first_10s.len().max(1) as f32;
    let last_mean = last_10s.iter().sum::<f32>() / last_10s.len().max(1) as f32;

    eprintln!(
        "HLDS 65s differential: walk_ticks={walk_tick} checks={} spawn_teleports={spawn_teleports} \
         teleports={teleports} origin(worst={worst_origin:.5}, p99={p99_origin:.5}, mean={mean_origin:.5}, \
         first10s={first_mean:.5}, last10s={last_mean:.5}) vel(worst={worst_vel:.5}, p99={p99_vel:.5}, mean={mean_vel:.5})",
        checks.len()
    );

    let over_1 = origins.iter().filter(|&&e| e > 1.0).count();
    assert!(
        checks.len() >= 1000,
        "expected >= 1000 acked checks, got {}",
        checks.len()
    );
    assert_eq!(teleports, 0, "live HLDS walk had {teleports} teleports");
    assert!(
        mean_origin <= MAX_ORIGIN_TOLERANCE,
        "live HLDS 65s mean origin error {mean_origin:.5} > {MAX_ORIGIN_TOLERANCE}"
    );
    assert!(
        p99_origin <= 0.05,
        "live HLDS 65s p99 origin error {p99_origin:.5} > 0.05"
    );
    assert!(
        last_mean <= first_mean + 0.01,
        "live HLDS drift grew over 65s: first10s={first_mean:.5}, last10s={last_mean:.5}"
    );
    assert!(
        p99_vel <= 0.25,
        "live HLDS 65s p99 velocity error {p99_vel:.5} > 0.25"
    );
    assert!(
        over_1 <= checks.len() / 100,
        "live HLDS spikes > 1.0 u: {over_1} of {}",
        checks.len()
    );
    assert!(
        worst_origin < 8.0,
        "live HLDS 65s worst origin error {worst_origin:.5} >= 8.0"
    );
}
