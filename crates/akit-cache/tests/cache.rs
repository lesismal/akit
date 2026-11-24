//! The `cache` middleware through akit's listener on akio, with fit's tests
//! (`middleware/cache/cache_test.go`, `auth_test.go`, parts of `cover_test.go`) ported, over
//! HTTP/1.1 and h2c, on the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
//!
//! The routes' handler is a test one, registered as `respond`, which answers by path as fit's
//! `origin` does and counts what it answers. Others go through the `proxy` handler, to an akit
//! serving that handler, or to an upstream of std sockets that breaks its bodies off.
//!
//! The tests run one at a time ([`serial`]): they share the origin's counts, and some hold the
//! cache's clock, which every cache in the process reads.
//!
//! Where fit's answer comes from its cache lock (I68: a stale response fetched again by the cache
//! while the request waits, and answered from what that keeps), akit's I67 sends the request on
//! with the stale response's validators and keeps what comes back: the tests say which.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use akio::{Backend, Engine, UringConfig};
use akio_http::Method;
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

/// One test at a time.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    let g = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    origin::reset();
    g
}

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

/// A body larger than a test's `max_entry_bytes`: fit's `long`, 1 MiB.
fn long() -> &'static str {
    static LONG: OnceLock<String> = OnceLock::new();
    LONG.get_or_init(|| "0123456789abcdef".repeat(64 << 10))
}

/// The clock, as the origin and the cache read it: held by [`at`] until the guard goes.
struct Clock;

static HELD: Mutex<Option<SystemTime>> = Mutex::new(None);

fn now() -> SystemTime {
    HELD.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .unwrap_or_else(SystemTime::now)
}

/// Holds the clock at `start` plus `secs`, for the origin and the cache.
fn at(start: SystemTime, secs: u64) -> Clock {
    let t = start + Duration::from_secs(secs);
    *HELD.lock().unwrap() = Some(t);
    akit_cache::hold_clock(Some(t));
    Clock
}

impl Drop for Clock {
    fn drop(&mut self) {
        *HELD.lock().unwrap_or_else(PoisonError::into_inner) = None;
        akit_cache::hold_clock(None);
    }
}

fn http_date(t: SystemTime) -> String {
    akio_http::message::format_http_date(
        t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
    )
}

/// The test origin, as fit's: it answers by path, counting what it answers.
mod origin {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use akio_http::{Context, Headers, Response};
    use akit_runtime::request::request_headers;
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    static HITS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
    /// Lets the slow stream go on.
    /// The sending end, and the receiving end the slow streams share.
    type Release = (Sender<()>, Arc<Mutex<Receiver<()>>>);
    static RELEASE: Mutex<Option<Release>> = Mutex::new(None);
    /// The slow streams whose client the server heard go.
    pub(crate) static GONE: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn reset() {
        *HITS.lock().unwrap() = Some(HashMap::new());
        let (tx, rx) = channel();
        *RELEASE.lock().unwrap() = Some((tx, Arc::new(Mutex::new(rx))));
        GONE.store(0, Ordering::SeqCst);
    }

