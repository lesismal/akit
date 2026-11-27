//! `stream_whip` and `stream_whep` (I124) on the streaming role's WebRTC port, through akit's
//! listener beside the role on one engine, as `akit run` runs them: fit's `TestStreamWHIPWHEP`,
//! `TestHandlerPaths` and `TestNotRunning` cases ported. akio-webrtc `Endpoint`s are the WHIP
//! publisher and the WHEP viewer, over loopback.
//!
//! - 401 without credentials, 403 with a token that may not publish, 400 for an offer with no
//!   media, 404 for a DELETE of no session;
//! - a WHEP viewer of a stream published over RTMP gets its video, its session's URL under the
//!   POST's path, and a DELETE of that URL ends the session;
//! - a WHIP publisher's stream plays over RTMP and HLS;
//! - an OPTIONS of the endpoint is answered by the server; paths outside the handler's are 404;
//! - without `streaming.webrtc` the handlers answer 503.
//!
//! Every wait is on what the test can see, with a deadline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use akio::Engine;
use akio_media::av::{SECOND, to_ticks};
use akio_media::codec::h264;
use akio_media::flv::{Decoder, Demuxer, Event};
use akio_media::rtmp::client::{self as rtmp, ClientState, RtmpClientConfig};
use akio_media::webrtc::rtc_config;
use akio_media::{Codec, Hub, Packet, PeerInfo, Track, Tracks};
use akio_webrtc::{
    Codec as RtcCodec, Direction, Endpoint, Frequency, KeyframeRequest, MediaData, MediaKind,
    MediaTime, Peer, PeerHandler,
};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

const FLV: &[u8] = include_bytes!("data/h264_aac.flv");
const PUBLISH: &str = "0123456789abcdef-publish";
const VIEW: &str = "0123456789abcdef-view-only";

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
                if stop.load(Ordering::SeqCst) {
                    return;
                }
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

/// A request with `body`; the response read whole (it has a length, or none).
fn request(
    addr: SocketAddr,
    method: &str,
    target: &str,
    fields: &[(&str, &str)],
    body: &str,
) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
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
    let mut out = Vec::new();
    let _ = r.read_to_end(&mut out);
    Resp {
        status,
        headers,
        body: out,
    }
}

fn post_sdp(addr: SocketAddr, target: &str, token: Option<&str>, sdp: &str) -> Resp {
    let bearer = token.map(|t| format!("Bearer {t}"));
    let mut fields = vec![("Content-Type", "application/sdp")];
    if let Some(b) = &bearer {
        fields.push(("Authorization", b));
    }
    request(addr, "POST", target, &fields, sdp)
}

struct Akit {
    engine: Arc<Engine>,
    roles: Manager,
    clients: Engine,
    http: SocketAddr,
    endpoints: Mutex<Vec<Endpoint>>,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.clients.shutdown();
    }
}

impl Akit {
    /// A client endpoint of its own for each peer, kept alive while its peer is.
    fn endpoint(&self) -> Endpoint {
        let ep = Endpoint::bind(&self.clients, "127.0.0.1:0".parse().unwrap())
            .unwrap()
            .config(rtc_config);
        self.endpoints.lock().unwrap().push(ep.clone());
        ep
    }

    /// The WHIP and WHEP sessions the role holds.
    fn sessions(&self) -> usize {
        let key = akit_media::auth::registry::key(&self.engine);
        akit_media::streaming::lookup(key)
            .with(|c| c.and_then(|c| c.rtc.as_ref().map(|r| r.sessions())))
            .unwrap_or(0)
    }

    /// Whether the role's hub has a publisher of `path`.
    fn publishing(&self, path: &str) -> bool {
        let key = akit_media::auth::registry::key(&self.engine);
        akit_media::streaming::lookup(key).with(|c| {
            c.and_then(|c| c.hub.stream_info(path))
                .is_some_and(|s| s.publishing && !s.tracks.is_empty())
        })
    }
}

