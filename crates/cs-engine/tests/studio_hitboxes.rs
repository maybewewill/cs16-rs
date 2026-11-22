//! Traces against studio models: hitbox hulls, hitgroups, the pose a model is traced in, and
//! what happens to models the engine cannot use. The synthetic models are built here byte by
//! byte; the real player models come from the installed game (those tests return early without
//! it).
//!
//! The test game precaches `models/test.mdl` at model index 2, so the loader given to the
//! server answers that name with the model under test.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::Harness;
use cs_assets::mdl::StudioModel;
use cs_engine::consts::{FL_CLIENT, SOLID_BBOX, SOLID_SLIDEBOX};
use cs_engine::{EdictId, EngineApi, EntTrace, MoveFilter};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const HEAD: i32 = 1;
const CHEST: i32 = 2;

/// Runs `f` as the game at the start of the next frame and returns what it returned.
fn run<T: 'static>(h: &mut Harness, f: impl FnOnce(&mut EngineApi<'_>) -> T + 'static) -> T {
    let out = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&out);
    h.server
        .game_mut()
        .act(move |eng| *sink.borrow_mut() = Some(f(eng)));
    h.step(&mut [], 1);
    let result = out.borrow_mut().take();
    result.expect("the game ran the action")
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

// ---- a model made of bytes -------------------------------------------------------------------

struct BoxDef {
    bone: i32,
    group: i32,
    min: [f32; 3],
    max: [f32; 3],
}

struct BoneDef {
    parent: i32,
    /// `value[0..3]` position, `value[3..6]` rotation.
    value: [f32; 6],
    /// An animation value for the rotation about z (channel 5) in each blend: one run of one
    /// frame, `value * scale[5]` radians on top of `value[5]`.
    rot_z_blend: [i16; 2],
    scale: f32,
}

impl BoneDef {
    fn at(parent: i32, pos: [f32; 3]) -> Self {
        Self {
            parent,
            value: [pos[0], pos[1], pos[2], 0.0, 0.0, 0.0],
            rot_z_blend: [0, 0],
            scale: 1.0,
        }
    }
}