    pub(crate) fn hits(path: &str) -> u64 {
        HITS.lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.get(path).copied())
            .unwrap_or(0)
    }

    pub(crate) fn reset_hits() {
        *HITS.lock().unwrap() = Some(HashMap::new());
    }

    /// Lets one slow stream go on.
    pub(crate) fn let_go() {
        if let Some((tx, _)) = RELEASE.lock().unwrap().as_ref() {
            let _ = tx.send(());
        }
    }

    fn released() -> Arc<Mutex<Receiver<()>>> {
        RELEASE.lock().unwrap().as_ref().unwrap().1.clone()
    }

    fn count(path: &str) -> u64 {
        let mut m = HITS.lock().unwrap();
        let n = m
            .get_or_insert_with(HashMap::new)
            .entry(path.to_owned())
            .or_insert(0);
        *n += 1;
        *n
    }

    struct Origin;

    impl akio_http::Service for Origin {
        fn call(&self, ctx: &mut Context) {
            let path = ctx.path().to_owned();
            let n = count(&path);
            let now = super::now();
            let date = super::http_date(now);
            let mut h = Headers::new();
            h.append("Content-Type", "text/plain");
            h.append("Date", &date);
            let mut body = format!("{path} {n}").into_bytes();
            let mut status = 200;
            let fields = request_headers(ctx).into_owned();
            if ctx.req().method.as_str() == "POST" {
                if path.starts_with("/custom") {
                    let mut r = Response::new(201);
                    r.headers.append("Location", "/fresh");
                    r.headers
                        .append("Content-Location", "http://elsewhere/s-maxage");
                    return ctx.respond(r);
                }
                return ctx.respond(Response::text(200, "posted"));
            }
            match path.as_str() {
                "/fresh" => {
                    h.append("Cache-Control", "max-age=60");
                    h.append("ETag", "\"v1\"");
                    h.append("Last-Modified", "Mon, 02 Jan 2006 15:04:05 GMT");
                }
                "/s-maxage" => h.append("Cache-Control", "max-age=0, s-maxage=60"),
                "/expires" => h.append("Expires", super::http_date(now + Duration::from_secs(60))),
                "/expires-bad" => h.append("Expires", "0"),
                "/heuristic" => {
                    h.append(
                        "Last-Modified",
                        super::http_date(now - Duration::from_secs(100)),
                    );
                }
                "/plain" => {}
                "/no-store" => h.append("Cache-Control", "max-age=60, no-store"),
                "/private" => h.append("Cache-Control", "private, max-age=60"),
                "/no-cache" => h.append("Cache-Control", "no-cache, max-age=60"),
                "/cookie" => {
                    h.append("Cache-Control", "max-age=60");
                    h.append("Set-Cookie", "a=b");
                }
                "/vary-star" => {
                    h.append("Cache-Control", "max-age=60");
                    h.append("Vary", "*");
                }
                "/error" => {
                    h.append("Cache-Control", "max-age=60");
                    status = 500;
                }
                "/found" => {
                    h.append("Cache-Control", "max-age=60");
                    h.append("Location", "/fresh");
                    status = 302;
                }
                "/found-plain" => {
                    h.append("Location", "/fresh");
                    status = 302;
                }
                "/aged" => {
                    h.append("Cache-Control", "max-age=60");
                    h.append("Age", "70");
                }
                "/vary" => {
                    h.append("Cache-Control", "max-age=60");
                    h.append("Vary", "Accept-Language");
                    body.push(b' ');
                    body.extend_from_slice(fields.get("accept-language").unwrap_or("").as_bytes());
                }
                "/must" => h.append("Cache-Control", "max-age=60, must-revalidate"),
                "/public" => h.append("Cache-Control", "public, max-age=60"),
                "/large" => {
                    h.append("Cache-Control", "max-age=60");
                    body = super::long().as_bytes().to_vec();
                }
                "/stream" | "/slow" => {
                    let mut sh = Headers::new();
                    sh.append("Cache-Control", "max-age=60");
                    sh.append("Content-Type", "text/plain");
                    if path == "/slow" {
                        ctx.on_gone(|| {
                            GONE.fetch_add(1, Ordering::SeqCst);
                        });
                    }
                    let Some(s) = ctx.stream_response(200, sh) else {
                        return;
                    };
                    let release = released();
                    thread::spawn(move || {
                        let long = super::long().as_bytes();
                        for i in 0..64 {
                            if path == "/slow" && i == 1 {
                                let r = release.lock().unwrap();
                                let _ = r.recv_timeout(Duration::from_secs(5));
                            }
                            if s.write(&long[i * 1024..(i + 1) * 1024]).is_err() {
                                return;
                            }
                        }
                        s.finish();
                    });
                    return;
                }
                "/revalidate" => {
                    h.append("Cache-Control", "max-age=60");
                    h.append("ETag", "\"r1\"");
                    if fields.get("if-none-match") == Some("\"r1\"") {
                        let mut r = Response::new(304);
                        r.headers.append("Date", &date);
                        r.headers.append("Cache-Control", "max-age=120");
                        r.headers.append("ETag", "\"r1\"");
                        return ctx.respond(r);
                    }
                }
                "/gzip" => {
                    h.set("Content-Type", "application/json");
                    h.append("Cache-Control", "max-age=60");
                    body = "{\"a\": 1}".repeat(1000).into_bytes();
                }
                _ => status = 404,
            }
            let mut r = Response::new(status);
            r.headers = h;
            r.body = body;
            ctx.respond(r);
        }
    }

    impl Handler for Origin {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Origin) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] = &[Info::handler(
        "http.handlers.respond",
        "Answers by path.",
        new,
    )];
}

/// akit serving one route.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let slot = EngineSlot::new();
        let registry = Registry::new(&[
            origin::MODULES,
            akit_middleware::MODULES,
            akit_cache::MODULES,
            akit_proxy::MODULES,
        ])
        .unwrap();
        let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
        let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
        engine.start().unwrap();
        assert!(slot.set(engine.clone()));
        let addr = *engine.local_addrs().first().unwrap();
        Akit {
            engine,
            addr,
            _cell: cell,
        }
    }

    /// The origin behind `middleware` (a JSON array).
    fn serve(middleware: &str) -> Akit {
        Akit::route(middleware, r#"{"module": "respond"}"#, "")
    }

    fn route(middleware: &str, handler: &str, extra: &str) -> Akit {
        Akit::start(&format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
                {extra}
                "routes": [{{"middleware": {middleware}, "handler": {handler}}}]}}"#
        ))
    }

    /// The `proxy` handler to `up` behind `middleware`.
    fn proxy(middleware: &str, up: SocketAddr) -> Akit {
        Akit::route(
            middleware,
            r#"{"module": "proxy", "params": {"upstream": "app"}}"#,
            &format!(r#""upstreams": [{{"name": "app", "addresses": ["{up}"]}}],"#),
        )
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

const CACHE_ONLY: &str = r#"[{"module": "cache", "params": {"zone": "test"}}]"#;

fn clients() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        let e = Engine::builder().client_threads(2).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    })
    .clone()
}

/// A response as a client got it.
#[derive(Debug, Default)]
struct Got {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// It ended as it should (a broken one is cut short or reset).
    complete: bool,
}

