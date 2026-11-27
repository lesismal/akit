//! WHIP and WHEP through akit (I113), as `akit run` serves them: the routes' `whip` and `whep`
//! handlers on an akit listener, the media server `webrtc.media` on the same engine, and
//! akio-webrtc's `Endpoint`s as publishers and viewers over loopback. fit's `webrtc/media` tests
//! of the media server, ported: a publisher's media reaches its viewers; a publisher gone ends
//! them; DELETE ends a session; the limits refuse; the HTTP edges (methods, types, sizes).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

mod support;

use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use akio_webrtc::{Codec, Direction, Frequency, MediaKind, MediaTime};
use support::{Akit, Client, Got, Probe, T, flow, http, session, wait_for};

/// fit's `TestPublishPlay`: a publisher's audio and video reach two viewers through akit; the
/// answers name akit's port alone, with the ICE servers in `Link` headers; a DELETE ends a
/// viewer; the publisher gone ends the rest.
#[test]
fn publish_and_play() {
    let akit = Akit::start(
        r#"{"listen": "127.0.0.1:0", "ice_servers": [{"urls": ["stun:stun.example.com:3478"]}]}"#,
    );
    // A client a session: akit tells peers apart by their addresses once they check.
    let (c, c1, c2) = (Client::new(), Client::new(), Client::new());
    let p = session(&akit, &c, "/whip/cam1", Direction::SendOnly)
        .unwrap_or_else(|r| panic!("whip: {} {}", r.status, r.body));
    assert!(p.location.starts_with("/whip/cam1/"), "{}", p.location);
    assert_eq!(
        p.links,
        [r#"<stun:stun.example.com:3478>; rel="ice-server""#]
    );
    wait_for(&p.rx, "the publisher connected", |g| {
        matches!(g, Got::Connected)
    });
    let v1 = session(&akit, &c1, "/whep/cam1", Direction::RecvOnly)
        .unwrap_or_else(|r| panic!("whep: {} {}", r.status, r.body));
    wait_for(&v1.rx, "a viewer connected", |g| {
        matches!(g, Got::Connected)
    });
    let v2 = session(&akit, &c2, "/whep/cam1", Direction::RecvOnly)
        .ok()
        .unwrap();
    wait_for(&v2.rx, "another viewer connected", |g| {
        matches!(g, Got::Connected)
    });
    flow(&p, &v1);
    flow(&p, &v2);
    let m = akit.metrics();
    assert!(m.contains("akit_webrtc_media_streams 1"), "{m}");
    assert!(
        m.contains(r#"akit_webrtc_media_peer_connections{role="viewer"} 2"#),
        "{m}"
    );
    // A DELETE of a viewer's URL ends it alone; a second one finds nothing.
    assert_eq!(
        http(akit.addr, "DELETE", &v1.location, None, "").status,
        200
    );
    wait_for(&v1.rx, "the viewer closed", |g| matches!(g, Got::Closed));
    assert_eq!(
        http(akit.addr, "DELETE", &v1.location, None, "").status,
        404
    );
    // The publisher's DELETE ends the stream and its viewers.
    assert_eq!(http(akit.addr, "DELETE", &p.location, None, "").status, 200);
    wait_for(&v2.rx, "the viewer of a gone publisher closed", |g| {
        matches!(g, Got::Closed)
    });
    let m = akit.metrics();
    assert!(m.contains("akit_webrtc_media_streams 0"), "{m}");
    assert!(
        m.contains(r#"akit_webrtc_media_sessions_ended_total{reason="publisher_left"} 1"#),
        "{m}"
    );
    assert!(
        m.contains(r#"akit_webrtc_media_sessions_ended_total{reason="delete"} 2"#),
        "{m}"
    );
}

/// fit's limits: one stream past `max_streams`, one viewer past `max_viewers` and one
/// PeerConnection past `max_peer_connections` are answered 503 with `Retry-After`; a second
/// publisher of a stream 409; a viewer of a stream not published 404.
#[test]
fn limits() {
    let akit = Akit::start(
        r#"{"listen": "127.0.0.1:0", "max_streams": 1, "max_viewers": 1, "max_peer_connections": 3}"#,
    );
    let c = Client::new();
    let _p = session(&akit, &c, "/whip/a", Direction::SendOnly)
        .ok()
        .unwrap();
    let r = session(&akit, &c, "/whip/a", Direction::SendOnly)
        .err()
        .unwrap();
    assert_eq!(r.status, 409, "{}", r.body);
    let r = session(&akit, &c, "/whip/b", Direction::SendOnly)
        .err()
        .unwrap();
    assert_eq!((r.status, r.header("Retry-After")), (503, Some("5")));
    assert_eq!(r.body, "too many streams\n");
    let r = session(&akit, &c, "/whep/none", Direction::RecvOnly)
        .err()
        .unwrap();
    assert_eq!(r.status, 404);
    let _v = session(&akit, &c, "/whep/a", Direction::RecvOnly)
        .ok()
        .unwrap();
    let r = session(&akit, &c, "/whep/a", Direction::RecvOnly)
        .err()
        .unwrap();
    assert_eq!(
        (r.status, r.body.as_str()),
        (503, "too many viewers of the stream\n")
    );
    let m = akit.metrics();
    for want in [
        r#"akit_webrtc_media_refused_total{reason="conflict"} 1"#,
        r#"akit_webrtc_media_refused_total{reason="streams"} 1"#,
        r#"akit_webrtc_media_refused_total{reason="no_stream"} 1"#,
        r#"akit_webrtc_media_refused_total{reason="viewers"} 1"#,
    ] {
        assert!(m.contains(want), "{want}:\n{m}");
    }
}

/// fit's `http_edges_test.go`: the methods, types and sizes of the endpoint and the session's
/// URL.
#[test]
fn http_edges() {
    let mut akit = Akit::start(r#"{"listen": "127.0.0.1:0", "max_sdp_bytes": 64}"#);
    let a = akit.addr;
    let r = http(a, "OPTIONS", "/whip/cam", None, "");
    assert_eq!(
        (r.status, r.header("Allow"), r.header("Accept-Post")),
        (204, Some("POST, OPTIONS"), Some("application/sdp"))
    );
    assert_eq!(http(a, "GET", "/whip/cam", None, "").status, 405);
    assert_eq!(
        http(a, "POST", "/whip/cam", Some("text/plain"), "x").status,
        415
    );
    assert_eq!(
        http(a, "POST", "/whip/a%2Fb", Some("application/sdp"), "x").status,
        404
    );
    let big = "v=0\r\n".repeat(20);
    assert_eq!(
        http(a, "POST", "/whip/cam", Some("application/sdp"), &big).status,
        413
    );
    let r = http(
        a,
        "POST",
        "/whip/cam",
        Some("application/sdp; charset=utf-8"),
        "v=0\r\n",
    );
    assert_eq!(r.status, 422, "{}", r.body);
    let id = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        http(a, "DELETE", &format!("/whip/cam/{id}"), None, "").status,
        404
    );
    assert_eq!(
        http(a, "PATCH", &format!("/whip/cam/{id}"), None, "").status,
        404
    );
    // A session's URL takes DELETE and OPTIONS; PATCH is refused.
    akit.apply(r#"{"listen": "127.0.0.1:0"}"#);
    let c = Client::new();
    let p = session(&akit, &c, "/whip/cam", Direction::SendOnly)
        .ok()
        .unwrap();
    let r = http(
        a,
        "PATCH",
        &p.location,
        Some("application/trickle-ice-sdpfrag"),
        "",
    );
    assert_eq!(
        (r.status, r.header("Allow")),
        (405, Some("DELETE, OPTIONS"))
    );
    assert_eq!(http(a, "OPTIONS", &p.location, None, "").status, 204);
    // whep's DELETE does not end a publisher.
    let whep_url = p.location.replace("/whip/", "/whep/");
    assert_eq!(http(a, "DELETE", &whep_url, None, "").status, 404);
    // The role gone: the endpoint answers 503.
    akit.roles.stop(Duration::ZERO);
    let r = http(a, "POST", "/whip/cam", Some("application/sdp"), "v=0\r\n");
    assert_eq!(
        (r.status, r.body.as_str()),
        (503, "webrtc.media is not running\n")
    );
}

/// A reload that keeps `listen` keeps the sessions and applies the limits to the next offer; one
/// that changes it ends them (fit's `TestReload`).
#[test]
fn reload() {
    let port = std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listen = format!(r#""listen": "127.0.0.1:{port}""#);
    let mut akit = Akit::start(&format!("{{{listen}}}"));
    let c = Client::new();
    let p = session(&akit, &c, "/whip/a", Direction::SendOnly)
        .ok()
        .unwrap();
    wait_for(&p.rx, "connected", |g| matches!(g, Got::Connected));
    akit.apply(&format!(r#"{{{listen}, "max_streams": 1}}"#));
    assert_eq!(
        session(&akit, &c, "/whip/b", Direction::SendOnly)
            .err()
            .unwrap()
            .status,
        503
    );
    assert!(akit.metrics().contains("akit_webrtc_media_streams 1"));
    akit.apply(r#"{"listen": "127.0.0.1:0"}"#);
    wait_for(&p.rx, "the session ended by the restart", |g| {
        matches!(g, Got::Closed)
    });
    let m = akit.metrics();
    assert!(m.contains("akit_webrtc_media_restarts_total 1"), "{m}");
    assert!(m.contains("akit_webrtc_media_streams 0"), "{m}");
}

/// fit's `cut_test.go`: a viewer cut midway (its endpoint gone without a word) holds neither the
/// publisher nor the other viewers.
#[test]
fn cut_viewer() {
    let akit = Akit::start(r#"{"listen": "127.0.0.1:0"}"#);
    let (c, c1, c2) = (Client::new(), Client::new(), Client::new());
    let p = session(&akit, &c, "/whip/s", Direction::SendOnly)
        .ok()
        .unwrap();
    wait_for(&p.rx, "connected", |g| matches!(g, Got::Connected));
    let v1 = session(&akit, &c1, "/whep/s", Direction::RecvOnly)
        .ok()
        .unwrap();
    wait_for(&v1.rx, "connected", |g| matches!(g, Got::Connected));
    let v2 = session(&akit, &c2, "/whep/s", Direction::RecvOnly)
        .ok()
        .unwrap();
    wait_for(&v2.rx, "connected", |g| matches!(g, Got::Connected));
    flow(&p, &v1);
    drop(c1);
    flow(&p, &v2);
}

/// `connect_timeout` ends a session whose peer never connects; `max_bitrate` drops a
/// publisher's media past two seconds' burst, counted.
#[test]
fn timeout_and_bitrate() {
    let akit = Akit::start(
        r#"{"listen": "127.0.0.1:0", "connect_timeout": "500ms", "max_bitrate": 16000}"#,
    );
    let (c, c1, c2) = (Client::new(), Client::new(), Client::new());
    // An offer posted, its answer never taken.
    let (tx, _rx) = channel();
    let (_peer, offer) = c2
        .ep
        .create_offer(Probe(Mutex::new(tx)), |api| {
            api.add_media(MediaKind::Video, Direction::SendOnly, None, None);
        })
        .unwrap();
    assert_eq!(
        http(
            akit.addr,
            "POST",
            "/whip/late",
            Some("application/sdp"),
            &offer
        )
        .status,
        201
    );
    let deadline = Instant::now() + T;
    while !akit
        .metrics()
        .contains(r#"akit_webrtc_media_sessions_ended_total{reason="connect_timeout"} 1"#)
    {
        assert!(Instant::now() < deadline, "{}", akit.metrics());
        std::thread::sleep(Duration::from_millis(50));
    }
    // 16 kb/s: 4 KB of burst; each frame is 3 KB.
    let p = session(&akit, &c, "/whip/s", Direction::SendOnly)
        .ok()
        .unwrap();
    wait_for(&p.rx, "connected", |g| matches!(g, Got::Connected));
    let v = session(&akit, &c1, "/whep/s", Direction::RecvOnly)
        .ok()
        .unwrap();
    wait_for(&v.rx, "connected", |g| matches!(g, Got::Connected));
    let vp8 = p.peer.payload_type(p.video, Codec::Vp8).unwrap();
    for i in 0..20u64 {
        p.peer
            .write_media(
                p.video,
                vp8,
                Instant::now(),
                MediaTime::new(i * 3000, Frequency::NINETY_KHZ),
                &[0x10; 3000],
            )
            .unwrap();
    }
    let deadline = Instant::now() + T;
    while akit
        .metrics()
        .contains(r#"akit_webrtc_media_dropped_total{reason="bitrate"} 0"#)
    {
        assert!(Instant::now() < deadline, "{}", akit.metrics());
        std::thread::sleep(Duration::from_millis(50));
    }
}