fn put_i32(b: &mut [u8], at: usize, v: i32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(b: &mut [u8], at: usize, v: f32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// An IDST v10 file with the given bones and hitboxes and one sequence of one frame, of
/// `blends` blends (1 or 2). Animations are at the end: per blend, one 12-byte entry per bone,
/// then the runs of the bones that animate the rotation about z.
fn build_mdl(bones: &[BoneDef], boxes: &[BoxDef], header_flags: i32, blends: usize) -> Vec<u8> {
    let bone_off = 244;
    let box_off = bone_off + bones.len() * 112;
    let seq_off = box_off + boxes.len() * 32;
    let anim_off = seq_off + 176;
    let runs_off = anim_off + blends * bones.len() * 12;
    let mut b = vec![0u8; runs_off + blends * bones.len() * 4 + 16];
    b[0..4].copy_from_slice(b"IDST");
    put_i32(&mut b, 4, 10);
    b[8..12].copy_from_slice(b"test");
    put_i32(&mut b, 136, header_flags);
    put_i32(&mut b, 140, bones.len() as i32);
    put_i32(&mut b, 144, bone_off as i32);
    put_i32(&mut b, 156, boxes.len() as i32);
    put_i32(&mut b, 160, box_off as i32);
    put_i32(&mut b, 164, 1);
    put_i32(&mut b, 168, seq_off as i32);

    for (i, bone) in bones.iter().enumerate() {
        let at = bone_off + i * 112;
        put_i32(&mut b, at + 32, bone.parent);
        for c in 0..6 {
            put_i32(&mut b, at + 40 + c * 4, -1);
            put_f32(&mut b, at + 64 + c * 4, bone.value[c]);
            put_f32(&mut b, at + 88 + c * 4, bone.scale);
        }
    }
    for (i, def) in boxes.iter().enumerate() {
        let at = box_off + i * 32;
        put_i32(&mut b, at, def.bone);
        put_i32(&mut b, at + 4, def.group);
        for c in 0..3 {
            put_f32(&mut b, at + 8 + c * 4, def.min[c]);
            put_f32(&mut b, at + 20 + c * 4, def.max[c]);
        }
    }
    put_i32(&mut b, seq_off + 56, 1); // numframes
    put_i32(&mut b, seq_off + 120, blends as i32);
    put_i32(&mut b, seq_off + 124, anim_off as i32);
    for blend in 0..blends {
        for (i, bone) in bones.iter().enumerate() {
            let entry = anim_off + (blend * bones.len() + i) * 12;
            if bone.rot_z_blend[blend] != 0 {
                let run = runs_off + (blend * bones.len() + i) * 4;
                // The run: valid 1, total 1, then the value; the offset is from the entry.
                b[run] = 1;
                b[run + 1] = 1;
                b[run + 2..run + 4].copy_from_slice(&bone.rot_z_blend[blend].to_le_bytes());
                put_u16(&mut b, entry + 10, (run - entry) as u16);
            }
        }
    }
    b
}

/// A harness whose `models/test.mdl` is `bytes`.
fn harness_with_model(bytes: Vec<u8>) -> Harness {
    let mut h = Harness::new(2);
    h.server
        .set_model_loader(Arc::new(move |_name| Some(bytes.clone())));
    h
}

/// A studio entity of the test model: a slide box (the player's solid) of `mins..maxs`.
fn studio_entity(
    eng: &mut EngineApi<'_>,
    origin: Vec3,
    mins: Vec3,
    maxs: Vec3,
    flags: u32,
) -> EdictId {
    let id = eng.create_entity("test_studio").expect("free edict");
    assert!(eng.set_model(id, "models/test.mdl"));
    {
        let v = eng.ent_mut(id).unwrap();
        v.solid = SOLID_SLIDEBOX;
        v.flags = flags;
    }
    eng.set_size(id, mins, maxs);
    eng.set_origin(id, origin);
    id
}

fn ray(eng: &EngineApi<'_>, from: Vec3, to: Vec3) -> EntTrace {
    eng.trace_line(from, to, MoveFilter::Normal, None)
}

/// A head and a chest box on one bone at the model's origin, 100 units up the x axis.
fn two_box_model() -> Vec<u8> {
    build_mdl(
        &[BoneDef::at(-1, [0.0, 0.0, 0.0])],
        &[
            BoxDef {
                bone: 0,
                group: HEAD,
                min: [-5.0, -5.0, 60.0],
                max: [5.0, 5.0, 72.0],
            },
            BoxDef {
                bone: 0,
                group: CHEST,
                min: [-8.0, -8.0, 30.0],
                max: [8.0, 8.0, 50.0],
            },
        ],
        0,
        1,
    )
}

const STANDING_MINS: Vec3 = Vec3::new(-16.0, -16.0, 0.0);
const STANDING_MAXS: Vec3 = Vec3::new(16.0, 16.0, 72.0);

#[test]
fn hitboxes_give_their_hitgroups_and_misses_miss() {
    let mut h = harness_with_model(two_box_model());
    let (id, head, chest, between, above) = run(&mut h, |eng| {
        let id = studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            0,
        );
        let at = |z: f32| ray(eng, Vec3::new(0.0, 0.0, z), Vec3::new(200.0, 0.0, z));
        (id, at(66.0), at(40.0), at(55.0), at(80.0))
    });
    assert_eq!(head.ent, Some(id));
    assert_eq!(head.trace.hitgroup, HEAD);
    // The head box is 5 wide: the near face is at x = 95, and a trace stops a little short.
    assert!(near(head.trace.fraction, (95.0 - 0.03125) / 200.0), "{head:?}");
    assert_eq!(chest.ent, Some(id));
    assert_eq!(chest.trace.hitgroup, CHEST);
    assert!(near(chest.trace.fraction, (92.0 - 0.03125) / 200.0), "{chest:?}");
    // Between the boxes, inside the entity's own box: a miss. The box alone would have hit.
    assert_eq!(between.trace.fraction, 1.0);
    assert_eq!(between.trace.hitgroup, 0);
    assert_ne!(between.ent, Some(id));
    assert_ne!(above.ent, Some(id));
}

#[test]
fn clients_are_traced_with_a_grown_hull_and_others_with_none() {
    let mut h = harness_with_model(two_box_model());
    let (client_in, client_out, other_in, other_out) = run(&mut h, |eng| {
        let client = studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            FL_CLIENT,
        );
        let other = studio_entity(
            eng,
            Vec3::new(100.0, 100.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            0,
        );
        let at = |y: f32, z: f32| {
            let t = ray(eng, Vec3::new(0.0, y, z), Vec3::new(200.0, y, z));
            (t.ent, t.trace.hitgroup)
        };
        // The head box ends at z = 72. A client's hull is grown by 0.5 (sv_clienttrace 1),
        // so z = 72.4 is a hit and 72.6 is not; another entity's is not grown at all.
        // (A client's pose has pitch 0 and so the same boxes.)
        let _ = (client, other);
        (at(0.0, 72.4), at(0.0, 72.6), at(100.0, 71.9), at(100.0, 72.1))
    });
    assert_eq!(client_in.1, HEAD);
    assert_eq!(client_out.1, 0);
    assert_eq!(other_in.1, HEAD);
    assert_eq!(other_out.1, 0);
}

#[test]
fn sv_clienttrace_zero_traces_a_client_as_its_box() {
    let mut h = harness_with_model(two_box_model());
    run(&mut h, |eng| eng.set_cvar("sv_clienttrace", "0"));
    let (id, t) = run(&mut h, |eng| {
        let id = studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            FL_CLIENT,
        );
        // Between the two boxes: the hitboxes miss, the entity's box does not.
        (id, ray(eng, Vec3::new(0.0, 0.0, 55.0), Vec3::new(200.0, 0.0, 55.0)))
    });
    assert_eq!(t.ent, Some(id));
    assert_eq!(t.trace.hitgroup, 0);
    assert!(near(t.trace.fraction, (84.0 - 0.03125) / 200.0), "{t:?}");
}