impl Got {
    fn one(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn cache_status(&self) -> &str {
        self.one("cache-status").unwrap_or("")
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Answered with a fresh response the cache keeps.
    fn fresh_hit(&self) -> bool {
        let cs = self.cache_status();
        cs.starts_with("fit; hit; ttl=") && !cs.contains("ttl=-") && !cs.contains("detail")
    }

    fn hit(&self) -> bool {
        self.cache_status().starts_with("fit; hit")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Proto {
    H1,
    H2c,
}

const PROTOS: [Proto; 2] = [Proto::H1, Proto::H2c];

fn get(proto: Proto, addr: SocketAddr, method: &str, path: &str, fields: &[(&str, &str)]) -> Got {
    match proto {
        Proto::H1 => {
            let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}");
            for (n, v) in fields {
                head.push_str(&format!("\r\n{n}: {v}"));
            }
            if method == "POST" {
                head.push_str("\r\nContent-Length: 0");
            }
            h1(addr, &head)
        }
        Proto::H2c => h2(addr, Method::parse(method), path, fields),
    }
}

/// `req` over h2c to `addr`, with `fields`.
fn h2(addr: SocketAddr, method: Method, path: &str, fields: &[(&str, &str)]) -> Got {
    let c = H2Client::dial_blocking(&clients(), addr, None, H2Options::default()).unwrap();
    let mut req = ClientRequest::new(method, &format!("http://{addr}{path}"));
    for (n, v) in fields {
        req.headers.append(n, v);
    }
    let (tx, rx) = channel();
    let mut got = Got::default();
    let up = c.open(req, move |e| match e {
        H2Event::Head { status, headers } => {
            got.status = status;
            got.headers = headers
                .iter()
                .map(|(n, v)| (n.to_owned(), v.to_owned()))
                .collect();
        }
        H2Event::Data(d) => got.body.extend_from_slice(d),
        H2Event::Trailers(_) => {}
        H2Event::End => {
            got.complete = true;
            let _ = tx.send(std::mem::take(&mut got));
        }
        H2Event::Error(_) => {
            let _ = tx.send(std::mem::take(&mut got));
        }
    });
    up.finish();
    let got = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    c.close();
    got
}

/// A raw HTTP/1.1 exchange: `head` (its request line and fields, without the blank line), the
/// response read as it is framed.
fn h1(addr: SocketAddr, head: &str) -> Got {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(format!("{head}\r\n\r\n").as_bytes()).unwrap();
    h1_response(BufReader::new(s), head.starts_with("HEAD "))
}

fn h1_response(mut r: BufReader<TcpStream>, head_only: bool) -> Got {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap() == 0 {
            break;
        }
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((n, v)) = l.split_once(':') {
            headers.push((n.to_owned(), v.trim().to_owned()));
        }
    }
    let mut got = Got {
        status,
        headers,
        ..Got::default()
    };
    if head_only || matches!(got.status, 204 | 304) {
        got.complete = true;
        return got;
    }
    if got
        .one("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let mut size = String::new();
            if r.read_line(&mut size).unwrap_or(0) == 0 {
                return got;
            }
            let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
                return got;
            };
            if n == 0 {
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap_or(0) == 0 || l.trim_end().is_empty() {
                        got.complete = true;
                        return got;
                    }
                }
            }
            let mut chunk = vec![0; n + 2];
            if r.read_exact(&mut chunk).is_err() {
                return got;
            }
            got.body.extend_from_slice(&chunk[..n]);
        }
    }
    match got
        .one("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(n) => {
            let mut body = vec![0; n];
            let mut read = 0;
            while read < n {
                match r.read(&mut body[read..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => read += k,
                }
            }
            body.truncate(read);
            got.complete = read == n;
            got.body = body;
        }
        None => {
            let _ = r.read_to_end(&mut got.body);
            got.complete = true;
        }
    }
    got
}

// fit's TestHit.
#[test]
fn hit() {
    let _s = serial();
    for proto in PROTOS {
        origin::reset_hits();
        let a = Akit::serve(CACHE_ONLY);
        let first = get(proto, a.addr, "GET", "/fresh", &[]);
        assert_eq!(
            (first.status, first.text().as_str(), first.cache_status()),
            (200, "/fresh 1", "fit; fwd=uri-miss; stored"),
            "{proto:?}"
        );
        let second = get(proto, a.addr, "GET", "/fresh", &[]);
        assert_eq!(second.text(), "/fresh 1", "{proto:?}");
        assert!(
            second.cache_status().starts_with("fit; hit; ttl="),
            "{second:?}"
        );
        assert!(second.one("age").is_some() && second.one("etag") == Some("\"v1\""));
        assert_eq!(origin::hits("/fresh"), 1);
        let head = get(proto, a.addr, "HEAD", "/fresh", &[]);
        assert_eq!((head.status, head.body.len()), (200, 0), "{proto:?}");
        assert!(head.hit() && origin::hits("/fresh") == 1, "{head:?}");
        if proto == Proto::H1 {
            assert_eq!(head.one("content-length"), Some("8"));
        }
        let other = get(proto, a.addr, "GET", "/fresh?x=1", &[]);
        assert_eq!(
            (other.text().as_str(), other.cache_status()),
            ("/fresh 2", "fit; fwd=uri-miss; stored")
        );
    }
}

