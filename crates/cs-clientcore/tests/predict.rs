//! Movement prediction against our own server: `cs-engine` and `cs-game` in this process, over
//! a loopback, on a synthetic map (floor, a step, a wall, a block brush entity).

mod predict_support;

use cs_assets::bsp::Entity;
use cs_clientcore::{PredictInputs, Predictor, split_usercmd};
use cs_proto48::delta::GoldSrcUserCmd;
use glam::Vec3;
use predict_support::*;

/// How far the predicted origin may be from the server's (units).
const TOLERANCE: f32 = 0.02;

/// The commands of the walk: forward, strafing, jumping, ducking, over the step, against the
/// wall, along it and back.
fn script(i: usize) -> GoldSrcUserCmd {
    let yaw = match i {
        0..=249 => 0.0,
        250..=319 => 40.0,
        320..=399 => 180.0 + 15.0 * (i as f32 / 9.0).sin(),
        _ => 90.0 + 30.0 * (i as f32 / 25.0).sin(),
    };
    let mut buttons = IN_FORWARD;
    // Jumps (held for a few commands): over the step first, then now and then.
    if (85..90).contains(&i) || (i >= 150 && i % 57 == 30) {
        buttons |= IN_JUMP;
    }
    if (280..300).contains(&i) || (420..450).contains(&i) {
        buttons |= IN_DUCK;
    }
    if i % 97 == 50 {
        buttons |= IN_JUMP | IN_DUCK;
    }
    let side = if i < 250 {
        0.0
    } else if i % 60 < 15 {
        250.0
    } else if i % 60 < 30 {
        -250.0
    } else {
        0.0
    };
    let forward = if (60..64).contains(&i) { 0.0 } else { 250.0 };
    walk(10, buttons, forward, side, yaw)
}

#[test]
fn prediction_matches_the_server_on_loopback() {
    let (map, world) = server_map(map_entities(None));
    let mut l = Loop::new(map, world);
    let start = l.server_origin();
    let first_check = l.conn.predictor().checks().len();

    let mut highest = start.z;
    let mut farthest = 0.0f32;
    for i in 0..500 {
        l.step(Some(script(i)));
        let p = l.conn.predictor().prediction();
        assert!(p.predicted, "command {i} is predicted");
        highest = highest.max(p.simorg.z);
        farthest = farthest.max(p.simorg.x);
    }
    // Let the last commands come back.
    for _ in 0..5 {
        l.step(None);
    }

    let checks: Vec<_> = l
        .conn
        .predictor()
        .checks()
        .iter()
        .skip(first_check)
        .copied()
        .collect();
    assert!(
        checks.len() >= 250,
        "the server answered nearly every command ({} checks)",
        checks.len()
    );
    let worst = checks.iter().map(|c| c.error).fold(0.0f32, f32::max);
    assert!(
        checks.iter().all(|c| !c.teleport),
        "no check saw a teleport"
    );
    // The server's origin reaches the client as a 1/128 fixed-point number and its velocity as
    // 1/8 (the delta tables of `clientdata_t`), so the replay starts a little off: the limit
    // is what that rounding costs over a few commands (the original client has the same).
    assert!(
        worst < TOLERANCE,
        "predicted origin vs the server's, worst of {} commands: {worst}",
        checks.len()
    );
    // The walk really covered the map: jumped, climbed the step, hit the wall.
    assert!(highest > start.z + 20.0, "jumped ({highest} from {})", start.z);
    assert!(farthest > STEP_X.1, "went over the step ({farthest})");
    assert!(
        farthest > WALL_X - 20.0 && farthest < WALL_X - 15.9,
        "the wall stopped it ({farthest} of {WALL_X})"
    );
    // The final predicted origin is where the server has the player.
    let server = l.server_origin();
    let predicted = l.conn.predictor().prediction().simorg;
    assert!(
        (server - predicted).length() < TOLERANCE,
        "{server:?} vs {predicted:?}"
    );
}

#[test]
fn long_commands_are_split() {
    // The halves: 100 ms is two of 50, the impulse only on the first.
    let long = GoldSrcUserCmd {
        msec: 100,
        impulse: 9,
        ..GoldSrcUserCmd::default()
    };
    let halves = split_usercmd(&long);
    assert_eq!(halves.iter().map(|c| c.msec).collect::<Vec<_>>(), [50, 50]);
    assert_eq!(halves.iter().map(|c| c.impulse).collect::<Vec<_>>(), [9, 0]);
    // The same split as the server's, for every length.
    for msec in 0..=255u8 {
        let c = GoldSrcUserCmd {
            msec,
            impulse: 3,
            ..GoldSrcUserCmd::default()
        };
        let ours: Vec<_> = split_usercmd(&c).iter().map(|c| (c.msec, c.impulse)).collect();
        let server: Vec<_> = cs_engine::usercmd::split_usercmd(&c)
            .iter()
            .map(|c| (c.msec, c.impulse))
            .collect();
        assert_eq!(ours, server, "msec {msec}");
    }

    // Through the loopback: a 100 ms command runs as two steps and lands where the server does.
    let (map, world) = server_map(map_entities(None));
    let mut l = Loop::new(map, world);
    l.conn.predictor_mut().record_steps(true);
    // The server gives a command no more time than the real time that passed (its budget): let
    // 150 ms of it bank before the long command comes.
    for _ in 0..15 {
        l.step(None);
    }
    let first_check = l.conn.predictor().checks().len();
    l.step(Some(walk(100, IN_FORWARD, 250.0, 0.0, 0.0)));
    for _ in 0..4 {
        l.step(None);
    }
    let steps = l.conn.predictor_mut().take_steps();
    assert!(
        steps.windows(2).any(|w| w == [(50, 0), (50, 0)]),
        "the 100 ms command ran as two halves: {steps:?}"
    );
    let worst = l
        .conn
        .predictor()
        .checks()
        .iter()
        .skip(first_check)
        .map(|c| c.error)
        .fold(0.0f32, f32::max);
    assert!(worst < TOLERANCE, "split prediction vs the server: {worst}");
    assert!(l.server_origin().x > 5.0, "the player moved");
}

