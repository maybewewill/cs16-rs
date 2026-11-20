//! BZ2 fragment-stream decompression (`compress.rs`) and its use by `NetChan`.

use cs_proto48::bitbuf::{MsgWriter, ProtoError};
use cs_proto48::compress::{decompress_stream, decompress_stream_default};
use cs_proto48::crypto::com_munge2;
use cs_proto48::netchan::{NetChan, make_frag_id};

const TEXT_BZ2: &[u8] = include_bytes!("fixtures/text.bz2");
const BOMB_BZ2: &[u8] = include_bytes!("fixtures/bomb.bz2");

fn text_plain() -> Vec<u8> {
    b"cs16-rs netchan bz2 fixture ".repeat(64)
}

fn bz2_prefixed(body: &[u8]) -> Vec<u8> {
    let mut p = b"BZ2\0".to_vec();
    p.extend_from_slice(body);
    p
}

#[test]
fn bz2_prefixed_stream_is_decompressed() {
    let p = bz2_prefixed(TEXT_BZ2);
    assert_eq!(decompress_stream_default(&p).unwrap(), text_plain());
    // The limit is on the output: exactly the output's size is enough, one byte less is not.
    let len = text_plain().len();
    assert_eq!(decompress_stream(&p, len).unwrap(), text_plain());
    assert!(matches!(
        decompress_stream(&p, len - 1),
        Err(ProtoError::InvalidPayload(_))
    ));
}

#[test]
fn plain_stream_passes_through() {
    assert_eq!(decompress_stream_default(b"hello").unwrap(), b"hello");
}

#[test]
fn decompression_bomb_is_rejected() {
    let p = bz2_prefixed(BOMB_BZ2);
    assert!(matches!(
        decompress_stream_default(&p),
        Err(ProtoError::InvalidPayload(_))
    ));
}

#[test]
fn truncated_bz2_is_an_error_not_a_panic() {
    let p = bz2_prefixed(&TEXT_BZ2[..10]);
    assert!(decompress_stream_default(&p).is_err());
}

/// A packet with sequence `seq` whose only content is `piece` sent as the single fragment of a
/// one-piece message in the normal stream, munged like the sender does.
fn one_piece_packet(seq: u32, piece: &[u8]) -> Vec<u8> {
    let mut w = MsgWriter::new();
    w.write_u32(seq | (1 << 30));
    w.write_u32(0);
    w.write_u8(1);
    w.write_u32(make_frag_id(1, 1));
    w.write_u16(0);
    w.write_u16(piece.len() as u16);
    w.write_u8(0);
    w.write_bytes(piece);
    let mut bytes = w.into_bytes();
    com_munge2(&mut bytes[8..], (seq as u8) as i32);
    bytes
}

#[test]
fn compressed_stream_past_the_channel_limit_is_refused() {
    let mut chan = NetChan::new(64);
    // The compressed piece fits the limit; its output does not.
    chan.set_max_incoming_stream(1024);
    let piece = bz2_prefixed(TEXT_BZ2);
    assert!(piece.len() < 1024 && text_plain().len() > 1024);
    assert!(matches!(
        chan.process_packet(&one_piece_packet(1, &piece)),
        Err(ProtoError::StreamRefused(_))
    ));
    assert_eq!(chan.incoming_buffered_bytes(), 0);

    // The channel still works: a stream within the limit and a plain packet go through.
    chan.set_max_incoming_stream(4096);
    let inc = chan
        .process_packet(&one_piece_packet(2, &piece))
        .unwrap()
        .unwrap();
    assert_eq!(inc.completed_normal_stream, Some(text_plain()));
    let mut peer = NetChan::new(64);
    peer.outgoing_sequence = 3;
    let inc = chan
        .process_packet(&peer.build_packet(&[7]))
        .unwrap()
        .unwrap();
    assert_eq!(inc.payload[0], 7);
}

#[test]
fn netchan_delivers_decompressed_fragment_stream() {
    let mut server = NetChan::new(64);
    let mut client = NetChan::new(64);
    server.create_fragments(&bz2_prefixed(TEXT_BZ2));

    let mut assembled = None;
    for step in 0..20u8 {
        let pkt = server.build_packet(&[0x01, step]);
        let inc = client.process_packet(&pkt).unwrap().unwrap();
        if let Some(full) = inc.completed_normal_stream {
            assembled = Some(full);
            break;
        }
        let ack = client.build_packet(&[]);
        let _ = server.process_packet(&ack).unwrap();
    }

    assert_eq!(assembled, Some(text_plain()));
}