// fit's TestStorable: a second request for a response that was not kept goes to the origin again.
#[test]
fn storable() {
    let _s = serial();
    let a = Akit::serve(
        r#"[{"module": "cache", "params": {"zone": "test", "max_bytes": 1048576, "max_entry_bytes": 65536}}]"#,
    );
    let auth = ("Authorization", "Bearer x");
    for (path, fields, kept) in [
        ("/fresh", vec![], true),
        ("/s-maxage", vec![], true),
        ("/expires", vec![], true),
        ("/heuristic", vec![], true),
        ("/found", vec![], true),
        ("/public", vec![auth], true),
        ("/plain", vec![], false),       // no lifetime
        ("/expires-bad", vec![], false), // expired
        ("/no-store", vec![], false),
        ("/private", vec![], false),
        ("/no-cache", vec![], false),
        ("/cookie", vec![], false),
        ("/vary-star", vec![], false),
        ("/error", vec![], false),
        ("/found-plain", vec![], false), // a 302 needs a lifetime of its own
        ("/aged", vec![], false),        // older than its lifetime
        ("/large", vec![], false),       // past max_entry_bytes
        ("/must", vec![auth], true),
        ("/s-maxage?auth", vec![auth], true),
        ("/fresh?auth", vec![auth], false),
        (
            "/fresh?no-store",
            vec![("Cache-Control", "no-store")],
            false,
        ),
    ] {
        let bare = path.split('?').next().unwrap();
        origin::reset_hits();
        let first = get(Proto::H1, a.addr, "GET", path, &fields);
        let second = get(Proto::H1, a.addr, "GET", path, &fields);
        if kept {
            assert_eq!(
                (first.status, &first.body),
                (second.status, &second.body),
                "{path}"
            );
        }
        assert_eq!(
            origin::hits(bare) == 1,
            kept,
            "{path} {fields:?}: {} hits; {:?} then {:?}",
            origin::hits(bare),
            first.cache_status(),
            second.cache_status()
        );
        if path == "/large" {
            assert_eq!(second.body.len(), long().len());
        }
    }
}

// fit's TestFreshness. A stale response is sent on with its validators and kept again (fit
// answers it from its cache lock's fetch, I68).
#[test]
fn freshness() {
    let _s = serial();
    let start = SystemTime::now();
    let _c = at(start, 0);
    let a = Akit::serve(CACHE_ONLY);
    let c = Proto::H2c;
    get(c, a.addr, "GET", "/fresh", &[]);
    get(c, a.addr, "GET", "/heuristic", &[]); // fresh for 10s
    get(c, a.addr, "GET", "/expires", &[]);
    let _c = at(start, 30);
    let r = get(c, a.addr, "GET", "/fresh", &[]);
    // The Date it was given is up to a second old already.
    assert_eq!(r.one("age"), Some("30"));
    assert!(
        matches!(r.cache_status(), "fit; hit; ttl=29" | "fit; hit; ttl=30"),
        "{r:?}"
    );
    let r = get(c, a.addr, "GET", "/heuristic", &[]);
    assert_eq!(r.cache_status(), "fit; fwd=stale; stored");
    assert_eq!(origin::hits("/heuristic"), 2);
    assert!(get(c, a.addr, "GET", "/expires", &[]).hit());
    let _c = at(start, 61);
    let r = get(c, a.addr, "GET", "/fresh", &[]);
    assert_eq!(
        (r.cache_status(), r.text().as_str()),
        ("fit; fwd=stale; stored", "/fresh 2")
    );
    let r = get(c, a.addr, "GET", "/fresh", &[]);
    assert!(r.fresh_hit() && r.text() == "/fresh 2", "{r:?}");
}