#[test]
fn the_shield_hitbox_is_left_out_while_gamestate_is_one() {
    // 21 hitboxes: 20 small boxes far away and, last, a big one in front of the player: the
    // shield (index 20).
    let mut boxes: Vec<BoxDef> = (0..20)
        .map(|i| BoxDef {
            bone: 0,
            group: 2,
            min: [-1.0, -1.0, 200.0 + i as f32 * 4.0],
            max: [1.0, 1.0, 202.0 + i as f32 * 4.0],
        })
        .collect();
    boxes.push(BoxDef {
        bone: 0,
        group: 8,
        min: [-10.0, -10.0, 0.0],
        max: [10.0, 10.0, 70.0],
    });
    let mut h = harness_with_model(build_mdl(
        &[BoneDef::at(-1, [0.0, 0.0, 0.0])],
        &boxes,
        0,
        1,
    ));
    let (with_shield, without) = run(&mut h, |eng| {
        let id = studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            0,
        );
        let shot = |eng: &EngineApi<'_>| {
            ray(eng, Vec3::new(0.0, 0.0, 30.0), Vec3::new(200.0, 0.0, 30.0))
                .trace
                .hitgroup
        };
        let with_shield = shot(eng);
        eng.ent_mut(id).unwrap().gamestate = 1;
        // Touch the entity so its link is current; the pose is read from the variables.
        eng.set_origin(id, Vec3::new(100.0, 0.0, 0.0));
        (with_shield, shot(eng))
    });
    assert_eq!(with_shield, 8, "the shield box is hit");
    assert_eq!(without, 0, "the shield box is skipped while gamestate is 1");
}