/// The port the streaming role's listener of `proto` bound (its `listen` says port 0).
fn bound(m: &Manager, proto: &str) -> u16 {
    m.api("streaming")
        .expect("the streaming role's handle")
        .downcast_ref::<akit_media::streaming::StreamingApi>()
        .unwrap()
        .addr(proto)
        .expect("bound")
        .port()
}

/// akit serving `json`'s routes and running its roles on one engine, as `akit run`.
fn start(json: &str) -> Akit {
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
    let errs = m.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    let clients = Engine::builder()
        .client_threads(1)
        .udp_client_threads(1)
        .build()
        .unwrap();
    clients.start().unwrap();
    Akit {
        engine,
        roles: m,
        clients,
        http,
        endpoints: Mutex::default(),
        _cell: cell,
    }
}

/// The role with RTMP on `rtmp`, WebRTC on a port of loopback (when `webrtc`), and the routes.
/// The routes and the roles, RTMP (and, with `webrtc`, WebRTC) on port 0.
fn config(webrtc: bool) -> String {
    let rtc = if webrtc {
        r#", "webrtc": {"listen": "127.0.0.1:0"}"#
    } else {
        ""
    };
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_auth": [{{"name": "live", "tokens": [
            {{"token": "{PUBLISH}", "publish": ["live/*"], "play": ["live/*"]}},
            {{"token": "{VIEW}", "play": ["live/*"]}}]}}],
          "streaming": {{"rtmp": {{"listen": "127.0.0.1:0", "auth": "live"}}{rtc}}},
          "routes": [
            {{"match": {{"path": ["/whip/*"]}}, "handler": {{"module": "stream_whip", "params": {{"auth": "live"}}}}}},
            {{"match": {{"path": ["/whep/*"]}}, "handler": {{"module": "stream_whep", "params": {{"auth": "live"}}}}}},
            {{"match": {{"path": ["/in", "/in/*", "/elsewhere"]}}, "handler": {{"module": "stream_whip", "params": {{"prefix": "/in"}}}}}},
            {{"match": {{"path": ["/hls/*"]}}, "handler": {{"module": "hls", "params": {{"prefix": "/hls/", "auth": "live", "target_duration": "1s", "play_wait": "10s"}}}}}}]}}"#
    )
}

#[derive(Debug)]
enum Got {
    Connected,
    Keyframe,
    Media,
}

struct Client(Mutex<Sender<Got>>);

impl PeerHandler for Client {
    fn on_connected(&mut self, _p: &Peer) {
        let _ = self.0.lock().unwrap().send(Got::Connected);
    }
    fn on_keyframe_request(&mut self, _p: &Peer, _r: &KeyframeRequest) {
        let _ = self.0.lock().unwrap().send(Got::Keyframe);
    }
    fn on_media_data(&mut self, _p: &Peer, _d: &MediaData) {
        let _ = self.0.lock().unwrap().send(Got::Media);
    }
}

fn next(rx: &Receiver<Got>, what: &str, mut f: impl FnMut(&Got) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let g = rx
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        if f(&g) {
            return;
        }
    }
}

/// An AVCC access unit as Annex B, the parameter sets of `track` before a keyframe.
fn annexb(track: &Track, p: &Packet) -> Vec<u8> {
    let mut out = Vec::new();
    if p.keyframe {
        let c = h264::Config::parse(&track.config).unwrap();
        for s in c.sps.into_iter().chain(c.pps) {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&s);
        }
    }
    h264::avcc_to_annexb(&p.data, &mut out);
    out
}