// fit's TestDefaultTTL.
#[test]
fn default_ttl() {
    let _s = serial();
    let a =
        Akit::serve(r#"[{"module": "cache", "params": {"zone": "test", "default_ttl": "1m"}}]"#);
    let c = Proto::H1;
    get(c, a.addr, "GET", "/plain", &[]);
    let r = get(c, a.addr, "GET", "/plain", &[]);
    assert!(r.hit() && origin::hits("/plain") == 1, "{r:?}");
    get(c, a.addr, "GET", "/nothing", &[]); // 404 is heuristically cacheable
    let r = get(c, a.addr, "GET", "/nothing", &[]);
    assert!(r.status == 404 && r.hit(), "{r:?}");
    get(c, a.addr, "GET", "/error", &[]);
    get(c, a.addr, "GET", "/error", &[]);
    assert_eq!(origin::hits("/error"), 2, "a 500 was kept");
}

// fit's TestVary.
#[test]
fn vary() {
    let _s = serial();
    let a = Akit::serve(CACHE_ONLY);
    for proto in PROTOS {
        origin::reset_hits();
        let url = format!("/vary?{proto:?}");
        let en = get(proto, a.addr, "GET", &url, &[("Accept-Language", "en")]);
        let fr = get(proto, a.addr, "GET", &url, &[("Accept-Language", "fr")]);
        assert_eq!(fr.cache_status(), "fit; fwd=vary-miss; stored", "{proto:?}");
        let en2 = get(proto, a.addr, "GET", &url, &[("Accept-Language", "en")]);
        let fr2 = get(proto, a.addr, "GET", &url, &[("Accept-Language", "fr")]);
        assert!(en2.body == en.body && fr2.body == fr.body && en.body != fr.body && en2.hit());
        let none = get(proto, a.addr, "GET", &url, &[]);
        assert!(
            !none.cache_status().is_empty() && !none.hit(),
            "{proto:?}: {none:?}"
        );
    }
}

// fit's TestConditional.
#[test]
fn conditional() {
    let _s = serial();
    let a = Akit::serve(CACHE_ONLY);
    let c = Proto::H1;
    get(c, a.addr, "GET", "/fresh", &[]);
    for fields in [
        vec![("If-None-Match", "\"v1\"")],
        vec![("If-None-Match", "W/\"v1\"")],
        vec![("If-None-Match", "\"x\", \"v1\"")],
        vec![("If-None-Match", "*")],
        vec![("If-Modified-Since", "Mon, 02 Jan 2006 15:04:05 GMT")],
        vec![("If-Modified-Since", "Tue, 03 Jan 2006 15:04:05 GMT")],
    ] {
        let r = get(c, a.addr, "GET", "/fresh", &fields);
        assert!(
            r.status == 304
                && r.body.is_empty()
                && r.one("etag") == Some("\"v1\"")
                && r.one("cache-control") == Some("max-age=60")
                && r.one("content-type").is_none()
                && r.hit(),
            "{fields:?}: {r:?}"
        );
    }
    for fields in [
        vec![("If-None-Match", "\"v2\"")],
        vec![("If-Modified-Since", "Sun, 01 Jan 2006 15:04:05 GMT")],
        vec![
            ("If-None-Match", "\"v2\""),
            ("If-Modified-Since", "Tue, 03 Jan 2006 15:04:05 GMT"),
        ],
    ] {
        let r = get(c, a.addr, "GET", "/fresh", &fields);
        assert_eq!(
            (r.status, r.text().as_str()),
            (200, "/fresh 1"),
            "{fields:?}"
        );
    }
    assert_eq!(origin::hits("/fresh"), 1);
}

/// fit's TestRevalidate, against `a`: a stale response is fetched again by the cache with its
/// own validators and freshened by the origin's 304, for a client that sent none and for one
/// that sent its own.
fn revalidates(a: &Akit, start: SystemTime) {
    let c = Proto::H2c;
    let _c = at(start, 0);
    get(c, a.addr, "GET", "/revalidate", &[]);
    let _c = at(start, 90);
    let r = get(c, a.addr, "GET", "/revalidate", &[]);
    assert!(
        r.status == 200
            && r.text() == "/revalidate 1"
            && r.hit()
            && r.one("cache-control") == Some("max-age=120")
            && origin::hits("/revalidate") == 2,
        "revalidated: {r:?}, {} fetches",
        origin::hits("/revalidate")
    );
    let _c = at(start, 300);
    let r = get(
        c,
        a.addr,
        "GET",
        "/revalidate",
        &[("If-None-Match", "\"r1\"")],
    );
    assert!(
        r.status == 304 && r.hit() && origin::hits("/revalidate") == 3,
        "with the client's validators: {r:?}"
    );
    let r = get(c, a.addr, "GET", "/revalidate", &[]);
    assert!(
        r.status == 200 && r.hit() && origin::hits("/revalidate") == 3,
        "then: {r:?}"
    );
}

#[test]
fn revalidate() {
    let _s = serial();
    revalidates(&Akit::serve(CACHE_ONLY), SystemTime::now());
}

/// The same through the proxy: the validators go upstream, and its 304 comes back.
#[test]
fn revalidate_through_the_proxy() {
    let _s = serial();
    let origin = Akit::serve("[]");
    revalidates(&Akit::proxy(CACHE_ONLY, origin.addr), SystemTime::now());
}

// fit's TestRequestDirectives.
#[test]
fn request_directives() {
    let _s = serial();
    let start = SystemTime::now();
    let _c = at(start, 0);
    let a = Akit::serve(CACHE_ONLY);
    let c = Proto::H1;
    let r = get(
        c,
        a.addr,
        "GET",
        "/fresh",
        &[("Cache-Control", "only-if-cached")],
    );
    assert_eq!(r.status, 504, "only-if-cached, nothing kept");
    get(c, a.addr, "GET", "/fresh", &[]);
    get(c, a.addr, "GET", "/must", &[]);
    let _c = at(start, 20);
    for (fields, want) in [
        (vec![("Cache-Control", "only-if-cached")], "fit; hit"),
        (vec![("Cache-Control", "max-age=30")], "fit; hit"),
        (vec![("Cache-Control", "min-fresh=30")], "fit; hit"),
        (
            vec![("Cache-Control", "max-age=10")],
            "fit; fwd=request; stored",
        ),
        // From here on, what is kept is the response stored just now.
        (
            vec![("Cache-Control", "min-fresh=61")],
            "fit; fwd=request; stored",
        ),
        (
            vec![("Cache-Control", "no-cache")],
            "fit; fwd=request; stored",
        ),
        (vec![("Pragma", "no-cache")], "fit; fwd=request; stored"),
        (
            vec![("Pragma", "no-cache"), ("Cache-Control", "max-age=60")],
            "fit; hit",
        ),
        (vec![("Range", "bytes=0-1")], "fit; fwd=bypass; stored"),
    ] {
        let r = get(c, a.addr, "GET", "/fresh", &fields);
        assert!(
            r.cache_status().starts_with(want),
            "{fields:?}: {:?}, want {want:?}",
            r.cache_status()
        );
    }
    let _c = at(start, 100); // /fresh was stored again at 20s
    for (path, fields, want) in [
        (
            "/fresh",
            vec![("Cache-Control", "max-stale")],
            "fit; hit; ttl=-2",
        ),
        (
            "/fresh",
            vec![("Cache-Control", "max-stale=30")],
            "fit; hit",
        ),
        // Sent on with its validators, and kept again (fit: "fit; hit; ttl=59", from its lock).
        (
            "/fresh",
            vec![("Cache-Control", "max-stale=10")],
            "fit; fwd=stale; stored",
        ),
        (
            "/must",
            vec![("Cache-Control", "max-stale")],
            "fit; fwd=stale; stored",
        ),
    ] {
        let r = get(c, a.addr, "GET", path, &fields);
        assert!(
            r.cache_status().starts_with(want),
            "{path} {fields:?}: {:?}, want {want:?}",
            r.cache_status()
        );
    }
}

// fit's TestInvalidate.
#[test]
fn invalidate() {
    let _s = serial();
    let a = Akit::serve(CACHE_ONLY);
    let c = Proto::H1;
    for path in ["/fresh", "/s-maxage", "/expires"] {
        get(c, a.addr, "GET", path, &[]);
    }
    get(c, a.addr, "POST", "/expires", &[]);
    get(c, a.addr, "POST", "/custom", &[]);
    let r = get(c, a.addr, "GET", "/expires", &[]);
    assert_eq!(
        r.cache_status(),
        "fit; fwd=uri-miss; stored",
        "the POSTed URI"
    );
    let r = get(c, a.addr, "GET", "/fresh", &[]);
    assert_eq!(
        r.cache_status(),
        "fit; fwd=uri-miss; stored",
        "the Location"
    );
    let r = get(c, a.addr, "GET", "/s-maxage", &[]);
    assert!(r.hit(), "another host's Content-Location: {r:?}");
    // OPTIONS changes nothing (fit's TestTLSKeys, its part on OPTIONS).
    let r = get(c, a.addr, "OPTIONS", "/s-maxage", &[]);
    assert!(r.cache_status().is_empty(), "{r:?}");
    assert!(get(c, a.addr, "GET", "/s-maxage", &[]).hit());
}

// fit's TestBodies: a body written in parts, kept once it has all gone, and one past
// max_entry_bytes, passed through whole.
#[test]
fn bodies() {
    let _s = serial();
    let a = Akit::serve(
        r#"[{"module": "cache", "params": {"zone": "test", "max_entry_bytes": 131072}}]"#,
    );
    let want = &long().as_bytes()[..64 << 10];
    for proto in PROTOS {
        origin::reset_hits();
        let r = get(proto, a.addr, "GET", &format!("/stream?{proto:?}"), &[]);
        assert_eq!(r.body, want, "{proto:?}");
        let r = get(proto, a.addr, "GET", &format!("/stream?{proto:?}"), &[]);
        assert!(
            r.body == want && r.hit(),
            "{proto:?}: {} bytes, {:?}",
            r.body.len(),
            r.cache_status()
        );
        if proto == Proto::H1 {
            assert_eq!(r.one("content-length"), Some("65536"));
        }
        assert_eq!(origin::hits("/stream"), 1);
        for _ in 0..2 {
            let r = get(proto, a.addr, "GET", "/large", &[]);
            assert_eq!(r.body.len(), long().len(), "{proto:?}");
        }
        assert_eq!(
            origin::hits("/large"),
            2,
            "{proto:?}: a body past max_entry_bytes was kept"
        );
    }
}

/// The proxy's bodies: one streamed (past the proxy's 64 KiB whole-response threshold) is kept
/// as it goes, and answered whole from then on; one of 1 MiB too.
#[test]
fn bodies_through_the_proxy() {
    let _s = serial();
    let origin = Akit::serve("[]");
    let a = Akit::proxy(
        r#"[{"module": "cache", "params": {"zone": "test", "max_entry_bytes": 2097152}}]"#,
        origin.addr,
    );
    for proto in PROTOS {
        origin::reset_hits();
        for path in ["/stream", "/large"] {
            let url = format!("{path}?{proto:?}");
            let want = if path == "/large" {
                long().as_bytes()
            } else {
                &long().as_bytes()[..64 << 10]
            };
            let first = get(proto, a.addr, "GET", &url, &[]);
            assert!(
                first.body == want && first.complete,
                "{proto:?} {path}: {} bytes",
                first.body.len()
            );
            assert_eq!(first.cache_status(), "fit; fwd=uri-miss; stored");
            let second = get(proto, a.addr, "GET", &url, &[]);
            assert!(
                second.body == want && second.hit(),
                "{proto:?} {path}: {:?}",
                second.cache_status()
            );
            assert_eq!(origin::hits(path), 1, "{proto:?} {path}");
        }
    }
}

/// An upstream of std sockets that answers every request with `max-age=60` and a body cut short
/// of its declared length, counting the requests.
fn broken_upstream(declared: usize, sent: usize) -> (SocketAddr, Arc<AtomicU64>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let n = Arc::new(AtomicU64::new(0));
    let count = n.clone();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            count.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim_end().is_empty() {
                        break;
                    }
                }
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nCache-Control: max-age=60\r\nContent-Type: text/plain\r\nContent-Length: {declared}\r\n\r\n"
                );
                let _ = s.write_all(&vec![b'x'; sent]);
                let _ = s.flush();
                thread::sleep(Duration::from_millis(50));
            });
        }
    });
    (addr, n)
}

