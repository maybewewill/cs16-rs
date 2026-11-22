//! The movevars the engine builds from its cvars (`SV_SetMoveVars`) and sends to the clients
//! (`svc_newmovevars`), once at the `new` stage and again when they change.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::Harness;
use cs_cvar::{CVarFlags, CVarRegistry};
use cs_engine::{ClientState, SignonStage};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::messages::{MoveVarsMsg, SvcMessage, svc};
use cs_transport::LoopbackEndpoint;

type Probe = ProbeClient<LoopbackEndpoint>;

/// The movevars the engine reports after `sets` were applied at the start of a frame.
fn move_vars_after(sets: &'static [(&'static str, &'static str)]) -> MoveVarsMsg {
    let mut h = Harness::new(2);
    let seen = Rc::new(RefCell::new(None));
    let out = Rc::clone(&seen);
    h.server.game_mut().act(move |eng| {
        for (name, value) in sets {
            eng.set_cvar(name, value);
        }
        *out.borrow_mut() = Some(eng.move_vars());
    });
    h.step(&mut [], 1);
    seen.take().expect("the action ran")
}

fn in_game_probe(h: &mut Harness) -> Probe {
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
}

fn newmovevars_count(probe: &Probe) -> usize {
    probe
        .opcode_log()
        .iter()
        .filter(|&&op| op == svc::NEWMOVEVARS)
        .count()
}

fn last_movevars(probe: &Probe) -> MoveVarsMsg {
    probe
        .messages()
        .iter()
        .rev()
        .find_map(|m| match m {
            SvcMessage::NewMoveVars(mv) => Some(MoveVarsMsg::clone(mv)),
            _ => None,
        })
        .expect("the probe was sent movevars")
}

#[test]
fn move_vars_follow_every_cvar() {
    // Each field of `movevars_t` moves with its own cvar; every value differs from the default
    // and from the others, so a field read from the wrong cvar shows.
    let mv = move_vars_after(&[
        ("sv_gravity", "601"),
        ("sv_stopspeed", "102"),
        ("sv_maxspeed", "303"),
        ("sv_spectatormaxspeed", "504"),
        ("sv_accelerate", "7"),
        ("sv_airaccelerate", "11"),
        ("sv_wateraccelerate", "12"),
        ("sv_friction", "3.5"),
        ("edgefriction", "2.5"),
        ("sv_waterfriction", "1.5"),
        ("sv_bounce", "0.5"),
        ("sv_stepsize", "20"),
        ("sv_maxvelocity", "1500"),
        ("sv_zmax", "8192"),
        ("sv_wateramp", "3"),
        ("mp_footsteps", "0"),
        ("sv_rollangle", "2"),
        ("sv_rollspeed", "250"),
        ("sv_skycolor_r", "10"),
        ("sv_skycolor_g", "20"),
        ("sv_skycolor_b", "30"),
        ("sv_skyvec_x", "0.25"),
        ("sv_skyvec_y", "0.5"),
        ("sv_skyvec_z", "0.75"),
        ("sv_skyname", "des"),
    ]);
    assert_eq!(
        mv,
        MoveVarsMsg {
            gravity: 601.0,
            stopspeed: 102.0,
            maxspeed: 303.0,
            spectatormaxspeed: 504.0,
            accelerate: 7.0,
            airaccelerate: 11.0,
            wateraccelerate: 12.0,
            friction: 3.5,
            edgefriction: 2.5,
            waterfriction: 1.5,
            // `SV_SetMoveVars` always writes 1.
            entgravity: 1.0,
            bounce: 0.5,
            stepsize: 20.0,
            maxvelocity: 1500.0,
            zmax: 8192.0,
            wave_height: 3.0,
            footsteps: false,
            rollangle: 2.0,
            rollspeed: 250.0,
            skycolor: [10.0, 20.0, 30.0],
            skyvec: [0.25, 0.5, 0.75],
            sky_name: "des".to_string(),
        }
    );
    // `movevars->footsteps = sv_footsteps.value` is a float stored in an int: 0.5 is off.
    assert!(!move_vars_after(&[("mp_footsteps", "0.5")]).footsteps);
    assert!(move_vars_after(&[("mp_footsteps", "2")]).footsteps);
}

