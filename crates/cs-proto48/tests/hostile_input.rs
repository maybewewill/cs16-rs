//! Hostile input for the codec: deterministic pseudo-random bytes fed to every entry point a
//! peer can reach. Each test only asserts that nothing panics, every call returns, and state
//! kept across calls stays bounded.

use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::bitbuf::{MsgWriter, ProtoError};
use cs_proto48::crypto::{com_block_sequence_crc_byte, com_munge, com_munge2};
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::handshake::parse_oob_packet;
use cs_proto48::messages::{SvcParseContext, clc, parse_clc_stream, parse_svc_stream};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, FRAGMENT_S2C_MAX_SIZE, NetChan, make_frag_id};

const ROUNDS: usize = 10_000;
/// Number of `clc` opcodes (`clc::BAD` = 0 to `clc::CVARVALUE2` = 11).
const CLC_OPCODES: usize = 12;
/// Number of `svc` opcodes (`svc::BAD` = 0 to `svc::EXEC` = 59).
const SVC_OPCODES: usize = 60;

/// 64-bit LCG (Knuth's MMIX constants) seeded `0xC516`; each step yields the state's high byte.
struct Lcg(u64);

impl Lcg {
    fn new() -> Self {
        Self(0xC516)
    }

    fn next_byte(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 56) as u8
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_byte()).collect()
    }

    fn next_u32(&mut self) -> u32 {
        u32::from_le_bytes([
            self.next_byte(),
            self.next_byte(),
            self.next_byte(),
            self.next_byte(),
        ])
    }
}

#[test]
fn clc_stream_survives_garbage() {
    let reg = DeltaRegistry::cs16_default();
    let schema = reg.get("usercmd_t").expect("usercmd_t schema");
    let mut rng = Lcg::new();
    for i in 0..ROUNDS {
        let mut data = rng.bytes(i % 97);
        if i.is_multiple_of(2) && !data.is_empty() {
            data[0] = ((i / 2) % CLC_OPCODES) as u8;
        }
        let _ = parse_clc_stream(&data, schema, i as u32);
    }
}

/// Garbage payload `i` for the `svc` parser: half of them start with a valid opcode.
fn svc_garbage(rng: &mut Lcg, i: usize) -> Vec<u8> {
    let mut data = rng.bytes(i % 97);
    if i.is_multiple_of(2) && !data.is_empty() {
        data[0] = ((i / 2) % SVC_OPCODES) as u8;
    }
    data
}

#[test]
fn svc_stream_survives_garbage_with_fresh_context() {
    let mut rng = Lcg::new();
    for i in 0..ROUNDS {
        let data = svc_garbage(&mut rng, i);
        let _ = parse_svc_stream(&data, &mut SvcParseContext::new());
    }
}

/// Generous bound on any collection garbage can grow in a long-lived context or channel.
const MAX_ENTRIES: usize = 4_096;

#[test]
fn svc_stream_survives_garbage_with_long_lived_context() {
    let mut rng = Lcg::new();
    let mut ctx = SvcParseContext::new();
    let schemas_before = ctx.deltas.iter().count();
    for i in 0..ROUNDS {
        let data = svc_garbage(&mut rng, i);
        ctx.last_incoming_seq = i as u8;
        let _ = parse_svc_stream(&data, &mut ctx);
    }
    let frame_entities: usize = ctx.entity_frames.values().map(|f| f.len()).sum();
    let sizes = [
        ("baselines", ctx.baselines.len()),
        ("instance_baselines", ctx.instance_baselines.len()),
        ("clientdata_history", ctx.clientdata_history.len()),
        ("entity_frames", ctx.entity_frames.len()),
        ("entity_frames entities", frame_entities),
        (
            "delta schemas added",
            ctx.deltas.iter().count() - schemas_before,
        ),
        ("user messages", ctx.usermsgs.iter().count()),
    ];
    for (name, size) in sizes {
        assert!(size <= MAX_ENTRIES, "{name}: {size} entries");
    }
}

#[test]
fn netchan_survives_garbage() {
    let mut chan = NetChan::new(FRAGMENT_C2S_SIZE);
    let mut rng = Lcg::new();
    for i in 0..ROUNDS {
        let mut data = rng.bytes(i % 97);
        if i.is_multiple_of(3) && data.len() >= 8 {
            // A header the channel accepts, so the body (and, with the flag, the fragment
            // headers) is reached.
            let seq = chan.incoming_sequence + 1 + u32::from(rng.next_byte() % 4);
            let reliable = u32::from(rng.next_byte() & 0x80) << 24;
            let fragments = if (i / 3).is_multiple_of(2) {
                1 << 30
            } else {
                0
            };
            data[..4].copy_from_slice(&(seq | reliable | fragments).to_le_bytes());
            data[4..8].copy_from_slice(&rng.next_u32().to_le_bytes());
        }
        let _ = chan.process_packet(&data);
    }
    // Garbage never queues outgoing reliable data. The incoming fragment buffers have no
    // accessor, so their size is not checked here.
    assert!(chan.pending_reliable_bytes() <= MAX_ENTRIES);
}