/// A body the upstream breaks off is not kept, whole (fit's `/short`) or streamed.
#[test]
fn bodies_cut_short_are_not_kept() {
    let _s = serial();
    for (declared, sent) in [(100, 15), (200_000, 100_000)] {
        let (up, n) = broken_upstream(declared, sent);
        let a = Akit::proxy(CACHE_ONLY, up);
        for proto in PROTOS {
            n.store(0, Ordering::SeqCst);
            for _ in 0..2 {
                let r = get(proto, a.addr, "GET", &format!("/short?{proto:?}"), &[]);
                assert!(
                    !r.complete || r.status >= 500,
                    "{proto:?} {declared}: {r:?}"
                );
            }
            assert_eq!(
                n.load(Ordering::SeqCst),
                2,
                "{proto:?} {declared}: a body short of its length was kept"
            );
        }
    }
}

/// Waits for `f`, up to 5 seconds.
fn wait_for(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn capturing(zone: &str) -> u64 {
    akit_cache::zone_stats(zone).map_or(0, |(_, _, c)| c)
}

// fit's TestClientGone: a response whose client goes before it ends is not kept, and gives back
// what it held.
#[test]
fn client_gone() {
    let _s = serial();
    let a = Akit::serve(r#"[{"module": "cache", "params": {"zone": "gone"}}]"#);
    let mut s = TcpStream::connect(a.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut buf = [0u8; 512];
    s.read_exact(&mut buf).unwrap();
    // The tap counts a write once the write call returns, which may be after the client read it.
    wait_for("nothing held while the body goes", || capturing("gone") > 0);
    drop(s);
    // The origin goes on only once the server heard the client go: a rest of the body that went
    // before it does is the whole response, which is kept.
    wait_for("the server did not hear the client go", || {
        origin::GONE.load(Ordering::SeqCst) > 0
    });
    origin::let_go();
    wait_for("bytes still held", || capturing("gone") == 0);
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(100));
        origin::let_go();
    });
    let r = get(Proto::H1, a.addr, "GET", "/slow", &[]);
    assert_eq!(
        (r.cache_status(), r.body.len()),
        ("fit; fwd=uri-miss; stored", 64 << 10)
    );
    let r = get(Proto::H1, a.addr, "GET", "/slow", &[]);
    assert!(r.hit() && r.body.len() == 64 << 10, "{r:?}");
    assert_eq!(origin::hits("/slow"), 2);
}

