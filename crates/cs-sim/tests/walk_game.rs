//! `Server<WalkGame>` on de_dust2 with `cs-probe` clients over the loopback. Every test returns
//! without assertions when the installed game is absent.

use std::sync::Arc;

use cs_assets::bsp::{BspMap, SpawnPoint, SpawnTeam};
use cs_assets::materials::MaterialTable;
use cs_cvar::CVarRegistry;
use cs_engine::{EntVars, Server, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcUserCmd};
use cs_proto48::info::InfoString;
use cs_sim::pmove::{MoveParams, VEC_VIEW, snap_spawn_to_floor};
use cs_sim::trace::{BspCollision, Hull};
use cs_sim::walk_game::{WALK_MAX_SPEED, WalkGame, buttons, ent_flags};
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

/// Seconds of one frame.
const FRAME: f64 = 0.01;

/// Position tolerance of the project.
const TOLERANCE: f32 = 0.03125;

/// `Server<WalkGame>` on de_dust2 plus the loopback hub the probes connect through.
struct World {
    server: Server<WalkGame>,
    hub: LoopbackHub,
    now: f64,
    /// A copy of the collision the game moves players with, for the test's own traces.
    collision: BspCollision,
    /// The map's spawn points, as the game was given them.
    spawns: Vec<SpawnPoint>,
}

impl World {
    fn dust2(vfs: &Vfs) -> Self {
        let bsp = BspMap::load(vfs, "de_dust2").expect("load de_dust2");
        let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("read de_dust2");
        let collision = BspCollision::from_map(&bsp, MaterialTable::load(vfs));
        let spawns = bsp.spawns.clone();
        let map = ServerMap::from_bsp("de_dust2", &raw, Arc::new(bsp)).expect("BSP header");
        let game = WalkGame::new(collision.clone(), spawns.clone(), MoveParams::default());
        let cfg = ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        };
        let server = Server::new(
            cfg,
            map,
            game,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        Self {
            server,
            hub: LoopbackHub::new(),
            now: 0.0,
            collision,
            spawns,
        }
    }

    /// Advances `frames` frames: each pumps every listed probe, then runs one server frame.
    fn step(&mut self, probes: &mut [&mut Probe], frames: usize) {
        for _ in 0..frames {
            // Counted in whole frames so the clock does not drift.
            self.now = ((self.now / FRAME).round() + 1.0) * FRAME;
            for probe in probes.iter_mut() {
                probe.pump(self.now);
            }
            self.server.frame(&mut self.hub, self.now);
        }
    }

    /// Connects a new probe and runs, pumping `others` too, until it is in the game.
    fn join(&mut self, name: &str, others: &mut [&mut Probe]) -> Probe {
        let endpoint = self.hub.connect();
        let cfg = ProbeConfig {
            name: name.to_string(),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(format!("probe {}", endpoint.id()).as_bytes()),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        probe.connect();
        for _ in 0..300 {
            let mut all: Vec<&mut Probe> = others.iter_mut().map(|p| &mut **p).collect();
            all.push(&mut probe);
            self.step(&mut all, 1);
            if probe.state() == &ProbeState::InGame {
                return probe;
            }
        }
        panic!("{name} did not get in the game: {:?}", probe.state());
    }

    fn vars(&self, slot: u8) -> &EntVars {
        let edicts = self.server.edicts();
        &edicts.get(edicts.client(slot)).expect("client edict").vars
    }

    /// Runs one frame per command, the command queued on `probe` before the frame.
    fn run_cmds(&mut self, probe: &mut Probe, cmds: impl IntoIterator<Item = GoldSrcUserCmd>) {
        for cmd in cmds {
            probe.queue_cmd(cmd);
            self.step(&mut [probe], 1);
        }
    }
}

fn slot(probe: &Probe) -> u8 {
    probe.player_slot().expect("slot")
}

/// A 10 ms command looking along `yaw` with `buttons` held.
fn cmd(yaw: f32, forwardmove: f32, buttons: u16) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec: 10,
        viewangles: Vec3::new(0.0, yaw, 0.0),
        forwardmove,
        buttons,
        ..Default::default()
    }
}

