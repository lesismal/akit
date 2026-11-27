//! What the media and proxy tests share: akit serving a config with its WebRTC roles, an HTTP
//! client, and akio-webrtc `Endpoint`s publishing and playing.
#![allow(dead_code, unreachable_pub)]

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

pub(crate) const T: Duration = Duration::from_secs(15);

/// akit serving `doc`: its listener, routes and roles on one engine.
pub(crate) struct Akit {
    pub(crate) engine: Arc<akio::Engine>,
    pub(crate) roles: Manager,
    pub(crate) addr: SocketAddr,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

pub(crate) fn options() -> akit_listener::Options {
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

pub(crate) fn config(media: &str) -> String {
    format!(
        r#"{{"engine": {{"event_loops": 2}},
            "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
            "routes": [{{"match": {{"path": ["/whip/*"]}}, "handler": {{"module": "whip"}}}},
                       {{"match": {{"path": ["/whep/*"]}}, "handler": {{"module": "whep"}}}}],
            "webrtc": {{"media": {media}}}}}"#
    )
}

impl Akit {
    /// akit serving `doc`, the roles `infos` with it.
    pub(crate) fn serve(doc: &str, infos: &[akit_roles::host::Info]) -> Akit {
        let config = akit_config::from_json(doc, None).unwrap_or_else(|e| panic!("{doc}: {e}"));
        let registry: Registry<HandlerParams> = Registry::new(&[akit_webrtc::MODULES]).unwrap();
        let slot = EngineSlot::new();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = *engine.local_addrs().first().unwrap();
        let mut roles = Manager::new(infos, Arc::clone(&engine), None);
        let errs = roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        Akit {
            engine,
            roles,
            addr,
            _cell: cell,
        }
    }

    pub(crate) fn start(media: &str) -> Akit {
        Akit::serve(&config(media), &[akit_webrtc::MEDIA])
    }

    pub(crate) fn apply(&mut self, media: &str) {
        let config = akit_config::from_json(&config(media), None).unwrap();
        let errs = self.roles.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
    }

    pub(crate) fn metrics(&self) -> String {
        let mut out = Vec::new();
        akit_roles::host::write_meters(&self.roles.meters(), &mut out);
        String::from_utf8(out).unwrap()
    }
}

/// An HTTP/1.1 response: its status, header fields and body.
pub(crate) struct Resp {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl Resp {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

pub(crate) fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    ctype: Option<&str>,
    body: &str,
) -> Resp {
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

pub(crate) enum Got {
    Connected,
    Media(MediaKind, Vec<u8>),
    Closed,
}

pub(crate) struct Probe(pub(crate) Mutex<Sender<Got>>);

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
pub(crate) struct Client {
    pub(crate) engine: Arc<akio::Engine>,
    pub(crate) ep: Endpoint,
}

impl Client {
    pub(crate) fn new() -> Client {
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
pub(crate) struct Session {
    pub(crate) peer: Peer,
    pub(crate) rx: Receiver<Got>,
    pub(crate) audio: Mid,
    pub(crate) video: Mid,
    pub(crate) location: String,
    pub(crate) links: Vec<String>,
}

pub(crate) fn session(
    akit: &Akit,
    c: &Client,
    path: &str,
    dir: Direction,
) -> Result<Session, Resp> {
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

pub(crate) fn wait_for(rx: &Receiver<Got>, what: &str, f: impl Fn(&Got) -> bool) {
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
pub(crate) fn flow(p: &Session, viewer: &Session) {
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
