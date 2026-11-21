//! The server console: `Server::console_command` driven against probe clients on the loopback.

mod common;

use common::Harness;
use cs_engine::{ClientState, ConsoleOutcome, MAX_CONSOLE_LINE};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::messages::SvcMessage;
use cs_transport::LoopbackEndpoint;

type Probe = ProbeClient<LoopbackEndpoint>;

fn joined(h: &mut Harness, name: &str, others: &mut [&mut Probe]) -> Probe {
    let mut probe = h.add_probe(name);
    for _ in 0..300 {
        let mut all: Vec<&mut Probe> = others.iter_mut().map(|p| &mut **p).collect();
        all.push(&mut probe);
        h.step(&mut all, 1);
        if probe.state() == &ProbeState::InGame {
            return probe;
        }
    }
    panic!("{name} did not get in the game: {:?}", probe.state());
}

fn out(h: &mut Harness, line: &str) -> String {
    match h.server.console_command(line) {
        ConsoleOutcome::Output(text) => text,
        other => panic!("{line:?}: expected Output, got {other:?}"),
    }
}

fn prints(p: &Probe) -> Vec<String> {
    p.messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Print(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn status_lists_connected_clients() {
    let mut h = Harness::new(4);
    let text = out(&mut h, "status");
    assert_eq!(
        text,
        format!("hostname: {}\nmap: test\n", h.server.config().hostname)
    );

    let mut a = joined(&mut h, "Alice", &mut []);
    let mut b = joined(&mut h, "Bob", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 5);
    let text = out(&mut h, "STATUS");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines[0].starts_with("hostname: "));
    assert_eq!(lines[1], "map: test");
    let id_a = h.server.client_userid(0).unwrap();
    let id_b = h.server.client_userid(1).unwrap();
    assert!(
        lines[2].starts_with(&format!("1 {id_a} \"Alice\" active ")),
        "{}",
        lines[2]
    );
    assert!(lines[2].contains("ms loopback:"), "{}", lines[2]);
    assert!(
        lines[3].starts_with(&format!("2 {id_b} \"Bob\" active ")),
        "{}",
        lines[3]
    );

    // One source for the name: the configuration. The `hostname` cvar is not consulted, so
    // `status`, `svc_serverinfo` and the A2S info reply cannot disagree.
    out(&mut h, "hostname \"My Server\"");
    let configured = h.server.config().hostname.clone();
    assert!(out(&mut h, "status").starts_with(&format!("hostname: {configured}\nmap: test\n")));
    let served = a.messages().iter().find_map(|m| match m {
        SvcMessage::ServerInfo(info) => Some(info.hostname.clone()),
        _ => None,
    });
    assert_eq!(served, Some(configured));
}

#[test]
fn status_cleans_client_names() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "Alice", &mut []);
    h.server
        .game_mut()
        .act(|eng| eng.set_client_key(0, "name", "ev\x1b[2Jil\x07\n9 99 \"forged\""));
    h.step(&mut [&mut a], 3);
    let text = out(&mut h, "status");
    assert!(text.contains("\"ev?[2Jil??9 99 \"forged\"\""), "{text:?}");
    assert_eq!(text.lines().count(), 3, "forged line: {text:?}");
    assert!(!text.contains('\x1b') && !text.contains('\x07'));
}

#[test]
fn kick_by_userid_slot_and_name() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "Alice", &mut []);
    let mut b = joined(&mut h, "Bob", &mut [&mut a]);
    let mut c = joined(&mut h, "Carol", &mut [&mut a, &mut b]);
    let id_a = h.server.client_userid(0).unwrap();

    // Nobody is kicked for a bad target.
    for line in ["kick", "kick 99", "kick 0", "kick nobody", "kick #4242", "kick #", "kick #x"] {
        let text = out(&mut h, line);
        assert!(!text.is_empty() && text.ends_with('\n'), "{line}: {text:?}");
    }
    assert_eq!(h.server.client_count(), 3);

    // By userid, with a reason made of the remaining arguments.
    let text = out(&mut h, &format!("kick #{id_a} too   good \"for here\""));
    assert!(text.contains("Alice"), "{text}");
    h.step(&mut [&mut a, &mut b, &mut c], 5);
    assert_eq!(
        a.state(),
        &ProbeState::Disconnected("too good for here".to_string())
    );
    assert_eq!(h.server.client_state(0), ClientState::Free);

    // By slot as `status` shows it (1-based): Bob is slot 2.
    out(&mut h, "kick 2");
    h.step(&mut [&mut b, &mut c], 5);
    assert_eq!(
        b.state(),
        &ProbeState::Disconnected("Kicked by server".to_string())
    );

    // By name, ignoring case.
    out(&mut h, "kick cAROL bye");
    h.step(&mut [&mut c], 5);
    assert_eq!(c.state(), &ProbeState::Disconnected("bye".to_string()));
    assert_eq!(h.server.client_count(), 0);
}

