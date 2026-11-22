//! What a client is sent of the other entities, the game's part (`AddToFullPack` and the delta
//! encoders of ReGameDLL `client.cpp`): the `FL_SPECTATOR` and `FL_SKIPLOCALHOST` filters, the
//! second of grace an entity keeps in the packet after it leaves the PVS, and the local player's
//! origin, which is not sent. The wire is what the probe client decodes.

mod common;

use common::{Bench, stub_spawns};
use cs_engine::consts::{FL_SKIPLOCALHOST, FL_SPECTATOR};
use cs_engine::{EdictId, FullPackCtx, GameDll, LinkSnapshot};
use cs_proto48::delta::GoldSrcEntityState;
use glam::Vec3;

/// A model entity at `origin` with `flags`, made by the engine alone (the game has no class for
/// it), owned by `owner`.
fn spawn_model_entity(
    b: &mut Bench,
    origin: Vec3,
    flags: u32,
    owner: Option<EdictId>,
) -> EdictId {
    b.run(move |_, eng| {
        let id = eng.create_entity("test_prop").expect("an edict");
        let model = eng.model_index("models/w_weaponbox.mdl").expect("precached by the weapons");
        let v = eng.ent_mut(id).expect("live");
        v.modelindex = model;
        v.flags |= flags;
        v.owner = owner;
        eng.set_origin(id, origin);
        id
    })
}

#[test]
fn a_spectator_flagged_entity_is_sent_to_nobody_but_itself() {
    let mut b = Bench::stub(stub_spawns());
    b.step(30);
    let prop = spawn_model_entity(&mut b, Vec3::new(50.0, 0.0, 0.0), 0, None);
    b.step(30);
    assert!(b.probe.entities().contains_key(&prop.index), "a plain entity is sent");
    b.run(move |_, eng| eng.ent_mut(prop).unwrap().flags |= FL_SPECTATOR);
    b.step(30);
    assert!(
        !b.probe.entities().contains_key(&prop.index),
        "`FL_SPECTATOR` on another entity: not sent"
    );
    // The joining player is itself `FL_SPECTATOR` (`ClientPutInServer`) and is in its own packet.
    let own = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().flags);
    assert_ne!(own & FL_SPECTATOR, 0);
    assert!(b.probe.entities().contains_key(&1), "the host sees itself");
}

#[test]
fn skiplocalhost_hides_an_owned_entity_from_a_predicting_owner_only() {
    // The probe predicts (`cl_lw 1`, the default): `hostflags & 1`.
    let mut b = Bench::stub(stub_spawns());
    b.step(30);
    let owner = b.run(|_, eng| eng.client_edict(0));
    let prop = spawn_model_entity(&mut b, Vec3::new(50.0, 0.0, 0.0), FL_SKIPLOCALHOST, Some(owner));
    b.step(30);
    assert!(
        !b.probe.entities().contains_key(&prop.index),
        "a predicting owner does not get its own `FL_SKIPLOCALHOST` entity"
    );
    // The same entity of an owner that does not predict is sent.
    let mut b = Bench::with_userinfo(
        cs_engine::ServerMap::stub_with_entities("test", stub_spawns()),
        2,
        &[("cl_lw", "0")],
    );
    b.step(30);
    let owner = b.run(|_, eng| eng.client_edict(0));
    let prop = spawn_model_entity(&mut b, Vec3::new(50.0, 0.0, 0.0), FL_SKIPLOCALHOST, Some(owner));
    b.step(30);
    assert!(b.probe.entities().contains_key(&prop.index), "`cl_lw 0`: sent");
}

#[test]
fn the_local_players_origin_is_not_sent() {
    let mut b = Bench::stub(stub_spawns());
    common::join(&mut b, 0, 2, 1);
    b.step(30);
    let origin = b.run(|_, eng| eng.ent(eng.client_edict(0)).unwrap().origin);
    assert_ne!(origin, Vec3::ZERO, "the player stands on a spawn point");
    let state = b.probe.entities().get(&1).expect("the player is in its own packet");
    // `Player_Encode`: "Never send origin to local player, it's sent with more resolution in
    // clientdata_t structure": the client never got a value for it.
    assert_eq!(state.origin, Vec3::ZERO);
}

/// `CheckEntityRecentlyInPVS`: an entity that was seen is still sent for a second after it is no
/// longer in the PVS; the history is forgotten when the leaves the viewer's edict touches change.
/// The questions are asked for the slot nobody plays in, whose history the server's own frames
/// leave alone.
#[test]
fn an_entity_keeps_its_place_in_the_packet_for_a_second_out_of_the_pvs() {
    let mut b = Bench::stub(stub_spawns());
    b.step(30);
    let prop = spawn_model_entity(&mut b, Vec3::new(50.0, 0.0, 0.0), 0, None);
    let leaves = LinkSnapshot { leafnums: vec![3, 4], headnode: -1, area: None };
    let ask = move |game: &mut cs_game::CsGame,
                    eng: &mut cs_engine::EngineApi<'_>,
                    in_pvs: bool,
                    leaves: &LinkSnapshot| {
        let ctx = FullPackCtx {
            ent: prop.index,
            ent_id: prop,
            viewer: 1,
            viewer_id: eng.client_edict(1),
            hostflags: 0,
            in_pvs,
            viewer_leaves: leaves,
        };
        game.add_to_full_pack(eng, &ctx, &mut GoldSrcEntityState::default())
    };
    let l = leaves.clone();
    // Never seen and not visible: out.
    assert!(!b.run(move |g, eng| ask(g, eng, false, &l)));
    // Seen: in, and for the next second in whatever the engine says.
    let l = leaves.clone();
    assert!(b.run(move |g, eng| ask(g, eng, true, &l)));
    let l = leaves.clone();
    assert!(b.run(move |g, eng| ask(g, eng, false, &l)), "less than a second after it was seen");
    // More than a second later it is out (the frames are 10 ms).
    b.step(105);
    let l = leaves.clone();
    assert!(!b.run(move |g, eng| ask(g, eng, false, &l)), "more than a second after");
    // Seen again, then the viewer's leaves change: the history is forgotten.
    let l = leaves.clone();
    assert!(b.run(move |g, eng| ask(g, eng, true, &l)));
    let other = LinkSnapshot { leafnums: vec![9], headnode: -1, area: None };
    assert!(!b.run(move |g, eng| ask(g, eng, false, &other)), "other leaves: the history is reset");
}
