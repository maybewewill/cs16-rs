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
use cs_sim::trace::{BspCollision, CONTENTS_SOLID, Hull};
use cs_sim::walk_game::{
    WALK_MAX_SPEED, WalkGame, WorldAnswer, WorldQuery, buttons, ent_flags,
};
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

/// Seconds of one frame.
const FRAME: f64 = 0.01;

/// Position tolerance of the project.
const TOLERANCE: f32 = 0.03125;

/// `Server<WalkGame>` on one map plus the loopback hub the probes connect through.
struct World {
    server: Server<WalkGame>,
    hub: LoopbackHub,
    now: f64,
    /// The legacy collision of the map, for the test's own traces.
    collision: BspCollision,
    /// The map's spawn points, as the map file parser lists them.
    spawns: Vec<SpawnPoint>,
}

/// A map as the engine takes it, with the legacy collision and spawn list of the same file.
fn load_map(vfs: &Vfs, name: &str) -> (ServerMap, BspCollision, Vec<SpawnPoint>, BspMap) {
    let bsp = BspMap::load(vfs, name).unwrap_or_else(|e| panic!("load {name}: {e:?}"));
    let raw = vfs
        .read_bytes(&format!("maps/{name}.bsp"))
        .unwrap_or_else(|e| panic!("read {name}: {e:?}"));
    let collision = BspCollision::from_map(&bsp, MaterialTable::load(vfs));
    let spawns = bsp.spawns.clone();
    let map = ServerMap::from_bsp(name, &raw, Arc::new(bsp.clone())).expect("BSP header");
    (map, collision, spawns, bsp)
}

impl World {
    fn dust2(vfs: &Vfs) -> Self {
        Self::on(vfs, "de_dust2")
    }

