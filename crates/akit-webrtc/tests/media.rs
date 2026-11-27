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

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio_webrtc::{
    Codec, Direction, Endpoint, Frequency, MediaData, MediaKind, MediaTime, Mid, Peer, PeerHandler,
};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

const T: Duration = Duration::from_secs(15);

/// akit serving `doc`: its listener, routes and roles on one engine.
struct Akit {
    engine: Arc<akio::Engine>,
    roles: Manager,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => akio::Backend::IoUring({
            let mut v = akio::UringConfig::default();
            v.strict = true;
            v
        }),
        _ => akio::Backend::Epoll,
    };
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

fn config(media: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "routes": [{{"match": {{"path": ["/whip/*"]}}, "handler": {{"module": "whip"}}}},
                       {{"match": {{"path": ["/whep/*"]}}, "handler": {{"module": "whep"}}}}],
            "webrtc": {{"media": {media}}}}}"#
    )
}

impl Akit {
    fn start(media: &str) -> Akit {
        let doc = config(media);
        let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}: {e}"));
        let registry: Registry<HandlerParams> = Registry::new(&[akit_webrtc::MODULES]).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = *engine.local_addrs().first().unwrap();
        let mut roles = Manager::new(&[akit_webrtc::MEDIA], Arc::clone(&engine), None);
        let errs = roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        Akit {
            engine,
            roles,
            addr,
            _cell: cell,
        }
    }

    fn apply(&mut self, media: &str) {
        let config = akit_config::from_json(&config(media), None).unwrap();
        let errs = self.roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
    }

    fn metrics(&self) -> String {
        let mut out = Vec::new();
        akit_roles::host::write_meters(&self.roles.meters(), &mut out);
        String::from_utf8(out).unwrap()
    }
}

/// An HTTP/1.1 response: its status, header fields and body.
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

fn http(addr: SocketAddr, method: &str, path: &str, ctype: Option<&str>, body: &str) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(T)).unwrap();
    let ct = ctype.map_or(String::new(), |c| format!("Content-Type: {c}\r\n"));
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: akit\r\n{ct}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Resp {
        status,
        headers,
        body: body.to_owned(),
    }
}

enum Got {
    Connected,
    Media(MediaKind, Vec<u8>),
    Closed,
}

struct Probe(Mutex<Sender<Got>>);

impl PeerHandler for Probe {
    fn on_connected(&mut self, _p: &Peer) {
        let _ = self.0.lock().unwrap().send(Got::Connected);
    }
    fn on_media_data(&mut self, _p: &Peer, d: &MediaData) {
        let kind = if d.codec == Some(Codec::Opus) {
            MediaKind::Audio
        } else {
            MediaKind::Video
        };
        let _ = self
            .0
            .lock()
            .unwrap()
            .send(Got::Media(kind, d.data.to_vec()));
    }
    fn on_close(&mut self, _p: &Peer) {
        let _ = self.0.lock().unwrap().send(Got::Closed);
    }
}

/// A client engine and its endpoint.
struct Client {
    engine: Arc<akio::Engine>,
    ep: Endpoint,
}

impl Client {
    fn new() -> Client {
        let engine = akio::Engine::builder()
            .udp_client_threads(1)
            .build()
            .unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        let ep = Endpoint::bind(&engine, "127.0.0.1:0".parse().unwrap()).unwrap();
        Client { engine, ep }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.ep.close();
        self.engine.shutdown();
        self.engine.wait();
    }
}

/// A peer of `c` offering `dir` for audio and video, its offer posted to `path`.
struct Session {
    peer: Peer,
    rx: Receiver<Got>,
    audio: Mid,
    video: Mid,
    location: String,
    links: Vec<String>,
}

fn session(akit: &Akit, c: &Client, path: &str, dir: Direction) -> Result<Session, Resp> {
    let (tx, rx) = channel();
    let mut mids = (None, None);
    let (peer, offer) =
        c.ep.create_offer(Probe(Mutex::new(tx)), |api| {
            mids.0 = Some(api.add_media(MediaKind::Audio, dir, None, None));
            mids.1 = Some(api.add_media(MediaKind::Video, dir, None, None));
        })
        .unwrap();
    let r = http(akit.addr, "POST", path, Some("application/sdp"), &offer);
    if r.status != 201 {
        peer.close();
        return Err(r);
    }
    assert_eq!(r.header("Content-Type"), Some("application/sdp"));
    peer.accept_answer(&r.body).unwrap();
    let location = r.header("Location").unwrap().to_owned();
    let links = r.all("Link").into_iter().map(str::to_owned).collect();
    Ok(Session {
        peer,
        rx,
        audio: mids.0.unwrap(),
        video: mids.1.unwrap(),
        location,
        links,
    })
}

fn wait_for(rx: &Receiver<Got>, what: &str, f: impl Fn(&Got) -> bool) {
    let deadline = Instant::now() + T;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(g) if f(&g) => return,
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

/// Publishes a few frames of audio and video on `p` until `viewer` got both kinds.
fn flow(p: &Session, viewer: &Session) {
    let opus = p.peer.payload_type(p.audio, Codec::Opus).expect("opus");
    let vp8 = p.peer.payload_type(p.video, Codec::Vp8).expect("vp8");
    let frame: Vec<u8> = (0..3000u32).map(|i| (i * 7) as u8).collect();
    let (mut audio, mut video) = (false, false);
    let start = Instant::now();
    for i in 0..400u64 {
        p.peer
            .write_media(
                p.audio,
                opus,
                Instant::now(),
                MediaTime::new(i * 960, Frequency::FORTY_EIGHT_KHZ),
                &[0xfc, i as u8, 1, 2, 3],
            )
            .unwrap();
        let mut f = frame.clone();
        f[0] = 0x10;
        p.peer
            .write_media(
                p.video,
                vp8,
                Instant::now(),
                MediaTime::new(i * 3000, Frequency::NINETY_KHZ),
                &f,
            )
            .unwrap();
        while let Ok(g) = viewer.rx.recv_timeout(Duration::from_millis(20)) {
            match g {
                Got::Media(MediaKind::Audio, d) => {
                    assert_eq!(&d[2..], &[1, 2, 3]);
                    audio = true;
                }
                Got::Media(MediaKind::Video, d) => {
                    assert_eq!(&d[1..], &frame[1..]);
                    video = true;
                }
                _ => {}
            }
        }
        if audio && video {
            return;
        }
        assert!(start.elapsed() < T, "no media reached the viewer");
    }
    panic!("audio {audio}, video {video}");
}

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