#[test]
fn prediction_uses_brush_entities() {
    // A `func_wall` block in the way, 100 units ahead: the server stops the player in front of
    // it, and so does the prediction, which has to take the block from the entity list.
    let block = Vec3::new(100.0, 0.0, 32.0);
    let (map, world) = server_map(map_entities(Some(block)));
    let mut l = Loop::new(map, world);
    // The block is in the frame the client holds.
    for _ in 0..10 {
        l.step(Some(idle(10)));
    }
    let first_check = l.conn.predictor().checks().len();
    for _ in 0..150 {
        l.step(Some(walk(10, IN_FORWARD, 250.0, 0.0, 0.0)));
    }
    for _ in 0..5 {
        l.step(None);
    }
    let predicted = l.conn.predictor().prediction().simorg;
    // The block's face is at 100 - 16 = 84; the player's hull is 16 wide.
    assert!(
        predicted.x > 60.0 && predicted.x < 68.5,
        "stopped by the block: {predicted:?}"
    );
    let worst = l
        .conn
        .predictor()
        .checks()
        .iter()
        .skip(first_check)
        .map(|c| c.error)
        .fold(0.0f32, f32::max);
    assert!(worst < TOLERANCE, "against the server's: {worst}");
    assert!((l.server_origin() - predicted).length() < TOLERANCE);
}

/// Without the block in the world's entity list the same walk passes through where the block
/// would be: the stop above is the entity's.
#[test]
fn without_the_block_the_walk_goes_on() {
    let (map, world) = server_map(map_entities(None));
    let mut l = Loop::new(map, world);
    for _ in 0..150 {
        l.step(Some(walk(10, IN_FORWARD, 250.0, 0.0, 0.0)));
    }
    assert!(l.conn.predictor().prediction().simorg.x > 100.0);
}

// ---- the predictor on its own: what the server cannot be made to do ------------------------

mod bare {
    use super::*;
    use cs_clientcore::{Frame, PlayerPose, PrecacheTables};
    use cs_clientcore::frames::FrameEntity;
    use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState};
    use cs_proto48::messages::ResourceEntry;
    use cs_pm::collision::WorldCollision;
    use cs_pm::pm::MoveVars;
    use std::sync::Arc;

    pub fn precache() -> PrecacheTables {
        let mut t = PrecacheTables::default();
        t.add(&[ResourceEntry {
            res_type: 2,
            filename: "*1".to_string(),
            index: 2,
            download_size: 0,
            flags: 0,
            md5: None,
            reserved: None,
        }]);
        t
    }

    pub fn world() -> Arc<WorldCollision> {
        Arc::new(WorldCollision::from_bsp(&synthetic_bsp(Vec::<Entity>::new())))
    }

    pub fn local_state(number: u16) -> GoldSrcEntityState {
        GoldSrcEntityState {
            number,
            modelindex: 3,
            movetype: 3,
            solid: 3,
            friction: 1.0,
            gravity: 1.0,
            mins: Vec3::new(-16.0, -16.0, -36.0),
            maxs: Vec3::new(16.0, 16.0, 36.0),
            ..GoldSrcEntityState::default()
        }
    }

    pub fn block_state(number: u16, origin: Vec3) -> GoldSrcEntityState {
        GoldSrcEntityState {
            number,
            modelindex: 2,
            movetype: 7,
            solid: 4,
            origin,
            mins: -BLOCK_HALF,
            maxs: BLOCK_HALF,
            ..GoldSrcEntityState::default()
        }
    }

    pub fn frame(sequence: u32, origin: Vec3, entities: Vec<GoldSrcEntityState>) -> Frame {
        Frame {
            sequence,
            valid: true,
            time: 10.0,
            received_time: 10.0,
            entities: entities
                .into_iter()
                .map(|state| FrameEntity {
                    state,
                    is_new: false,
                })
                .collect(),
            clientdata: Some(GoldSrcClientData {
                origin,
                view_ofs: Vec3::new(0.0, 0.0, 17.0),
                maxspeed: 250.0,
                health: 100.0,
                flags: 1 << 9,
                ..GoldSrcClientData::default()
            }),
            weapon_data: Default::default(),
        }
    }

    pub fn inputs<'a>(
        now: f64,
        ack: u32,
        outgoing: u32,
        frame: Option<&'a Frame>,
        precache: &'a PrecacheTables,
        poses: &'a [PlayerPose],
        remote_is_local: bool,
    ) -> PredictInputs<'a> {
        PredictInputs {
            now,
            incoming_acknowledged: ack,
            outgoing_sequence: outgoing,
            frame,
            playernum: 0,
            max_clients: 4,
            precache,
            movevars: MoveVars::default(),
            max_velocity: 2000.0,
            poses,
            entities: None,
            remote_is_local,
            intermission: false,
        }
    }

    /// A predictor with commands `1..=n` sent in packets `1..=n`, run once from `frame` 1.
    pub fn run_walk(
        p: &mut Predictor,
        n: u32,
        frame: &Frame,
        precache: &PrecacheTables,
        remote_is_local: bool,
    ) {
        for seq in 1..=n {
            p.set_now(f64::from(seq) * 0.01);
            p.push_cmd(walk(10, IN_FORWARD, 250.0, 0.0, 0.0));
            p.packet_sent(seq, 1, None);
        }
        let now = f64::from(n) * 0.01;
        p.update(&inputs(now, 0, n + 1, Some(frame), precache, &[], remote_is_local));
    }
}