    fn on(vfs: &Vfs, name: &str) -> Self {
        let (map, collision, spawns, _) = load_map(vfs, name);
        let cfg = ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        };
        let server = Server::new(
            cfg,
            map,
            WalkGame::new(MoveParams::default()),
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
    clear_yaw_for(collision, start, spawn_yaw, CLEAR_RUN)
}

/// [`clear_yaw`] for a run of `run` units.
fn clear_yaw_for(collision: &BspCollision, start: Vec3, spawn_yaw: f32, run: f32) -> f32 {
    let from = start + Vec3::new(0.0, 0.0, 4.0);
    (0..8)
        .map(|k| (spawn_yaw + 45.0 * k as f32).rem_euclid(360.0))
        .find(|yaw| {
            let r = yaw.to_radians();
            let to = from + Vec3::new(r.cos(), r.sin(), 0.0) * run;
            let tr = collision.trace_hull(from, to, Hull::Standing);
            !tr.start_solid && !tr.all_solid && tr.fraction >= 1.0
        })
        .unwrap_or_else(|| {
            panic!("no clear {run}-unit run from {start:?} (spawn yaw {spawn_yaw})")
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

impl World {
    /// Switches the server to `name` and runs until `probe` has left the game on the
    /// `reconnect` and is in the game again.
    fn change_map(&mut self, vfs: &Vfs, name: &str, probe: &mut Probe) {
        let (map, collision, spawns, _) = load_map(vfs, name);
        self.server.change_map(map);
        self.collision = collision;
        self.spawns = spawns;
        let mut left = false;
        for _ in 0..600 {
            self.step(&mut [&mut *probe], 1);
            left |= probe.state() != &ProbeState::InGame;
            if left && probe.state() == &ProbeState::InGame {
                return;
            }
        }
        panic!("did not get back in the game on {name}: {:?}", probe.state());
    }
}

/// Number of spawns of `team`.
fn count(spawns: &[SpawnPoint], team: SpawnTeam) -> usize {
    spawns.iter().filter(|s| s.team == team).count()
}

#[test]
fn walk_game_follows_changelevel() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let mut w = World::dust2(&vfs);
    let mut probe = w.join("Traveller", &mut []);
    let yaw = w.vars(0).v_angle.y;
    w.run_cmds(&mut probe, (0..50).map(|_| cmd(yaw, WALK_MAX_SPEED, 0)));
    let dust_spawns = w.spawns.len();

    w.change_map(&vfs, "cs_office", &mut probe);
    w.step(&mut [&mut probe], 20);
    assert_eq!(slot(&probe), 0);
    // The game holds the spawns of the new map, none of the old one.
    let game_spawns = w.server.game().spawns().len();
    println!("de_dust2 {dust_spawns} spawns, cs_office {game_spawns}");
    assert_eq!(game_spawns, w.spawns.len());
    assert_ne!(game_spawns, dust_spawns);

    // Slot 0 stands on the first counter-terrorist spawn of cs_office.
    let first = w
        .spawns
        .iter()
        .find(|s| s.team == SpawnTeam::CounterTerrorist)
        .expect("a counter-terrorist spawn");
    let expected = snap_spawn_to_floor(first.position, &w.collision);
    let start = w.vars(0).origin;
    println!("cs_office spawn {start:?} expected {expected:?}");
    assert!(
        (start - expected).abs().max_element() <= 1.0,
        "{start:?} vs {expected:?}"
    );

    let yaw = w.vars(0).v_angle.y;
    w.run_cmds(&mut probe, (0..100).map(|_| cmd(yaw, WALK_MAX_SPEED, 0)));
    let v = w.vars(0);
    let floor = floor_distance(&w.collision, v.origin);
    println!("after 100 commands {:?} floor {floor:?}", v.origin);
    assert_ne!(v.flags & ent_flags::FL_ONGROUND, 0, "flags {:#x}", v.flags);
    assert!(floor.is_some_and(|f| f <= 1.0), "floor {floor:?}");
}

#[test]
fn spawns_come_from_the_map_entity_lump() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    for name in ["de_dust2", "cs_office"] {
        let w = World::on(&vfs, name);
        let game = w.server.game().spawns();
        println!(
            "{name}: game {} CT {} T, map {} CT {} T",
            count(game, SpawnTeam::CounterTerrorist),
            count(game, SpawnTeam::Terrorist),
            count(&w.spawns, SpawnTeam::CounterTerrorist),
            count(&w.spawns, SpawnTeam::Terrorist),
        );
        assert!(!w.spawns.is_empty(), "{name} has no spawns");
        for team in [SpawnTeam::CounterTerrorist, SpawnTeam::Terrorist] {
            assert_eq!(count(game, team), count(&w.spawns, team), "{name} {team:?}");
        }
        // Same points in the same order, with the yaw of the lump.
        assert_eq!(game.len(), w.spawns.len());
        for (g, m) in game.iter().zip(&w.spawns) {
            assert_eq!(
                (g.team, g.position, g.yaw_deg),
                (m.team, m.position, m.yaw_deg)
            );
        }
    }
}

/// Maps tried, in order, for a solid brush entity that stands in a clear run.
const BRUSH_MAPS: [&str; 12] = [
    "de_dust2",
    "cs_office",
    "de_dust",
    "cs_italy",
    "de_inferno",
    "de_nuke",
    "de_aztec",
    "cs_assault",
    "cs_militia",
    "de_train",
    "de_cbble",
    "cs_backalley",
];

/// A run that ends at a solid brush entity: the player stands on the floor some units from
/// the face of the brush, on a line along an axis that is clear but for the brush.
struct BrushRun {
    map: &'static str,
    classname: String,
    model_index: usize,
    start: Vec3,
    dir: Vec3,
    yaw: f32,
    /// Position along `dir` where the hull's centre stops against the brush.
    stop: f32,
}

/// The first run of [`BRUSH_MAPS`] found with the legacy collision: its trace along `dir` is
/// stopped by the brush entity, and clear of everything when that brush is disabled.
fn find_brush_run(vfs: &Vfs) -> Option<BrushRun> {
    const GAP: f32 = 60.0;
    for map in BRUSH_MAPS {
        let Ok(bsp) = BspMap::load(vfs, map) else {
            continue;
        };
        let collision = BspCollision::from_map(&bsp, MaterialTable::load(vfs));
        for b in &bsp.brush_entities {
            if !matches!(
                b.classname.as_str(),
                "func_wall" | "func_breakable" | "func_door" | "func_door_rotating"
            ) {
                continue;
            }
            let centre = (b.mins + b.maxs) * 0.5;
            let half = (b.maxs - b.mins) * 0.5;
            for (dir, yaw, half_along) in [
                (Vec3::X, 0.0, half.x),
                (Vec3::Y, 90.0, half.y),
                (-Vec3::X, 180.0, half.x),
                (-Vec3::Y, 270.0, half.y),
            ] {
                // The hull's centre stops 16 units before the face.
                let stop = dir.dot(centre) - half_along - 16.0;
                let lateral = centre - dir * dir.dot(centre);
                // Floor under the run's first point, found from just above the brush's base.
                let above = Vec3::new(lateral.x, lateral.y, b.mins.z + 40.0) + dir * (stop - GAP);
                let down = collision.trace_hull(above, above - Vec3::new(0.0, 0.0, 128.0), Hull::Standing);
                if down.start_solid || down.fraction >= 1.0 {
                    continue;
                }
                let start = down.end_pos + Vec3::new(0.0, 0.0, 0.5);
                if collision.point_contents(start, Hull::Standing) == CONTENTS_SOLID {
                    continue;
                }
                let end = start + dir * (GAP + 16.0 + half_along * 2.0 + 64.0);
                let blocked = collision.trace_hull(start, end, Hull::Standing);
                let mut open = collision.clone();
                open.disabled_models.insert(b.model_index);
                let free = open.trace_hull(start, end, Hull::Standing);
                let reaches = blocked.fraction < 1.0
                    && (dir.dot(blocked.end_pos) - stop).abs() < 0.5
                    && !blocked.start_solid;
                if reaches && free.fraction >= 1.0 && !free.start_solid {
                    return Some(BrushRun {
                        map,
                        classname: b.classname.clone(),
                        model_index: b.model_index,
                        start,
                        dir,
                        yaw,
                        stop: dir.dot(blocked.end_pos),
                    });
                }
            }
        }
    }
    None
}

#[test]
fn solid_brush_entities_block_movement() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let run = find_brush_run(&vfs).expect("no solid brush entity with a clear run in the maps");
    assert_eq!(
        (run.map, run.classname.as_str()),
        ("de_dust2", "func_breakable")
    );
    println!(
        "map {} entity {} *{} start {:?} dir {:?} stop {}",
        run.map, run.classname, run.model_index, run.start, run.dir, run.stop
    );
    let mut w = World::on(&vfs, run.map);

    // The brush is an entity of the engine: solid, a pusher at rest, linked; the player's
    // movement trace hits it.
    let model = format!("*{}", run.model_index);
    let (brush, brush_vars) = (0..2048)
        .filter_map(|i| w.server.edicts().by_index(i))
        .find(|(_, e)| e.vars.classname == run.classname && e.vars.model == model)
        .map(|(id, e)| (id, e.vars.clone()))
        .expect("the brush entity is in the engine");
    // SOLID_BSP, MOVETYPE_PUSH.
    assert_eq!((brush_vars.solid, brush_vars.movetype), (4, 7));
    assert_eq!(brush_vars.velocity, Vec3::ZERO);
    assert!(
        brush_vars.modelindex > 0 && brush_vars.absmin.cmplt(brush_vars.absmax).all(),
        "{brush_vars:?}"
    );

    let mut probe = w.join("Walker", &mut []);
    assert!(w.server.game_mut().set_player_origin(0, run.start));
    w.run_cmds(&mut probe, (0..150).map(|_| cmd(run.yaw, WALK_MAX_SPEED, 0)));
    let v = w.vars(0);
    let along = run.dir.dot(v.origin);
    println!("ended {:?}, {along} along the run, stop {}", v.origin, run.stop);
    // Reached the brush ...
    assert!(along >= run.stop - 2.0, "stopped short: {along} vs {}", run.stop);
    // ... and was stopped by it.
    assert!(along <= run.stop + 0.1, "went into the brush: {along} vs {}", run.stop);

    // The movement's touches reached the engine: the brush entity is still the same live one,
    // and the game was told the player and the brush touched (the brush's half comes first).
    assert!(w.server.edicts().get(brush).is_some());
    let player = w.server.edicts().client(0);
    let touches = w.server.game().touches();
    assert!(touches.contains(&(brush, player)), "{touches:?}");
    assert!(touches.contains(&(player, brush)), "{touches:?}");
}

#[test]
fn ladder_check_matches_the_legacy_overlap_test() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    // cs_office has func_ladder entities.
    let (_, _, _, bsp) = load_map(&vfs, "cs_office");
    let ladders: Vec<_> = bsp
        .brush_entities
        .iter()
        .filter(|b| b.classname == "func_ladder")
        .collect();
    assert!(!ladders.is_empty(), "cs_office has no func_ladder");
    let w = World::on(&vfs, "cs_office");
    let (mut on, mut off) = (0, 0);
    for ladder in &ladders {
        for hull in [Hull::Standing, Hull::Crouched] {
            // The box edges of the overlap test: just inside and just outside each face.
            let ext = hull.half_extents() + Vec3::new(3.0, 3.0, 0.0);
            let axis = |lo: f32, hi: f32, e: f32| {
                [(lo + hi) * 0.5, lo - e - 0.5, lo - e + 0.5, hi + e - 0.5, hi + e + 0.5]
            };
            for x in axis(ladder.mins.x, ladder.maxs.x, ext.x) {
                for y in axis(ladder.mins.y, ladder.maxs.y, ext.y) {
                    for z in axis(ladder.mins.z, ladder.maxs.z, ext.z) {
                        let pos = Vec3::new(x, y, z);
                        let legacy = w.collision.is_on_ladder(pos, hull);
                        assert_eq!(
                            w.server.game().is_on_ladder(pos, hull),
                            legacy,
                            "{pos:?} {hull:?}"
                        );
                        if legacy {
                            on += 1;
                        } else {
                            off += 1;
                        }
                    }
                }
            }
        }
    }
    println!("{} ladders, {on} points on, {off} off", ladders.len());
    assert!(on > 0 && off > 0);
}

