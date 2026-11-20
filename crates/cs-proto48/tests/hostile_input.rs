//! Hostile input for the codec: deterministic pseudo-random bytes fed to every entry point a
//! peer can reach. Each test only asserts that nothing panics, every call returns, and state
//! kept across calls stays bounded.

use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::handshake::parse_oob_packet;
use cs_proto48::messages::{SvcParseContext, parse_clc_stream, parse_svc_stream};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan};

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
