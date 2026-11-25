mod common;

use common::Harness;
use cs_engine::ConsoleOutcome;
use cs_probe::ProbeState;
use cs_proto48::handshake::{
    OobPacket, default_cs16_protinfo, default_cs16_userinfo, encode_challenge_rcon,
    encode_connect_48, encode_getchallenge, encode_rcon, lan_steam_auth_ticket, parse_oob_packet,
};
use cs_proto48::messages::SvcMessage;
use cs_proto48::query::{A2S_RULES, S2A_RULES};
use cs_transport::{PacketTransport, PeerAddr};

fn step_and_recv(h: &mut Harness, ep: &mut impl PacketTransport) -> Vec<u8> {
    for _ in 0..10 {
        h.step(&mut [], 1);
        if let Some((_, pkt)) = ep.recv() {
            return pkt;
        }
    }
    panic!("expected OOB reply");
}

#[test]
fn rcon_executes_commands_and_logs_and_masks_a2s_rules() {
    let mut h = Harness::new(4);
    h.server.console_command("rcon_password topsecret");
    h.server.console_command("sv_password hiddenpw");
    h.server.console_command("log on");

    let mut admin = h.hub.connect();
    let srv_addr = PeerAddr::Loopback(0);
    admin.send(srv_addr, &encode_challenge_rcon()).unwrap();
    let ch_pkt = step_and_recv(&mut h, &mut admin);
    let OobPacket::S2cChallengeRcon(rcon_ch) = parse_oob_packet(&ch_pkt).unwrap() else {
        panic!("expected challenge rcon");
    };
    admin
        .send(srv_addr, &encode_rcon(rcon_ch, "topsecret", "status"))
        .unwrap();
    let out_pkt = step_and_recv(&mut h, &mut admin);
    let OobPacket::A2aPrint(reply) = parse_oob_packet(&out_pkt).unwrap() else {
        panic!("expected A2aPrint reply");
    };
    assert!(reply.contains("hostname:"), "{reply:?}");
    assert!(
        h.server
            .logger()
            .history
            .iter()
            .any(|l| l.starts_with("L ") && l.contains("Rcon:")),
        "{:?}",
        h.server.logger().history
    );

    // A2S_RULES masks FCVAR_PROTECTED cvars like rcon_password and sv_password.
    let mut probe = h.hub.connect();
    let mut a2s_req = vec![0xFF, 0xFF, 0xFF, 0xFF, A2S_RULES];
    a2s_req.extend_from_slice(&(-1i32).to_le_bytes());
    probe.send(srv_addr, &a2s_req).unwrap();
    let ch_reply = step_and_recv(&mut h, &mut probe);
    let ch = i32::from_le_bytes(ch_reply[5..9].try_into().unwrap());
    let mut a2s_req2 = vec![0xFF, 0xFF, 0xFF, 0xFF, A2S_RULES];
    a2s_req2.extend_from_slice(&ch.to_le_bytes());
    probe.send(srv_addr, &a2s_req2).unwrap();
    let rules_pkt = step_and_recv(&mut h, &mut probe);
    assert_eq!(rules_pkt[4], S2A_RULES);
    let body = String::from_utf8_lossy(&rules_pkt);
    assert!(!body.contains("topsecret"));
    assert!(!body.contains("hiddenpw"));
}

#[test]
fn password_and_ip_ban_reject_connecting_client() {
    let mut h = Harness::new(4);
    h.server.console_command("sv_password secret");

    let mut bad = h.add_probe("Bad");
    for _ in 0..40 {
        h.step(&mut [&mut bad], 1);
        if matches!(bad.state(), ProbeState::Rejected(_)) {
            break;
        }
    }
    assert!(
        matches!(bad.state(), ProbeState::Rejected(r) if r.contains("password")),
        "{:?}",
        bad.state()
    );

    h.server.console_command("sv_password \"\"");
    let mut c = h.hub.connect();
    let srv_addr = PeerAddr::Loopback(0);
    c.send(srv_addr, &encode_getchallenge(true)).unwrap();
    let ch_pkt = step_and_recv(&mut h, &mut c);
    let OobPacket::S2cChallenge { challenge, .. } = parse_oob_packet(&ch_pkt).unwrap() else {
        panic!();
    };
    let my_addr = PeerAddr::Loopback(c.id());
    h.server.console_command(&format!("addip 0 {my_addr}"));

    c.send(
        srv_addr,
        &encode_connect_48(
            challenge,
            &default_cs16_protinfo("0123456789abcdef0123456789abcdef"),
            &default_cs16_userinfo("BannedGuy"),
            &lan_steam_auth_ticket(1234),
        ),
    )
    .unwrap();
    let reject = step_and_recv(&mut h, &mut c);
    assert!(matches!(
        parse_oob_packet(&reject).unwrap(),
        OobPacket::S2cReject(_)
    ));
}

#[test]
fn pause_freezes_sv_time_and_status_full_users_work() {
    let mut h = Harness::new(4);
    let mut a = h.add_probe("Alice");
    for _ in 0..200 {
        h.step(&mut [&mut a], 1);
        if a.state() == &ProbeState::InGame {
            break;
        }
    }
    assert_eq!(a.state(), &ProbeState::InGame);

    h.server.console_command("pausable 1");
    assert_eq!(
        h.server.console_command("pause"),
        ConsoleOutcome::Output("Server paused\n".into())
    );
    assert!(h.server.is_paused());
    h.step(&mut [&mut a], 15);
    assert!(
        a.messages()
            .iter()
            .any(|m| matches!(m, SvcMessage::SetPause(true)))
    );

    let last_time = |p: &cs_probe::ProbeClient<_>| {
        p.messages()
            .iter()
            .rev()
            .find_map(|m| match m {
                SvcMessage::Time(t) => Some(*t),
                _ => None,
            })
            .unwrap_or(0.0)
    };
    let t0 = last_time(&a);
    h.step(&mut [&mut a], 20);
    let t1 = last_time(&a);
    assert!((t1 - t0).abs() < 1e-6, "paused sv_time advanced: {t0} -> {t1}");

    assert_eq!(
        h.server.console_command("pause"),
        ConsoleOutcome::Output("Server unpaused\n".into())
    );
    let users_out = match h.server.console_command("users") {
        ConsoleOutcome::Output(s) => s,
        _ => panic!(),
    };
    assert!(users_out.contains("Alice"));
    let full_out = match h.server.console_command("status full") {
        ConsoleOutcome::Output(s) => s,
        _ => panic!(),
    };
    assert!(full_out.contains("uniqueid"));
    assert!(full_out.contains("Alice"));
}