/// A point inside the first `func_breakable` of de_dust2 whose centre is solid only because of
/// the brush, found with the legacy collision, and the brush's model number.
fn point_inside_breakable(bsp: &BspMap, collision: &BspCollision) -> (Vec3, usize) {
    for b in bsp.brush_entities.iter().filter(|b| b.classname == "func_breakable") {
        let mut open = collision.clone();
        open.disabled_models.insert(b.model_index);
        let c = (b.mins + b.maxs) * 0.5;
        if collision.point_contents(c, Hull::Standing) == CONTENTS_SOLID
            && open.point_contents(c, Hull::Standing) != CONTENTS_SOLID
        {
            return (c, b.model_index);
        }
    }
    panic!("no func_breakable on de_dust2 with a centre outside the world's solid");
}

#[test]
fn adapter_mirrors_legacy_inside_a_solid_brush() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let mut w = World::dust2(&vfs);
    let (_, _, _, bsp) = load_map(&vfs, "de_dust2");
    let (inside, model) = point_inside_breakable(&bsp, &w.collision);
    println!("point {inside:?} in *{model}");
    let outside = inside + Vec3::new(0.0, 0.0, 300.0);
    let cases = [
        // Starts inside the brush and stays in it.
        (inside, inside + Vec3::new(2.0, 0.0, 0.0)),
        // Starts inside the brush and leaves it.
        (inside, outside),
        // Starts outside and enters.
        (outside, inside),
    ];
    let hulls = [Hull::Standing, Hull::Crouched, Hull::Point];
    for (start, end) in cases {
        for hull in hulls {
            w.server
                .game_mut()
                .query_world(WorldQuery::Trace { start, end, hull });
        }
    }
    w.server.game_mut().query_world(WorldQuery::Contents {
        point: inside,
        hull: Hull::Standing,
    });
    w.server.game_mut().query_world(WorldQuery::Snap(inside));
    w.step(&mut [], 1);
    let mut answers = w.server.game_mut().take_answers().into_iter();
    let mut started_solid = 0;
    for (start, end) in cases {
        for hull in hulls {
            let Some(WorldAnswer::Trace(got)) = answers.next() else {
                panic!("missing trace answer");
            };
            let want = w.collision.trace_hull(start, end, hull);
            let ctx = format!("{start:?} -> {end:?} {hull:?}: {got:?} vs {want:?}");
            assert_eq!(got.start_solid, want.start_solid, "{ctx}");
            assert_eq!(got.all_solid, want.all_solid, "{ctx}");
            assert!((got.fraction - want.fraction).abs() <= 0.001, "{ctx}");
            assert!(
                (got.end_pos - want.end_pos).abs().max_element() <= 0.1,
                "{ctx}"
            );
            started_solid += usize::from(want.start_solid);
        }
    }
    assert!(started_solid > 0, "no case started inside solid");
    let Some(WorldAnswer::Contents(c)) = answers.next() else {
        panic!("missing contents answer");
    };
    assert_eq!(c, w.collision.point_contents(inside, Hull::Standing));
    assert_eq!(c, CONTENTS_SOLID);
    // `snap_spawn_to_floor` from a point inside the brush gives the same through both.
    let Some(WorldAnswer::Snap(snapped)) = answers.next() else {
        panic!("missing snap answer");
    };
    let want = snap_spawn_to_floor(inside, &w.collision);
    println!("snap {snapped:?} legacy {want:?}");
    assert!(
        (snapped - want).abs().max_element() <= 0.1,
        "{snapped:?} vs {want:?}"
    );
}

