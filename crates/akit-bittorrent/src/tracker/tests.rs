//! fit's `bittorrent/tracker` tests, ported (I151): announces and scrapes over HTTP (through the
//! `bt_tracker` handler on the role's engine) and UDP (on its `listen`), with akio-bittorrent's
//! tracker client and with bytes written by hand; the allow-list and its files; access; the
//! bounds; reloads; clients that misbehave; the statistics and the console's routes; two
//! akio-bittorrent clients finding each other through the tracker.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use akio_bittorrent::bencode::{self, Value};
use akio_bittorrent::metainfo::InfoHash;
use akio_bittorrent::tracker::{AnnounceRequest, Event, TrackerClient, TrackerError, udp};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;

use super::service as tests_handler;
use super::{TRACKER, key_of, stats};

/// The tracker's UDP on port 0: where it listens is asked (`Env::udp`).
const ANY: &str = "127.0.0.1:0";

/// The role on an engine of the test's own, with `bt_tracker` served on a listener of it.
struct Env {
    engine: Arc<akio::Engine>,
    roles: Manager,
    http: SocketAddr,
    client: TrackerClient,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

impl Env {
    fn start(tracker: &str) -> Env {
        let engine = akio::Engine::builder()
            .client_threads(1)
            .udp_client_threads(1)
            .build()
            .unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        // The handler's listener on port 0: its address as it bound it.
        let svc = tests_handler(EngineSlot::with(Arc::clone(&engine)));
        let id = engine
            .add_listener(
                akio::Listener::tcp(SocketAddr::from(([127, 0, 0, 1], 0)))
                    .serve(akio_http::serve(svc)),
            )
            .unwrap();
        let http = engine
            .listeners()
            .into_iter()
            .find(|l| l.id == id)
            .unwrap()
            .local_addr;
        let mut env = Env {
            roles: Manager::new(&[TRACKER], Arc::clone(&engine), None),
            client: TrackerClient::new(Arc::clone(&engine), Duration::from_millis(300), 2),
            engine,
            http,
        };
        env.apply(tracker).unwrap();
        env
    }