/// Length of the trace a run direction must clear.
const CLEAR_RUN: f32 = 600.0;

/// First of the spawn's yaw and its seven other multiples of 45 degrees along which a
/// standing-hull trace of [`CLEAR_RUN`] units from `start`, lifted 4 units, hits nothing.
fn clear_yaw(collision: &BspCollision, start: Vec3, spawn_yaw: f32) -> f32 {
    let from = start + Vec3::new(0.0, 0.0, 4.0);
    (0..8)
        .map(|k| (spawn_yaw + 45.0 * k as f32).rem_euclid(360.0))
        .find(|yaw| {
            let r = yaw.to_radians();
            let to = from + Vec3::new(r.cos(), r.sin(), 0.0) * CLEAR_RUN;
            let tr = collision.trace_hull(from, to, Hull::Standing);
            !tr.start_solid && !tr.all_solid && tr.fraction >= 1.0
        })
        .unwrap_or_else(|| {
            panic!("no clear {CLEAR_RUN}-unit run from {start:?} (spawn yaw {spawn_yaw})")
        })
}

/// Distance from `origin` down to the floor under the standing hull, looking at most 64 units
/// down; 64 when nothing is found. `None` when the hull at `origin` is already in solid: the
/// trace then ends where it starts, and a distance of 0 would hide a player sunk in the floor.
fn floor_distance(collision: &BspCollision, origin: Vec3) -> Option<f32> {
    let tr = collision.trace_hull(origin, origin - Vec3::new(0.0, 0.0, 64.0), Hull::Standing);
    (!tr.start_solid && !tr.all_solid).then_some(origin.z - tr.end_pos.z)
}

/// A sample taken during the run: whether `FL_ONGROUND` is set, and the floor distance.
#[derive(Debug, Clone, Copy)]
struct FloorSample {
    on_ground: bool,
    floor: Option<f32>,
}

/// The run of the runner probe.
struct ForwardRun {
    world: World,
    probe: Probe,
    /// Edict origin before the run.
    start: Vec3,
    /// Samples after commands 40, 80, 120 and 160.
    samples: Vec<FloorSample>,
}

/// Runs 200 commands of full forward speed along a clear direction with the player in slot 1
/// (the first terrorist spawn); a first probe only occupies slot 0 and keeps its connection
/// alive. Slot 1 because on de_dust2 none of the first nine counter-terrorist spawns has
/// [`CLEAR_RUN`] clear units in any of the eight directions. The clear direction from the
/// first terrorist spawn goes down the ramp in front of it.
fn run_forward(vfs: &Vfs) -> ForwardRun {
    let mut w = World::dust2(vfs);
    let mut keeper = w.join("Keeper", &mut []);
    let mut probe = w.join("Runner", &mut [&mut keeper]);
    let s = slot(&probe);
    assert_eq!(s, 1);
    let start = w.vars(s).origin;
    let yaw = clear_yaw(&w.collision, start, w.vars(s).v_angle.y);
    let mut samples = Vec::new();
    for n in 1..=200 {
        probe.queue_cmd(cmd(yaw, WALK_MAX_SPEED, 0));
        w.step(&mut [&mut keeper, &mut probe], 1);
        if n % 40 == 0 && n < 200 {
            let v = w.vars(s);
            samples.push(FloorSample {
                on_ground: v.flags & ent_flags::FL_ONGROUND != 0,
                floor: floor_distance(&w.collision, v.origin),
            });
        }
    }
    // Lets the last update reach the probe.
    w.step(&mut [&mut keeper, &mut probe], 10);
    ForwardRun {
        world: w,
        probe,
        start,
        samples,
    }
}

