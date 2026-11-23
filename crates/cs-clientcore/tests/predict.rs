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
