mod common;

use common::Harness;
use cs_engine::ConsoleOutcome;
use cs_transport::{PacketTransport, PeerAddr, UdpTransport};

#[test]
fn setmaster_console_commands_manage_list() {
    let mut h = Harness::new(4);
    let ConsoleOutcome::Output(out) = h.server.console_command("setmaster add 127.0.0.1:27010") else {
        panic!("expected output");
    };
    assert!(out.contains("127.0.0.1:27010"), "{out}");

    let ConsoleOutcome::Output(list) = h.server.console_command("setmaster list") else {
        panic!("expected output");
    };
    assert!(list.contains("127.0.0.1:27010"), "{list}");
    assert!(list.contains("enabled: 1"), "{list}");

    h.server.console_command("setmaster disable");
    assert!(!h.server.masters().enabled);
    h.server.console_command("setmaster enable");
    assert!(h.server.masters().enabled);

    let ConsoleOutcome::Output(rem) = h.server.console_command("setmaster remove 127.0.0.1:27010") else {
        panic!("expected output");
    };
    assert!(rem.contains("Removed"), "{rem}");
    assert!(h.server.masters().targets().is_empty());

    h.server.console_command("setmaster add 127.0.0.1 27011");
    assert_eq!(h.server.masters().targets().len(), 1);
    h.server.console_command("setmaster clear");
    assert!(h.server.masters().targets().is_empty());
}

#[test]
fn server_emits_master_heartbeat_challenge_update_and_shutdown() {
    let mut master_udp = UdpTransport::bind("127.0.0.1:0").unwrap();
    let master_addr = master_udp.local_addr();
    let mut srv_udp = UdpTransport::bind("127.0.0.1:0").unwrap();
    let srv_addr = PeerAddr::Udp(srv_udp.local_addr());

    let mut h = Harness::new(8);
    h.server
        .console_command(&format!("setmaster add {master_addr}"));
    h.server.console_command("sv_password secretpass");

    // Server frame triggers initial 'q' challenge request to registered master.
    h.server.frame(&mut srv_udp, 1.0);
    let (from, q_pkt) = master_udp.recv().expect("expected master 'q' packet");
    assert_eq!(from, srv_addr);
    assert_eq!(q_pkt, b"q");

    // Master replies with 0xFFFFFFFF "s\n" + u32 le challenge.
    let mut ch_reply = Vec::from(&b"\xFF\xFF\xFF\xFFs\n"[..]);
    ch_reply.extend_from_slice(&0x1234_5678u32.to_le_bytes());
    master_udp.send(srv_addr, &ch_reply).unwrap();

    h.server.frame(&mut srv_udp, 1.1);
    let (_, update_pkt) = master_udp.recv().expect("expected heartbeat 0\\n packet");
    let update = String::from_utf8(update_pkt).expect("utf8 heartbeat");
    assert!(update.starts_with("0\n\\protocol\\48\\challenge\\305419896\\"), "{update}");
    assert!(update.contains("\\map\\test\\"), "{update}");
    assert!(update.contains("\\password\\1\\"), "{update}");
    assert!(update.contains("\\max\\8\\"), "{update}");

    // Shutdown notifies master with "b\n".
    h.server.shutdown(&mut srv_udp, "bye");
    let (_, bye_pkt) = master_udp.recv().expect("expected master shutdown b\\n");
    assert_eq!(bye_pkt, b"b\n");
}