#[test]
fn the_pose_follows_the_blend_and_the_angles() {
    // One bone turned by a quarter turn about z in blend 1 only; a thin box along x in it.
    let mut bone = BoneDef::at(-1, [0.0, 0.0, 0.0]);
    bone.rot_z_blend = [0, 1571]; // 1.571 rad: a quarter turn
    bone.scale = 0.001;
    let model = build_mdl(
        &[bone],
        &[BoxDef {
            bone: 0,
            group: CHEST,
            min: [-20.0, -2.0, 0.0],
            max: [20.0, 2.0, 70.0],
        }],
        0,
        2,
    );
    let mut h = harness_with_model(model);
    let (unturned, turned, yawed) = run(&mut h, |eng| {
        let id = studio_entity(eng, Vec3::ZERO, STANDING_MINS, STANDING_MAXS, 0);
        let along_y = |eng: &EngineApi<'_>| {
            ray(eng, Vec3::new(0.0, -100.0, 30.0), Vec3::new(0.0, 100.0, 30.0))
                .trace
                .fraction
        };
        let unturned = along_y(eng);
        eng.ent_mut(id).unwrap().blending = [255, 0];
        let turned = along_y(eng);
        // Back to the first blend, the entity itself turned a quarter about z: the same.
        let v = eng.ent_mut(id).unwrap();
        v.blending = [0, 0];
        v.angles = Vec3::new(0.0, 90.0, 0.0);
        let yawed = along_y(eng);
        (unturned, turned, yawed)
    });
    // The box is 2 deep in y when unturned, 20 when turned.
    assert!(near(unturned, (98.0 - 0.03125) / 200.0), "{unturned}");
    assert!(near(turned, (80.0 - 0.03125) / 200.0), "{turned}");
    assert!((yawed - turned).abs() < 0.001, "{yawed} vs {turned}");
}

#[test]
fn models_are_read_when_precached_and_again_after_a_map_change() {
    let mut h = harness_with_model(two_box_model());
    h.server.change_map(cs_engine::ServerMap::stub("second"));
    h.step(&mut [], 1);
    let t = run(&mut h, |eng| {
        studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            0,
        );
        ray(eng, Vec3::new(0.0, 0.0, 66.0), Vec3::new(200.0, 0.0, 66.0))
    });
    assert_eq!(t.trace.hitgroup, HEAD);
}

#[test]
fn hull_traces_ignore_hitboxes() {
    // A moving box (the hull of a player) is clipped against the entity's own box, as it is
    // against a bare box entity of the same size.
    let mut h = harness_with_model(two_box_model());
    let (studio, plain) = run(&mut h, |eng| {
        studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            0,
        );
        let boxed = eng.create_entity("test_box").unwrap();
        eng.ent_mut(boxed).unwrap().solid = SOLID_BBOX;
        eng.set_size(boxed, STANDING_MINS, STANDING_MAXS);
        eng.set_origin(boxed, Vec3::new(100.0, 300.0, 0.0));
        let hull = |eng: &EngineApi<'_>, y: f32| {
            eng.trace_hull_ents(
                Vec3::new(0.0, y, 36.0),
                Vec3::new(200.0, y, 36.0),
                cs_engine::collision::Hull::Standing,
                MoveFilter::Normal,
                None,
            )
        };
        (hull(eng, 0.0), hull(eng, 300.0))
    });
    assert!(studio.ent.is_some() && plain.ent.is_some());
    assert_eq!(studio.trace.fraction, plain.trace.fraction);
    assert_eq!(studio.trace.hitgroup, 0);
    assert_eq!(plain.trace.hitgroup, 0);
}

