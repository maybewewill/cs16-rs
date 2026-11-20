//! `Server<WalkGame>` on de_dust2 with `cs-probe` clients over the loopback. Every test returns
//! without assertions when the installed game is absent.

use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_assets::materials::MaterialTable;
use cs_cvar::CVarRegistry;
use cs_engine::{EntVars, Server, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcUserCmd};
use cs_proto48::info::InfoString;
use cs_sim::pmove::{MoveParams, VEC_VIEW};
use cs_sim::trace::BspCollision;
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
}

impl World {
    fn dust2(vfs: &Vfs) -> Self {
        let bsp = BspMap::load(vfs, "de_dust2").expect("load de_dust2");
        let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("read de_dust2");
        let collision = BspCollision::from_map(&bsp, MaterialTable::load(vfs));
        let spawns = bsp.spawns.clone();
        let map = ServerMap::from_bsp("de_dust2", &raw, Arc::new(bsp)).expect("BSP header");
        let game = WalkGame::new(collision, spawns, MoveParams::default());
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

/// Joins one probe and runs 200 commands of full forward speed along the spawn's yaw.
/// Returns the world, the probe and the edict origin before the run.
fn run_forward(vfs: &Vfs) -> (World, Probe, Vec3) {
    let mut w = World::dust2(vfs);
    let mut probe = w.join("Runner", &mut []);
    let s = slot(&probe);
    let yaw = w.vars(s).v_angle.y;
    let start = w.vars(s).origin;
    w.run_cmds(&mut probe, (0..200).map(|_| cmd(yaw, WALK_MAX_SPEED, 0)));
    // Lets the last update reach the probe.
    w.step(&mut [&mut probe], 10);
    (w, probe, start)
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
    let (w, probe, start) = run_forward(&vfs);
    let v = w.vars(slot(&probe));
    let run = v.origin - start;
    let horizontal = Vec3::new(run.x, run.y, 0.0).length();
    println!(
        "yaw {} start {start:?} end {:?} velocity {:?} horizontal {horizontal:.3} dz {:.3}",
        v.v_angle.y, v.origin, v.velocity, run.z
    );
    assert!((400.0..=520.0).contains(&horizontal), "ran {horizontal}");
    assert!(run.z.abs() <= 18.0, "dz {}", run.z);
    assert_ne!(v.flags & ent_flags::FL_ONGROUND, 0, "flags {:#x}", v.flags);
    assert_ne!(v.flags & ent_flags::FL_CLIENT, 0, "flags {:#x}", v.flags);
}

#[test]
fn clientdata_matches_server_origin() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let (w, probe, start) = run_forward(&vfs);
    let server = w.vars(slot(&probe)).origin;
    let client = probe.client_data().expect("client data").origin;
    println!("server {server:?} client {client:?}");
    assert!((server - start).length() > 100.0, "the player moved");
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