#[test]
fn a_teleport_snaps() {
    let precache = bare::precache();
    let spawn = Vec3::new(0.0, 0.0, 36.03125);
    let frame1 = bare::frame(1, spawn, vec![bare::local_state(1)]);

    // A small miss: the server says 3 units further than the client predicted. The correction
    // is smoothed (a remote server), so the view does not jump.
    let mut p = Predictor::new();
    p.set_world(Some(bare::world()));
    bare::run_walk(&mut p, 10, &frame1, &precache, false);
    let predicted_end = p.predicted_origin(10).expect("predicted");
    assert!(predicted_end.x > 5.0, "walked: {predicted_end:?}");
    // The server's frame 2 acknowledges packet 10 and puts the player 3 units off.
    let off = predicted_end + Vec3::new(3.0, 0.0, 0.0);
    let frame2 = bare::frame(2, off, vec![bare::local_state(1)]);
    p.set_now(0.11);
    p.update(&bare::inputs(0.11, 10, 11, Some(&frame2), &precache, &[], false));
    let check = *p.checks().back().expect("checked");
    assert_eq!(check.ack, 10);
    assert!((check.error - 3.0).abs() < 1e-3 && !check.teleport);
    assert!((p.prediction().prediction_error.x - 3.0).abs() < 1e-3);
    // Nothing is in flight: the view is the server's frame.
    assert!(!p.prediction().predicted);

    // A teleport: the server puts the player 500 units away. No error is kept and the view
    // takes the server's place at once.
    let mut p = Predictor::new();
    p.set_world(Some(bare::world()));
    bare::run_walk(&mut p, 10, &frame1, &precache, false);
    let predicted_end = p.predicted_origin(10).expect("predicted");
    let far = predicted_end + Vec3::new(0.0, 300.0, 0.0);
    let frame2 = bare::frame(2, far, vec![bare::local_state(1)]);
    // One more command in flight, so the prediction runs from the new place.
    p.set_now(0.11);
    p.push_cmd(walk(10, IN_FORWARD, 250.0, 0.0, 0.0));
    p.packet_sent(11, 1, None);
    p.update(&bare::inputs(0.11, 10, 12, Some(&frame2), &precache, &[], false));
    let check = *p.checks().back().expect("checked");
    assert!(check.teleport, "{check:?}");
    assert_eq!(p.prediction().prediction_error, Vec3::ZERO);
    let now = p.prediction().simorg;
    assert!(
        (now.y - far.y).abs() < 1.0,
        "the view is at the teleport target, not smoothed from the old place: {now:?} vs {far:?}"
    );
}

#[test]
fn a_small_error_is_smoothed_on_a_remote_server_and_not_on_a_local_one() {
    let precache = bare::precache();
    let spawn = Vec3::new(0.0, 0.0, 36.03125);
    let frame1 = bare::frame(1, spawn, vec![bare::local_state(1)]);

    for (local, smoothed) in [(true, false), (false, true)] {
        let mut p = Predictor::new();
        p.set_world(Some(bare::world()));
        bare::run_walk(&mut p, 10, &frame1, &precache, local);
        let seen = p.prediction().simorg;
        let end = p.predicted_origin(10).unwrap();
        // The server disagrees by 2 units sideways; one more command is in flight.
        let frame2 = bare::frame(2, end + Vec3::new(0.0, 2.0, 0.0), vec![bare::local_state(1)]);
        p.set_now(0.11);
        p.push_cmd(walk(10, IN_FORWARD, 250.0, 0.0, 0.0));
        p.packet_sent(11, 1, None);
        p.update(&bare::inputs(0.11, 10, 12, Some(&frame2), &precache, &[], local));
        let view = p.prediction().simorg;
        // Unsmoothed, the view is on the server's line (y = 2); smoothed it has moved only a
        // part of the way from where it was (y = 0) towards it.
        if smoothed {
            assert!(view.y < 1.0, "smoothed: {view:?} (was {seen:?})");
        } else {
            assert!((view.y - 2.0).abs() < 0.5, "not smoothed: {view:?}");
        }
    }
}