/// What responses being stored hold at once is bounded by a quarter of `max_bytes` (fit's
/// TestCapture): past it, a response goes to its client without being kept.
#[test]
fn captures_are_bounded() {
    let _s = serial();
    // max_bytes 160 KiB: a quarter is 40 KiB, less than /stream's 64 KiB.
    let a = Akit::serve(
        r#"[{"module": "cache", "params": {"zone": "bounded", "max_bytes": 163840, "max_entry_bytes": 10240}}]"#,
    );
    for _ in 0..2 {
        let r = get(Proto::H1, a.addr, "GET", "/stream", &[]);
        assert_eq!(r.body.len(), 64 << 10);
    }
    assert_eq!(origin::hits("/stream"), 2);
    assert_eq!(capturing("bounded"), 0);
}

// fit's TestCompressed: the cache in front of compress keeps each encoding as a variant of its
// own, whole bodies and streamed ones.
#[test]
fn compressed() {
    let _s = serial();
    let a =
        Akit::serve(r#"[{"module": "cache", "params": {"zone": "test"}}, {"module": "compress"}]"#);
    let c = Proto::H2c;
    let gz = get(c, a.addr, "GET", "/gzip", &[("Accept-Encoding", "gzip")]);
    let plain = get(c, a.addr, "GET", "/gzip", &[]);
    let gz2 = get(c, a.addr, "GET", "/gzip", &[("Accept-Encoding", "gzip")]);
    let plain2 = get(c, a.addr, "GET", "/gzip", &[]);
    assert_eq!(gz.one("content-encoding"), Some("gzip"));
    assert_eq!(plain.one("content-encoding"), None);
    assert!(
        gz2.body == gz.body && plain2.body == plain.body,
        "the bodies differ"
    );
    assert!(
        gz2.hit() && plain2.hit() && origin::hits("/gzip") == 2,
        "{gz2:?} {plain2:?}"
    );
    assert_eq!(gz2.one("content-encoding"), Some("gzip"));
    // A streamed body is kept as it was encoded.
    for _ in 0..2 {
        let r = get(
            Proto::H1,
            a.addr,
            "GET",
            "/stream",
            &[("Accept-Encoding", "gzip")],
        );
        assert_eq!(r.one("content-encoding"), Some("gzip"));
        let plain = akio_deflate::decompress(akio_deflate::Format::Gzip, &r.body, 1 << 20).unwrap();
        assert_eq!(plain, &long().as_bytes()[..64 << 10]);
    }
    assert_eq!(origin::hits("/stream"), 1);
}

// fit's TestAuthenticatedNotShared: a response to a request an auth middleware let in is kept for
// others only when it says it may be, though basic_auth removed the Authorization. The cache
// may come before the auth middleware or after it.
#[test]
fn authenticated_not_shared() {
    let _s = serial();
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    let auth = format!(
        r#"{{"module": "basic_auth", "params": {{"users": [
            {{"username": "alice", "password_hash": "{hash}"}},
            {{"username": "bob", "password_hash": "{hash}"}}]}}}}"#
    );
    let cache = r#"{"module": "cache", "params": {"zone": "test"}}"#;
    for chain in [format!("[{auth}, {cache}]"), format!("[{cache}, {auth}]")] {
        let a = Akit::serve(&chain);
        let alice = [("Authorization", "Basic YWxpY2U6cHc=")];
        let bob = [("Authorization", "Basic Ym9iOnB3")];
        for (path, kept) in [
            ("/fresh", false),
            ("/public", true),
            ("/must", true),
            ("/s-maxage", true),
        ] {
            origin::reset_hits();
            let first = get(Proto::H1, a.addr, "GET", path, &alice);
            let second = get(Proto::H1, a.addr, "GET", path, &bob);
            assert_eq!((first.status, second.status), (200, 200), "{path}");
            assert_eq!(
                origin::hits(path) == 1,
                kept,
                "{path}: {:?} then {:?}",
                first.cache_status(),
                second.cache_status()
            );
        }
    }
}