#[test]
fn bad_model_data_is_refused_quietly() {
    let good = two_box_model();
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("garbage", vec![0xAB; 5000]),
        ("header only", good[..244].to_vec()),
        ("cut inside the hitboxes", good[..244 + 112 + 40].to_vec()),
    ];
    let mut bad = good.clone();
    put_i32(&mut bad, 140, 100_000); // numbones
    cases.push(("huge bone count", bad));
    let mut bad = good.clone();
    put_i32(&mut bad, 144, -4); // boneindex
    cases.push(("negative bone index", bad));
    let mut bad = good.clone();
    put_i32(&mut bad, 244 + 32, 7); // the bone's parent: itself or later
    cases.push(("bad parent", bad));
    let mut bad = good.clone();
    put_i32(&mut bad, 244 + 112, 9); // the first hitbox's bone
    cases.push(("hitbox on a missing bone", bad));
    let mut bad = good.clone();
    put_i32(&mut bad, 244 + 40, 99); // a bone controller out of range
    cases.push(("controller out of range", bad));
    // Animation offsets that lead out of the file.
    let mut bad = good.clone();
    let seq_off = 244 + 112 + 2 * 32;
    let anim = seq_off + 176;
    put_u16(&mut bad, anim + 10, 0xFFF0);
    cases.push(("animation offset out of the file", bad));
    let mut bad = good.clone();
    put_i32(&mut bad, seq_off + 124, 0x7FFF_FFF0); // animindex
    cases.push(("animindex out of the file", bad));
    let mut bad = good.clone();
    put_i32(&mut bad, seq_off + 156, 3); // an external sequence group
    cases.push(("animations in another file", bad));

    // A run of no frames (it would walk the file without ever using up the frame).
    cases.push(("run of no frames", zero_run_model(1)));

    for (name, bytes) in cases {
        let mut h = harness_with_model(bytes);
        let (id, t) = run(&mut h, |eng| {
            let id = studio_entity(
                eng,
                Vec3::new(100.0, 0.0, 0.0),
                STANDING_MINS,
                STANDING_MAXS,
                FL_CLIENT,
            );
            // Hostile entity variables too.
            let v = eng.ent_mut(id).unwrap();
            v.sequence = 60_000;
            v.frame = f32::NAN;
            (id, ray(eng, Vec3::new(0.0, 0.0, 55.0), Vec3::new(200.0, 0.0, 55.0)))
        });
        // Between the boxes: only the entity's own box is hit, which is what a refused model
        // leaves.
        assert_eq!(t.ent, Some(id), "{name}");
        assert_eq!(t.trace.hitgroup, 0, "{name}");
    }
}

#[test]
fn hostile_pose_variables_do_not_panic() {
    let mut h = harness_with_model(two_box_model());
    run(&mut h, |eng| {
        let id = studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            FL_CLIENT,
        );
        for (sequence, frame, angles) in [
            (0u16, f32::INFINITY, Vec3::ZERO),
            (0, -1.0e30, Vec3::ZERO),
            (0, 1.0e30, Vec3::ZERO),
            (u16::MAX, 255.0, Vec3::new(1.0e30, f32::NAN, 5.0)),
            (0, 300.0, Vec3::splat(-720.0)),
        ] {
            let v = eng.ent_mut(id).unwrap();
            v.sequence = sequence;
            v.frame = frame;
            v.angles = angles;
            let _ = ray(eng, Vec3::new(0.0, 0.0, 66.0), Vec3::new(200.0, 0.0, 66.0));
        }
    });
}

#[test]
fn pellets_and_players_cost_one_pose_each() {
    // The work of the traces is counted in poses made (`studio_setups`), not in time: nine
    // pellets at ten players, all within one call, make at most ten poses.
    let mut h = harness_with_model(two_box_model());
    let (setups, hits) = run(&mut h, |eng| {
        let mut ids = Vec::new();
        for i in 0..10 {
            ids.push(studio_entity(
                eng,
                Vec3::new(100.0, i as f32 * 40.0, 0.0),
                STANDING_MINS,
                STANDING_MAXS,
                FL_CLIENT,
            ));
        }
        let mut hits = 0;
        for pellet in 0..9 {
            for i in 0..10 {
                let y = i as f32 * 40.0;
                let z = 60.0 + pellet as f32;
                let t = ray(eng, Vec3::new(0.0, y, z), Vec3::new(200.0, y, z));
                hits += usize::from(t.trace.hitgroup == HEAD);
            }
        }
        (eng.studio_setups(), hits)
    });
    assert_eq!(hits, 90);
    assert!(setups <= 10, "{setups} poses for 90 traces");
}

// ---- the player models of the installed game -------------------------------------------------

const GIGN: &str = "models/player/gign/gign.mdl";

/// A harness whose test model is the GIGN player model, with the parsed model for its
/// sequences. `None` without the game.
fn gign_harness() -> Option<(Harness, StudioModel)> {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        eprintln!("skipped: the installed game is not found (set CS16_GAMEDIR)");
        return None;
    };
    let Ok(bytes) = vfs.read_bytes(GIGN) else {
        eprintln!("skipped: {GIGN} is not in the installed game");
        return None;
    };
    let Ok(model) = StudioModel::load(&vfs, GIGN) else {
        eprintln!("skipped: {GIGN} does not parse");
        return None;
    };
    Some((harness_with_model(bytes), model))
}