#[test]
fn new_movement_cvars_have_rehlds_defaults() {
    // ReHLDS `sv_main.cpp`, `sv_user.cpp`, `sv_phys.cpp` (read 2026-11-22).
    let reg = CVarRegistry::new_with_defaults();
    let server = CVarFlags::SERVER;
    let cases: [(&str, &str, bool); 13] = [
        ("sv_spectatormaxspeed", "500", false),
        ("sv_wateraccelerate", "10", true),
        ("edgefriction", "2", true),
        ("sv_waterfriction", "1", true),
        ("sv_rollangle", "0", false),
        ("sv_rollspeed", "0", false),
        ("mp_footsteps", "1", true),
        ("sv_skycolor_r", "0", false),
        ("sv_skycolor_g", "0", false),
        ("sv_skycolor_b", "0", false),
        ("sv_skyvec_x", "0", false),
        ("sv_skyvec_y", "0", false),
        ("sv_skyvec_z", "0", false),
    ];
    for (name, default, is_server) in cases {
        let cvar = reg.get(name).unwrap_or_else(|| panic!("{name} is registered"));
        assert_eq!(cvar.value, default, "{name}");
        assert_eq!(cvar.flags.contains(server), is_server, "{name} flags");
    }
    // `sv_stopspeed` stays the engine's 100; the game sets 75 itself.
    assert_eq!(reg.get_f32("sv_stopspeed"), 100.0);

    // The movevars a fresh server reports are those defaults (rollspeed 0, not the 200 of
    // `MoveVarsMsg::default`).
    let mv = move_vars_after(&[]);
    assert_eq!(mv.spectatormaxspeed, 500.0);
    assert_eq!(mv.wateraccelerate, 10.0);
    assert_eq!(mv.edgefriction, 2.0);
    assert_eq!(mv.waterfriction, 1.0);
    assert_eq!((mv.rollangle, mv.rollspeed), (0.0, 0.0));
    assert!(mv.footsteps);
    assert_eq!(mv.entgravity, 1.0);
    assert_eq!(mv.stopspeed, 100.0);
}

#[test]
fn changed_movevars_are_resent_once() {
    let mut h = Harness::new(2);
    let mut probe = in_game_probe(&mut h);
    assert_eq!(newmovevars_count(&probe), 1, "sent at the new stage");
    assert_eq!(last_movevars(&probe).gravity, 800.0);

    h.server.game_mut().act(|eng| eng.set_cvar("sv_gravity", "600"));
    h.step(&mut [&mut probe], 50);
    assert_eq!(newmovevars_count(&probe), 2, "resent once after the change");
    let mv = last_movevars(&probe);
    assert_eq!(mv.gravity, 600.0);
    assert_eq!(mv, move_vars_after(&[("sv_gravity", "600")]));

    // Another field changes: once more; a third frame set later adds nothing.
    h.server.game_mut().act(|eng| eng.set_cvar("edgefriction", "3"));
    h.step(&mut [&mut probe], 50);
    assert_eq!(newmovevars_count(&probe), 3);
    assert_eq!(last_movevars(&probe).edgefriction, 3.0);
}

#[test]
fn unchanged_movevars_are_not_resent() {
    let mut h = Harness::new(2);
    let mut probe = in_game_probe(&mut h);
    h.step(&mut [&mut probe], 100);
    assert_eq!(newmovevars_count(&probe), 1);

    // Setting a cvar to the value it has changes nothing, and neither does a cvar that is not
    // a movevar.
    h.server.game_mut().act(|eng| {
        eng.set_cvar("sv_gravity", "800");
        eng.set_cvar("mp_roundtime", "3");
    });
    h.step(&mut [&mut probe], 50);
    assert_eq!(newmovevars_count(&probe), 1);
}

#[test]
fn only_clients_in_the_game_are_resent_and_a_signing_on_client_gets_the_new_value() {
    let mut h = Harness::new(3);
    let mut a = in_game_probe(&mut h);
    assert_eq!(newmovevars_count(&a), 1);

    // B connects, then is no longer pumped as soon as the server waits for its `new`.
    let mut b = h.add_probe("Second");
    for _ in 0..50 {
        if h.server.client_state(1) == ClientState::Signon(SignonStage::AwaitNew) {
            break;
        }
        h.step(&mut [&mut a, &mut b], 1);
    }
    assert_eq!(
        h.server.client_state(1),
        ClientState::Signon(SignonStage::AwaitNew)
    );

    h.server.game_mut().act(|eng| eng.set_cvar("sv_gravity", "700"));
    h.step(&mut [&mut a], 50);
    assert_eq!(newmovevars_count(&a), 2, "A: exactly one extra message");
    assert_eq!(last_movevars(&a).gravity, 700.0);
    assert_eq!(newmovevars_count(&b), 0, "B was not resent anything");

    // B finishes the sign-on: its `new` stage carries the new value, and nothing follows it.
    h.step(&mut [&mut a, &mut b], 300);
    assert_eq!(b.state(), &ProbeState::InGame);
    assert_eq!(newmovevars_count(&b), 1);
    assert_eq!(last_movevars(&b).gravity, 700.0);
    assert_eq!(newmovevars_count(&a), 2);
}

#[test]
fn every_client_in_the_game_is_resent_exactly_once() {
    let mut h = Harness::new(3);
    let mut a = in_game_probe(&mut h);
    let mut b = h.add_probe("Second");
    h.step(&mut [&mut a, &mut b], 300);
    assert_eq!(b.state(), &ProbeState::InGame);
    assert_eq!(newmovevars_count(&a), 1);
    assert_eq!(newmovevars_count(&b), 1);

    h.server.game_mut().act(|eng| eng.set_cvar("sv_gravity", "650"));
    h.step(&mut [&mut a, &mut b], 100);
    assert_eq!(newmovevars_count(&a), 2);
    assert_eq!(newmovevars_count(&b), 2);
    assert_eq!(last_movevars(&a).gravity, 650.0);
    assert_eq!(last_movevars(&b).gravity, 650.0);
}

