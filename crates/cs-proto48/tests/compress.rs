//! BZ2 fragment-stream decompression (`compress.rs`) and its use by `NetChan`.

use cs_proto48::bitbuf::ProtoError;
use cs_proto48::compress::decompress_stream;
use cs_proto48::netchan::NetChan;

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
    assert_eq!(decompress_stream(&p).unwrap(), text_plain());
}

#[test]
fn plain_stream_passes_through() {
    assert_eq!(decompress_stream(b"hello").unwrap(), b"hello");
}

#[test]
fn decompression_bomb_is_rejected() {
    let p = bz2_prefixed(BOMB_BZ2);
    assert!(matches!(
        decompress_stream(&p),
        Err(ProtoError::InvalidPayload(_))
    ));
}

#[test]
fn truncated_bz2_is_an_error_not_a_panic() {
    let p = bz2_prefixed(&TEXT_BZ2[..10]);
    assert!(decompress_stream(&p).is_err());
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