/// The hitgroup met at each height `z` by a ray along +x through a player entity of the GIGN
/// model that stands at the origin, in `sequence` at `frame`.
fn sweep(h: &mut Harness, sequence: u16, frame: f32, shield_disabled: bool) -> Vec<(i32, i32)> {
    run(h, move |eng| {
        let id = studio_entity(
            eng,
            Vec3::ZERO,
            Vec3::new(-16.0, -16.0, -36.0),
            Vec3::new(16.0, 16.0, 36.0),
            FL_CLIENT,
        );
        let v = eng.ent_mut(id).unwrap();
        v.sequence = sequence;
        v.frame = frame;
        v.gamestate = i32::from(shield_disabled);
        let hits = (-60..=100)
            .map(|z| {
                let t = ray(
                    eng,
                    Vec3::new(-100.0, 0.0, z as f32),
                    Vec3::new(100.0, 0.0, z as f32),
                );
                (z, if t.ent == Some(id) { t.trace.hitgroup } else { -1 })
            })
            .collect();
        // Gone again: the next sweep must not meet this one.
        eng.remove_entity(id);
        hits
    })
}

fn top_of(sweep: &[(i32, i32)], group: i32) -> Option<i32> {
    sweep.iter().filter(|(_, g)| *g == group).map(|(z, _)| *z).max()
}

fn bottom_of(sweep: &[(i32, i32)], group: i32) -> Option<i32> {
    sweep.iter().filter(|(_, g)| *g == group).map(|(z, _)| *z).min()
}

#[test]
fn bullet_hits_head_hitbox() {
    let Some((mut h, model)) = gign_harness() else {
        return;
    };
    let idle = model.find_sequence("idle1").expect("idle1") as u16;
    // A player without a shield: `gamestate` 1, as the game sets it.
    let s = sweep(&mut h, idle, 0.0, true);
    let head_top = top_of(&s, HEAD).expect("the head is hit");
    let head_bottom = bottom_of(&s, HEAD).unwrap();
    let chest_top = top_of(&s, CHEST).expect("the chest is hit");
    let stomach = top_of(&s, 3).expect("the stomach is hit");
    let leg = top_of(&s, 6).or_else(|| top_of(&s, 7)).expect("a leg is hit");
    // The head is the top of the player, over the chest, over the stomach, over the legs.
    assert!(head_top > chest_top, "{s:?}");
    assert!(head_bottom > stomach, "{s:?}");
    assert!(chest_top > stomach, "{s:?}");
    assert!(stomach > leg, "{s:?}");
    // The origin of a player is the middle of its hull and the head ends near the top of it.
    assert!((20..=36).contains(&head_top), "head top {head_top}");
    // Above the head and below the feet: nothing.
    assert_eq!(s.iter().find(|(z, _)| *z == 100).unwrap().1, -1);
    assert_eq!(s.iter().find(|(z, _)| *z == -60).unwrap().1, -1);
    // The shield box (group 8, the model's hitbox 20) is not met while the shield is off ...
    assert!(top_of(&s, 8).is_none(), "{s:?}");
    // ... and is met when it is not.
    let with_shield = sweep(&mut h, idle, 0.0, false);
    assert!(top_of(&with_shield, 8).is_some(), "{with_shield:?}");
}