#[test]
fn a_solid_entity_of_the_frame_is_in_the_way() {
    let precache = bare::precache();
    let spawn = Vec3::new(0.0, 0.0, 36.03125);
    let walk_to = |entities: Vec<cs_proto48::delta::GoldSrcEntityState>| {
        let frame = bare::frame(1, spawn, entities);
        let mut p = Predictor::new();
        p.set_world(Some(bare::world()));
        bare::run_walk(&mut p, 60, &frame, &precache, true);
        p.prediction().simorg
    };
    let open = walk_to(vec![bare::local_state(1)]);
    let blocked = walk_to(vec![
        bare::local_state(1),
        bare::block_state(7, Vec3::new(100.0, 0.0, 32.0)),
    ]);
    assert!(open.x > 100.0, "{open:?}");
    assert!(blocked.x < 68.5 && blocked.x > 60.0, "{blocked:?}");
    // The owner's own entity is not in its way (`owner == playernum + 1`).
    let mut own = bare::block_state(7, Vec3::new(100.0, 0.0, 32.0));
    own.owner = 1;
    let passed = walk_to(vec![bare::local_state(1), own]);
    assert!(passed.x > 100.0, "{passed:?}");
    // Another player collides where it is drawn.
    let mut other = bare::local_state(2);
    other.origin = Vec3::new(100.0, 0.0, 36.03125);
    let frame = bare::frame(1, spawn, vec![bare::local_state(1), other]);
    let poses = [cs_clientcore::PlayerPose {
        number: 2,
        origin: Vec3::new(100.0, 0.0, 36.03125),
        angles: Vec3::ZERO,
    }];
    let mut p = Predictor::new();
    p.set_world(Some(bare::world()));
    for seq in 1..=60u32 {
        p.set_now(f64::from(seq) * 0.01);
        p.push_cmd(walk(10, IN_FORWARD, 250.0, 0.0, 0.0));
        p.packet_sent(seq, 1, None);
    }
    p.update(&bare::inputs(0.6, 0, 61, Some(&frame), &precache, &poses, true));
    let x = p.prediction().simorg.x;
    assert!(x < 68.5 && x > 60.0, "stopped by the other player: {x}");
}

/// The same walk on a stock map (returns without assertions when the game is not installed):
/// real hulls, slopes and stairs.
#[test]
fn prediction_matches_the_server_on_dust2() {
    use cs_assets::bsp::BspMap;
    use cs_pm::collision::WorldCollision;
    use cs_vfs::{Vfs, VfsConfig};
    use std::sync::Arc;

    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        eprintln!("skipped: the installed game is not found (set CS16_GAMEDIR)");
        return;
    };
    let bsp = BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses");
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let world = Arc::new(WorldCollision::from_bsp(&bsp));
    let map = cs_engine::ServerMap::from_bsp("de_dust2", &raw, Arc::new(bsp)).expect("header");
    let mut l = Loop::new(map, world);
    let first_check = l.conn.predictor().checks().len();
    for i in 0..500 {
        l.step(Some(script(i)));
    }
    for _ in 0..5 {
        l.step(None);
    }
    let checks: Vec<_> = l
        .conn
        .predictor()
        .checks()
        .iter()
        .skip(first_check)
        .copied()
        .collect();
    assert!(checks.len() >= 250, "{} checks", checks.len());
    let worst = checks.iter().map(|c| c.error).fold(0.0f32, f32::max);
    assert!(checks.iter().all(|c| !c.teleport));
    assert!(worst < TOLERANCE, "worst of {} commands: {worst}", checks.len());
}

#[test]
fn without_the_world_the_server_is_shown() {
    let precache = bare::precache();
    let spawn = Vec3::new(0.0, 0.0, 36.03125);
    let frame1 = bare::frame(1, spawn, vec![bare::local_state(1)]);
    let mut p = Predictor::new();
    assert!(!p.has_world());
    bare::run_walk(&mut p, 10, &frame1, &precache, true);
    // No prediction in empty space: the view stays on the server's origin.
    assert!(!p.prediction().predicted);
    assert_eq!(p.prediction().simorg, spawn);
    p.set_world(Some(bare::world()));
    assert!(p.has_world());
    bare::run_walk(&mut p, 10, &frame1, &precache, true);
    assert!(p.prediction().predicted);
    assert!(p.prediction().simorg.x > spawn.x + 1.0);
    // A new map forgets the world.
    p.reset();
    assert!(!p.has_world());
}

#[test]
fn a_new_map_stops_the_prediction_until_the_world_is_given() {
    let (map, world) = server_map(map_entities(None));
    let mut l = Loop::new(map, world);
    l.step(Some(walk(10, IN_FORWARD, 250.0, 0.0, 0.0)));
    assert!(l.conn.predictor().has_world());
    assert!(l.conn.predictor().prediction().predicted);
    l.conn.set_world_collision(None);
    l.step(Some(walk(10, IN_FORWARD, 250.0, 0.0, 0.0)));
    assert!(!l.conn.predictor().prediction().predicted);
}

/// Several commands to a packet (`cl_cmdrate 30`), with no loss.
#[test]
fn several_commands_per_packet_match_the_server() {
    let (map, world) = server_map(map_entities(None));
    let mut l = Loop::new(map, world);
    l.conn.set_cvar("cl_cmdrate", "30");
    let first_check = l.conn.predictor().checks().len();
    for i in 0..300 {
        l.step(Some(script(i)));
    }
    for _ in 0..10 {
        l.step(None);
    }
    let checks: Vec<_> = l
        .conn
        .predictor()
        .checks()
        .iter()
        .skip(first_check)
        .copied()
        .collect();
    assert!(checks.len() >= 60, "{} checks", checks.len());
    let worst = checks.iter().map(|c| c.error).fold(0.0f32, f32::max);
    assert!(worst < TOLERANCE, "worst of {} checks: {worst}", checks.len());
}

