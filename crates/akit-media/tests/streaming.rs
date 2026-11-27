//! The streaming role (I122) on akio, with fit's `streaming/streaming_test.go`,
//! `listeners_test.go`, `limits_test.go` and `access_test.go` cases ported: akio-media's RTMP,
//! RTSP and SRT clients publish into the role's hub and play from it, with and without the
//! credentials its `media_auth` policy asks for; limits, the access lists and reloads hold.
//!
//! Every wait is on state the test can see (the hub's streams, a client's state, a connection's
//! end), each with a deadline, never on time alone: akio-media's RTSP and RTMP tests have shown
//! load-sensitive races (`docs/progress.md`).
//!
//! `tests/data/h264_aac.flv` is akio-media's test file (`crates/akio-media/testdata`, MIT).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use akio::Engine;
use akio_media::flv::{Decoder, Demuxer, Event};
use akio_media::hub::{HubError, Sink, SubscribeOptions};
use akio_media::rtmp::client::{self as rtmp, ClientState, RtmpClientConfig};
use akio_media::rtsp::{RtspClientConfig, client as rtsp};
use akio_media::srt;
use akio_media::{Hub, Packet, PeerInfo, Publisher, Tracks};
use akit_roles::host::Manager;

const FLV: &[u8] = include_bytes!("data/h264_aac.flv");
const TOKEN: &str = "0123456789abcdef-publish";

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Tracks and packets of an FLV file.
fn decode(flv: &[u8]) -> (Tracks, Vec<Packet>) {
    let mut dec = Decoder::new();
    let mut tracks = None;
    let mut pkts = Vec::new();
    for t in Demuxer::new().feed(flv).unwrap() {
        for e in dec.write(t.tag_type, t.ts, &t.body) {
            match e {
                Event::Tracks(t) => tracks = Some(t),
                Event::Packet(p) => pkts.push(p),
                _ => {}
            }
        }
    }
    (tracks.unwrap(), pkts)
}

/// A source hub with the file published at `path`, all at once.
fn source(path: &str) -> (Hub, Publisher, usize) {
    let hub = Hub::default();
    let (tracks, pkts) = decode(FLV);
    let videos = pkts.iter().filter(|p| p.track == 0).count();
    let p = hub.publish(path, tracks, PeerInfo::default()).unwrap();
    for pkt in pkts {
        p.write(pkt);
    }
    (hub, p, videos)
}

/// Counts what a hub subscription hears.
#[derive(Default)]
struct Counts {
    video: AtomicUsize,
    closed: AtomicUsize,
}

struct Count(Arc<Counts>);

