//! Local play over the loopback: the engine and the game in process, the real client core on
//! the other end. Time is counted in frames, never read from a clock.

use cs_game::player::Team;
use cs_local::{LocalGame, LocalOptions};
use cs_vfs::{Vfs, VfsConfig};

fn discover() -> Option<Vfs> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok();
    if vfs.is_none() {
        eprintln!("SKIPPED: no game directory (set CS16_GAMEDIR)");
    }
    vfs
}

/// Joins the counter-terrorists as the menus do (`jointeam 2`, `joinclass 1`) and runs until
/// the game has the player on that team.
fn join_counter_terrorists(game: &mut LocalGame, slot: u8) {
    game.connection_mut().send_stringcmd("jointeam 2");
    game.run_frames(20);
    game.connection_mut().send_stringcmd("joinclass 1");
    for _ in 0..600 {
        game.step();
        let joined = game.server().game().state().players[usize::from(slot)]
            .as_ref()
            .is_some_and(|p| p.team == Team::Ct && p.health > 0.0);
        if joined {
            return;
        }
    }
    panic!("the player never joined the counter-terrorists");
}

#[test]
fn local_game_starts_and_spawns_the_player() {
    let Some(vfs) = discover() else { return };
    let mut game = LocalGame::start(&vfs, "de_dust2", &LocalOptions::default()).expect("start");
    let slot = game.connection().player_slot().expect("slot");
    join_counter_terrorists(&mut game, slot);
    // Let the world settle: a few frames of delta updates after the spawn.
    game.run_frames(200);

    let conn = game.connection();
    assert_eq!(conn.stats().parse_errors, 0);
    assert!(conn.stats().delta_updates > 0);
    assert!(conn.server_info().is_some_and(|i| i.map_path.contains("de_dust2")));

    let player = game.server().game().state().players[usize::from(slot)]
        .as_ref()
        .expect("the game has the player");
    assert_eq!(player.team, Team::Ct);
    assert!(player.health > 0.0);

    // The client sees its own entity and, in the client data, where the server put it.
    let ent = conn
        .entities()
        .get(&(u16::from(slot) + 1))
        .expect("the client has its own entity");
    assert!(ent.modelindex > 0, "{ent:?}");
    // The own player's position travels in the client data (the entity's origin is not used).
    let cd = conn.client_data().expect("client data");
    assert!(cd.origin.length() > 1.0, "{:?}", cd.origin);
}

#[test]
fn local_game_with_bots_plays_a_round() {
    let Some(vfs) = discover() else { return };
    let opts = LocalOptions {
        bots: 4,
        ..LocalOptions::default()
    };
    let mut game = LocalGame::start(&vfs, "de_dust2", &opts).expect("start");
    let slot = game.connection().player_slot().expect("slot");
    join_counter_terrorists(&mut game, slot);

    let mut finished = false;
    for _ in 0..400 {
        game.run_frames(100);
        let state = game.server().game().state();
        if state.num_terrorist_wins + state.num_ct_wins >= 1 {
            finished = true;
            break;
        }
    }
    let bots = (0..16u8)
        .filter(|&s| s != slot && game.server().game().state().players[usize::from(s)].is_some())
        .count();
    assert!(bots >= 1, "bot_quota brought bots in");
    assert!(finished, "a round must end with a winner");
    assert_eq!(game.connection().stats().parse_errors, 0);
    assert!(!game.connection().state().is_terminal());
}