/// Origin on the floor under the standing hull at the (x, y) of `at`, found with the legacy
/// collision from 40 units above `at.z`.
fn on_floor(collision: &BspCollision, at: Vec3) -> Vec3 {
    let above = at + Vec3::new(0.0, 0.0, 40.0);
    let down = collision.trace_hull(above, above - Vec3::new(0.0, 0.0, 128.0), Hull::Standing);
    assert!(!down.start_solid && down.fraction < 1.0, "no floor under {at:?}");
    down.end_pos + Vec3::new(0.0, 0.0, 0.5)
}

/// Queues an idle command on each probe (so its edict follows its movement state) and runs one
/// frame, `times` times.
fn idle(w: &mut World, probes: &mut [&mut Probe], times: usize) {
    for _ in 0..times {
        for p in probes.iter_mut() {
            p.queue_cmd(cmd(0.0, 0.0, 0));
        }
        w.step(probes, 1);
    }
}

/// Two joined players, A in slot 0 and B in slot 1, and the clear run of B's spawn: B's origin,
/// the unit direction of the run and its yaw.
fn two_players_and_a_run(vfs: &Vfs) -> (World, Probe, Probe, Vec3, Vec3, f32) {
    let mut w = World::dust2(vfs);
    let mut a = w.join("A", &mut []);
    let b = w.join("B", &mut [&mut a]);
    let start = w.vars(slot(&b)).origin;
    let yaw = clear_yaw(&w.collision, start, w.vars(slot(&b)).v_angle.y);
    let r = yaw.to_radians();
    (w, a, b, start, Vec3::new(r.cos(), r.sin(), 0.0), yaw)
}