#[test]
fn oob_parser_survives_garbage() {
    let mut rng = Lcg::new();
    for i in 0..ROUNDS {
        let mut data = CONNECTIONLESS_HEADER.to_le_bytes().to_vec();
        data.extend(rng.bytes(i % 97));
        let _ = parse_oob_packet(&data);
    }
}

#[test]
fn clc_move_bodies_reach_the_command_decoder() {
    let reg = DeltaRegistry::cs16_default();
    let schema = reg.get("usercmd_t").expect("usercmd_t schema");
    let mut rng = Lcg::new();
    let mut past_crc = 0;
    for i in 0..ROUNDS {
        // A random inner body (its first three bytes are the loss, `num_backup` and
        // `num_cmds` bytes) with a correct length byte and checksum, munged like the sender.
        let sequence = i as u32;
        let mut body = rng.bytes(3 + i % 200);
        let crc = com_block_sequence_crc_byte(&body, sequence);
        com_munge(&mut body, sequence as i32);
        let mut data = vec![clc::MOVE, body.len() as u8, crc];
        data.extend(body);
        match parse_clc_stream(&data, schema, sequence) {
            Err(ProtoError::InvalidPayload(text)) if text.contains("CRC mismatch") => {}
            _ => past_crc += 1,
        }
    }
    eprintln!("clc_move: {past_crc} of {ROUNDS} past the checksum");
    assert!(
        past_crc >= 9_000,
        "{past_crc} of {ROUNDS} past the checksum"
    );
}

/// Limit on each incoming stream of the channel in `netchan_reassembles_hostile_fragments`.
const FUZZ_STREAM_LIMIT: usize = 64 * 1024;

#[test]
fn netchan_reassembles_hostile_fragments() {
    let mut chan = NetChan::new(FRAGMENT_C2S_SIZE);
    chan.set_max_incoming_stream(FUZZ_STREAM_LIMIT);
    let mut rng = Lcg::new();
    let (mut stored, mut completed, mut refused, mut peak) = (0, 0, 0, 0);
    for i in 0..ROUNDS {
        let sequence = chan.incoming_sequence + 1;
        // Alternating blocks: small messages that complete, and a 1000-piece message that
        // runs into the stream limit.
        let total = if (i / 500).is_multiple_of(2) {
            1 + u16::from(rng.next_byte() % 4)
        } else {
            1000
        };
        let streams = [
            !rng.next_byte().is_multiple_of(3),
            rng.next_byte().is_multiple_of(3),
        ];
        let mut headers = MsgWriter::new();
        let mut pieces = Vec::new();
        for present in streams {
            if !present {
                headers.write_u8(0);
                continue;
            }
            // One fragment in eight has a hostile index (0 or past the total).
            let idx = if rng.next_byte().is_multiple_of(8) {
                u16::from_le_bytes([rng.next_byte(), rng.next_byte()])
            } else {
                1 + u16::from_le_bytes([rng.next_byte(), rng.next_byte()]) % total
            };
            let len = if rng.next_byte().is_multiple_of(2) {
                usize::from(rng.next_byte() % 64)
            } else {
                usize::from(u16::from_le_bytes([rng.next_byte(), rng.next_byte()]))
                    % (FRAGMENT_S2C_MAX_SIZE + 1)
            };
            let mut piece = rng.bytes(len);
            if len >= 8 && rng.next_byte().is_multiple_of(4) {
                piece[..4].copy_from_slice(b"BZ2\0");
            }
            headers.write_u8(1);
            headers.write_u32(make_frag_id(idx, total));
            headers.write_u16(pieces.len() as u16);
            headers.write_u16(piece.len() as u16);
            pieces.extend(piece);
        }
        let mut packet = MsgWriter::new();
        packet.write_u32(sequence | (1 << 31) | (1 << 30));
        packet.write_u32(0);
        packet.write_bytes(headers.as_slice());
        packet.write_bytes(&pieces);
        packet.write_bytes(&rng.bytes(i % 8));
        let mut packet = packet.into_bytes();
        com_munge2(&mut packet[8..], (sequence as u8) as i32);

        match chan.process_packet(&packet) {
            Ok(Some(incoming)) => {
                stored += 1;
                completed += usize::from(incoming.completed_normal_stream.is_some());
                completed += usize::from(incoming.completed_file_stream.is_some());
            }
            Ok(None) => {}
            Err(_) => refused += 1,
        }
        peak = peak.max(chan.incoming_buffered_bytes());
        assert!(
            chan.incoming_buffered_bytes() <= 2 * FUZZ_STREAM_LIMIT,
            "packet {i}: {} bytes buffered",
            chan.incoming_buffered_bytes()
        );
    }
    eprintln!(
        "fragments: {stored} packets stored, {completed} reassemblies, {refused} refused, peak {peak} bytes"
    );
    assert!(stored > 1_000, "{stored} packets stored");
    assert!(completed > 0, "no reassembly completed");
    assert!(refused > 0, "no fragment refused");
}