fn model_key(probe: &Probe, slot: u8) -> Option<String> {
    let info = probe.userinfos().get(&slot)?;
    InfoString::parse(info).get("model").map(str::to_string)
}

#[test]
fn player_runs_forward_on_de_dust2() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let r = run_forward(&vfs);
    let v = r.world.vars(slot(&r.probe));
    let run = v.origin - r.start;
    let horizontal = Vec3::new(run.x, run.y, 0.0).length();
    let speed = Vec3::new(v.velocity.x, v.velocity.y, 0.0).length();
    let floor = floor_distance(&r.world.collision, v.origin);
    println!(
        "yaw {} start {:?} end {:?} velocity {:?} horizontal {horizontal:.3} dz {:.3} speed {speed:.3} floor {floor:?} samples {:?}",
        v.v_angle.y, r.start, v.origin, v.velocity, run.z, r.samples
    );
    assert!((440.0..=500.0).contains(&horizontal), "ran {horizontal}");
    assert!((speed - WALK_MAX_SPEED).abs() <= 5.0, "final speed {speed}");
    // The run goes down a ramp: the player follows the floor, neither hovering nor sunk.
    assert!(
        run.z < -10.0,
        "the run no longer goes downhill: dz {}",
        run.z
    );
    assert_ne!(v.flags & ent_flags::FL_ONGROUND, 0, "flags {:#x}", v.flags);
    // `None`: the hull is in solid, sunk into the floor.
    assert!(
        floor.is_some_and(|f| f <= 1.0),
        "floor {floor:?} below the player at the end"
    );
    assert_eq!(r.samples.len(), 4);
    for (i, sample) in r.samples.iter().enumerate() {
        assert!(sample.floor.is_some(), "sample {i} in solid: {sample:?}");
        if sample.on_ground {
            assert!(
                sample.floor.is_some_and(|f| f <= 1.0),
                "sample {i}: {sample:?}"
            );
        }
    }
    let grounded = r.samples.iter().filter(|s| s.on_ground).count();
    assert!(grounded >= 3, "on the ground in {grounded} of 4 samples");
    assert_ne!(v.flags & ent_flags::FL_CLIENT, 0, "flags {:#x}", v.flags);
}

#[test]
fn clientdata_matches_server_origin() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let r = run_forward(&vfs);
    let server = r.world.vars(slot(&r.probe)).origin;
    let client = r.probe.client_data().expect("client data").origin;
    println!("server {server:?} client {client:?}");
    assert!((server - r.start).length() > 100.0, "the player moved");
    assert!(
        (client - server).abs().max_element() <= TOLERANCE,
        "{client:?} vs {server:?}"
    );
}

#[test]
fn spawns_alternate_teams_and_set_model_key() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let mut w = World::dust2(&vfs);
    let mut a = w.join("A", &mut []);
    let mut b = w.join("B", &mut [&mut a]);
    w.step(&mut [&mut a, &mut b], 20);
    assert_eq!((slot(&a), slot(&b)), (0, 1));
    assert_eq!(model_key(&a, 0).as_deref(), Some("gign"));
    assert_eq!(model_key(&a, 1).as_deref(), Some("leet"));
    let (pa, pb) = (w.vars(0).origin, w.vars(1).origin);
    println!("slot 0 {pa:?} slot 1 {pb:?}");
    assert!((pa - pb).length() > 32.0, "{pa:?} vs {pb:?}");
    // Slot 0 stands on the first counter-terrorist spawn, slot 1 on the first terrorist one.
    let first_of = |team: SpawnTeam| {
        let spawn = w.spawns.iter().find(|s| s.team == team).expect("spawn");
        snap_spawn_to_floor(spawn.position, &w.collision)
    };
    for (origin, team) in [
        (pa, SpawnTeam::CounterTerrorist),
        (pb, SpawnTeam::Terrorist),
    ] {
        let expected = first_of(team);
        assert!(
            (origin - expected).abs().max_element() <= TOLERANCE,
            "{team:?}: {origin:?} vs {expected:?}"
        );
    }
}

