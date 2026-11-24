mod common;

use common::{Harness, TestGame};
use cs_probe::ProbeState;
use cs_proto48::messages::SvcMessage;

#[test]
fn server_voice_init_relay_vban_and_enable_flag() {
    let mut h = Harness::with_game(4, TestGame::default());
    let mut a = h.add_probe("Alice");
    let mut b = h.add_probe("Bob");
    h.step(&mut [&mut a, &mut b], 200);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(b.state(), &ProbeState::InGame);

    assert!(
        a.messages().iter().any(|m| matches!(
            m,
            SvcMessage::VoiceInit { codec, quality } if codec == "voice_opus" && *quality == 3
        )),
        "spawn tail must advertise sv_voicecodec and sv_voicequality"
    );

    a.clear_messages();
    b.clear_messages();

    let payload = vec![0x43, 0x53, 0x56, 0x4F, 1, 2, 3, 4];
    a.core_mut().send_voice_data(&payload);
    h.step(&mut [&mut a, &mut b], 20);

    assert!(
        b.messages().iter().any(|m| matches!(
            m,
            SvcMessage::VoiceData { player_slot: 0, data } if *data == payload
        )),
        "Bob must receive Alice's voice packet"
    );
    assert!(
        !a.messages().iter().any(|m| matches!(m, SvcMessage::VoiceData { .. })),
        "server must not echo sender's voice packet back to sender"
    );
    assert!(
        !b.core().fx().voice.pending().is_empty(),
        "clientcore VoiceQueue must record incoming VoiceData events"
    );

    // Bob mutes slot 0 via vban 1.
    b.clear_messages();
    b.send_stringcmd("vban 1");
    h.step(&mut [&mut a, &mut b], 10);

    a.core_mut().send_voice_data(&[9, 8, 7, 6]);
    h.step(&mut [&mut a, &mut b], 20);
    assert!(
        !b.messages().iter().any(|m| matches!(m, SvcMessage::VoiceData { .. })),
        "vban 1 must mute slot 0 for Bob"
    );

    // Unban, disable sv_voiceenable, and ensure relay stops.
    b.send_stringcmd("vban 0");
    h.step(&mut [&mut a, &mut b], 10);
    h.server.set_cvar("sv_voiceenable", "0");
    b.clear_messages();
    a.core_mut().send_voice_data(&[5, 6, 7, 8]);
    h.step(&mut [&mut a, &mut b], 20);
    assert!(
        !b.messages().iter().any(|m| matches!(m, SvcMessage::VoiceData { .. })),
        "sv_voiceenable 0 must block relay"
    );
}