impl Sink for Count {
    fn on_tracks(&mut self, _: &Tracks) {}
    fn on_packet(&mut self, p: &Arc<Packet>) {
        if p.track == 0 {
            self.0.video.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn on_close(&mut self, _: HubError) {
        self.0.closed.fetch_add(1, Ordering::SeqCst);
    }
}

/// The role on an engine of its own, as `akit run` runs it after the listeners.
struct Role {
    engine: Arc<Engine>,
    roles: Manager,
    clients: Engine,
}

impl Role {
    fn start(json: &str) -> Role {
        let engine = Arc::new(Engine::builder().client_threads(1).build().unwrap());
        engine.start().unwrap();
        let mut roles = Manager::new(
            &[
                akit_media::role::MEDIA_AUTH,
                akit_media::streaming::STREAMING,
            ],
            Arc::clone(&engine),
            None,
        );
        let errs = roles.apply(&akit_config::from_json(json, None).unwrap());
        assert!(errs.is_empty(), "{errs:?}");
        let clients = Engine::builder()
            .client_threads(1)
            .udp_client_threads(1)
            .build()
            .unwrap();
        clients.start().unwrap();
        Role {
            engine,
            roles,
            clients,
        }
    }

    fn apply(&mut self, json: &str) -> Vec<String> {
        self.roles
            .apply(&akit_config::from_json(json, None).unwrap())
    }

    /// The hub the role serves now.
    fn hub(&self) -> Hub {
        let slot = akit_media::streaming::lookup(akit_media::auth::registry::key(&self.engine));
        slot.with(|c| c.map(|c| c.hub.clone())).unwrap()
    }

    /// The address of listener `proto`'s, from the engine.
    fn addr(&self, port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }
}

impl Drop for Role {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.clients.shutdown();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn policy(public_play: bool) -> String {
    format!(
        r#"{{"name": "live", {}"tokens": [{{"name": "studio", "token": "{TOKEN}", "publish": ["live/*"], "play": ["*"]}}],
        "users": [{{"name": "alice", "password": "alice-pw", "publish": ["alice/*"], "play": ["*"]}}]}}"#,
        if public_play {
            r#""public": ["play"], "#
        } else {
            ""
        }
    )
}

/// RTMP: a publish without credentials and with a token for another stream refused, with the
/// token let in; a play public; the stream played back through akio's client; the refusals
/// counted.
#[test]
fn rtmp_publish_and_play() {
    let port = free_port();
    let role = Role::start(&format!(
        r#"{{"media_auth": [{}], "streaming": {{"rtmp": {{"listen": "127.0.0.1:{port}", "auth": "live"}}}}}}"#,
        policy(true)
    ));
    let addr = role.addr(port);
    let hub = role.hub();
    let (src, _p, videos) = source("src");
    // No credentials: refused, the client closed.
    let bad = rtmp::push(
        &role.clients,
        &format!("rtmp://{addr}/live/cam"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the refused push to end", || bad.is_closed());
    assert!(hub.stream_info("live/cam").is_none_or(|s| !s.publishing));
    // A token that may not publish this stream.
    let bad = rtmp::push(
        &role.clients,
        &format!("rtmp://{addr}/other/cam?token={TOKEN}"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the forbidden push to end", || bad.is_closed());
    // Alice's password in the query.
    let alice = rtmp::push(
        &role.clients,
        &format!("rtmp://{addr}/alice/cam?user=alice&pass=alice-pw"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("alice publishing", || {
        hub.stream_info("alice/cam").is_some_and(|s| s.publishing)
    });
    alice.close();
    // The token: published, and played back by a player without credentials (public play).
    let (counts, _sub) = {
        let c = Arc::new(Counts::default());
        let mut o = SubscribeOptions::new(PeerInfo::default());
        o.start_wait = Some(Duration::from_secs(20));
        let sub = hub.subscribe("live/cam", Count(Arc::clone(&c)), o).unwrap();
        (c, sub)
    };
    let pusher = rtmp::push(
        &role.clients,
        &format!("rtmp://{addr}/live/cam?token={TOKEN}"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the push streaming", || {
        assert!(!pusher.is_closed(), "{:?}", pusher.error());
        pusher.state() == ClientState::Streaming
    });
    // What the pusher sent of the source's cached group of pictures reaches the role's hub.
    wait_for("the stream in the role's hub", || {
        counts.video.load(Ordering::SeqCst) > 0
    });
    assert!(counts.video.load(Ordering::SeqCst) <= videos);
    let played = Hub::default();
    let player = rtmp::pull(
        &role.clients,
        &format!("rtmp://{addr}/live/cam"),
        &played,
        "p",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the play streaming", || {
        assert!(!player.is_closed(), "{:?}", player.error());
        player.state() == ClientState::Streaming
    });
    wait_for("the played stream's tracks", || {
        played.stream_info("p").is_some_and(|s| s.publishing)
    });
    player.close();
    pusher.close();
    // The metrics: two refusals of RTMP, one unauthorized and one forbidden.
    let mut w = akit_observe::metrics::TextWriter::new(Vec::new());
    akit_media::streaming::write_metrics_of(&role.engine, &mut w);
    let text = String::from_utf8(w.out).unwrap();
    for want in [
        "akit_streaming_auth_refused_total{protocol=\"rtmp\",reason=\"unauthorized\"} 1",
        "akit_streaming_auth_refused_total{protocol=\"rtmp\",reason=\"forbidden\"} 1",
    ] {
        assert!(text.contains(want), "{want}\n{text}");
    }
    assert!(text.contains("akit_streaming_published_total 2"), "{text}");
}

/// RTSP: a publish with the token in the URL's query (Digest and Basic wait for akio A264); a
/// play that needs credentials refused without them, and with them too until A264: akio-media's
/// DESCRIBE names the stream's tracks without the query, so the SETUP and PLAY a client sends
/// carry none; public play let in.
#[test]
fn rtsp_with_a_query_token() {
    for public in [false, true] {
        let port = free_port();
        let role = Role::start(&format!(
            r#"{{"media_auth": [{}], "streaming": {{"rtsp": {{"listen": "127.0.0.1:{port}", "auth": "live"}}}}}}"#,
            policy(public)
        ));
        let addr = role.addr(port);
        let hub = role.hub();
        let (src, _p, _) = source("src");
        let mut cfg = RtspClientConfig::default();
        cfg.push_wait = Duration::from_secs(10);
        // Refused at ANNOUNCE: 403 (the challenges wait for akio A264).
        match rtsp::push(
            &role.clients,
            &format!("rtsp://{addr}/live/cam"),
            &src,
            "src",
            cfg.clone(),
        ) {
            Err(e) => assert!(format!("{e:?}").contains("403"), "{e:?}"),
            Ok(bad) => wait_for("the refused push to end", || bad.is_closed()),
        }
        assert!(hub.stream_info("live/cam").is_none_or(|s| !s.publishing));
        let pusher = rtsp::push(
            &role.clients,
            &format!("rtsp://{addr}/live/cam?token={TOKEN}"),
            &src,
            "src",
            cfg,
        )
        .unwrap();
        wait_for("the stream published", || {
            assert!(!pusher.is_closed(), "{:?}", pusher.error());
            hub.stream_info("live/cam").is_some_and(|s| s.publishing)
        });
        let played = Hub::default();
        let player = rtsp::pull(
            &role.clients,
            &format!("rtsp://{addr}/live/cam"),
            &played,
            "p",
            RtspClientConfig::default(),
        );
        if public {
            let player = player.unwrap();
            wait_for("the played stream", || {
                assert!(!player.is_closed(), "{:?}", player.error());
                played.stream_info("p").is_some_and(|s| s.publishing)
            });
            player.close();
        } else {
            if let Ok(bad) = player {
                wait_for("the refused play to end", || bad.is_closed());
            }
            assert!(played.stream_info("p").is_none_or(|s| !s.publishing));
        }
        pusher.close();
    }
}

/// SRT: the token as the stream ID's `s=`, a user's password as `u=` and `s=`; refused with 1401
/// without, 1403 for a stream the token may not publish; fit's `r=live/cam?token=T` refused as
/// a bad request, 1400 (akio A265); a plain stream ID's query taken; a stream without a
/// publisher rejected with 1404.
#[test]
fn srt_with_stream_ids() {
    use akio_media::srt::{RejectReason, SrtError, SrtHandler};
    struct Closed(std::sync::mpsc::Sender<Option<SrtError>>);
    impl SrtHandler for Closed {
        fn on_data(&mut self, _: Vec<u8>) {}
        fn on_close(&mut self, err: Option<SrtError>) {
            let _ = self.0.send(err);
        }
    }
    let port = free_udp_port();
    let role = Role::start(&format!(
        r#"{{"media_auth": [{}], "streaming": {{"srt": {{"listen": "127.0.0.1:{port}", "auth": "live"}}}}}}"#,
        policy(true)
    ));
    let addr = role.addr(port);
    let hub = role.hub();
    let dial = |sid: &str| {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut cfg = srt::Config::default();
        cfg.stream_id = sid.to_owned();
        let s = srt::dial(&role.clients, addr, cfg, Closed(tx)).unwrap();
        (s, rx)
    };
    let rejected = |sid: &str, want: RejectReason| {
        let (_s, closed) = dial(sid);
        assert_eq!(
            closed.recv_timeout(Duration::from_secs(20)).unwrap(),
            Some(SrtError::Rejected(want)),
            "{sid}"
        );
    };
    rejected("#!::r=live/cam,m=publish", RejectReason::UNAUTHORIZED);
    rejected(
        &format!("#!::r=other/cam,m=publish,s={TOKEN}"),
        RejectReason::FORBIDDEN,
    );
    rejected(
        &format!("#!::r=live/cam?token={TOKEN},m=publish"),
        RejectReason::BAD_REQUEST,
    );
    rejected(
        "#!::r=live/cam,m=publish,u=alice,s=nope",
        RejectReason::UNAUTHORIZED,
    );
    let ts = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/h264_aac.ts"
    ))
    .unwrap();
    let mut publishers = Vec::new();
    for (sid, path) in [
        (format!("#!::r=live/cam,m=publish,s={TOKEN}"), "live/cam"),
        (
            "#!::r=alice/cam,m=publish,u=alice,s=alice-pw".to_owned(),
            "alice/cam",
        ),
    ] {
        let (s, closed) = dial(&sid);
        wait_for("the publisher connected", || s.is_connected());
        for chunk in ts.chunks(1316) {
            if s.send(chunk).is_err() {
                break;
            }
        }
        wait_for("the stream published", || {
            hub.stream_info(path).is_some_and(|i| i.publishing)
        });
        publishers.push((s, closed));
    }
    // A plain stream ID plays, its query's credentials taken (anyone may play here).
    let (player, _closed) = dial(&format!("live/cam?token={TOKEN}"));
    wait_for("the player connected", || player.is_connected());
    rejected("#!::r=none/cam", RejectReason::NOT_FOUND);
    player.close();
    for (s, _) in publishers {
        s.close();
    }
}

/// fit's `TestLimits`: connections past `max_conns` closed as they open.
#[test]
fn rtmp_max_conns() {
    let port = free_port();
    let role = Role::start(&format!(
        r#"{{"streaming": {{"rtmp": {{"listen": "127.0.0.1:{port}", "max_conns": 2}}}}}}"#
    ));
    let addr = role.addr(port);
    let open = |n: usize| -> Vec<TcpStream> {
        (0..n)
            .map(|_| {
                let s = TcpStream::connect(addr).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                s
            })
            .collect()
    };
    let kept = open(2);
    // Each kept one answered (S0 to its C0 and C1) before the third opens: the server took both
    // first, whichever I/O threads accept them.
    for mut s in &kept {
        let mut c = vec![0u8; 1537];
        c[0] = 3;
        s.write_all(&c).unwrap();
        let mut s0 = [0u8; 1];
        s.read_exact(&mut s0).unwrap();
        assert_eq!(s0[0], 3);
    }
    let mut third = open(1).pop().unwrap();
    let mut b = [0u8; 1];
    // Closed by the server: EOF or a reset, not a timeout.
    match third.read(&mut b) {
        Ok(0) => {}
        Err(e) => assert!(
            e.kind() != std::io::ErrorKind::WouldBlock && e.kind() != std::io::ErrorKind::TimedOut,
            "{e}"
        ),
        Ok(n) => panic!("read {n} bytes"),
    }
    // One goes: another may open.
    drop(kept);
    wait_for("a connection to be taken again", || {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        // Open while the server keeps it: a read times out rather than ending.
        matches!(s.read(&mut b), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut)
    });
}

/// fit's `TestAccess`: a connection the role's access refuses is closed as it is accepted, and a
/// reload that allows it lets the next one in, the listener kept.
#[test]
fn access_and_reload() {
    let port = free_port();
    let cfg = |access: &str, max: u32| {
        format!(
            r#"{{"streaming": {{"max_streams": {max}, "rtmp": {{"listen": "127.0.0.1:{port}"}}, "access": {access}}}}}"#
        )
    };
    let mut role = Role::start(&cfg(r#"{"deny": ["127.0.0.0/8"]}"#, 8));
    let addr = role.addr(port);
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut b = [0u8; 1];
    assert!(matches!(s.read(&mut b), Ok(0) | Err(_)));
    let hub = role.hub();
    assert!(role.apply(&cfg("{}", 8)).is_empty());
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    assert!(
        matches!(s.read(&mut b), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut)
    );
    // The same hub: only the access changed.
    let (src, _p, _) = source("src");
    let pusher = rtmp::push(
        &role.clients,
        &format!("rtmp://{addr}/live/cam"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the stream published", || {
        hub.stream_info("live/cam").is_some_and(|s| s.publishing)
    });
    // The hub's limits changed: everything starts again, the stream ended.
    assert!(role.apply(&cfg("{}", 9)).is_empty());
    wait_for("the push ended", || pusher.is_closed());
    assert!(role.hub().stream_info("live/cam").is_none());
    let mut w = akit_observe::metrics::TextWriter::new(Vec::new());
    akit_media::streaming::write_metrics_of(&role.engine, &mut w);
    let text = String::from_utf8(w.out).unwrap();
    assert!(text.contains("akit_streaming_restarts_total 1"), "{text}");
}

/// A reload that changes a listener's `auth` keeps the listener and its connections, the next
/// publish checked by the policy as it is now.
#[test]
fn auth_changed_in_place() {
    let port = free_port();
    let cfg = |auth: &str| {
        format!(
            r#"{{"media_auth": [{}], "streaming": {{"rtmp": {{"listen": "127.0.0.1:{port}"{auth}}}}}}}"#,
            policy(false)
        )
    };
    let mut role = Role::start(&cfg(""));
    let addr = role.addr(port);
    let hub = role.hub();
    let (src, _p, _) = source("src");
    let open = rtmp::push(
        &role.clients,
        &format!("rtmp://{addr}/a/b"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the open push", || {
        hub.stream_info("a/b").is_some_and(|s| s.publishing)
    });
    assert!(role.apply(&cfg(r#", "auth": "live""#)).is_empty());
    // The publisher connected before stays.
    assert!(!open.is_closed());
    let bad = rtmp::push(
        &role.clients,
        &format!("rtmp://{addr}/c/d"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the next push refused", || bad.is_closed());
    assert!(hub.stream_info("c/d").is_none_or(|s| !s.publishing));
    open.close();
}
