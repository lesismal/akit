//! Media servers behind akit (I114): two akit media servers (`whip` and `whep` on
//! `webrtc.media`) behind a third akit's `webrtc_proxy` routes and `webrtc.proxy` port, with
//! akio-webrtc `Endpoint`s publishing and playing through it. fit's `proxy_test.go` and
//! `proxy_edges_test.go`, ported: each media server gets its own peers through one UDP port;
//! the answers name that port alone; a peer's datagrams before its Binding request are dropped;
//! DELETE forgets a session; the limits refuse.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

mod support;

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use akio_webrtc::Direction;
use support::{Akit, Client, Got, T, flow, http, session, wait_for};

/// A media server whose routes are below `/{prefix}/`.
fn media(prefix: &str) -> Akit {
    Akit::serve(
        &format!(
            r#"{{"engine": {{"event_loops": 2}},
            "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "routes": [{{"match": {{"path": ["/{prefix}/whip/*"]}}, "handler": {{"module": "whip"}}}},
                           {{"match": {{"path": ["/{prefix}/whep/*"]}}, "handler": {{"module": "whep"}}}}],
                "webrtc": {{"media": {{"listen": "127.0.0.1:0"}}}}}}"#
        ),
        &[akit_webrtc::MEDIA],
    )
}

/// The proxy in front of `a` (below `/a/`) and `b` (below `/b/`), with `extra` fields of its
/// `webrtc.proxy`.
fn front(a: &Akit, b: &Akit, extra: &str) -> Akit {
    Akit::serve(
        &format!(
            r#"{{"engine": {{"event_loops": 2}},
            "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "upstreams": [{{"name": "a", "addresses": ["{}"]}}, {{"name": "b", "addresses": ["{}"]}}],
                "routes": [{{"match": {{"path": ["/a/*"]}}, "handler": {{"module": "webrtc_proxy", "params": {{"upstream": "a"}}}}}},
                           {{"match": {{"path": ["/b/*"]}}, "handler": {{"module": "webrtc_proxy", "params": {{"upstream": "b"}}}}}}],
                "webrtc": {{"proxy": {{"listen": "127.0.0.1:0"{extra}}}}}}}"#,
            a.addr, b.addr
        ),
        &[akit_webrtc::PROXY],
    )
}

/// The proxy port's address, from an answer's candidates.
fn candidate_port(sdp: &str) -> Vec<String> {
    sdp.lines()
        .filter(|l| l.starts_with("a=candidate:"))
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            format!("{}:{}", f[4], f[5])
        })
        .collect()
}

fn metric(akit: &Akit, want: &str) {
    let deadline = Instant::now() + T;
    while !akit.metrics().contains(want) {
        assert!(
            Instant::now() < deadline,
            "{want} not in:\n{}",
            akit.metrics()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// fit's `TestProxyTwoServers`: a publisher and a viewer on each of two media servers, all
/// through one akit port; each media server gets its own peers; a DELETE through akit ends the
/// session at the media server and in akit.
#[test]
fn two_media_servers_behind_one_port() {
    let (a, b) = (media("a"), media("b"));
    let f = front(&a, &b, "");
    let clients: Vec<Client> = (0..4).map(|_| Client::new()).collect();
    let pa = session(&f, &clients[0], "/a/whip/cam", Direction::SendOnly)
        .unwrap_or_else(|r| panic!("{} {}", r.status, r.body));
    let pb = session(&f, &clients[1], "/b/whip/cam", Direction::SendOnly)
        .ok()
        .unwrap();
    assert!(pa.location.starts_with("/a/whip/cam/"), "{}", pa.location);
    wait_for(&pa.rx, "a's publisher connected", |g| {
        matches!(g, Got::Connected)
    });
    wait_for(&pb.rx, "b's publisher connected", |g| {
        matches!(g, Got::Connected)
    });
    let va = session(&f, &clients[2], "/a/whep/cam", Direction::RecvOnly)
        .ok()
        .unwrap();
    let vb = session(&f, &clients[3], "/b/whep/cam", Direction::RecvOnly)
        .ok()
        .unwrap();
    wait_for(&va.rx, "a's viewer connected", |g| {
        matches!(g, Got::Connected)
    });
    wait_for(&vb.rx, "b's viewer connected", |g| {
        matches!(g, Got::Connected)
    });
    flow(&pa, &va);
    flow(&pb, &vb);
    // Each media server has its own stream and peers; the proxy relays four addresses.
    for m in [&a, &b] {
        let s = m.metrics();
        assert!(s.contains("akit_webrtc_media_streams 1"), "{s}");
        assert!(
            s.contains(r#"akit_webrtc_media_peer_connections{role="viewer"} 1"#),
            "{s}"
        );
    }
    metric(&f, "akit_webrtc_proxy_relays 4");
    metric(&f, "akit_webrtc_proxy_sessions 4");
    // A DELETE through akit ends the viewer at its media server, and in akit.
    assert_eq!(http(f.addr, "DELETE", &va.location, None, "").status, 200);
    wait_for(&va.rx, "the viewer closed", |g| matches!(g, Got::Closed));
    metric(&f, "akit_webrtc_proxy_sessions 3");
    metric(&a, r#"akit_webrtc_media_peer_connections{role="viewer"} 0"#);
    assert_eq!(http(f.addr, "DELETE", &va.location, None, "").status, 404);
}

/// The answers name the proxy's port alone; a datagram before a Binding request, and a Binding
/// request of no session's ufrag, are dropped and counted (fit's `proxy_edges_test.go`).
#[test]
fn answers_and_drops() {
    let (a, b) = (media("a"), media("b"));
    let f = front(&a, &b, "");
    let c = Client::new();
    let (_tx, _rx) = std::sync::mpsc::channel::<Got>();
    let (peer, offer) =
        c.ep.create_offer(support::Probe(std::sync::Mutex::new(_tx)), |api| {
            api.add_media(
                akio_webrtc::MediaKind::Video,
                Direction::SendOnly,
                None,
                None,
            );
        })
        .unwrap();
    let r = http(f.addr, "POST", "/a/whip/x", Some("application/sdp"), &offer);
    assert_eq!(r.status, 201, "{}", r.body);
    let cands = candidate_port(&r.body);
    assert_eq!(cands.len(), 1, "{}", r.body);
    assert!(r.body.contains("a=end-of-candidates"));
    let port: std::net::SocketAddr = cands[0].parse().unwrap();
    assert_ne!(port, a.addr);
    peer.close();
    // Not STUN, and STUN of an unknown ufrag.
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.send_to(b"hello", port).unwrap();
    metric(
        &f,
        r#"akit_webrtc_proxy_dropped_total{reason="not_stun"} 1"#,
    );
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    let req = akio_turn::stun::MessageBuilder::new(
        akio_turn::stun::Method::BINDING,
        akio_turn::stun::Class::Request,
        akio_turn::stun::transaction_id(),
    )
    .attr(akio_turn::stun::attr::USERNAME, b"nobody:me")
    .build();
    s.send_to(&req, port).unwrap();
    metric(
        &f,
        r#"akit_webrtc_proxy_dropped_total{reason="unknown_ufrag"} 1"#,
    );
    // The edges of the endpoint.
    assert_eq!(
        http(f.addr, "POST", "/a/whip/x", Some("text/plain"), "x").status,
        415
    );
    assert_eq!(http(f.addr, "GET", "/a/whip/x", None, "").status, 405);
    let id = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        http(f.addr, "GET", &format!("/a/whip/x/{id}"), None, "").status,
        404
    );
}

/// `max_sessions` refuses one offer more with 503; the media server's refusals pass on.
#[test]
fn limits() {
    let (a, b) = (media("a"), media("b"));
    let f = front(&a, &b, r#", "max_sessions": 1"#);
    let c = Client::new();
    let _p = session(&f, &c, "/a/whip/one", Direction::SendOnly)
        .ok()
        .unwrap();
    let r = session(&f, &c, "/a/whip/two", Direction::SendOnly)
        .err()
        .unwrap();
    assert_eq!((r.status, r.header("Retry-After")), (503, Some("1")));
    metric(&f, "akit_webrtc_proxy_refused_total 1");
    // The media server's 404 for a stream not published comes through.
    let f2 = front(&a, &b, "");
    let r = session(&f2, &c, "/b/whep/none", Direction::RecvOnly)
        .err()
        .unwrap();
    assert_eq!(r.status, 404);
}

/// The proxy not running: 503.
#[test]
fn not_running() {
    let (a, b) = (media("a"), media("b"));
    let mut f = front(&a, &b, "");
    f.roles.stop(Duration::ZERO);
    let r = http(
        f.addr,
        "POST",
        "/a/whip/x",
        Some("application/sdp"),
        "v=0\r\n",
    );
    assert_eq!(
        (r.status, r.body.as_str()),
        (503, "webrtc.proxy is not running\n")
    );
}

/// The relays leave from `port_min`..`port_max`: a range of one port relays one peer address, and
/// the next is dropped and counted; a session nothing is relayed for is forgotten after
/// `session_timeout` (fit's `proxy_ports_test.go`, `TestSessionTimeout`).
#[test]
fn port_range_and_expiry() {
    let port = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (a, b) = (media("a"), media("b"));
    let f = front(
        &a,
        &b,
        &format!(r#", "port_min": {port}, "port_max": {port}, "session_timeout": "300ms""#),
    );
    let (c1, c2, c3) = (Client::new(), Client::new(), Client::new());
    let p1 = session(&f, &c1, "/a/whip/one", Direction::SendOnly)
        .ok()
        .unwrap();
    wait_for(&p1.rx, "connected", |g| matches!(g, Got::Connected));
    metric(&f, "akit_webrtc_proxy_relays 1");
    let _p2 = session(&f, &c2, "/a/whip/two", Direction::SendOnly)
        .ok()
        .unwrap();
    metric(&f, r#"akit_webrtc_proxy_dropped_total{reason="no_port"} 1"#);
    // An answer no peer follows is forgotten.
    let (tx, _rx) = std::sync::mpsc::channel::<Got>();
    let (_peer, offer) = c3
        .ep
        .create_offer(support::Probe(std::sync::Mutex::new(tx)), |api| {
            api.add_media(
                akio_webrtc::MediaKind::Video,
                Direction::SendOnly,
                None,
                None,
            );
        })
        .unwrap();
    assert_eq!(
        http(
            f.addr,
            "POST",
            "/b/whip/three",
            Some("application/sdp"),
            &offer
        )
        .status,
        201
    );
    metric(
        &f,
        r#"akit_webrtc_proxy_sessions_ended_total{reason="expired"} 2"#,
    );
}