#[test]
fn players_block_each_other() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let (mut w, mut a, mut b, start, dir, yaw) = two_players_and_a_run(&vfs);
    // B at its spawn looking along the run, A 100 units ahead of it looking back.
    let a_origin = on_floor(&w.collision, start + dir * 100.0);
    assert!(w.server.game_mut().set_player_origin(slot(&a), a_origin));
    assert!(w.server.game_mut().set_player_origin(slot(&b), start));
    idle(&mut w, &mut [&mut a, &mut b], 20);
    let (sa, sb) = (slot(&a), slot(&b));
    let gap = |w: &World| (w.vars(sa).origin - w.vars(sb).origin).length();
    assert!(gap(&w) > 90.0, "{}", gap(&w));

    // Both walk at each other for a second and a half: 375 units each, far past the gap.
    let mut closest = f32::MAX;
    for _ in 0..150 {
        a.queue_cmd(cmd(yaw + 180.0, WALK_MAX_SPEED, 0));
        b.queue_cmd(cmd(yaw, WALK_MAX_SPEED, 0));
        w.step(&mut [&mut a, &mut b], 1);
        closest = closest.min(gap(&w));
    }
    println!("closest {closest}, last {}", gap(&w));
    // The boxes are 32 wide: players never get closer than one body width.
    assert!(closest >= 32.0 - TOLERANCE, "walked through each other: {closest}");
    assert!(closest < 40.0, "never met: {closest}");
}