/// One client packet and one server frame are lost, with 3 or more commands to a packet. The
/// predictor assumes the lost packet's commands ran, so the check that follows is off by about
/// their movement; it stays bounded, nothing is a teleport, and the error is gone within a few
/// frames.
#[test]
fn prediction_recovers_from_loss() {
    let (map, world) = server_map(map_entities(None));
    let mut l = Loop::new(map, world);
    l.conn.set_cvar("cl_cmdrate", "30");
    // Settle at the new rate.
    for i in 0..30 {
        l.step(Some(script(i)));
    }
    let first_check = l.conn.predictor().checks().len();
    let mut lost_at = 0;
    for i in 30..260 {
        if i == 100 || i == 180 {
            lost_at = l.conn.predictor().checks().len();
            l.lose_one_step(Some(script(i)));
        } else {
            l.step(Some(script(i)));
        }
    }
    for _ in 0..10 {
        l.step(None);
    }
    let checks: Vec<_> = l
        .conn
        .predictor()
        .checks()
        .iter()
        .skip(first_check)
        .copied()
        .collect();
    assert!(lost_at > first_check);
    assert!(checks.iter().all(|c| !c.teleport), "no loss is a teleport");
    // Bounded: a packet holds about 4 commands of 10 ms at 250 u/s (jumps and landings add
    // some): well under the 64 of a teleport.
    let worst = checks.iter().map(|c| c.error).fold(0.0f32, f32::max);
    assert!(worst < 24.0, "worst error {worst}");
    // And it comes back: the checks of the last part of the walk are exact again.
    let tail = &checks[checks.len() - 15..];
    assert!(
        tail.iter().all(|c| c.error < TOLERANCE),
        "the error is gone: {:?}",
        tail.iter().map(|c| c.error).collect::<Vec<_>>()
    );
    // Right after the second loss, the error is gone within a few frames.
    let after: Vec<_> = checks.iter().filter(|c| c.error > TOLERANCE).collect();
    assert!(after.len() <= 12, "{} bad checks", after.len());
    let server = l.server_origin();
    let predicted = l.conn.predictor().prediction().simorg;
    assert!((server - predicted).length() < TOLERANCE, "{server:?} vs {predicted:?}");
}

