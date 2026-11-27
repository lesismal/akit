//! The `httpflv` and `hls` handlers (I123) through akit's listener, beside the streaming role on
//! the same engine, as `akit run` runs them: fit's `streaming/streaming_test.go` handler cases
//! ported. A stream published over RTMP plays over HTTP-FLV and HLS (fMP4 and MPEG-TS) with
//! credentials and is refused without; a playlist fetched with `?token=` hands the token to what
//! it names; a handler answers 503 while the role does not run.
//!
//! Every wait is on what the test can see, with a deadline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use akio::Engine;
use akio_media::av::SECOND;
use akio_media::flv::{Decoder, Demuxer, Event};
use akio_media::rtmp::client::{self as rtmp, ClientState, RtmpClientConfig};
use akio_media::{Hub, Packet, PeerInfo, Tracks};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

const FLV: &[u8] = include_bytes!("data/h264_aac.flv");
const TOKEN: &str = "0123456789abcdef-publish";

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

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

/// The file published at `path` of `hub` in real time, looped until `stop`.
fn paced(hub: &Hub, path: &str, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    let (tracks, pkts) = decode(FLV);
    let p = hub.publish(path, tracks, PeerInfo::default()).unwrap();
    let span = pkts.iter().map(|p| p.dts).max().unwrap() + SECOND / 25;
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut n = 0i64;
        while !stop.load(Ordering::SeqCst) {
            for pkt in &pkts {
                let at = Duration::from_nanos((pkt.dts + span * n).max(0) as u64);
                if let Some(d) = at.checked_sub(start.elapsed()) {
                    std::thread::sleep(d);
                }
                let mut q = pkt.clone();
                q.dts += span * n;
                q.pts += span * n;
                p.write(q);
            }
            n += 1;
        }
    })
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A GET; a body of a known length read whole, a chunked one's first `peek` bytes.
fn get(addr: SocketAddr, target: &str, fields: &[(&str, &str)], peek: usize) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut req = format!("GET {target} HTTP/1.1\r\nHost: x\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    (&s).write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (n, v) = l.split_once(':').unwrap();
        headers.push((n.to_owned(), v.trim().to_owned()));
    }
    let mut resp = Resp {
        status,
        headers,
        body: Vec::new(),
    };
    if let Some(len) = resp.header("content-length") {
        let mut b = vec![0; len.parse().unwrap()];
        r.read_exact(&mut b).unwrap();
        resp.body = b;
    } else if resp.header("transfer-encoding").is_some() {
        // The first chunk's size line, then its bytes.
        let mut size = String::new();
        r.read_line(&mut size).unwrap();
        let n = usize::from_str_radix(size.trim(), 16).unwrap().min(peek);
        let mut b = vec![0; n];
        r.read_exact(&mut b).unwrap();
        resp.body = b;
    }
    resp
}

struct Akit {
    engine: Arc<Engine>,
    roles: Manager,
    clients: Engine,
    http: SocketAddr,
    _cell: GraphCell,
}