#[test]
fn kick_name_must_match_exactly_one_client() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "Twin", &mut []);
    let mut b = joined(&mut h, "Other", &mut [&mut a]);
    // The engine keeps names unique at connect; a game can still set a duplicate.
    h.server
        .game_mut()
        .act(|eng| eng.set_client_key(1, "name", "twin"));
    h.step(&mut [&mut a, &mut b], 3);
    let text = out(&mut h, "kick TWIN");
    assert!(text.contains("More than one"), "{text}");
    h.step(&mut [&mut a, &mut b], 5);
    assert_eq!(h.server.client_count(), 2);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(b.state(), &ProbeState::InGame);
    // A prefix is not a match.
    assert!(out(&mut h, "kick Tw").contains("Can't find"));
}

#[test]
fn say_reaches_everyone() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    assert_eq!(out(&mut h, "say hello   \"big world\""), "");
    h.step(&mut [&mut a, &mut b], 5);
    for p in [&a, &b] {
        assert!(
            prints(p).contains(&"Console: hello big world\n".to_string()),
            "{:?}",
            prints(p)
        );
    }
    assert!(out(&mut h, "say").starts_with("Usage"));
    // An empty quoted argument is still text: an empty line is said.
    assert_eq!(out(&mut h, "say \"\""), "");
    h.step(&mut [&mut a, &mut b], 5);
    assert!(prints(&a).contains(&"Console: \n".to_string()));
}

#[test]
fn changelevel_validates_the_name() {
    let mut h = Harness::new(2);
    assert_eq!(
        h.server.console_command("changelevel de_dust2"),
        ConsoleOutcome::ChangeLevel("de_dust2".to_string())
    );
    assert_eq!(
        h.server.console_command("MAP cs_Office-2"),
        ConsoleOutcome::ChangeLevel("cs_Office-2".to_string())
    );
    let max = "a".repeat(64);
    assert_eq!(
        h.server.console_command(&format!("map {max}")),
        ConsoleOutcome::ChangeLevel(max)
    );
    let invalid = "Invalid map name: use 1 to 64 letters, digits, '_' or '-'\n";
    let usage = "Usage: changelevel <map>\n";
    for (line, expected) in [
        ("changelevel ../x", invalid),
        ("changelevel", usage),
        ("changelevel \"\"", invalid),
        (&format!("changelevel {}", "a".repeat(65)), invalid),
        ("changelevel a b", usage),
        ("map de dust", usage),
        ("changelevel a/b", invalid),
        ("changelevel a\\b", invalid),
        ("changelevel \u{43a}arta", invalid),
        ("changelevel a.bsp", invalid),
    ] {
        assert_eq!(out(&mut h, line), expected, "{line}");
    }
}

#[test]
fn quit_and_exit_are_quit() {
    let mut h = Harness::new(2);
    assert_eq!(h.server.console_command("quit"), ConsoleOutcome::Quit);
    assert_eq!(h.server.console_command("  Exit  now"), ConsoleOutcome::Quit);
}

#[test]
fn cvars_can_be_read_and_set() {
    let mut h = Harness::new(2);
    let before = h.server.console_command("sv_gravity");
    assert_eq!(before, ConsoleOutcome::Output("\"sv_gravity\" is \"800\"\n".to_string()));
    assert_eq!(out(&mut h, "sv_gravity 600 extra ignored"), "");
    assert_eq!(out(&mut h, "SV_Gravity"), "\"sv_gravity\" is \"600\"\n");
    assert_eq!(out(&mut h, "sv_gravity \"700\""), "");
    assert_eq!(out(&mut h, "sv_gravity"), "\"sv_gravity\" is \"700\"\n");
    assert_eq!(out(&mut h, "nosuch 1"), "Unknown command \"nosuch\"\n");
    assert_eq!(out(&mut h, "nosuch"), "Unknown command \"nosuch\"\n");
}

