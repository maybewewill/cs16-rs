//! Protocol 48 packet sanity validation for the WS-to-UDP relay (prevents arbitrary UDP reflection).

use cs_transport::MAX_UDP_PACKET;

const OOB_PREFIX: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const SPLIT_PREFIX: [u8; 4] = [0xFE, 0xFF, 0xFF, 0xFF];

/// Client-to-server connectionless OOB message prefixes allowed across the relay.
const ALLOWED_C2S_OOB: &[&[u8]] = &[
    b"getchallenge",
    b"connect ",
    b"TSource Engine Query\0",
    b"info",
    b"players",
    b"rules",
    b"ping",
    b"details",
    b"challenge ",
    b"V",
    b"U",
    b"W",
];

/// Returns `true` only if `data` matches a valid GoldSrc Protocol 48 client datagram.
pub fn is_valid_c2s_protocol48(data: &[u8]) -> bool {
    if data.is_empty() || data.len() > MAX_UDP_PACKET {
        return false;
    }
    if data.starts_with(&OOB_PREFIX) {
        let body = &data[4..];
        return !body.is_empty()
            && ALLOWED_C2S_OOB
                .iter()
                .any(|prefix| body.starts_with(prefix));
    }
    if data.starts_with(&SPLIT_PREFIX) {
        return data.len() >= 9;
    }
    // Regular GoldSrc NetChan header requires at least 8 bytes (seq + ack).
    data.len() >= 8
}

/// Returns `true` only if `data` matches a valid GoldSrc Protocol 48 server datagram.
pub fn is_valid_s2c_protocol48(data: &[u8]) -> bool {
    if data.is_empty() || data.len() > MAX_UDP_PACKET {
        return false;
    }
    if data.starts_with(&OOB_PREFIX) {
        return data.len() >= 5;
    }
    if data.starts_with(&SPLIT_PREFIX) {
        return data.len() >= 9;
    }
    data.len() >= 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_goldsrc_packets_and_rejects_arbitrary_udp_payloads() {
        assert!(is_valid_c2s_protocol48(b"\xff\xff\xff\xffgetchallenge steam\n"));
        assert!(is_valid_c2s_protocol48(b"\xff\xff\xff\xffconnect 48 1234 \"\\name\\p\""));
        assert!(is_valid_c2s_protocol48(b"\xff\xff\xff\xffTSource Engine Query\0"));
        assert!(is_valid_c2s_protocol48(&[1, 0, 0, 0, 0, 0, 0, 0, 99]));

        // Reject arbitrary DNS/NTP/SSDP/empty/short payloads.
        assert!(!is_valid_c2s_protocol48(b""));
        assert!(!is_valid_c2s_protocol48(b"\xff\xff\xff\xffM-SEARCH * HTTP/1.1"));
        assert!(!is_valid_c2s_protocol48(&[0x17, 0x00, 0x03, 0x2a])); // monlist < 8 bytes
        assert!(!is_valid_c2s_protocol48(&vec![0xAA; MAX_UDP_PACKET + 1]));

        assert!(is_valid_s2c_protocol48(b"\xff\xff\xff\xffA00000000 123 3 1"));
        assert!(!is_valid_s2c_protocol48(&[1, 2, 3]));
    }
}