#[test]
fn predicted_movement_emits_material_footsteps_and_handles_rotated_submodels() {
    use cs_assets::bsp::{BspMap, BspModel, ClipNode, Face, Leaf, LightmapAtlas, Node, Plane, TexInfo};
    use cs_assets::wad::DecodedTexture;
    use cs_clientcore::{Frame, PlayerPose, PrecacheTables};
    use cs_clientcore::frames::FrameEntity;
    use cs_pm::collision::{CONTENTS_EMPTY, CONTENTS_SOLID, Hull, WorldCollision};
    use cs_pm::pm::MoveVars;
    use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState};
    use cs_proto48::messages::ResourceEntry;
    use std::sync::Arc;

    // Build a multi-zone BSP floor at z = 0:
    //   x in [0, 200]:    "-0metal_zone" -> M -> player/pl_metal*.wav
    //   x in [200, 400]:  "duct_zone"    -> V -> player/pl_duct*.wav
    //   x in [400, 600]:  "snow_zone"    -> O -> player/pl_snow*.wav
    //   x in [600, 800]:  "wood_zone"    -> W -> player/pl_step*.wav (wood maps to STEP_CONCRETE in pm_shared)
    // Plus submodel *1 (a platform/door box [-64..64] x [-32..32] x [-16..0]) whose top face at z = 0
    // is "grate_zone" -> G -> player/pl_grate*.wav, placed at origin (1000, 1000, 0) rotated 90 deg yaw.
    let mut planes = Vec::new();
    let mut nodes = Vec::new();
    let mut clipnodes = Vec::new();
    let mut vertices_raw = Vec::new();
    let mut edges = Vec::new();
    let mut surf_edges = Vec::new();
    let mut faces = Vec::new();
    let mut texinfos = Vec::new();
    let mut textures = Vec::new();

    let zone_specs = [
        ("-0metal_zone", 0.0f32, 200.0f32),
        ("duct_zone", 200.0f32, 400.0f32),
        ("snow_zone", 400.0f32, 600.0f32),
        ("wood_zone", 600.0f32, 800.0f32),
    ];
    for (i, (name, x0, x1)) in zone_specs.iter().enumerate() {
        textures.push(DecodedTexture {
            name: (*name).to_string(),
            width: 16,
            height: 16,
            rgba: Vec::new(),
            alpha_test: false,
        });
        texinfos.push(TexInfo {
            s_vec: Vec3::X,
            s_shift: 0.0,
            t_vec: Vec3::Y,
            t_shift: 0.0,
            miptex: i as i32,
            flags: 0,
        });
        let base_v = vertices_raw.len() as u16;
        vertices_raw.push(Vec3::new(*x0, -128.0, 0.0));
        vertices_raw.push(Vec3::new(*x1, -128.0, 0.0));
        vertices_raw.push(Vec3::new(*x1, 128.0, 0.0));
        vertices_raw.push(Vec3::new(*x0, 128.0, 0.0));
        let base_e = edges.len() as i32;
        edges.push([base_v, base_v + 1]);
        edges.push([base_v + 1, base_v + 2]);
        edges.push([base_v + 2, base_v + 3]);
        edges.push([base_v + 3, base_v]);
        surf_edges.extend_from_slice(&[base_e, base_e + 1, base_e + 2, base_e + 3]);
        faces.push(Face {
            plane: 0,
            plane_side: 0,
            first_edge: base_e,
            num_edges: 4,
            texinfo: i as u16,
            styles: [0; 4],
            lightmap_offset: -1,
        });
    }

    // World point hull: node 0 is plane z = 0 carrying all 4 zone faces (each checked by its extents!).
    planes.push(Plane {
        normal: Vec3::Z,
        dist: 0.0,
        plane_type: 2,
    });
    nodes.push(Node {
        plane: 0,
        children: [-1, -2],
        mins: [-4096; 3],
        maxs: [4096; 3],
        first_face: 0,
        num_faces: 4,
    });
    // World standing hull: clipnode 0 is z = 36 (player standing half-height).
    planes.push(Plane {
        normal: Vec3::Z,
        dist: Hull::Standing.half_extents().z,
        plane_type: 2,
    });
    clipnodes.push(ClipNode {
        plane: 1,
        children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
    });

    // Submodel *1: local box [-64..64] x [-32..32] x [-16..0], top face at z = 0 textured "grate_zone".
    textures.push(DecodedTexture {
        name: "grate_zone".to_string(),
        width: 16,
        height: 16,
        rgba: Vec::new(),
        alpha_test: false,
    });
    texinfos.push(TexInfo {
        s_vec: Vec3::X,
        s_shift: 0.0,
        t_vec: Vec3::Y,
        t_shift: 0.0,
        miptex: 4,
        flags: 0,
    });
    let base_v = vertices_raw.len() as u16;
    vertices_raw.push(Vec3::new(-64.0, -32.0, 0.0));
    vertices_raw.push(Vec3::new(64.0, -32.0, 0.0));
    vertices_raw.push(Vec3::new(64.0, 32.0, 0.0));
    vertices_raw.push(Vec3::new(-64.0, 32.0, 0.0));
    let base_e = edges.len() as i32;
    edges.push([base_v, base_v + 1]);
    edges.push([base_v + 1, base_v + 2]);
    edges.push([base_v + 2, base_v + 3]);
    edges.push([base_v + 3, base_v]);
    surf_edges.extend_from_slice(&[base_e, base_e + 1, base_e + 2, base_e + 3]);
    faces.push(Face {
        plane: 0,
        plane_side: 0,
        first_edge: base_e,
        num_edges: 4,
        texinfo: 4,
        styles: [0; 4],
        lightmap_offset: -1,
    });
    nodes.push(Node {
        plane: 0,
        children: [-1, -2],
        mins: [-64, -32, -16],
        maxs: [64, 32, 0],
        first_face: 4,
        num_faces: 1,
    });
    // Submodel *1 standing hull: full grown box [-80..80] x [-48..48] x [-52..36].
    let half = Hull::Standing.half_extents();
    let sub_mins = Vec3::new(-64.0, -32.0, -16.0) - half;
    let sub_maxs = Vec3::new(64.0, 32.0, 0.0) + half;
    let box_planes = [
        (0usize, sub_maxs.x),
        (0usize, sub_mins.x),
        (1usize, sub_maxs.y),
        (1usize, sub_mins.y),
        (2usize, sub_maxs.z),
        (2usize, sub_mins.z),
    ];
    let first_sub_clip = clipnodes.len() as i32;
    for (idx, &(axis, dist)) in box_planes.iter().enumerate() {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        let plane_idx = planes.len() as i32;
        planes.push(Plane {
            normal,
            dist,
            plane_type: axis as i32,
        });
        let side = idx & 1;
        let next = if idx < 5 {
            first_sub_clip as i16 + idx as i16 + 1
        } else {
            CONTENTS_SOLID as i16
        };
        let mut children = [0i16; 2];
        children[side] = CONTENTS_EMPTY as i16;
        children[side ^ 1] = next;
        clipnodes.push(ClipNode {
            plane: plane_idx,
            children,
        });
    }

    let leaf = |contents: i32| Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    };
    let bsp = BspMap {
        name: "mat_zones".into(),
        skyname: String::new(),
        entities: Vec::new(),
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes,
        vertices_raw,
        visibility: Vec::new(),
        nodes,
        texinfos,
        faces,
        lighting: Vec::new(),
        clipnodes,
        leafs: vec![leaf(CONTENTS_EMPTY), leaf(CONTENTS_SOLID)],
        mark_surfaces: Vec::new(),
        edges,
        surf_edges,
        models: vec![
            BspModel {
                mins: Vec3::splat(-4096.0),
                maxs: Vec3::splat(4096.0),
                origin: Vec3::ZERO,
                head_nodes: [0, 0, 0, 0],
                vis_leafs: 1,
                first_face: 0,
                num_faces: 4,
            },
            BspModel {
                mins: Vec3::new(-64.0, -32.0, -16.0),
                maxs: Vec3::new(64.0, 32.0, 0.0),
                origin: Vec3::ZERO,
                head_nodes: [1, first_sub_clip, first_sub_clip, first_sub_clip],
                vis_leafs: 1,
                first_face: 4,
                num_faces: 1,
            },
        ],
        textures,
        lightmap_atlas: LightmapAtlas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            packed_faces: 0,
        },
        render_vertices: Vec::new(),
        render_indices: Vec::new(),
        batches: Vec::new(),
    };

    let materials_txt = b"\
M metal_zone
V duct_zone
N snow_zone
W wood_zone
G grate_zone
";

    let world = Arc::new(WorldCollision::from_bsp(&bsp));
    let mut precache = PrecacheTables::default();
    precache.add(&[ResourceEntry {
        res_type: 2,
        filename: "*1".to_string(),
        index: 2,
        download_size: 0,
        flags: 0,
        md5: None,
        reserved: None,
    }]);

    let run_walk_at = |start_origin: Vec3, yaw: f32, submodel_origin: Option<Vec3>| -> Vec<String> {
        let mut p = Predictor::new();
        p.set_world(Some(Arc::clone(&world)));
        p.set_materials(materials_txt);

        let mut entities = vec![GoldSrcEntityState {
            number: 1,
            modelindex: 3,
            movetype: 3,
            solid: 3,
            friction: 1.0,
            gravity: 1.0,
            mins: Vec3::new(-16.0, -16.0, -36.0),
            maxs: Vec3::new(16.0, 16.0, 36.0),
            ..GoldSrcEntityState::default()
        }];
        if let Some(org) = submodel_origin {
            entities.push(GoldSrcEntityState {
                number: 7,
                modelindex: 2,
                movetype: 7,
                solid: 4,
                origin: org,
                angles: Vec3::new(0.0, 90.0, 0.0),
                mins: Vec3::new(-64.0, -32.0, -16.0),
                maxs: Vec3::new(64.0, 32.0, 0.0),
                ..GoldSrcEntityState::default()
            });
        }
        let frame = Frame {
            sequence: 1,
            valid: true,
            time: 10.0,
            received_time: 10.0,
            entities: entities
                .into_iter()
                .map(|state| FrameEntity {
                    state,
                    is_new: false,
                })
                .collect(),
            clientdata: Some(GoldSrcClientData {
                origin: start_origin,
                view_ofs: Vec3::new(0.0, 0.0, 17.0),
                maxspeed: 250.0,
                health: 100.0,
                flags: 1 << 9,
                ..GoldSrcClientData::default()
            }),
            weapon_data: Default::default(),
        };
        let mut all_sounds = Vec::new();
        let poses: [PlayerPose; 0] = [];
        let mv = MoveVars {
            footsteps: true,
            ..MoveVars::default()
        };
        for seq in 1..=45u32 {
            let now = 10.0 + f64::from(seq) * 0.01;
            p.set_now(now);
            p.push_cmd(walk(10, IN_FORWARD, 250.0, 0.0, yaw));
            p.packet_sent(seq, 1, None);
            p.update(&PredictInputs {
                now,
                incoming_acknowledged: 0,
                outgoing_sequence: seq + 1,
                frame: Some(&frame),
                playernum: 0,
                max_clients: 4,
                precache: &precache,
                movevars: mv,
                max_velocity: 2000.0,
                poses: &poses,
                entities: None,
                remote_is_local: true,
                intermission: false,
            });
            for s in p.take_sounds() {
                all_sounds.push(s.sample);
            }
        }
        all_sounds
    };

    // World zones: metal, duct, snow, wood.
    let metal_sounds = run_walk_at(Vec3::new(32.0, 0.0, 36.03125), 0.0, None);
    assert!(
        !metal_sounds.is_empty() && metal_sounds.iter().all(|s| s.starts_with("player/pl_metal")),
        "expected pl_metal*, got {metal_sounds:?}"
    );
    let duct_sounds = run_walk_at(Vec3::new(232.0, 0.0, 36.03125), 0.0, None);
    assert!(
        !duct_sounds.is_empty() && duct_sounds.iter().all(|s| s.starts_with("player/pl_duct")),
        "expected pl_duct*, got {duct_sounds:?}"
    );
    let snow_sounds = run_walk_at(Vec3::new(432.0, 0.0, 36.03125), 0.0, None);
    assert!(
        !snow_sounds.is_empty() && snow_sounds.iter().all(|s| s.starts_with("player/pl_snow")),
        "expected pl_snow*, got {snow_sounds:?}"
    );
    let wood_sounds = run_walk_at(Vec3::new(632.0, 0.0, 36.03125), 0.0, None);
    assert!(
        !wood_sounds.is_empty() && wood_sounds.iter().all(|s| s.starts_with("player/pl_step")),
        "wood maps to STEP_CONCRETE (pl_step*) in GoldSrc pm_shared, got {wood_sounds:?}"
    );

    // Rotated submodel *1 at (1000, 1000, 100) with 90 deg yaw: long axis is along world Y.
    // Walking along +Y (yaw 90) stays on top of the rotated platform and emits pl_grate*.
    let grate_sounds = run_walk_at(
        Vec3::new(1000.0, 960.0, 136.03125),
        90.0,
        Some(Vec3::new(1000.0, 1000.0, 100.0)),
    );
    assert!(
        !grate_sounds.is_empty() && grate_sounds.iter().all(|s| s.starts_with("player/pl_grate")),
        "expected pl_grate* on rotated brush submodel, got {grate_sounds:?}"
    );
}

