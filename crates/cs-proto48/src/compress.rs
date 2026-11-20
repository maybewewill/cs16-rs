//! BZ2 decompression of reassembled netchan fragment streams (`Netchan_CopyNormalFragments`,
//! `ReHLDS/engine/net_chan.cpp`): a stream that starts with `b"BZ2\0"` carries a bzip2 body.

use std::io::Read;

use bzip2_rs::DecoderReader;

use crate::bitbuf::ProtoError;

/// Upper bound on a decompressed fragment stream; decoding stops with an error once the output
/// would grow past it.
pub const MAX_DECOMPRESSED_STREAM: usize = 4 * 1024 * 1024;

const BZ2_PREFIX: &[u8] = b"BZ2\0";

/// Decompress a `b"BZ2\0"`-prefixed stream; any other payload is returned unchanged.
///
/// Decoding is incremental and fails with [`ProtoError::InvalidPayload`] as soon as the output
/// passes [`MAX_DECOMPRESSED_STREAM`], on corrupt data, and on a truncated bzip2 body.
pub fn decompress_stream(payload: &[u8]) -> Result<Vec<u8>, ProtoError> {
    let Some(body) = payload.strip_prefix(BZ2_PREFIX) else {
        return Ok(payload.to_vec());
    };

    let mut reader = DecoderReader::new(body);
    let mut out = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| ProtoError::InvalidPayload(format!("bz2 stream: {e}")))?;
        if n == 0 {
            break;
        }
        if out.len() + n > MAX_DECOMPRESSED_STREAM {
            return Err(ProtoError::InvalidPayload(format!(
                "bz2 stream decompresses past {MAX_DECOMPRESSED_STREAM} bytes"
            )));
        }
        out.extend_from_slice(&chunk[..n]);
    }
    Ok(out)
}