#[test]
fn game_commands_are_dispatched() {
    let mut h = Harness::new(2);
    assert_eq!(out(&mut h, "gamecmd 1 \"two words\""), "game says hi\n");
    assert_eq!(
        h.server.game().server_commands,
        vec![vec!["gamecmd".to_string(), "1".to_string(), "two words".to_string()]]
    );
    // The game's text is cleaned of control characters but keeps its newlines.
    assert_eq!(out(&mut h, "gameesc"), "a?[31mb\nc");
    // Engine commands and cvars come first; the game is not asked.
    h.server.game_mut().server_commands.clear();
    out(&mut h, "status");
    out(&mut h, "sv_gravity");
    assert!(h.server.game().server_commands.is_empty());
    // An unclaimed command is offered to the game, then reported with a cleaned name.
    assert_eq!(out(&mut h, "bogus\x1b[1m"), "Unknown command \"bogus?[1m\"\n");
    assert_eq!(h.server.game().server_commands.len(), 1);
}

#[test]
fn hostile_console_input_is_harmless() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "Alice", &mut []);
    let huge = "x".repeat(100 * 1024);
    let multibyte = "\u{44f}".repeat(MAX_CONSOLE_LINE); // cut lands inside a character
    let lines = [
        "".to_string(),
        "   \t ".to_string(),
        huge.clone(),
        format!("say {huge}"),
        format!("kick {huge}"),
        format!("sv_gravity {huge}"),
        format!("say {multibyte}"),
        "say \"unclosed quote".to_string(),
        "kick \"".to_string(),
        "\"".to_string(),
        "\"\"".to_string(),
        "say a\0kick 1".to_string(),
        "\0".to_string(),
        "\x01\x02\x1b[2J\x7f".to_string(),
        "kick".to_string(),
        "kick #".to_string(),
        "kick #99999999999999999999".to_string(),
        "kick 99999999999999999999".to_string(),
        "kick -1".to_string(),
        "changelevel".to_string(),
        "map \"\"".to_string(),
        "status extra args".to_string(),
        "\u{feff}status".to_string(),
    ];
    for line in &lines {
        match h.server.console_command(line) {
            ConsoleOutcome::Output(text) => {
                assert!(
                    !text.chars().any(|c| (c < ' ' && c != '\n') || c == '\x7f'),
                    "control character in the output for {:.40?}",
                    line
                );
            }
            other => panic!("{:.40?}: {other:?}", line),
        }
        h.step(&mut [&mut a], 1);
    }
    // NUL cut the line: nothing after it ran, and the client is still in the game.
    h.step(&mut [&mut a], 10);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_count(), 1);
    // The NUL cut: nothing after the NUL is parsed.
    assert_eq!(out(&mut h, "sv_gravity 5\0 7"), "");
    assert_eq!(out(&mut h, "sv_gravity"), "\"sv_gravity\" is \"5\"\n");
    // The length clip: a command that starts after the limit never runs...
    let late = format!("{}kick 1", " ".repeat(MAX_CONSOLE_LINE + 6));
    assert_eq!(out(&mut h, &late), "");
    // ...and a long value is cut at the limit minus the prefix.
    assert_eq!(out(&mut h, &format!("sv_gravity {}", "9".repeat(2000))), "");
    assert_eq!(
        out(&mut h, "sv_gravity").len(),
        "\"sv_gravity\" is \"\"\n".len() + MAX_CONSOLE_LINE - "sv_gravity ".len()
    );
    h.step(&mut [&mut a], 5);
    assert_eq!(a.state(), &ProbeState::InGame);
    // A blank line and an empty quoted name are no-ops.
    assert_eq!(out(&mut h, "  "), "");
    assert_eq!(out(&mut h, "\"\""), "");
    // The line is cut at the limit: the long `say` arrived with a bounded text.
    let said = prints(&a);
    assert!(said.iter().all(|t| t.len() <= MAX_CONSOLE_LINE + 16), "{}", said.len());
}

#[test]
fn the_game_can_ask_for_a_map_change() {
    let mut h = Harness::new(2);
    assert_eq!(h.server.take_pending_change_level(), None);
    let accepted = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = std::rc::Rc::clone(&accepted);
    h.server.game_mut().act(move |eng| {
        let mut sink = sink.borrow_mut();
        // Names the console's `changelevel` refuses are ignored and nothing is recorded.
        for bad in ["", "../x", "a/b", "a b", "a.bsp", &"a".repeat(65)] {
            sink.push(eng.change_level(bad));
        }
        sink.push(eng.change_level("de_dust2"));
        // The last valid request wins.
        sink.push(eng.change_level("cs_office"));
        sink.push(eng.change_level("not/valid"));
    });
    h.step(&mut [], 1);
    assert_eq!(
        *accepted.borrow(),
        [false, false, false, false, false, false, true, true, false]
    );
    assert_eq!(
        h.server.take_pending_change_level(),
        Some("cs_office".to_string())
    );
    // Taken once.
    assert_eq!(h.server.take_pending_change_level(), None);
}