/// 401, 403, 400 and 404 before any media; a WHEP viewer of an RTMP stream; a WHIP publisher
/// played over RTMP and HLS.
#[test]
fn whip_and_whep() {
    let akit = start(&config(true));
    let rtmp_port = bound(&akit.roles, "rtmp");
    let a = akit.http;
    let src = Hub::default();
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = paced(&src, "src", Arc::clone(&stop));
    let pusher = rtmp::push(
        &akit.clients,
        &format!("rtmp://127.0.0.1:{rtmp_port}/live/cam?token={PUBLISH}"),
        &src,
        "src",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the RTMP push", || {
        assert!(!pusher.is_closed(), "{:?}", pusher.error());
        pusher.state() == ClientState::Streaming && akit.publishing("live/cam")
    });

    // fit's refusals: no credentials, a token that may not publish, an offer with no media,
    // and a DELETE of a session there is not.
    let r = post_sdp(a, "/whip/live/x", None, "v=0\r\n");
    assert_eq!(r.status, 401);
    assert_eq!(r.header("www-authenticate"), Some("Bearer realm=\"akit\""));
    assert_eq!(post_sdp(a, "/whep/live/cam", None, "v=0\r\n").status, 401);
    assert_eq!(
        post_sdp(a, "/whip/live/x", Some(VIEW), "v=0\r\n").status,
        403
    );
    assert_eq!(
        post_sdp(a, "/whip/live/x", Some(PUBLISH), "v=0\r\n").status,
        400
    );
    assert_eq!(post_sdp(a, "/whip/live/x", Some(PUBLISH), "").status, 400);
    let big = format!("v=0\r\n{}", "a=x\r\n".repeat(8000));
    assert_eq!(post_sdp(a, "/whip/live/x", Some(PUBLISH), &big).status, 400);
    let r = request(
        a,
        "DELETE",
        "/whep/live/cam/0123456789abcdef0123456789abcdef",
        &[],
        "",
    );
    assert_eq!(r.status, 404);

    // A WHEP viewer of the RTMP stream: its session under the POST's path, ended by a DELETE.
    let (tx, rx) = channel();
    let (viewer, offer) = akit
        .endpoint()
        .create_offer(Client(Mutex::new(tx)), |api| {
            api.add_media(MediaKind::Video, Direction::RecvOnly, None, None);
            api.add_media(MediaKind::Audio, Direction::RecvOnly, None, None);
        })
        .unwrap();
    let r = post_sdp(a, "/whep/live/cam", Some(VIEW), &offer);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let location = r.header("location").unwrap().to_owned();
    let id = location
        .strip_prefix("/whep/live/cam/")
        .unwrap_or_else(|| panic!("{location}"));
    assert_eq!(id.len(), 32, "{location}");
    assert_eq!(r.header("etag"), Some(format!("\"{id}\"").as_str()));
    viewer
        .accept_answer(&String::from_utf8(r.body).unwrap())
        .unwrap();
    next(&rx, "the viewer connected", |g| matches!(g, Got::Connected));
    let n = AtomicUsize::new(0);
    next(&rx, "the viewer's media", |g| {
        matches!(g, Got::Media) && n.fetch_add(1, Ordering::SeqCst) >= 10
    });
    let before = akit.sessions();
    assert!(before >= 1);
    assert_eq!(request(a, "DELETE", &location, &[], "").status, 200);
    assert_eq!(akit.sessions(), before - 1);
    viewer.close();

    // A WHIP publisher, played over RTMP and HLS.
    let (tracks, pkts) = decode(FLV);
    let vi = tracks.iter().position(Track::is_video).unwrap();
    let vtrack = tracks[vi].clone();
    assert_eq!(vtrack.codec, Codec::H264);
    let (tx, rx) = channel();
    let mut video = None;
    let (publisher, offer) = akit
        .endpoint()
        .create_offer(Client(Mutex::new(tx)), |api| {
            video = Some(api.add_media(MediaKind::Video, Direction::SendOnly, None, None));
        })
        .unwrap();
    let r = post_sdp(a, "/whip/live/webrtc", Some(PUBLISH), &offer);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let location = r.header("location").unwrap().to_owned();
    assert!(location.starts_with("/whip/live/webrtc/"), "{location}");
    publisher
        .accept_answer(&String::from_utf8(r.body).unwrap())
        .unwrap();
    next(&rx, "the publisher connected", |g| {
        matches!(g, Got::Connected)
    });
    next(&rx, "the ingest's keyframe request", |g| {
        matches!(g, Got::Keyframe)
    });
    let video = video.unwrap();
    let pt = publisher.payload_type(video, RtcCodec::H264).unwrap();
    let sending = Arc::new(AtomicBool::new(true));
    let (p2, s2) = (publisher.clone(), Arc::clone(&sending));
    let sender = std::thread::spawn(move || {
        let frames: Vec<&Packet> = pkts.iter().filter(|p| p.track == vi).collect();
        let first = frames.iter().position(|p| p.keyframe).unwrap();
        let span = frames.last().unwrap().dts - frames[first].dts + SECOND / 25;
        let start = Instant::now();
        let mut round = 0i64;
        while s2.load(Ordering::SeqCst) {
            for p in &frames[first..] {
                if !s2.load(Ordering::SeqCst) {
                    return;
                }
                let t = p.dts - frames[first].dts + round * span;
                let _ = p2.write_media(
                    video,
                    pt,
                    Instant::now(),
                    MediaTime::new(to_ticks(t, 90_000) as u64, Frequency::NINETY_KHZ),
                    &annexb(&vtrack, p),
                );
                let due = start + Duration::from_nanos(t as u64);
                if let Some(d) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(d);
                }
            }
            round += 1;
        }
    });
    wait_for("the WHIP stream", || akit.publishing("live/webrtc"));
    // Over RTMP, pulled into a hub of the test's.
    let sink = Hub::default();
    let puller = rtmp::pull(
        &akit.clients,
        &format!("rtmp://127.0.0.1:{rtmp_port}/live/webrtc?token={VIEW}"),
        &sink,
        "got",
        RtmpClientConfig::default(),
    )
    .unwrap();
    wait_for("the RTMP player of the WHIP stream", || {
        assert!(!puller.is_closed(), "{:?}", puller.error());
        sink.stream_info("got")
            .is_some_and(|s| s.tracks.iter().any(|t| t.codec == Codec::H264))
    });
    // Over HLS: a segment of the stream.
    wait_for("an HLS segment of the WHIP stream", || {
        let r = request(
            a,
            "GET",
            &format!("/hls/live/webrtc/index.m3u8?token={VIEW}"),
            &[],
            "",
        );
        r.status == 200 && String::from_utf8_lossy(&r.body).contains("seg-")
    });
    // A DELETE of the publisher's session ends the stream.
    assert_eq!(request(a, "DELETE", &location, &[], "").status, 200);
    wait_for("the WHIP stream to end", || !akit.publishing("live/webrtc"));
    sending.store(false, Ordering::SeqCst);
    sender.join().unwrap();
    puller.close();
    publisher.close();
    stop.store(true, Ordering::SeqCst);
    feeder.join().unwrap();
    pusher.close();
}

/// fit's `TestHandlerPaths` for WHIP: outside the prefix, the prefix alone, an OPTIONS of the
/// endpoint answered by the server.
#[test]
fn paths() {
    let akit = start(&config(true));
    let a = akit.http;
    for p in ["/elsewhere", "/in"] {
        assert_eq!(request(a, "GET", p, &[], "").status, 404, "{p}");
    }
    let r = request(a, "OPTIONS", "/in/live/cam", &[], "");
    assert!(r.status < 300, "{}", r.status);
    assert!(r.header("allow").unwrap_or("").contains("POST"));
    // A GET of the endpoint is not a method it takes.
    assert_eq!(request(a, "GET", "/in/live/cam", &[], "").status, 405);
}

/// Without `streaming.webrtc` the handlers answer 503 (fit's `TestNotRunning`).
#[test]
fn without_webrtc() {
    let akit = start(&config(false));
    let r = post_sdp(akit.http, "/whip/live/x", Some(PUBLISH), "v=0\r\n");
    assert_eq!(r.status, 503);
}
