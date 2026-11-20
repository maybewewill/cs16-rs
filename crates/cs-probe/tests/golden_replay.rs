//! Replays every golden capture in `CS16_GOLDEN_DIR` through the `cs-proto48` decoder.
//! Without the variable (no reference stand) the test returns without assertions.

use cs_probe::{Direction, read_capture};
use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::messages::{SvcMessage, SvcParseContext, parse_svc_stream};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};

#[test]
fn golden_captures_parse_without_ignored_or_errors() {
    let Some(dir) = std::env::var_os("CS16_GOLDEN_DIR") else {
        return;
    };
    let entries = std::fs::read_dir(&dir).expect("CS16_GOLDEN_DIR is not a readable directory");
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("cs48cap") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let records = read_capture(&bytes)
            .unwrap_or_else(|e| panic!("{}: unreadable capture: {e}", path.display()));
        replay(&path.display().to_string(), &records);
    }
}

fn replay(name: &str, records: &[cs_probe::CaptureRecord]) {
    let mut chan = NetChan::new(FRAGMENT_C2S_SIZE);
    let mut ctx = SvcParseContext::new();
    let mut splits = SplitPacketAssembler::default();

    for (i, record) in records.iter().enumerate() {
        if record.dir != Direction::Received {
            continue;
        }
        let Some(datagram) = splits.push(&record.data) else {
            continue;
        };
        if datagram.len() >= 4 && datagram[..4] == CONNECTIONLESS_HEADER.to_le_bytes() {
            continue;
        }
        let incoming = chan
            .process_packet(&datagram)
            .unwrap_or_else(|e| panic!("{name}: record {i}: netchan error: {e}"));
        let Some(incoming) = incoming else {
            continue;
        };
        ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;
        let streams = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for stream in streams {
            let messages = parse_svc_stream(&stream, &mut ctx)
                .unwrap_or_else(|e| panic!("{name}: record {i}: svc parse error: {e}"));
            for msg in messages {
                assert!(
                    !matches!(msg, SvcMessage::Ignored(_)),
                    "{name}: record {i}: ignored message {msg:?}"
                );
            }
        }
    }
}