#[test]
fn a_walking_player_stays_visible_across_leaves() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    // The observer (slot 0) stands at its counter-terrorist spawn; the runner (slot 1) starts
    // at its terrorist spawn, far away and out of the observer's sight.
    let mut w = World::dust2(&vfs);
    let mut observer = w.join("Observer", &mut []);
    let mut runner = w.join("Runner", &mut [&mut observer]);
    let (so, sr) = (slot(&observer), slot(&runner));
    let runner_entity = u16::from(sr) + 1;
    w.step(&mut [&mut observer, &mut runner], 20);
    assert!(
        observer.entities().get(&runner_entity).is_none(),
        "the runner's spawn was in sight of the observer's"
    );

    // Puts the runner 100 units from the observer along a clear run, in the observer's sight,
    // and walks it away for a second, 250 units.
    let seat = w.vars(so).origin;
    let yaw = clear_yaw_for(&w.collision, seat, w.vars(so).v_angle.y, 300.0);
    let r = yaw.to_radians();
    let dir = Vec3::new(r.cos(), r.sin(), 0.0);
    let from = on_floor(&w.collision, seat + dir * 100.0);
    assert!(w.server.game_mut().set_player_origin(sr, from));
    for _ in 0..100 {
        runner.queue_cmd(cmd(yaw, WALK_MAX_SPEED, 0));
        observer.queue_cmd(cmd(0.0, 0.0, 0));
        w.step(&mut [&mut observer, &mut runner], 1);
    }
    // Lets the runner come to rest and the last update reach the observer.
    idle(&mut w, &mut [&mut observer, &mut runner], 80);
    let actual = w.vars(sr).origin;
    println!("runner ended at {actual:?}, observer at {seat:?}");
    let seen = observer
        .entities()
        .get(&runner_entity)
        .expect("the observer cannot see the runner that walked up to it");
    assert!(
        (seen.origin - actual).abs().max_element() <= TOLERANCE,
        "{:?} vs {actual:?}",
        seen.origin
    );
}

#[test]
fn legacy_numbers_unchanged() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    // A lone player on the engine's entity traces ends a run where the movement code on the
    // legacy collision alone ends it, within the project tolerance. (The probe delivers a
    // command a frame late now and then, so only the end of the run is compared, after the
    // late commands have run.)
    let mut w = World::dust2(&vfs);
    let mut keeper = w.join("Keeper", &mut []);
    let mut probe = w.join("Runner", &mut [&mut keeper]);
    let s = slot(&probe);
    let start = w.vars(s).origin;
    let yaw = clear_yaw(&w.collision, start, w.vars(s).v_angle.y);
    let mut legacy = cs_sim::pmove::PlayerMoveState::at_spawn(start);
    let params = MoveParams::default();
    for _ in 0..200 {
        probe.queue_cmd(cmd(yaw, WALK_MAX_SPEED, 0));
        w.step(&mut [&mut keeper, &mut probe], 1);
        // The view angle crosses the wire quantized: the legacy run uses the one the server got.
        let received = cmd(w.vars(s).v_angle.y, WALK_MAX_SPEED, 0);
        let input = cs_sim::walk_game::usercmd_to_move_input(&received, WALK_MAX_SPEED);
        cs_sim::pmove::step_player_move(&mut legacy, &input, WALK_MAX_SPEED, &params, &w.collision, 0.01);
    }
    w.step(&mut [&mut keeper, &mut probe], 5);
    let (engine, expected) = (w.vars(s).origin, legacy.origin);
    println!("engine {engine:?} legacy {expected:?}");
    assert!((engine - expected).abs().max_element() <= TOLERANCE, "{engine:?} vs {expected:?}");
}

/// Players are solid: a player who joins must not spawn inside another. Slot 0 stands on the
/// spawn slot 2 would get; slot 2 takes the first free spawn of the team instead.
#[test]
fn a_player_does_not_spawn_inside_another() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let mut w = World::dust2(&vfs);
    let ct: Vec<SpawnPoint> = w
        .spawns
        .iter()
        .filter(|s| s.team == SpawnTeam::CounterTerrorist)
        .cloned()
        .collect();
    assert!(ct.len() >= 2, "de_dust2 has several CT spawns");
    let mut a = w.join("A", &mut []);
    // A stands on the second CT spawn, where slot 2 would spawn by its index.
    let on_second = snap_spawn_to_floor(ct[1].position, &w.collision);
    assert!(w.server.game_mut().set_player_origin(slot(&a), on_second));
    idle(&mut w, &mut [&mut a], 5);
    assert!((w.vars(0).origin - on_second).length() < 1.0);

    let mut b = w.join("B", &mut [&mut a]);
    let mut c = w.join("C", &mut [&mut a, &mut b]);
    w.step(&mut [&mut a, &mut b, &mut c], 5);
    assert_eq!(slot(&c), 2);
    let gap = (w.vars(0).origin - w.vars(2).origin).length();
    assert!(gap >= 32.0, "C spawned inside A: {gap}");
    // The first CT spawn was free: C takes it.
    let first = snap_spawn_to_floor(ct[0].position, &w.collision);
    assert!(
        (w.vars(2).origin - first).abs().max_element() <= TOLERANCE,
        "{:?} vs {first:?}",
        w.vars(2).origin
    );
}