/// Golden points of `idle1` at frame 0 of the GIGN model (a player without a shield, a ray
/// along +x through the middle of the player): taken once from the engine's output, after
/// checking them against the model's data. The check, with the renderer's own pose
/// (`StudioModel::evaluate_bones`, an independent implementation) of the same sequence:
/// - the lowest hitbox points are the feet (hitboxes 3 and 6, bones at z -30.6 and -30.0, boxes
///   reaching -37.0 and -36.6); the engine's lowest hit is z = -37;
/// - the legs (groups 6 and 7) overlap along the middle line: left thigh/calf/foot centres at
///   z -10, -25, -34, right ones at -12, -25, -34; hits z -36..-10 are legs;
/// - the pelvis (hitbox 0, group 3, bone z -2.6, centre -3.0) and Spine1 (hitbox 7, group 3,
///   bone z 3.2, centre 3.9) are boxes of 13.5 and 11 units: the stomach from z -7 to 10;
/// - Spine2/Spine3 (hitboxes 8, 9, group 2, bones z 6.8 and 10.0, centres 9.5 and 12.1) and the
///   neck: the chest above the stomach, z 11..20;
/// - the head (hitbox 11, group 1, bone z 15.9, box 8.3 long along the bone's x, which points
///   up, so its top is near 15.9 + 8.3 = 24..26): the engine's head is z 21..26;
/// - the arms (groups 4, 5) are at |y| of 3 to 18, off the middle line, and the shield (group 8)
///   is skipped by `gamestate` 1: neither is met.
///
/// The heights agree within the boxes' own thickness (the renderer poses a lerp of frames and
/// is not the engine's code, so only agreement of that size is expected).
#[test]
fn idle1_golden_points() {
    let Some((mut h, model)) = gign_harness() else {
        return;
    };
    let idle1 = model.find_sequence("idle1").expect("idle1") as u16;
    let s = sweep(&mut h, idle1, 0.0, true);
    let at = |z: i32| s.iter().find(|(zz, _)| *zz == z).unwrap().1;
    // (z, hitgroup); -1 is a miss.
    let golden = [
        (24, HEAD),
        (15, CHEST),
        (0, 3),
        (-15, 6),
        (-30, 7),
        (40, -1),
        (-50, -1),
    ];
    for (z, group) in golden {
        assert_eq!(at(z), group, "z = {z}");
    }
    // A miss beside the model: the same height as the chest, 40 units to the side.
    let beside = run(&mut h, move |eng| {
        let id = studio_entity(
            eng,
            Vec3::ZERO,
            Vec3::new(-16.0, -16.0, -36.0),
            Vec3::new(16.0, 16.0, 36.0),
            FL_CLIENT,
        );
        let v = eng.ent_mut(id).unwrap();
        v.sequence = idle1;
        v.gamestate = 1;
        let t = ray(eng, Vec3::new(-100.0, 40.0, 15.0), Vec3::new(100.0, 40.0, 15.0));
        (t.ent == Some(id), t.trace.hitgroup, t.trace.fraction)
    });
    assert_eq!(beside, (false, 0, 1.0));
}

#[test]
fn pose_follows_sequence_and_frame() {
    let Some((mut h, model)) = gign_harness() else {
        return;
    };
    let stand = model.find_sequence("ref_aim_knife").expect("ref_aim_knife") as u16;
    let crouch = model.find_sequence("crouch_aim_knife").expect("crouch_aim_knife") as u16;
    let death = model.find_sequence("death1").expect("death1") as u16;
    let lowest = |sweep: &[(i32, i32)]| sweep.iter().filter(|(_, g)| *g >= 0).map(|(z, _)| *z).min();

    // The crouching sequences are posed for a hull that is 18 lower: the feet are 18 higher
    // and the head top is higher relative to the origin.
    let standing = sweep(&mut h, stand, 0.0, true);
    let crouched = sweep(&mut h, crouch, 0.0, true);
    let (stand_feet, crouch_feet) = (lowest(&standing).unwrap(), lowest(&crouched).unwrap());
    assert!(
        (14..=22).contains(&(crouch_feet - stand_feet)),
        "feet {stand_feet} standing, {crouch_feet} crouching"
    );
    assert!(top_of(&crouched, HEAD).unwrap() > top_of(&standing, HEAD).unwrap());

    // A death animation starts standing and ends on the floor; the frame (0..255) moves it.
    let falling_start = top_of(&sweep(&mut h, death, 0.0, true), HEAD).expect("a head at the start");
    let falling_middle = top_of(&sweep(&mut h, death, 128.0, true), HEAD);
    let falling_end = top_of(&sweep(&mut h, death, 255.0, true), HEAD);
    assert!(falling_start > 0, "{falling_start}");
    assert_ne!(falling_middle, Some(falling_start));
    assert!(
        falling_end.is_none_or(|z| z < falling_start - 20),
        "the head ends lower: {falling_end:?} vs {falling_start}"
    );
    // The same pose twice is the same hulls.
    assert_eq!(sweep(&mut h, stand, 0.0, true), standing);
}

