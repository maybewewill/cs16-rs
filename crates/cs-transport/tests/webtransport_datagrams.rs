//! Deterministic integration tests for HTTP/3 WebTransport datagrams (`wt_native`).
#![cfg(not(target_arch = "wasm32"))]

use std::time::{Duration, Instant};

use cs_transport::{
    MAX_UDP_PACKET, MultiTransport, PacketTransport, PeerAddr, PeerKind, TransportError,
    UdpTransport, WsClientTransport, WsServerConfig, WsServerTransport, WtCertDigest,
    WtClientTransport, WtServerConfig, WtServerTransport,
};
use wtransport::Identity;

fn poll_recv<T: PacketTransport>(tr: &mut T, timeout: Duration) -> Option<(PeerAddr, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(pkt) = tr.recv() {
            return Some(pkt);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

#[test]
fn ephemeral_ecdsa_p256_cert_meets_w3c_spec_and_rejects_wrong_digest() {
    let mut srv = WtServerTransport::bind(WtServerConfig {
        bind_addr: "127.0.0.1:0".into(),
        ..WtServerConfig::default()
    })
    .expect("bind ephemeral WebTransport server");

    assert!(srv.is_self_signed());
    let der = srv.cert_der();
    assert!(!der.is_empty());

    // DER must contain OID id-ecPublicKey (1.2.840.10045.2.1) and prime256v1 (1.2.840.10045.3.1.7).
    const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
    const OID_PRIME256V1: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
    assert!(
        der.windows(OID_EC_PUBLIC_KEY.len())
            .any(|w| w == OID_EC_PUBLIC_KEY),
        "ephemeral certificate must use EC public key"
    );
    assert!(
        der.windows(OID_PRIME256V1.len())
            .any(|w| w == OID_PRIME256V1),
        "ephemeral certificate must use ECDSA P-256 (prime256v1)"
    );

    let digest = srv.cert_sha256();
    assert_eq!(digest.as_bytes().len(), 32);
    assert_eq!(digest.to_base64().len(), 44);
    assert_eq!(digest.to_hex().len(), 64);
    assert_eq!(digest.to_colon_hex().split(':').count(), 32);

    let url = format!("https://127.0.0.1:{}/wt", srv.local_addr().port());

    // Wrong digest must fail TLS serverCertificateHashes verification.
    let wrong = WtCertDigest::from_bytes([0xFF; 32]);
    assert!(
        WtClientTransport::connect_with_cert_hash(&url, wrong).is_err(),
        "mismatched SHA-256 hash must be rejected"
    );

    // Matching digest succeeds under W3C ServerHashVerification (enforcing ECDSA P-256 and <= 14 days).
    let mut client = WtClientTransport::connect_with_cert_hash(&url, digest)
        .expect("matching SHA-256 digest connects");
    client
        .send(PeerAddr::WebTransport(0), b"hello-wt")
        .expect("send datagram");
    let (peer, data) =
        poll_recv(&mut srv, Duration::from_secs(3)).expect("server receives datagram");
    assert!(matches!(peer, PeerAddr::WebTransport(1)));
    assert_eq!(data, b"hello-wt");

    srv.send(peer, b"world-wt").expect("server reply");
    let (from, reply) =
        poll_recv(&mut client, Duration::from_secs(3)).expect("client receives reply");
    assert_eq!(from, PeerAddr::WebTransport(0));
    assert_eq!(reply, b"world-wt");
}

#[test]
fn loads_pem_cert_and_key_files_and_rejects_partial_config() {
    let tmp = std::env::temp_dir().join(format!("cs16-wt-pem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let cert_path = tmp.join("cert.pem");
    let key_path = tmp.join("key.pem");

    // Partial config (cert without key) must fail cleanly.
    assert!(
        WtServerTransport::bind(WtServerConfig {
            bind_addr: "127.0.0.1:0".into(),
            cert_path: Some(cert_path.clone()),
            key_path: None,
            ..WtServerConfig::default()
        })
        .is_err()
    );

    let identity = Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap();
    let expected_digest =
        WtCertDigest::from_bytes(*identity.certificate_chain().as_slice()[0].hash().as_ref());
    std::fs::write(
        &cert_path,
        identity.certificate_chain().as_slice()[0].to_pem(),
    )
    .unwrap();
    std::fs::write(&key_path, identity.private_key().to_secret_pem()).unwrap();

    let mut srv = WtServerTransport::bind(WtServerConfig {
        bind_addr: "127.0.0.1:0".into(),
        cert_path: Some(cert_path),
        key_path: Some(key_path),
        ..WtServerConfig::default()
    })
    .expect("bind server from PEM files");

    assert!(!srv.is_self_signed());
    assert_eq!(srv.cert_sha256(), expected_digest);

    let url = format!("https://127.0.0.1:{}/wt", srv.local_addr().port());
    let mut client = WtClientTransport::connect_with_cert_hash(&url, expected_digest)
        .expect("connect to PEM server");
    client.send(PeerAddr::WebTransport(0), b"pem-ping").unwrap();
    let (peer, data) = poll_recv(&mut srv, Duration::from_secs(3)).expect("recv pem-ping");
    assert_eq!(data, b"pem-ping");
    srv.send(peer, b"pem-pong").unwrap();
    let (_, reply) = poll_recv(&mut client, Duration::from_secs(3)).expect("recv pem-pong");
    assert_eq!(reply, b"pem-pong");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn multi_transport_routes_udp_ws_and_webtransport_with_zero_collision() {
    let udp = UdpTransport::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr();

    let ws_srv = WsServerTransport::bind(WsServerConfig {
        bind_addr: "127.0.0.1:0".into(),
        static_dir: None,
        ws_path: "/ws".into(),
        max_clients: 8,
        upgrade_hook: None,
        http_hook: None,
    })
    .unwrap();
    let ws_addr = ws_srv.local_addr();

    let wt_srv = WtServerTransport::bind(WtServerConfig {
        bind_addr: "127.0.0.1:0".into(),
        wt_path: "/wt".into(),
        max_clients: 8,
        ..WtServerConfig::default()
    })
    .unwrap();
    let wt_addr = wt_srv.local_addr();
    let wt_digest = wt_srv.cert_sha256();

    let mut multi = MultiTransport::new();
    multi.add(PeerKind::Udp, udp);
    multi.add(PeerKind::Web, ws_srv);
    multi.add(PeerKind::WebTransport, wt_srv);

    // Unknown HTTP/3 path must be rejected (404).
    assert!(
        WtClientTransport::connect_with_cert_hash(
            &format!("https://127.0.0.1:{}/invalid-path", wt_addr.port()),
            wt_digest,
        )
        .is_err()
    );

    let mut udp_client = UdpTransport::bind("127.0.0.1:0").unwrap();
    let mut ws_client = WsClientTransport::connect(&format!("ws://{ws_addr}/ws")).unwrap();
    let wt_url = format!("https://127.0.0.1:{}/wt", wt_addr.port());
    let mut wt_client_1 = WtClientTransport::connect_with_cert_hash(&wt_url, wt_digest).unwrap();
    let mut wt_client_2 = WtClientTransport::connect_with_cert_hash(&wt_url, wt_digest).unwrap();

    udp_client
        .send(PeerAddr::Udp(udp_addr), b"from-udp")
        .unwrap();
    ws_client.send(PeerAddr::Web(0), b"from-ws").unwrap();
    wt_client_1
        .send(PeerAddr::WebTransport(0), b"from-wt-1")
        .unwrap();
    wt_client_2
        .send(PeerAddr::WebTransport(0), b"from-wt-2")
        .unwrap();

    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline && seen.len() < 4 {
        while let Some(pkt) = multi.recv() {
            seen.push(pkt);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(seen.len(), 4, "expected all 4 transports: {seen:?}");

    let udp_peer = seen.iter().find(|(_, d)| d == b"from-udp").unwrap().0;
    let ws_peer = seen.iter().find(|(_, d)| d == b"from-ws").unwrap().0;
    let wt_peer_1 = seen.iter().find(|(_, d)| d == b"from-wt-1").unwrap().0;
    let wt_peer_2 = seen.iter().find(|(_, d)| d == b"from-wt-2").unwrap().0;

    assert!(matches!(udp_peer, PeerAddr::Udp(_)));
    assert!(matches!(ws_peer, PeerAddr::Web(1)));
    assert!(matches!(wt_peer_1, PeerAddr::WebTransport(_)));
    assert!(matches!(wt_peer_2, PeerAddr::WebTransport(_)));
    assert_ne!(wt_peer_1, wt_peer_2);

    multi.send(udp_peer, b"to-udp").unwrap();
    multi.send(ws_peer, b"to-ws").unwrap();
    multi.send(wt_peer_1, b"to-wt-1").unwrap();
    multi.send(wt_peer_2, b"to-wt-2").unwrap();

    assert_eq!(
        poll_recv(&mut udp_client, Duration::from_secs(2)).map(|(_, d)| d),
        Some(b"to-udp".to_vec())
    );
    assert_eq!(
        poll_recv(&mut ws_client, Duration::from_secs(2)).map(|(_, d)| d),
        Some(b"to-ws".to_vec())
    );
    assert_eq!(
        poll_recv(&mut wt_client_1, Duration::from_secs(2)).map(|(_, d)| d),
        Some(b"to-wt-1".to_vec())
    );
    assert_eq!(
        poll_recv(&mut wt_client_2, Duration::from_secs(2)).map(|(_, d)| d),
        Some(b"to-wt-2".to_vec())
    );
}

#[test]
fn rejects_oversized_empty_and_rate_limited_datagrams_without_stalling() {
    let mut srv = WtServerTransport::bind(WtServerConfig {
        bind_addr: "127.0.0.1:0".into(),
        max_datagrams_per_sec: 100,
        max_datagram_burst: 50,
        ..WtServerConfig::default()
    })
    .unwrap();
    let url = format!("https://127.0.0.1:{}/wt", srv.local_addr().port());
    let digest = srv.cert_sha256();

    // Use 8192-byte loopback MTU on the test client so a real MAX_UDP_PACKET + 1 (4011 byte)
    // HTTP/3 datagram crosses the wire to prove server-side rejection.
    let mut client =
        WtClientTransport::connect_with_options(&url, Some(digest), Some(8192)).unwrap();

    // Standard PacketTransport::send rejects oversized payloads synchronously.
    assert!(matches!(
        client.send(PeerAddr::WebTransport(0), &vec![0xAB; MAX_UDP_PACKET + 1]),
        Err(TransportError::TooLarge(len)) if len == MAX_UDP_PACKET + 1
    ));

    // Send oversized (> MAX_UDP_PACKET) and empty datagrams directly over QUIC/H3.
    client
        .send_raw_unchecked(&vec![0xCD; MAX_UDP_PACKET + 1])
        .unwrap();
    client.send_raw_unchecked(&[]).unwrap();

    // Follow immediately with a valid GoldSrc A2S_INFO query on the same session.
    let a2s_info = b"\xff\xff\xff\xffTSource Engine Query\0";
    client.send(PeerAddr::WebTransport(0), a2s_info).unwrap();

    let (peer, received) =
        poll_recv(&mut srv, Duration::from_secs(3)).expect("valid packet delivered after drops");
    assert_eq!(received, a2s_info);
    assert!(
        srv.stats().dropped_oversized >= 1,
        "server must record dropped oversized datagram: {:?}",
        srv.stats()
    );
    assert!(
        srv.stats().dropped_empty >= 1,
        "server must record dropped empty datagram: {:?}",
        srv.stats()
    );

    // Session stays completely healthy in the reverse direction too.
    assert!(matches!(
        srv.send(peer, &vec![0xEF; MAX_UDP_PACKET + 1]),
        Err(TransportError::TooLarge(len)) if len == MAX_UDP_PACKET + 1
    ));
    srv.send(peer, b"still-alive").unwrap();
    let (_, reply) = poll_recv(&mut client, Duration::from_secs(3)).expect("client receives reply");
    assert_eq!(reply, b"still-alive");

    // Verify per-peer rate limiter drops burst excess without disconnecting the peer.
    let mut limited_srv = WtServerTransport::bind(WtServerConfig {
        bind_addr: "127.0.0.1:0".into(),
        max_datagrams_per_sec: 10,
        max_datagram_burst: 4,
        ..WtServerConfig::default()
    })
    .unwrap();
    let lim_url = format!("https://127.0.0.1:{}/wt", limited_srv.local_addr().port());
    let mut flood_client =
        WtClientTransport::connect_with_cert_hash(&lim_url, limited_srv.cert_sha256()).unwrap();

    for i in 0..30u8 {
        flood_client
            .send(PeerAddr::WebTransport(0), &[0xFF, 0xFF, 0xFF, 0xFF, i])
            .unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut received_count = 0usize;
    let mut first_peer = None;
    while Instant::now() < deadline {
        while let Some((p, _)) = limited_srv.recv() {
            first_peer = Some(p);
            received_count += 1;
        }
        if limited_srv.stats().dropped_rate_limited > 0 && received_count > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        limited_srv.stats().dropped_rate_limited > 0,
        "rate limiter should drop flood excess: {:?}",
        limited_srv.stats()
    );
    let peer = first_peer.expect("initial burst allowed");
    limited_srv.send(peer, b"post-limit-ok").unwrap();
    let (_, ok_reply) =
        poll_recv(&mut flood_client, Duration::from_secs(2)).expect("session stays open");
    assert_eq!(ok_reply, b"post-limit-ok");
}
