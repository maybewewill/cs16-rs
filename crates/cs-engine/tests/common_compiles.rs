mod common;

use common::TestGame;

#[test]
fn test_game_default_is_empty() {
    let game = TestGame::default();
    assert_eq!(game.refuse, None);
    assert_eq!(game.extra_entities, 0);
    assert_eq!(game.late_precache, None);
    assert!(game.commands.is_empty());
    assert_eq!(
        [
            game.server_activate_calls,
            game.server_deactivate_calls,
            game.client_connect_calls,
            game.client_put_in_server_calls,
            game.client_disconnect_calls,
            game.client_command_calls,
            game.client_userinfo_changed_calls,
            game.run_usercmd_calls,
            game.start_frame_calls,
            game.update_client_data_calls,
        ],
        [0; 10]
    );
}