// fit's TestParams, and the fields that wait for I68.
#[test]
fn params() {
    let cfg = |params: &str| {
        akit_config::from_json(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], "routes": [{{"middleware": [{{"module": "cache", "params": {params}}}], "handler": {{"module": "respond"}}}}]}}"#
            ),
            None,
        )
    };
    for (params, want) in [
        (
            r#"{"max_bytes": 1048576, "max_entry_bytes": 1048576}"#,
            "more than a sixteenth of max_bytes (65536)",
        ),
        (r#"{"max_bytes": -1}"#, "max_bytes"),
        (
            r#"{"max_bytes": 1, "max_size": 1}"#,
            "renamed \"max_bytes\"",
        ),
        (r#"{"default_ttl": "soon"}"#, "not a duration"),
        (r#"{"stale_while_revalidate": "1m"}"#, "not supported yet"),
        (r#"{"stale_if_error": "1m"}"#, "not supported yet"),
        (r#"{"disk": {"path": "/tmp/x"}}"#, "not supported yet"),
        (r#"{"Zone": "x"}"#, "did you mean \"zone\""),
    ] {
        let e = cfg(params)
            .err()
            .unwrap_or_else(|| panic!("{params}: accepted"));
        assert!(e.to_string().contains(want), "{params}: {e}, want {want:?}");
    }
    let c = cfg(r#"{"max_bytes": 1048576, "max_entry_size": 4096, "lock_timeout": "2s", "disable_lock": true}"#).unwrap();
    let akit_config::Handler::Cache(m) = &c.routes[0].middleware[0] else {
        panic!("not a cache");
    };
    assert_eq!(
        (m.max_bytes, m.max_entry_bytes, m.zone.as_str()),
        (1 << 20, 4096, "default")
    );
    assert!(m.disable_lock && m.lock_timeout == Duration::from_secs(2) && !m.authenticated);
    let c = cfg(r#"{"max_bytes": 1048576}"#).unwrap();
    let akit_config::Handler::Cache(m) = &c.routes[0].middleware[0] else {
        panic!("not a cache");
    };
    assert_eq!(m.max_entry_bytes, 1 << 16, "max_entry_bytes for 1 MiB");
    let c = cfg("{}").unwrap();
    let akit_config::Handler::Cache(m) = &c.routes[0].middleware[0] else {
        panic!("not a cache");
    };
    assert_eq!((m.max_bytes, m.max_entry_bytes), (128 << 20, 1 << 20));
}

/// The zone outlives a reload: a graph compiled while the old one serves takes its store.
#[test]
fn zones_outlive_a_reload() {
    let _s = serial();
    let a = Akit::serve(r#"[{"module": "cache", "params": {"zone": "reload"}}]"#);
    // The same Host on both, as a reloaded listener would see.
    h1(a.addr, "GET /fresh HTTP/1.1\r\nHost: x");
    let b = Akit::serve(r#"[{"module": "cache", "params": {"zone": "reload"}}]"#);
    drop(a);
    assert!(h1(b.addr, "GET /fresh HTTP/1.1\r\nHost: x").hit());
    assert_eq!(akit_cache::zone_stats("reload").map(|s| s.0), Some(1));
    // That a zone no cache uses goes is the store's unit test (`store::tests::zones`): an
    // engine's I/O threads let their graph go on their own time after a shutdown.
    drop(b);
}