    fn apply(&mut self, tracker: &str) -> Result<(), String> {
        let doc = format!(r#"{{"bittorrent": {{"tracker": {tracker}}}}}"#);
        let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
        let errs = self.roles.apply(&config);
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn udp(&self) -> SocketAddr {
        let a = super::udp_addr(&self.engine).expect("the tracker answers UDP");
        SocketAddr::new([127, 0, 0, 1].into(), a.port())
    }

    fn announce(
        &self,
        url: &str,
        req: &AnnounceRequest,
    ) -> Result<akio_bittorrent::tracker::AnnounceResponse, TrackerError> {
        let (tx, rx) = mpsc::channel();
        self.client.announce(url, req, move |r| {
            let _ = tx.send(r);
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn scrape(
        &self,
        url: &str,
        hashes: &[InfoHash],
    ) -> Result<
        std::collections::HashMap<InfoHash, akio_bittorrent::tracker::ScrapeStats>,
        TrackerError,
    > {
        let (tx, rx) = mpsc::channel();
        self.client.scrape(url, hashes, move |r| {
            let _ = tx.send(r);
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn http_url(&self) -> String {
        format!("http://{}/tracker/announce", self.http)
    }

    fn udp_url(&self) -> String {
        format!("udp://{}/announce", self.udp())
    }

    /// A raw HTTP GET of `target`, the answer's status and body.
    fn get(&self, target: &str) -> (u16, Vec<u8>) {
        get(self.http, target, false)
    }
}

/// A GET of `target` at `addr`, its bytes written one at a time when `slow`.
fn get(addr: SocketAddr, target: &str, slow: bool) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let req = format!("GET {target} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    if slow {
        for b in req.as_bytes() {
            s.write_all(&[*b]).unwrap();
            s.flush().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
    } else {
        s.write_all(req.as_bytes()).unwrap();
    }
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8_lossy(&out);
    let status = text[9..12].parse().unwrap();
    let body_at = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    (status, out[body_at..].to_vec())
}

fn pct(b: &[u8]) -> String {
    b.iter().map(|x| format!("%{x:02X}")).collect()
}

fn announce_target(ih: [u8; 20], peer: u8, port: u16, left: u64, extra: &str) -> String {
    format!(
        "/tracker/announce?info_hash={}&peer_id={}&port={port}&left={left}{extra}",
        pct(&ih),
        pct(&[peer; 20])
    )
}

fn failure_of(body: &[u8]) -> Option<String> {
    bencode::decode(body)
        .ok()?
        .str_at("failure reason")
        .map(str::to_owned)
}

fn req(ih: [u8; 20], peer: u8, port: u16, left: u64) -> AnnounceRequest {
    AnnounceRequest::new(InfoHash(ih), [peer; 20], port, left)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Announces and scrapes over HTTP and UDP, as fit's `TestAnnounceAndScrape`: a seeder over
/// HTTP, a leecher over UDP that gets it, the counts, the statistics.
#[test]
fn announce_and_scrape_over_http_and_udp() {
    let env = Env::start(&format!(r#"{{"listen": "{ANY}"}}"#));
    let ih = [5u8; 20];
    let r = env.announce(&env.http_url(), &req(ih, 1, 6000, 0)).unwrap();
    assert_eq!(r.interval, 1800);
    assert!(r.peers.is_empty());
    let r = env.announce(&env.udp_url(), &req(ih, 2, 6001, 10)).unwrap();
    assert_eq!(r.peers, vec![SocketAddr::from(([127, 0, 0, 1], 6000))]);
    assert_eq!((r.seeders, r.leechers), (Some(1), Some(1)));
    for url in [env.http_url(), env.udp_url()] {
        let s = env.scrape(&url, &[InfoHash(ih)]).unwrap();
        let st = &s[&InfoHash(ih)];
        assert_eq!((st.complete, st.incomplete), (1, 1), "{url}");
    }
    // A completed download counts.
    let mut done = req(ih, 2, 6001, 0);
    done.event = Event::Completed;
    env.announce(&env.http_url(), &done).unwrap();
    let s = env.scrape(&env.http_url(), &[InfoHash(ih)]).unwrap();
    assert_eq!(s[&InfoHash(ih)].downloaded, 1);
    let st = stats(&env.engine).unwrap();
    assert_eq!((st.torrents, st.announces, st.scrapes), (1, 3, 3));
    assert_eq!(st.max_torrents, 100_000);
    assert!(st.udp.is_some());
    // The answer as written: interval, counts, compact peers, the client's address.
    let (code, body) = env.get(&announce_target([6; 20], 3, 7000, 5, ""));
    assert_eq!(code, 200);
    let v = bencode::decode(&body).unwrap();
    assert_eq!(v.int("interval"), Some(1800));
    assert_eq!(v.int("incomplete"), Some(1));
    assert_eq!(v.bytes_at("peers"), Some(&[][..]));
    assert_eq!(v.bytes_at("external ip"), Some(&[127, 0, 0, 1][..]));
    assert!(v.get("min interval").is_none());
    // compact=0: a list of dictionaries.
    let (_, body) = env.get(&announce_target([6; 20], 4, 7001, 5, "&compact=0"));
    let v = bencode::decode(&body).unwrap();
    let peers = v.get("peers").and_then(Value::as_list).unwrap();
    assert_eq!(peers[0].str_at("ip"), Some("127.0.0.1"));
    assert_eq!(peers[0].int("port"), Some(7000));
    // Another path: 404; a stopped peer leaves.
    assert_eq!(env.get("/tracker/other").0, 404);
    let (_, _) = env.get(&announce_target([6; 20], 3, 7000, 5, "&event=stopped"));
    let (_, body) = env.get(&format!("/tracker/scrape?info_hash={}", pct(&[6; 20])));
    let v = bencode::decode(&body).unwrap();
    let f = v.get("files").and_then(|f| f.as_dict()).unwrap();
    assert_eq!(f[&vec![6u8; 20]].int("incomplete"), Some(1));
}

/// fib's query rules: the failure reasons of a bad announce, numwant and its bounds, a scrape
/// naming too many or none.
#[test]
fn bad_announces_and_bounds() {
    let env = Env::start(r#"{"max_numwant": 3, "max_scrape": 2, "min_interval": "1m"}"#);
    for (target, want) in [
        (
            "/announce?info_hash=%01&peer_id=x&port=1".to_owned(),
            "invalid info_hash",
        ),
        (
            format!("/announce?info_hash={}&peer_id=x&port=1", pct(&[1; 20])),
            "invalid peer_id",
        ),
        (
            format!(
                "/announce?info_hash={}&peer_id={}&port=x",
                pct(&[1; 20]),
                pct(&[1; 20])
            ),
            "invalid port",
        ),
        (
            announce_target([1; 20], 1, 1, 1, "&event=nope"),
            "invalid event",
        ),
        (
            announce_target([1; 20], 1, 1, 1, "&numwant=x"),
            "invalid number",
        ),
        ("/announce?info_hash=%zz".to_owned(), "invalid query string"),
    ] {
        let (code, body) = env.get(&target);
        assert_eq!(code, 200, "{target}");
        assert_eq!(failure_of(&body).as_deref(), Some(want), "{target}");
    }
    // Six seeders; a leecher asks for 10 and gets max_numwant, 3; asking for none, 50 capped.
    let ih = [2u8; 20];
    for p in 0..6u8 {
        env.get(&announce_target(ih, p + 10, 7000 + u16::from(p), 0, ""));
    }
    let (_, body) = env.get(&announce_target(ih, 1, 6000, 5, "&numwant=10"));
    let v = bencode::decode(&body).unwrap();
    assert_eq!(v.bytes_at("peers").unwrap().len(), 3 * 6);
    assert_eq!(v.int("min interval"), Some(60));
    let (_, body) = env.get(&announce_target(ih, 1, 6000, 5, "&numwant=0"));
    let v = bencode::decode(&body).unwrap();
    assert_eq!(v.bytes_at("peers").unwrap().len(), 0);
    // A scrape names at most max_scrape torrents; a full scrape gets nothing.
    let q: String = (0..4u8)
        .map(|i| format!("info_hash={}&", pct(&[if i == 0 { 2 } else { 90 + i }; 20])))
        .collect();
    let (_, body) = env.get(&format!("/scrape?{q}"));
    let v = bencode::decode(&body).unwrap();
    assert_eq!(v.get("files").and_then(|f| f.as_dict()).unwrap().len(), 1);
    let (_, body) = env.get("/scrape");
    let v = bencode::decode(&body).unwrap();
    assert!(v.get("files").and_then(|f| f.as_dict()).unwrap().is_empty());
}

/// The store's bounds: one torrent past max_torrents is refused "tracker is full"; a peer past
/// max_peers_per_torrent gets peers but is not handed out.
#[test]
fn store_bounds() {
    let env = Env::start(r#"{"max_torrents": 2, "max_peers_per_torrent": 2}"#);
    for i in 0..2u8 {
        env.announce(&env.http_url(), &req([i + 1; 20], 1, 6000, 1))
            .unwrap();
    }
    let e = env
        .announce(&env.http_url(), &req([9; 20], 1, 6000, 1))
        .unwrap_err();
    assert_eq!(e, TrackerError::Failure("tracker is full".into()));
    for p in 2..4u8 {
        env.announce(&env.http_url(), &req([1; 20], p, 6000 + u16::from(p), 1))
            .unwrap();
    }
    let s = env.scrape(&env.http_url(), &[InfoHash([1; 20])]).unwrap();
    assert_eq!(s[&InfoHash([1; 20])].incomplete, 2);
}

/// The allow-list, inline and in a file: other torrents refused "unregistered torrent" and their
/// scrapes empty; the file changed and applied again; a bad line or a missing file refusing the
/// reload with the list before kept.
#[test]
fn allow_list_and_its_files() {
    let dir = std::env::temp_dir().join(format!("akit-bt-allow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("allow.txt");
    let (a, b, c) = ([0xaa; 20], [0xbb; 20], [0xcc; 20]);
    std::fs::write(&file, format!("# allowed\n{}  # b\n\n", hex(&b))).unwrap();
    let cfg = |extra: &str| {
        format!(
            r#"{{"listen": "{ANY}", "allow": ["{}"], "allow_files": [{:?}]{extra}}}"#,
            hex(&a),
            file.to_str().unwrap()
        )
    };
    let mut env = Env::start(&cfg(""));
    for h in [a, b] {
        env.announce(&env.http_url(), &req(h, 1, 6000, 1)).unwrap();
        env.announce(&env.udp_url(), &req(h, 2, 6001, 1)).unwrap();
    }
    for url in [env.http_url(), env.udp_url()] {
        let e = env.announce(&url, &req(c, 1, 6000, 1)).unwrap_err();
        assert_eq!(
            e,
            TrackerError::Failure("unregistered torrent".into()),
            "{url}"
        );
    }
    let s = env
        .scrape(&env.http_url(), &[InfoHash(a), InfoHash(c)])
        .unwrap();
    assert!(s.contains_key(&InfoHash(a)) && !s.contains_key(&InfoHash(c)));
    let st = stats(&env.engine).unwrap();
    assert_eq!(st.allowed, 2);
    assert_eq!(st.refused[0], ("unregistered", 3));
    // The file is watched: what the role read is what -watch-certs compares.
    let files = env.roles.files();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, file.to_str().unwrap());
    // The file changed: c allowed, b no longer.
    std::fs::write(&file, hex(&c)).unwrap();
    env.apply(&cfg("")).unwrap();
    env.announce(&env.http_url(), &req(c, 1, 6000, 1)).unwrap();
    assert!(env.announce(&env.http_url(), &req(b, 1, 6000, 1)).is_err());
    // A bad line refuses the reload, naming the file and line; the list before stays.
    std::fs::write(&file, format!("{}\nnope\n", hex(&b))).unwrap();
    let e = env.apply(&cfg("")).unwrap_err();
    assert!(
        e.contains("allow.txt:2") && e.contains("not an info hash"),
        "{e}"
    );
    env.announce(&env.http_url(), &req(c, 3, 6000, 1)).unwrap();
    std::fs::remove_file(&file).unwrap();
    assert!(env.apply(&cfg("")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The role's access: UDP refused at akio's filter (no answer), HTTP with the failure "refused",
/// counted; lifted by a reload.
#[test]
fn access_refuses_and_a_reload_lifts_it() {
    let deny =
        format!(r#"{{"listen": "{ANY}", "access": {{"deny": ["127.0.0.0/8", "::1/128"]}}}}"#);
    let mut env = Env::start(&deny);
    let (_, body) = env.get(&announce_target([1; 20], 1, 6000, 1, ""));
    assert_eq!(failure_of(&body).as_deref(), Some("refused"));
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    sock.send_to(&udp::connect_request(7), env.udp()).unwrap();
    let mut buf = [0u8; 64];
    assert!(
        sock.recv_from(&mut buf).is_err(),
        "a refused client got an answer"
    );
    assert_eq!(stats(&env.engine).unwrap().refused[2], ("access", 1));
    env.apply(&format!(r#"{{"listen": "{ANY}"}}"#)).unwrap();
    env.announce(&env.http_url(), &req([1; 20], 1, 6000, 1))
        .unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    sock.send_to(&udp::connect_request(7), env.udp()).unwrap();
    let (n, _) = sock.recv_from(&mut buf).unwrap();
    assert_eq!(n, 16);
}

/// UDP as fib serves it: a connection ID that is not the client's gets no answer, garbage gets
/// none, a connect and an announce written by hand are answered, an answer stays within 1,400
/// bytes, a scrape names at most 74 torrents.
#[test]
fn udp_by_hand() {
    let env = Env::start(&format!(r#"{{"listen": "{ANY}", "max_numwant": 1000}}"#));
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut buf = [0u8; 2048];
    for junk in [&b""[..], &b"hello"[..], &[0u8; 15][..], &[0xffu8; 200][..]] {
        sock.send_to(junk, env.udp()).unwrap();
        assert!(sock.recv_from(&mut buf).is_err(), "junk {junk:?} answered");
    }
    // An announce with a made-up connection ID: dropped.
    sock.send_to(
        &udp::announce_request(42, 1, &req([1; 20], 1, 6000, 1)),
        env.udp(),
    )
    .unwrap();
    assert!(sock.recv_from(&mut buf).is_err());
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    sock.send_to(&udp::connect_request(9), env.udp()).unwrap();
    let (n, _) = sock.recv_from(&mut buf).unwrap();
    let conn = match udp::parse_reply(&buf[..n], false).unwrap() {
        udp::Reply::Connect { conn_id, tid } => {
            assert_eq!(tid, 9);
            conn_id
        }
        r => panic!("{r:?}"),
    };
    // 300 seeders over HTTP; a UDP leecher asking for all gets at most (1400-20)/6.
    let ih = [3u8; 20];
    for p in 0..300u16 {
        let mut pid = [0u8; 20];
        pid[..2].copy_from_slice(&p.to_be_bytes());
        let mut r = AnnounceRequest::new(InfoHash(ih), pid, 10000 + p, 0);
        r.num_want = 0;
        let (tx, rx) = mpsc::channel();
        env.client.announce(&env.http_url(), &r, move |x| {
            let _ = tx.send(x);
        });
        rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    }
    let mut r = req(ih, 1, 6000, 5);
    r.num_want = -1;
    sock.send_to(&udp::announce_request(conn, 10, &r), env.udp())
        .unwrap();
    let (n, _) = sock.recv_from(&mut buf).unwrap();
    assert_eq!(n, 20 + 50 * 6, "the default numwant");
    r.num_want = 1000;
    sock.send_to(&udp::announce_request(conn, 11, &r), env.udp())
        .unwrap();
    let (n, _) = sock.recv_from(&mut buf).unwrap();
    assert!(n <= 1400 && n == 20 + 230 * 6, "{n}");
    // A scrape of 100 hashes: 74 answered.
    let hashes: Vec<InfoHash> = (0..100u8).map(|i| InfoHash([i; 20])).collect();
    sock.send_to(&udp::scrape_request(conn, 12, &hashes), env.udp())
        .unwrap();
    let (n, _) = sock.recv_from(&mut buf).unwrap();
    assert_eq!(n, 8 + 74 * 12);
}

/// Reloads, as fit's `TestReload`: the interval changing in place keeps the swarms; new bounds
/// make a new store (until akio's A285); `listen` moved and removed; a taken port leaves all as
/// it was.
#[test]
fn reloads() {
    let mut env = Env::start(&format!(r#"{{"listen": "{ANY}"}}"#));
    let p1 = env.udp().port();
    let ih = [7u8; 20];
    env.announce(&env.http_url(), &req(ih, 1, 6000, 0)).unwrap();
    env.apply(&format!(r#"{{"listen": "{ANY}", "interval": "10m"}}"#))
        .unwrap();
    assert_eq!(env.udp().port(), p1, "the same listen keeps the binding");
    let r = env.announce(&env.udp_url(), &req(ih, 2, 6001, 5)).unwrap();
    assert_eq!((r.interval, r.peers.len()), (600, 1), "the swarm was kept");
    // A taken port: the reload fails, UDP stays where it was.
    let taken = UdpSocket::bind("127.0.0.1:0").unwrap();
    let tp = taken.local_addr().unwrap().port();
    let e = env
        .apply(&format!(
            r#"{{"listen": "127.0.0.1:{tp}", "interval": "10m"}}"#
        ))
        .unwrap_err();
    assert!(e.contains("listen on"), "{e}");
    assert_eq!(env.udp().port(), p1);
    env.announce(&env.udp_url(), &req(ih, 2, 6001, 5)).unwrap();
    // Moved: another `listen` (localhost's, on port 0), bound anew elsewhere.
    env.apply(r#"{"listen": "localhost:0", "interval": "10m"}"#)
        .unwrap();
    let p2 = env.udp().port();
    assert_ne!(p2, p1);
    env.announce(&env.udp_url(), &req(ih, 2, 6001, 5)).unwrap();
    // New bounds: a new store.
    env.apply(r#"{"listen": "localhost:0", "interval": "10m", "max_torrents": 5}"#)
        .unwrap();
    let s = env.scrape(&env.udp_url(), &[InfoHash(ih)]).unwrap();
    assert_eq!(s[&InfoHash(ih)].complete, 0, "new bounds start a new store");
    // No UDP.
    env.apply(r#"{"max_torrents": 5}"#).unwrap();
    assert!(super::udp_addr(&env.engine).is_none());
    env.announce(&env.http_url(), &req(ih, 2, 6001, 5)).unwrap();
}

/// A client sending its announce a byte at a time, and one cut midway, as fit's
/// `TestSlowAndCutClients`; the role stopped: 503.
#[test]
fn slow_and_cut_clients_and_a_stopped_role() {
    let mut env = Env::start("{}");
    let (code, body) = get(env.http, &announce_target([1; 20], 1, 6000, 1, ""), true);
    assert_eq!(code, 200);
    assert!(
        failure_of(&body).is_none(),
        "{}",
        String::from_utf8_lossy(&body)
    );
    for _ in 0..5 {
        let mut s = TcpStream::connect(env.http).unwrap();
        s.write_all(b"GET /tracker/announce?info_hash=").unwrap();
        drop(s);
    }
    env.announce(&env.http_url(), &req([1; 20], 2, 6001, 1))
        .unwrap();
    env.roles.stop(Duration::ZERO);
    let (code, body) = env.get("/announce");
    assert_eq!(code, 503);
    assert_eq!(body, b"the bittorrent.tracker role is not running\n");
    assert!(stats(&env.engine).is_none());
}

/// The console's routes, as fit's `TestConsoleRoutes`.
#[test]
fn console_routes() {
    let h = [0x11u8; 20];
    let env = Env::start(&format!(r#"{{"allow": ["{}"]}}"#, hex(&h)));
    env.announce(&env.http_url(), &req(h, 1, 6000, 0)).unwrap();
    let pages = super::pages::Pages::new(key_of(&env.engine));
    let call = |path: &str| {
        let r = super::tests_serve(&pages, path);
        (r.status, String::from_utf8(r.body).unwrap())
    };
    let (code, body) = call("/api/stats");
    assert_eq!(code, 200);
    assert!(body.contains(r#""torrents":1,"announces":1"#), "{body}");
    assert!(body.contains(r#""allowed":1"#), "{body}");
    let (code, body) = call("/api/torrents?limit=5");
    assert_eq!(code, 200);
    assert!(
        body.contains(&format!(r#""info_hash":"{}","seeders":1"#, hex(&h))),
        "{body}"
    );
    assert!(
        body.contains(r#""total":1,"source":"allow_list""#),
        "{body}"
    );
    let (code, body) = call(&format!("/api/torrents/{}", hex(&h)));
    assert_eq!(code, 200);
    assert!(body.contains(r#""tracked":true"#), "{body}");
    assert_eq!(call("/api/torrents/xyz").0, 400);
    assert_eq!(call("/api/passkey").0, 404);
    assert_eq!(call("/api/nope").0, 404);
}

fn data(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// Two akio-bittorrent clients that find each other only through the tracker, the seeder over
/// HTTP and the leecher over UDP, as fit's `TestDownloadThroughTracker`.
#[test]
fn two_clients_find_each_other_through_the_tracker() {
    use akio_bittorrent::client::{AddOptions, Client, ClientConfig, MemoryStorage};
    use akio_bittorrent::metainfo::{Builder, MetaInfo};
    let env = Env::start(&format!(r#"{{"listen": "{ANY}"}}"#));
    let payload = data(1 << 20, 3);
    let mi = Builder::new("t.bin")
        .piece_length(64 * 1024)
        .tracker(env.http_url())
        .build_bytes(&payload)
        .unwrap();
    let mi = MetaInfo::parse(&mi.to_bytes()).unwrap();
    let cfg = || {
        let mut c = ClientConfig::default();
        c.listen = "127.0.0.1:0".parse().unwrap();
        c.dht = false;
        c.lsd = false;
        c.pex = false;
        c.threads = 1;
        c.tracker_timeout = Duration::from_millis(500);
        c
    };
    let seeder = Client::new(cfg()).unwrap();
    let mut o = AddOptions::default();
    o.storage = Some(Arc::new(MemoryStorage::with_data(payload.clone())));
    let st = seeder.add_torrent(mi.clone(), o).unwrap();
    assert!(st.wait_complete(Duration::from_secs(10)));
    let leecher = Client::new(cfg()).unwrap();
    let store = Arc::new(MemoryStorage::new(mi.info.total_length));
    let mut o = AddOptions::default();
    o.storage = Some(store.clone());
    // The leecher's tracker is the same tracker over UDP.
    let mut mi2 = mi.clone();
    mi2.announce = Some(env.udp_url());
    mi2.announce_list = Vec::new();
    let t = leecher.add_torrent(mi2, o).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    assert!(
        t.wait_complete(deadline - Instant::now()),
        "{:?}",
        t.stats()
    );
    assert!(store.snapshot() == payload);
    leecher.close();
    seeder.close();
}

/// fit's private tracker tests (I159): with `auth`, an announce or scrape must carry a passkey of
/// the `media_auth` policy that plays the torrent, in its path or as `?passkey=` over HTTP and in
/// BEP 41's URL data over UDP; a user's passkey, as `/api/passkey` gives it, or a token; the
/// refusals counted; the passkey redacted from the logged target; a UDP scrape, which carries no
/// URL data, refused.
#[test]
fn a_private_tracker() {
    let engine = akio::Engine::builder()
        .client_threads(1)
        .udp_client_threads(1)
        .build()
        .unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    // The handler's listener on port 0: its address as it bound it.
    let slot = EngineSlot::with(Arc::clone(&engine));
    let svc = tests_handler(slot.clone());
    let id = engine
        .add_listener(
            akio::Listener::tcp(SocketAddr::from(([127, 0, 0, 1], 0))).serve(akio_http::serve(svc)),
        )
        .unwrap();
    let http = engine
        .listeners()
        .into_iter()
        .find(|l| l.id == id)
        .unwrap()
        .local_addr;
    let env = Env {
        roles: Manager::new(
            &[akit_media::role::MEDIA_AUTH, TRACKER],
            Arc::clone(&engine),
            None,
        ),
        client: TrackerClient::new(Arc::clone(&engine), Duration::from_millis(300), 2),
        engine,
        http,
    };
    let mut env = env;
    let ih = [7u8; 20];
    let doc = format!(
        r#"{{"media_auth": [{{"name": "bt", "secret": "0123456789abcdef0123456789abcdef",
              "users": [{{"name": "alice", "password": "alice-password", "play": ["bittorrent/*"]}},
                        {{"name": "bob", "password": "bob-password", "play": ["bittorrent/{}"]}}],
              "tokens": [{{"name": "t", "token": "token-0123456789ab", "play": ["bittorrent/*"]}}]}}],
            "bittorrent": {{"tracker": {{"listen": "{ANY}", "auth": "bt"}}}}}}"#,
        hex(&[8u8; 20])
    );
    let config = akit_config::from_json(&doc, None).unwrap();
    assert!(env.roles.apply(&config).is_empty());

    // The user's passkey, fit's, from the console's route.
    let pages = super::pages::Pages::new(key_of(&env.engine));
    let r = super::tests_serve(&pages, "/api/passkey?user=alice");
    assert_eq!(r.status, 200);
    let body = String::from_utf8(r.body).unwrap();
    assert_eq!(body, "{\"passkey\":\"c3751c196e55193e0c3be763bcc1b8ab\"}\n");
    assert_eq!(
        super::tests_serve(&pages, "/api/passkey?user=carol").status,
        404
    );
    assert_eq!(super::tests_serve(&pages, "/api/passkey").status, 404);
    let r = pages.passkey("/x?user=alice", true);
    assert_eq!(
        String::from_utf8(r.body).unwrap(),
        "{\"user\":\"alice\",\"passkey\":\"c3751c196e55193e0c3be763bcc1b8ab\"}\n"
    );
    assert_eq!(pages.passkey("/x", true).status, 400);
    let alice = "c3751c196e55193e0c3be763bcc1b8ab";

    // Over HTTP: none, unknown, in the path, in the query, a token; bob's grant lacks it.
    let target = |prefix: &str, extra: &str| {
        announce_target(ih, 1, 6000, 0, extra).replacen("/tracker", prefix, 1)
    };
    for (t, want) in [
        // fib's: the segment before `announce` is the passkey, `tracker` here.
        (target("", ""), Some("a passkey is required")),
        (target("/tracker", ""), Some("unknown passkey")),
        (target("/tracker/nope", ""), Some("unknown passkey")),
        (target(&format!("/tracker/{alice}"), ""), None),
        (target("", &format!("&passkey={alice}")), None),
        (target("/tracker/token-0123456789ab", ""), None),
    ] {
        let (s, body) = env.get(&t);
        assert_eq!(s, 200);
        assert_eq!(failure_of(&body).as_deref(), want, "{t}");
    }
    let bob = super::tests_serve(&pages, "/api/passkey?user=bob");
    let bob = String::from_utf8(bob.body).unwrap();
    let bob = bob.split('"').nth(3).unwrap().to_owned();
    let (_, body) = env.get(&target(&format!("/tracker/{bob}"), ""));
    assert_eq!(
        failure_of(&body).as_deref(),
        Some("the passkey may not use this torrent")
    );
    // A scrape with the passkey, and without.
    let (_, body) = env.get(&format!("/tracker/{alice}/scrape?info_hash={}", pct(&ih)));
    assert_eq!(
        failure_of(&body),
        None,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (_, body) = env.get(&format!("/scrape?info_hash={}", pct(&ih)));
    assert_eq!(failure_of(&body).as_deref(), Some("a passkey is required"));

    // Over UDP: the passkey in the URL data after the announce (BEP 41).
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 2048];
    sock.send_to(&udp::connect_request(1), env.udp()).unwrap();
    let (n, _) = sock.recv_from(&mut buf).unwrap();
    let udp::Reply::Connect { conn_id, .. } = udp::parse_reply(&buf[..n], false).unwrap() else {
        panic!("not a connect");
    };
    let mut announce = |data: &str| {
        let mut b = udp::announce_request(conn_id, 2, &req(ih, 2, 6001, 10));
        let path = data.as_bytes();
        for chunk in path.chunks(255) {
            b.push(2);
            b.push(chunk.len() as u8);
            b.extend_from_slice(chunk);
        }
        sock.send_to(&b, env.udp()).unwrap();
        let _ = sock.recv_from(&mut buf).unwrap();
        buf[..4].to_vec()
    };
    assert_eq!(announce(&format!("/{alice}/announce")), 1u32.to_be_bytes());
    assert_eq!(announce("/announce"), 3u32.to_be_bytes(), "an error");
    sock.send_to(&udp::scrape_request(conn_id, 3, &[InfoHash(ih)]), env.udp())
        .unwrap();
    let (n, _) = sock.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..4], 3u32.to_be_bytes());
    assert!(String::from_utf8_lossy(&buf[8..n]).contains("a passkey is required"));

    // The refusals counted; the tracker private.
    let st = stats(&env.engine).unwrap();
    assert!(st.private);
    let passkey = st.refused.iter().find(|(n, _)| *n == "passkey").unwrap().1;
    assert_eq!(passkey, 7);

    // The passkey redacted from the logged target: the path's, and the query's.
    let h = akit_runtime::Handler::log_target(&*super::handler::handler(slot)).unwrap();
    assert_eq!(
        h(&format!("/t/{alice}/announce?info_hash=x")).as_deref(),
        Some("/t/REDACTED/announce?info_hash=x")
    );
    assert_eq!(
        h("/announce?passkey=k&x=1").as_deref(),
        Some("/announce?passkey=REDACTED&x=1")
    );
    assert_eq!(h("/announce?x=1"), None);
}