/// A model with `boxes` head boxes whose one bone animates its z rotation by a run of no
/// frames (`total` 0).
fn zero_run_model(boxes: usize) -> Vec<u8> {
    let mut animated = BoneDef::at(-1, [0.0, 0.0, 0.0]);
    animated.rot_z_blend = [1, 0];
    let defs: Vec<BoxDef> = (0..boxes)
        .map(|_| BoxDef {
            bone: 0,
            group: HEAD,
            min: [-5.0, -5.0, 60.0],
            max: [5.0, 5.0, 72.0],
        })
        .collect();
    let mut b = build_mdl(&[animated], &defs, 0, 1);
    let runs_off = 244 + 112 + boxes * 32 + 176 + 12;
    b[runs_off + 1] = 0;
    b
}

#[test]
fn a_pose_that_cannot_be_made_is_tried_once_per_call() {
    let mut h = harness_with_model(zero_run_model(1));
    let (setups, ent, id) = run(&mut h, |eng| {
        let id = studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            0,
        );
        let mut last = None;
        for _ in 0..5 {
            last = ray(eng, Vec3::new(0.0, 0.0, 66.0), Vec3::new(200.0, 0.0, 66.0)).ent;
        }
        (eng.studio_setups(), last, id)
    });
    assert_eq!(ent, Some(id), "traced as its box");
    assert_eq!(setups, 1, "the failed pose is remembered");
}

#[test]
fn flagged_model_traces_a_box_through_its_hitboxes_grown_by_half_its_size() {
    // `STUDIO_TRACE_HITBOX` (0x200) in the header: a move of a box, not only a bullet, is
    // clipped against the hitboxes, grown by `(maxs - mins) * 0.5` (here 4 on each side).
    let head = || BoxDef {
        bone: 0,
        group: HEAD,
        min: [-5.0, -5.0, 60.0],
        max: [5.0, 5.0, 72.0],
    };
    let trace = |flags: i32| {
        let mut h = harness_with_model(build_mdl(
            &[BoneDef::at(-1, [0.0, 0.0, 0.0])],
            &[head()],
            flags,
            1,
        ));
        run(&mut h, |eng| {
            studio_entity(
                eng,
                Vec3::new(100.0, 0.0, 0.0),
                STANDING_MINS,
                STANDING_MAXS,
                0,
            );
            eng.trace_box(
                Vec3::new(0.0, 0.0, 66.0),
                Vec3::splat(-4.0),
                Vec3::splat(4.0),
                Vec3::new(200.0, 0.0, 66.0),
                MoveFilter::Normal,
                None,
            )
        })
    };
    let flagged = trace(0x200);
    assert_eq!(flagged.trace.hitgroup, HEAD);
    assert!(near(flagged.trace.fraction, (91.0 - 0.03125) / 200.0), "{flagged:?}");
    // Without the flag the same move meets the entity's own box (16 + 4 from its origin).
    let plain = trace(0);
    assert_eq!(plain.trace.hitgroup, 0);
    assert!(near(plain.trace.fraction, (80.0 - 0.03125) / 200.0), "{plain:?}");
}

#[test]
fn an_entity_moved_within_a_call_gets_a_fresh_pose() {
    let mut h = harness_with_model(two_box_model());
    let (first, second, setups) = run(&mut h, |eng| {
        let id = studio_entity(
            eng,
            Vec3::new(100.0, 0.0, 0.0),
            STANDING_MINS,
            STANDING_MAXS,
            0,
        );
        let shot = |eng: &EngineApi<'_>| {
            ray(eng, Vec3::new(0.0, 0.0, 66.0), Vec3::new(300.0, 0.0, 66.0))
                .trace
                .fraction
        };
        let first = shot(eng);
        eng.set_origin(id, Vec3::new(150.0, 0.0, 0.0));
        (first, shot(eng), eng.studio_setups())
    });
    assert!(near(first, (95.0 - 0.03125) / 300.0), "{first}");
    assert!(near(second, (145.0 - 0.03125) / 300.0), "{second}");
    assert_eq!(setups, 2);
}
