//! The join screen: what a player sees while it reads the MOTD and picks a team. The intro
//! cameras (`ClientPutInServer` and `JoiningThink` of ReGameDLL), the round timer a joiner is told
//! and the title of the MOTD window (`gmsgServerName`, the `hostname` cvar). No wall-clock
//! timing: the server's clock is the bench's counter of 10 ms frames.

mod common;

use common::{Bench, entity, stub_spawns};
use cs_engine::{EngineApi, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

/// A map with two cameras, each looking at its own `info_target`, and the usual spawn points.
fn camera_map(cameras: &[(&str, &str)], targets: &[(&str, &str)]) -> ServerMap {
    let mut entities = stub_spawns();
    for (origin, target) in cameras {
        entities.push(entity(&[
            ("classname", "trigger_camera"),
            ("origin", origin),
            ("target", target),
        ]));
    }
    for (name, origin) in targets {
        entities.push(entity(&[
            ("classname", "info_target"),
            ("targetname", name),
            ("origin", origin),
        ]));
    }
    ServerMap::stub_with_entities("test", entities)
}

fn config(hostname: &str) -> ServerConfig {
    ServerConfig {
        max_clients: 2,
        max_edicts: 256,
        hostname: hostname.to_string(),
        ..ServerConfig::default()
    }
}

/// Where the player of slot 0 is and looks, and what the game knows of its camera.
struct View {
    origin: Vec3,
    angles: Vec3,
    v_angle: Vec3,
    view_ofs: Vec3,
    has_camera: bool,
    cam_time: f64,
}

fn view(b: &mut Bench) -> View {
    b.run(|game: &mut CsGame, eng: &mut EngineApi<'_>| {
        let v = eng.ent(eng.client_edict(0)).unwrap().clone();
        let p = game.state().players[0].clone().unwrap();
        View {
            origin: v.origin,
            angles: v.angles,
            v_angle: v.v_angle,
            view_ofs: v.view_ofs,
            has_camera: p.intro_camera.is_some(),
            cam_time: p.intro_cam_time,
        }
    })
}

/// The view the server last forced on the probe: the engine writes `svc_setangle` for a player's
/// `fixangle` with its next client data and clears the field (`SV_WriteClientdataToMessage`).
fn forced_view(b: &Bench) -> Option<Vec3> {
    b.probe.messages().iter().rev().find_map(|m| match m {
        SvcMessage::SetAngle(angles) => Some(*angles),
        _ => None,
    })
}

const CAM1: Vec3 = Vec3::new(0.0, 0.0, 100.0);
const CAM2: Vec3 = Vec3::new(500.0, 0.0, 100.0);

/// Steps a frame at a time until the player's origin leaves `from`, at most `limit` frames.
fn step_until_moved(b: &mut Bench, from: Vec3, limit: usize) -> View {
    for _ in 0..limit {
        b.step(1);
        let v = view(b);
        if v.origin != from {
            return v;
        }
    }
    panic!("the player never left {from}");
}

#[test]
fn intro_camera_at_join() {
    let map = camera_map(
        &[("0 0 100", "t1"), ("500 0 100", "t2")],
        &[("t1", "100 0 100"), ("t2", "500 100 100")],
    );
    let mut b = Bench::with_config(map, config("x"), &[], true);
    b.idle_cmds = true;
    // The first camera, looking along +X at its target (the pitch is negated, so level is 0).
    b.step(5);
    let first = view(&mut b);
    assert_eq!(first.origin, CAM1);
    assert_eq!(first.angles, Vec3::ZERO);
    assert_eq!(first.v_angle, first.angles);
    assert_eq!(first.view_ofs, Vec3::ZERO);
    assert_eq!(forced_view(&b), Some(first.angles));
    assert!(first.has_camera);
    let join_time_plus_6 = first.cam_time;
    // It holds the view until its time is up (6 s after the join), then the second camera,
    // which looks along +Y, takes over at the first command that finds the time past.
    let second = step_until_moved(&mut b, CAM1, 800);
    assert_eq!(second.origin, CAM2);
    assert_eq!(second.angles, Vec3::new(0.0, 90.0, 0.0));
    assert_eq!(second.v_angle, second.angles);
    assert_eq!(second.view_ofs, Vec3::ZERO);
    b.step(3);
    assert_eq!(forced_view(&b), Some(second.angles));
    assert!(
        (second.cam_time - join_time_plus_6 - 6.0) >= 0.0
            && (second.cam_time - join_time_plus_6 - 6.0) < 0.1,
        "the next change is six seconds after this one: {} then {}",
        join_time_plus_6,
        second.cam_time
    );
    // No further camera: back to the first.
    let third = step_until_moved(&mut b, CAM2, 800);
    assert_eq!((third.origin, third.angles), (CAM1, Vec3::ZERO));
    assert!(third.cam_time - second.cam_time >= 6.0);
    // Once the player has picked a team the cameras let go of it.
    common::join(&mut b, 0, 2, 1);
    b.step(5);
    let p = b.run(|game, _| game.state().players[0].clone().unwrap());
    assert_eq!(p.team, Team::Ct);
    let joined = view(&mut b);
    assert_ne!(joined.origin, CAM1);
    assert_ne!(joined.origin, CAM2);
    b.step(700);
    let later = view(&mut b);
    assert_ne!(later.origin, CAM1);
    assert_ne!(later.origin, CAM2);
}