#[test]
fn predicted_movement_on_stock_maps_emits_non_concrete_footsteps() {
    use cs_assets::bsp::BspMap;
    use cs_clientcore::frames::FrameEntity;
    use cs_clientcore::{Frame, PlayerPose, PrecacheTables};
    use cs_pm::collision::{CONTENTS_EMPTY, Hull, WorldCollision};
    use cs_pm::pm::{MoveVars, TextureTable};
    use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState};
    use cs_vfs::{Vfs, VfsConfig};
    use std::sync::Arc;

    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        eprintln!("skipped: the installed game is not found (set CS16_GAMEDIR)");
        return;
    };
    let materials_txt = vfs
        .read_bytes("sound/materials.txt")
        .expect("sound/materials.txt in retail VFS");
    let tex_table = TextureTable::parse(&materials_txt);

    let find_spot_and_walk = |map_name: &str, expected_type: u8, expected_prefix: &str| {
        let bsp = BspMap::load(&vfs, map_name).expect("stock map parses");
        let world = Arc::new(WorldCollision::from_bsp(&bsp));
        let world_model = &bsp.models[0];
        let first = world_model.first_face as usize;
        let end = first + world_model.num_faces as usize;

        let mut spot = None;
        for face in &bsp.faces[first..end] {
            let Some(plane) = bsp.planes.get(usize::from(face.plane)) else {
                continue;
            };
            if plane.normal.z < 0.95 || face.num_edges < 3 {
                continue;
            }
            let Some(ti) = bsp.texinfos.get(usize::from(face.texinfo)) else {
                continue;
            };
            let Some(tex) = usize::try_from(ti.miptex).ok().and_then(|i| bsp.textures.get(i)) else {
                continue;
            };
            let stripped = tex
                .name
                .trim_start_matches(['-', '+'])
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start_matches(['{', '!', '~', ' ']);
            if tex_table.find(stripped) != expected_type {
                continue;
            }
            let mut sum = Vec3::ZERO;
            let mut count = 0usize;
            for i in 0..usize::from(face.num_edges) {
                let se = bsp.surf_edges[face.first_edge as usize + i];
                let v_idx = if se >= 0 {
                    bsp.edges[se as usize][0]
                } else {
                    bsp.edges[se.unsigned_abs() as usize][1]
                };
                sum += bsp.vertices_raw[usize::from(v_idx)];
                count += 1;
            }
            let center = sum / count as f32;
            let tr = world.trace_world(
                center + Vec3::Z * 48.0,
                center - Vec3::Z * 16.0,
                Hull::Standing,
            );
            if tr.start_solid || tr.all_solid || !tr.hit_solid || tr.plane_normal.z < 0.7 {
                continue;
            }
            let stand_org = tr.end_pos;
            if world.point_contents(stand_org, Hull::Standing) != CONTENTS_EMPTY {
                continue;
            }
            if world
                .trace_world(stand_org, stand_org + Vec3::X * 128.0, Hull::Standing)
                .fraction
                < 1.0
            {
                continue;
            }
            if world
                .trace_texture(0, stand_org, stand_org - Vec3::Z * 64.0)
                .is_some_and(|n| n == tex.name)
            {
                spot = Some((stand_org, tex.name.clone()));
                break;
            }
        }

        let (origin, tex_name) = spot.unwrap_or_else(|| {
            panic!("expected a walkable face of type {} in {map_name}", char::from(expected_type))
        });

        let mut p = Predictor::new();
        p.set_world(Some(world));
        p.set_materials(&materials_txt);
        let precache = PrecacheTables::default();
        let frame = Frame {
            sequence: 1,
            valid: true,
            time: 10.0,
            received_time: 10.0,
            entities: vec![FrameEntity {
                state: GoldSrcEntityState {
                    number: 1,
                    modelindex: 3,
                    movetype: 3,
                    solid: 3,
                    friction: 1.0,
                    gravity: 1.0,
                    mins: Vec3::new(-16.0, -16.0, -36.0),
                    maxs: Vec3::new(16.0, 16.0, 36.0),
                    ..GoldSrcEntityState::default()
                },
                is_new: false,
            }],
            clientdata: Some(GoldSrcClientData {
                origin,
                view_ofs: Vec3::new(0.0, 0.0, 17.0),
                maxspeed: 250.0,
                health: 100.0,
                flags: 1 << 9,
                ..GoldSrcClientData::default()
            }),
            weapon_data: Default::default(),
        };
        let mv = MoveVars {
            footsteps: true,
            ..MoveVars::default()
        };
        let poses: [PlayerPose; 0] = [];
        let mut sounds = Vec::new();
        for seq in 1..=60u32 {
            let now = 10.0 + f64::from(seq) * 0.01;
            p.set_now(now);
            p.push_cmd(walk(10, IN_FORWARD, 250.0, 0.0, 0.0));
            p.packet_sent(seq, 1, None);
            p.update(&PredictInputs {
                now,
                incoming_acknowledged: 0,
                outgoing_sequence: seq + 1,
                frame: Some(&frame),
                playernum: 0,
                max_clients: 4,
                precache: &precache,
                movevars: mv,
                max_velocity: 2000.0,
                poses: &poses,
                entities: None,
                remote_is_local: true,
                intermission: false,
            });
            for s in p.take_sounds() {
                sounds.push(s.sample);
            }
        }
        assert!(
            sounds.iter().any(|s| s.starts_with(expected_prefix)),
            "{map_name} face {tex_name}: expected {expected_prefix}*, got {sounds:?}"
        );
    };

    find_spot_and_walk("de_survivor", b'N', "player/pl_snow");
    find_spot_and_walk("de_nuke", b'M', "player/pl_metal");
    find_spot_and_walk("de_nuke", b'V', "player/pl_duct");
    find_spot_and_walk("as_oilrig", b'G', "player/pl_grate");
}
