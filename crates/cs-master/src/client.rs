//! Blocking UDP client for querying a GoldSrc Master Server (`0x31`).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use crate::protocol::{MasterError, encode_query_request, parse_query_reply};

/// Maximum number of paginated `0x31` replies fetched per query call.
pub const MAX_QUERY_PAGES: usize = 16;

/// Query a GoldSrc master server using the `0x31` protocol with automatic cursor pagination.
///
/// Stops when the server sends the `0.0.0.0:0` end sentinel, returns an empty page,
/// repeats the cursor, or reaches [`MAX_QUERY_PAGES`] (16) pages.
pub fn query_master_servers(
    master: SocketAddr,
    region: u8,
    filter: &str,
    timeout: Duration,
) -> Result<Vec<SocketAddrV4>, MasterError> {
    let bind_addr: SocketAddr = if master.is_ipv6() {
        "[::]:0".parse().expect("valid IPv6 unspecified addr")
    } else {
        "0.0.0.0:0".parse().expect("valid IPv4 unspecified addr")
    };
    let socket = UdpSocket::bind(bind_addr)?;
    socket.set_read_timeout(Some(timeout))?;
    socket.set_write_timeout(Some(timeout))?;

    let unspecified = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let mut cursor = unspecified;
    let mut servers = Vec::new();
    let mut buf = [0u8; 4096];

    for _page_idx in 0..MAX_QUERY_PAGES {
        let req = encode_query_request(region, cursor, filter);
        socket.send_to(&req, master)?;

        let (len, _) = socket.recv_from(&mut buf)?;
        let (page, has_more) = parse_query_reply(&buf[..len])?;
        if page.is_empty() {
            break;
        }

        let last = *page.last().expect("page is non-empty");
        servers.extend(page);

        if !has_more || last == cursor || last == unspecified {
            break;
        }
        cursor = last;
    }

    Ok(servers)
}