#[test]
fn camera_without_a_target_gives_no_view() {
    // The camera's target names nothing: the player stands at the spawn point as with no camera
    // (vanilla `else`), and the first `JoiningThink` ends the tour (vanilla's `m_fIntroCamTime`
    // is still 0 then: the "next" camera is the same one, with no target again).
    let map = camera_map(&[("0 0 100", "nothing")], &[]);
    let mut b = Bench::with_config(map, config("x"), &[], true);
    let at_join = view(&mut b);
    assert_eq!(at_join.origin, Vec3::new(100.0, 0.0, 1.0));
    // `v_angle` is zero at the join; the probe has taken the forced view (`svc_setangle` of the
    // spot's angles) by now, as a client does, and sends its commands looking that way.
    assert_eq!(at_join.v_angle, at_join.angles);
    assert_eq!(at_join.view_ofs.z, 17.0);
    b.idle_cmds = true;
    b.step(5);
    let later = view(&mut b);
    assert!(!later.has_camera);
    assert_eq!(later.origin, Vec3::new(100.0, 0.0, 1.0));
}

#[test]
fn no_camera_join_path_unchanged() {
    // A map with no `trigger_camera`: the counter-terrorists' first spawn point, its angles,
    // the standing view offset, and no camera ever.
    let mut b = Bench::with_config(
        ServerMap::stub_with_entities("test", stub_spawns()),
        config("x"),
        &[],
        true,
    );
    b.idle_cmds = true;
    b.step(5);
    let v = view(&mut b);
    assert_eq!(v.origin, Vec3::new(100.0, 0.0, 1.0));
    // The spot's angles (the forced view below). `v_angle` is not asserted: it is zero until the
    // probe's first command and then whatever that command looks along, which depends on how
    // many frames sign-on took.
    assert_eq!(v.angles.y, 90.0);
    assert_eq!(v.view_ofs, Vec3::new(0.0, 0.0, 17.0));
    assert_eq!(forced_view(&b), Some(v.angles));
    assert!(!v.has_camera);
    b.step(700);
    let later = view(&mut b);
    assert_eq!(later.origin, v.origin);
    assert!(!later.has_camera);
}

#[test]
fn server_name_follows_hostname() {
    // `--hostname` sets the `hostname` cvar (the server's configuration is its source), and the
    // MOTD window of a joiner is titled with the cvar.
    let map = ServerMap::stub_with_entities("test", stub_spawns());
    let mut b = Bench::with_config(map, config("cs16-rs verify"), &[], true);
    b.idle_cmds = true;
    b.step(20);
    let names: Vec<String> = b
        .messages()
        .into_iter()
        .filter_map(|m| match m {
            CsUserMsg::ServerName(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["cs16-rs verify"]);
    // Changed from the console, the next joiner is told the new name.
    b.run(|_, eng| eng.set_cvar("hostname", "Renamed"));
    let mut next = b.add_probe("next");
    b.step_both(&mut next, 40);
    let names: Vec<String> = next
        .messages()
        .iter()
        .filter_map(|m| match m {
            cs_proto48::messages::SvcMessage::UserMsg {
                decoded: CsUserMsg::ServerName(n),
                ..
            } => Some(n.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["Renamed"]);
}

/// `de_dust2` has `trigger_camera` entities, each aimed at an `info_target`: a joiner looks
/// through the first one. (Returns at once when the game is not installed.)
#[test]
fn dust2_joiner_looks_through_the_first_camera() {
    let Some(vfs) = common::discover() else {
        return;
    };
    let mut b = Bench::with_config(common::dust2(&vfs), config("x"), &[], true);
    let (camera, standing) = b.run(|_, eng| {
        let camera = eng.find_by_classname(None, "trigger_camera");
        (
            camera.and_then(|c| eng.ent(c)).map(|v| v.origin),
            eng.ent(eng.client_edict(0)).map(|v| v.origin),
        )
    });
    let camera = camera.expect("de_dust2 has cameras");
    assert_eq!(standing, Some(camera));
}
