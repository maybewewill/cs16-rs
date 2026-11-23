//! Replays the stock-server golden captures (`hlds_join`, `hlds_office_join`) through the
//! stream parser and checks the client-side readers: none of the svc messages this crate types
//! comes back as `Ignored`, every temp entity decodes, and every sound is well formed.
//!
//! The captures are not in the repository. The test reads `CS16_GOLDEN_DIR` and returns
//! without assertions when the variable or a capture is absent.

use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::messages::{SvcMessage, SvcParseContext, parse_svc_stream, svc};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};
use cs_proto48::tempent::TempEffect;

const MAGIC: &[u8; 8] = b"CS48CAP1";

/// The received datagrams of a `CS48CAP1` capture (`cs-probe` writes them; this crate cannot
/// depend on it).
fn received_datagrams(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut rest = bytes.strip_prefix(MAGIC.as_slice()).expect("not a capture");
    let mut out = Vec::new();
    while !rest.is_empty() {
        let dir = rest[0];
        let len = u32::from_le_bytes(rest[9..13].try_into().unwrap()) as usize;
        if dir == 0 {
            out.push(rest[13..13 + len].to_vec());
        }
        rest = &rest[13 + len..];
    }
    out
}

#[derive(Default)]
struct Seen {
    messages: usize,
    sounds: usize,
    temp_entities: usize,
    typed_temp_entities: usize,
    ignored: Vec<u8>,
}

fn replay(name: &str, bytes: &[u8]) -> Seen {
    let mut chan = NetChan::new(FRAGMENT_C2S_SIZE);
    let mut ctx = SvcParseContext::new();
    let mut splits = SplitPacketAssembler::default();
    let mut seen = Seen::default();

    for (i, datagram) in received_datagrams(bytes).iter().enumerate() {
        let Some(datagram) = splits.push(datagram) else {
            continue;
        };
        if datagram.len() >= 4 && datagram[..4] == CONNECTIONLESS_HEADER.to_le_bytes() {
            continue;
        }
        let Some(incoming) = chan
            .process_packet(&datagram)
            .unwrap_or_else(|e| panic!("{name}: record {i}: netchan error: {e}"))
        else {
            continue;
        };
        ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;
        let streams = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for stream in streams {
            let messages = parse_svc_stream(&stream, &mut ctx)
                .unwrap_or_else(|e| panic!("{name}: record {i}: svc parse error: {e}"));
            for msg in messages {
                seen.messages += 1;
                match &msg {
                    SvcMessage::Ignored(op) => seen.ignored.push(*op),
                    SvcMessage::Sound(s) => {
                        seen.sounds += 1;
                        assert!(s.entity < 2048 && s.channel < 8, "{name}: {s:?}");
                        assert!(s.origin.is_finite(), "{name}: {s:?}");
                    }
                    SvcMessage::TempEntity { te_type, raw } => {
                        seen.temp_entities += 1;
                        let effect = TempEffect::parse(*te_type, raw)
                            .unwrap_or_else(|e| panic!("{name}: record {i}: te {te_type}: {e}"));
                        if !matches!(effect, TempEffect::Other { .. }) {
                            seen.typed_temp_entities += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    seen
}

#[test]
fn golden_captures_parse_the_client_readers_without_ignored() {
    let Some(dir) = std::env::var_os("CS16_GOLDEN_DIR") else {
        return;
    };
    // The svc messages this crate has a typed reader for.
    let typed = [
        svc::PARTICLE,
        svc::DAMAGE,
        svc::SPAWNSTATIC,
        svc::SPAWNSTATICSOUND,
        svc::CUSTOMIZATION,
        svc::CROSSHAIRANGLE,
        svc::SOUNDFADE,
        svc::TIMESCALE,
        svc::TEMP_ENTITY,
        svc::SOUND,
    ];
    for file in ["hlds_join.cs48cap", "hlds_office_join.cs48cap"] {
        let path = std::path::Path::new(&dir).join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let seen = replay(file, &bytes);
        eprintln!(
            "{file}: {} messages, {} sounds, {} temp entities ({} typed), ignored {:?}",
            seen.messages, seen.sounds, seen.temp_entities, seen.typed_temp_entities, seen.ignored
        );
        assert!(seen.messages > 0, "{file}: no messages");
        for op in &seen.ignored {
            assert!(!typed.contains(op), "{file}: svc {op} came back Ignored");
        }
    }
}