impl Drop for Akit {
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

/// akit serving `json`'s routes and running its roles on one engine, as `akit run`.
fn start(json: &str, roles: bool) -> Akit {
    let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let slot = EngineSlot::new();
    let registry = Registry::new(&[akit_media::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine =
        Arc::new(akit_listener::engine(&cell, &akit_listener::Options::default()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(Arc::clone(&engine)));
    let http = engine.local_addrs()[0];
    let mut m = Manager::new(
        &[
            akit_media::role::MEDIA_AUTH,
            akit_media::streaming::STREAMING,
        ],
        Arc::clone(&engine),
        None,
    );
    if roles {
        let errs = m.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
    }
    let clients = Engine::builder().client_threads(1).build().unwrap();
    clients.start().unwrap();
    Akit {
        engine,
        roles: m,
        clients,
        http,
        _cell: cell,
    }
}

fn config(rtmp: u16) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_auth": [{{"name": "live", "tokens": [{{"token": "{TOKEN}", "publish": ["live/*"], "play": ["live/*"]}}]}}],
          "streaming": {{"rtmp": {{"listen": "127.0.0.1:{rtmp}", "auth": "live"}}}},
          "routes": [
            {{"match": {{"path": ["/hls/*"]}}, "handler": {{"module": "hls", "params": {{"prefix": "/hls/", "auth": "live", "target_duration": "1s", "play_wait": "10s"}}}}}},
            {{"match": {{"path": ["/live/*"]}}, "handler": {{"module": "httpflv", "params": {{"auth": "live", "cors": "*"}}}}}}]}}"#
    )
}

/// HTTP-FLV and HLS of a stream published over RTMP, refused without credentials.
#[test]
fn flv_and_hls() {
    let rtmp_port = free_port();
    let akit = start(&config(rtmp_port), true);
    let src = Hub::default();
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = paced(&src, "src", Arc::clone(&stop));
    let pusher = rtmp::push(
        &akit.clients,
        &format!("rtmp://127.0.0.1:{rtmp_port}/live/cam?token={TOKEN}"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the push streaming", || {
        assert!(!pusher.is_closed(), "{:?}", pusher.error());
        pusher.state() == ClientState::Streaming
    });
    let a = akit.http;
    // HTTP-FLV: 401 without credentials, with the challenge and the handler's CORS.
    let r = get(a, "/live/cam.flv", &[], 0);
    assert_eq!(r.status, 401);
    assert_eq!(r.header("www-authenticate"), Some("Bearer realm=\"akit\""));
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    let r = get(a, "/live/.flv", &[], 0);
    assert_eq!(r.status, 404);
    let r = get(
        a,
        "/live/cam.flv",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
        3,
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("video/x-flv"));
    assert_eq!(r.body, b"FLV");
    let r = get(a, &format!("/live/cam.flv?token={TOKEN}"), &[], 3);
    assert_eq!(r.body, b"FLV");
    // HLS: the playlist with the query's token hands it on; segments refused without it.
    assert_eq!(get(a, "/hls/live/cam/index.m3u8", &[], 0).status, 401);
    let r = get(
        a,
        &format!("/hls/live/cam/index.m3u8?token={TOKEN}"),
        &[],
        0,
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let pl = String::from_utf8(r.body).unwrap();
    assert!(pl.contains(&format!("init-0.mp4?token={TOKEN}")), "{pl}");
    let seg = pl
        .lines()
        .find(|l| l.starts_with("seg-"))
        .unwrap_or_else(|| panic!("{pl}"))
        .to_owned();
    assert!(seg.ends_with(&format!(".m4s?token={TOKEN}")), "{seg}");
    let r = get(a, &format!("/hls/live/cam/{seg}"), &[], 0);
    assert_eq!(r.status, 200);
    assert!(!r.body.is_empty());
    let bare = seg.split('?').next().unwrap();
    assert_eq!(get(a, &format!("/hls/live/cam/{bare}"), &[], 0).status, 401);
    // MPEG-TS with ?format=ts.
    let r = get(
        a,
        &format!("/hls/live/cam/index.m3u8?format=ts&token={TOKEN}"),
        &[],
        0,
    );
    assert_eq!(r.status, 200);
    let pl = String::from_utf8(r.body).unwrap();
    let seg = pl
        .lines()
        .find(|l| l.starts_with("seg-"))
        .unwrap()
        .to_owned();
    assert!(seg.contains(".ts?"), "{seg}");
    let r = get(a, &format!("/hls/live/cam/{seg}"), &[], 0);
    assert_eq!(r.status, 200);
    assert_eq!(r.body.first(), Some(&0x47), "MPEG-TS sync byte");
    stop.store(true, Ordering::SeqCst);
    feeder.join().unwrap();
    pusher.close();
}

/// Without the role, the handlers answer 503.
#[test]
fn no_role() {
    let akit = start(&config(free_port()), false);
    let r = get(akit.http, &format!("/live/cam.flv?token={TOKEN}"), &[], 0);
    assert_eq!(r.status, 503);
    let r = get(
        akit.http,
        &format!("/hls/live/cam/index.m3u8?token={TOKEN}"),
        &[],
        0,
    );
    assert_eq!(r.status, 503);
}