#[test]
fn two_players_see_each_other() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let mut w = World::dust2(&vfs);
    let mut a = w.join("A", &mut []);
    // Two CT players: slots 0 and 2, spawned next to each other.
    let mut b = w.join("B", &mut [&mut a]);
    let mut c = w.join("C", &mut [&mut a, &mut b]);
    w.step(&mut [&mut a, &mut b, &mut c], 20);
    assert_eq!((slot(&a), slot(&c)), (0, 2));

    for (viewer, other) in [(&a, &c), (&c, &a)] {
        let other_slot = slot(other);
        let seen = viewer
            .entities()
            .get(&(u16::from(other_slot) + 1))
            .unwrap_or_else(|| panic!("slot {} does not see slot {other_slot}", slot(viewer)));
        let actual = w.vars(other_slot).origin;
        assert!(
            (seen.origin - actual).abs().max_element() <= TOLERANCE,
            "{:?} vs {actual:?}",
            seen.origin
        );
        assert_eq!(model_key(viewer, other_slot).as_deref(), Some("gign"));
    }
}

#[test]
fn jump_and_duck_change_the_flags_and_hull() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let mut w = World::dust2(&vfs);
    let mut probe = w.join("Jumper", &mut []);
    let s = slot(&probe);
    let yaw = w.vars(s).v_angle.y;

    // Settles onto the floor.
    w.run_cmds(&mut probe, (0..20).map(|_| cmd(yaw, 0.0, 0)));
    assert_ne!(w.vars(s).flags & ent_flags::FL_ONGROUND, 0);
    let floor_z = w.vars(s).origin.z;

    // One jump, then standing still until landed.
    w.run_cmds(&mut probe, [cmd(yaw, 0.0, buttons::IN_JUMP)]);
    let mut airborne_frames = 0;
    let mut peak = floor_z;
    for _ in 0..150 {
        if w.vars(s).flags & ent_flags::FL_ONGROUND != 0 {
            break;
        }
        airborne_frames += 1;
        peak = peak.max(w.vars(s).origin.z);
        w.run_cmds(&mut probe, [cmd(yaw, 0.0, 0)]);
    }
    println!(
        "airborne {airborne_frames} frames, peak {:.3} above the floor",
        peak - floor_z
    );
    assert!(airborne_frames > 10, "airborne {airborne_frames} frames");
    assert_ne!(w.vars(s).flags & ent_flags::FL_ONGROUND, 0, "landed");
    assert!((w.vars(s).origin.z - floor_z).abs() < 1.0);

    // Holding duck.
    w.run_cmds(&mut probe, (0..30).map(|_| cmd(yaw, 0.0, buttons::IN_DUCK)));
    let v = w.vars(s);
    println!("ducked view_ofs {:?} origin {:?}", v.view_ofs, v.origin);
    assert_ne!(v.flags & ent_flags::FL_DUCKING, 0, "flags {:#x}", v.flags);
    assert_ne!(v.flags & ent_flags::FL_ONGROUND, 0, "flags {:#x}", v.flags);
    assert!(v.view_ofs.z < VEC_VIEW - 3.0, "view_ofs {:?}", v.view_ofs);
    assert_eq!(
        (v.mins, v.maxs),
        (Vec3::new(-16.0, -16.0, -18.0), Vec3::new(16.0, 16.0, 18.0))
    );

    // Releasing duck stands up again.
    w.run_cmds(&mut probe, (0..30).map(|_| cmd(yaw, 0.0, 0)));
    let v = w.vars(s);
    assert_eq!(v.flags & ent_flags::FL_DUCKING, 0, "flags {:#x}", v.flags);
    assert_eq!(
        (v.mins, v.maxs),
        (Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0))
    );
}
