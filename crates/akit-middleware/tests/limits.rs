//! The `rate_limit`, `concurrency_limit` and `body_limit` middleware through akit's listener on
//! akio, with fit's tests (`middleware/limit/limit_test.go`, `cut_test.go`) ported, on the backend
//! `AKIO_TEST_BACKEND` names (epoll by default, `uring`). Each limit is tested under, at and over
//! it; the token buckets' arithmetic, keys and eviction are tested on a clock the tests move, in
//! the crate (`src/limit.rs`).
//!
//! Each route's handler is a test one, registered as `respond`: `/hang/<name>` is kept unanswered
//! (detached) until the test answers or drops it, as a proxy waiting on a slow upstream keeps its
//! request; anything else is answered at once with the length of the (buffered) body it got.
//! Others go through the `proxy` handler to an upstream that answers with the length of the body
//! it got, streamed, by its length or chunked.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::channel;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akio_http::Method;
use akio_http::client::ClientRequest;
use akio_http::h2client::{H2Client, H2Event, H2Options};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Info, Registry, compile};

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

/// The test handler.
mod held {
    use std::sync::{Arc, Mutex};

    use akio_http::{BodyStream, Context, Detached, Headers, Response};
    use akit_runtime::{Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned};

    /// The requests kept unanswered, by the path they came for.
    pub(crate) static HELD: Mutex<Vec<(String, Detached)>> = Mutex::new(Vec::new());

    /// The streamed responses kept unfinished, by the path they came for.
    pub(crate) static STREAMS: Mutex<Vec<(String, BodyStream)>> = Mutex::new(Vec::new());

    /// One stream kept for `path`, taken from the kept.
    pub(crate) fn take_stream(path: &str) -> BodyStream {
        let mut s = STREAMS.lock().unwrap();
        let i = s.iter().position(|(p, _)| p == path).unwrap();
        s.remove(i).1
    }

    /// How many requests for `path` are kept.
    pub(crate) fn count(path: &str) -> usize {
        HELD.lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .count()
    }

    /// One request kept for `path`, taken from the kept.
    pub(crate) fn take(path: &str) -> Detached {
        let mut held = HELD.lock().unwrap();
        let i = held.iter().position(|(p, _)| p == path).unwrap();
        held.remove(i).1
    }

    struct Held;

    impl akio_http::Service for Held {
        fn call(&self, ctx: &mut Context) {
            let path = ctx.path().to_owned();
            if path.starts_with("/hang/") {
                if let Some(d) = ctx.detach() {
                    HELD.lock().unwrap().push((path, d));
                }
                return;
            }
            if path.starts_with("/stream/") {
                if let Some(s) = ctx.stream_response(200, Headers::new()) {
                    let _ = s.write(b"part");
                    STREAMS.lock().unwrap().push((path, s));
                }
                return;
            }
            let n = ctx.body().len();
            ctx.respond(Response::text(200, n.to_string()));
        }
    }

    impl Handler for Held {}

    fn new(_: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        Ok(Provisioned::new(Arc::new(Held) as Arc<dyn Handler>))
    }

    pub(crate) static MODULES: &[Info<HandlerParams>] =
        &[Info::handler("http.handlers.respond", "Holds.", new)];
}

/// akit serving one route with `middleware` (a JSON array) and `handler`.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str, handlers: &'static [Info<HandlerParams>]) -> Akit {
        let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
        let slot = EngineSlot::new();
        let registry =
            Registry::new(&[handlers, akit_middleware::MODULES, akit_proxy::MODULES]).unwrap();
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

    /// The test handler behind `middleware`.
    fn held(middleware: &str) -> Akit {
        Akit::start(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
                    "routes": [{{"middleware": {middleware}, "handler": {{"module": "respond"}}}}]}}"#
            ),
            held::MODULES,
        )
    }

    /// The proxy to `up` behind `middleware`.
    fn proxy(middleware: &str, up: SocketAddr) -> Akit {
        Akit::start(
            &format!(
                r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0", "protocols": ["http/1.1", "h2c"]}}],
                    "upstreams": [{{"name": "app", "addresses": ["{up}"]}}],
                    "routes": [{{"middleware": {middleware},
                                 "handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]}}"#
            ),
            akit_handler::MODULES,
        )
    }
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

/// A response as the client saw it.
#[derive(Debug)]
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
}

/// The body a request is sent with.
enum Body {
    None,
    /// By its length.
    Sized(usize),
    /// Chunked, in chunks of 16 KiB.
    Chunked(usize),
    /// A `Content-Length` of the first, and the second's bytes.
    Declared(u64, usize),
}

/// One request on a connection of its own, kept alive (so that the server does not close it on
/// a body it refused and has not read), the body written on a thread of its own while the
/// response is read: by its `Content-Length`.
fn exchange(
    addr: SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: example.com\r\n");
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    let (framing, size) = match body {
        Body::None => (String::new(), 0),
        Body::Sized(n) => (format!("Content-Length: {n}\r\n"), n),
        Body::Chunked(n) => ("Transfer-Encoding: chunked\r\n".to_owned(), n),
        Body::Declared(d, n) => (format!("Content-Length: {d}\r\n"), n),
    };
    let chunked = framing.starts_with("Transfer");
    head.push_str(&framing);
    head.push_str("\r\n");
    let mut w = s.try_clone().unwrap();
    let writer = thread::spawn(move || {
        w.write_all(head.as_bytes())?;
        let mut left = size;
        while left > 0 {
            let n = left.min(16 << 10);
            if chunked {
                w.write_all(format!("{n:x}\r\n").as_bytes())?;
            }
            w.write_all(&vec![b'x'; n])?;
            if chunked {
                w.write_all(b"\r\n")?;
            }
            left -= n;
        }
        if chunked {
            w.write_all(b"0\r\n\r\n")?;
        }
        Ok::<(), std::io::Error>(())
    });
    let resp = read_response(&mut BufReader::new(s));
    // The server may have answered before the body was all sent, and stopped reading it.
    let _ = writer.join();
    resp
}

fn read_response(r: &mut impl BufRead) -> Resp {
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line
        .split(' ')
        .nth(1)
        .unwrap_or_else(|| panic!("no status line: {line:?}"))
        .parse()
        .unwrap();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (n, v) = line.split_once(':').unwrap();
        headers.push((n.to_owned(), v.trim().to_owned()));
    }
    let mut resp = Resp {
        status,
        headers,
        body: String::new(),
    };
    let length: usize = resp
        .header("content-length")
        .map_or(0, |v| v.parse().unwrap());
    let mut body = vec![0; length];
    r.read_exact(&mut body).unwrap();
    resp.body = String::from_utf8(body).unwrap();
    resp
}

fn get(addr: SocketAddr, target: &str) -> Resp {
    exchange(addr, "GET", target, &[], Body::None)
}

/// Waits until `f` holds, for up to 10 seconds.
fn wait_for(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// An upstream on its own threads that answers each request with the length of its body,
/// framed by its length or chunked.
fn counting_upstream() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                while let Some(n) = read_body_length(&mut r) {
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{n}",
                        n.to_string().len()
                    );
                    if w.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// Reads one request and returns how long its body was; `None` when the connection ends first.
fn read_body_length(r: &mut impl BufRead) -> Option<usize> {
    let mut length = 0;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (n, v) = line.split_once(':').unwrap_or((line, ""));
        if n.eq_ignore_ascii_case("content-length") {
            length = v.trim().parse().ok()?;
        } else if n.eq_ignore_ascii_case("transfer-encoding") {
            chunked = v.trim().eq_ignore_ascii_case("chunked");
        }
    }
    if !chunked {
        let mut body = vec![0; length];
        r.read_exact(&mut body).ok()?;
        return Some(length);
    }
    let mut total = 0;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let size = usize::from_str_radix(line.trim().split(';').next()?, 16).ok()?;
        if size == 0 {
            // The trailers, to the empty line.
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).ok()? == 0 || line == "\r\n" {
                    return Some(total);
                }
            }
        }
        let mut chunk = vec![0; size + 2];
        r.read_exact(&mut chunk).ok()?;
        total += size;
    }
}

// fit's TestRateBucket and TestRateSlow, over the network with the real clock: a burst of 3,
// then 429 with Retry-After, the rate being one request in 100 seconds.
#[test]
fn rate_limit() {
    let akit = Akit::held(r#"[{"module": "rate_limit", "params": {"rate": 0.01, "burst": 3}}]"#);
    for i in 0..3 {
        assert_eq!(get(akit.addr, "/").status, 200, "request {i} of the burst");
    }
    let r = get(akit.addr, "/");
    assert_eq!((r.status, r.header("retry-after")), (429, Some("100")));
    assert_eq!(r.body, "");
}

// fit's TestRateKeys, by header, with fit's status for nginx's limit_req.
#[test]
fn rate_limit_by_header() {
    let akit = Akit::held(
        r#"[{"module": "rate_limit", "params": {"rate": 0.01, "burst": 1, "key": "header:X-Api-Key", "status": 503}}]"#,
    );
    let with =
        |key: &str| exchange(akit.addr, "GET", "/", &[("X-Api-Key", key)], Body::None).status;
    assert_eq!(with("a"), 200);
    assert_eq!(with("a"), 503);
    assert_eq!(with("b"), 200);
    // Without it, by the client's address.
    assert_eq!(get(akit.addr, "/").status, 200);
    assert_eq!(get(akit.addr, "/").status, 503);
}

// fit's TestConcurrency: inline answers give their place back; with the places taken one more
// is refused; one answered later makes room again.
#[test]
fn concurrency_limit() {
    let akit = Akit::held(r#"[{"module": "concurrency_limit", "params": {"max": 2}}]"#);
    for _ in 0..5 {
        assert_eq!(get(akit.addr, "/now").status, 200, "inline");
    }
    // Under the limit, then at it: two kept.
    let path = "/hang/concurrency";
    let clients: Vec<_> = (0..2)
        .map(|_| {
            let addr = akit.addr;
            thread::spawn(move || get(addr, path))
        })
        .collect();
    wait_for("two requests kept", || held::count(path) == 2);
    // Over it.
    let r = get(akit.addr, "/now");
    assert_eq!((r.status, r.body.as_str()), (429, ""));
    // One answered elsewhere: there is room again.
    held::take(path).respond(akio_http::Response::text(200, "later"));
    assert_eq!(get(akit.addr, "/now").status, 200, "after one answered");
    // The other let go unanswered (as a handler does whose client went): room for two.
    drop(held::take(path));
    let hang = (0..2)
        .map(|_| {
            let addr = akit.addr;
            thread::spawn(move || get(addr, "/hang/concurrency2"))
        })
        .collect::<Vec<_>>();
    wait_for("two more kept", || held::count("/hang/concurrency2") == 2);
    assert_eq!(get(akit.addr, "/now").status, 429);
    for _ in 0..2 {
        held::take("/hang/concurrency2").respond(akio_http::Response::text(200, "later"));
    }
    for c in hang {
        assert_eq!(c.join().unwrap().status, 200);
    }
    let answered: Vec<_> = clients.into_iter().map(|c| c.join()).collect();
    assert!(
        answered
            .iter()
            .any(|r| r.as_ref().is_ok_and(|r| r.body == "later"))
    );
    assert_eq!(get(akit.addr, "/now").status, 200);
}

// fit's TestConcurrencyClientGone: a request whose client hung up gives its place back then,
// while its handler still keeps it (akio's `Context::on_end`, ADR-127).
#[test]
fn concurrency_client_gone() {
    let akit =
        Akit::held(r#"[{"module": "concurrency_limit", "params": {"max": 1, "status": 503}}]"#);
    let path = "/hang/gone";
    for round in 0..3 {
        let mut s = TcpStream::connect(akit.addr).unwrap();
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .unwrap();
        wait_for("the request kept", || held::count(path) == 1);
        assert_eq!(
            get(akit.addr, "/ok").status,
            503,
            "round {round}: at the limit"
        );
        drop(s);
        wait_for("the place given back", || {
            get(akit.addr, "/ok").status == 200
        });
        // Answered later, to no one: the place is not given back twice.
        let d = held::take(path);
        d.respond(akio_http::Response::text(200, "to no one"));
    }
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .unwrap();
    wait_for("the request kept", || held::count(path) == 1);
    assert_eq!(get(akit.addr, "/ok").status, 503, "one place, once");
    held::take(path).respond(akio_http::Response::text(200, "done"));
    drop(s);
    wait_for("the place given back", || {
        get(akit.addr, "/ok").status == 200
    });
}

// A streamed response keeps its place until its body ends, not only its head (fit's
// `OnFinish`); one cut short gives it back when its client goes.
#[test]
fn concurrency_streamed_response() {
    let akit =
        Akit::held(r#"[{"module": "concurrency_limit", "params": {"max": 1, "status": 503}}]"#);
    let path = "/stream/one";
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .unwrap();
    let mut got = Vec::new();
    let mut b = [0u8; 1024];
    while !got.windows(4).any(|w| w == b"part") {
        let n = s.read(&mut b).unwrap();
        assert!(n > 0, "closed early");
        got.extend_from_slice(&b[..n]);
    }
    assert_eq!(
        get(akit.addr, "/ok").status,
        503,
        "its head sent, its body not"
    );
    held::take_stream(path).finish();
    wait_for("the place given back", || {
        get(akit.addr, "/ok").status == 200
    });
    // Cut short: its client goes while the body is kept unfinished.
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .unwrap();
    wait_for("the stream kept", || {
        held::STREAMS.lock().unwrap().len() == 1
    });
    assert_eq!(get(akit.addr, "/ok").status, 503);
    drop(s);
    wait_for("the place given back", || {
        get(akit.addr, "/ok").status == 200
    });
    drop(held::take_stream(path));
}

// fit's TestBody: bodies under, at and over max_bytes, one declaring far more than it sends.
#[test]
fn body_limit() {
    let akit = Akit::held(r#"[{"module": "body_limit", "params": {"max_bytes": 5}}]"#);
    for (body, want, what) in [
        (Body::None, 200, "none"),
        (Body::Sized(5), 200, "at the limit"),
        (Body::Sized(6), 413, "over it"),
        (Body::Declared(1000, 76), 413, "a declared length past it"),
        (Body::Chunked(5), 200, "chunked, at the limit"),
        (Body::Chunked(6), 413, "chunked, over it"),
    ] {
        let r = exchange(akit.addr, "POST", "/", &[], body);
        assert_eq!(r.status, want, "{what}");
        if want == 413 {
            // As fit (Go) answers it.
            assert_eq!(r.body, "Request Entity Too Large\n", "{what}");
            assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
        }
    }
    // Past the listener's max_body_bytes too: the listener refuses it first, as akio answers.
    let r = exchange(akit.addr, "POST", "/", &[], Body::Declared(1 << 40, 76));
    assert_eq!(r.status, 413);
    let zero = Akit::held(r#"[{"module": "body_limit", "params": {"max_bytes": 0}}]"#);
    assert_eq!(
        exchange(zero.addr, "POST", "/", &[], Body::None).status,
        200
    );
    assert_eq!(
        exchange(zero.addr, "POST", "/", &[], Body::Sized(1)).status,
        413
    );
}

// fit's TestBodyOverNetwork: bodies sent chunked, with no declared length, small enough to
// arrive whole and large enough to stream, and declared lengths; to a handler that takes the
// body whole and to the proxy, which streams it.
#[test]
fn body_limit_streamed() {
    let mw = r#"[{"module": "body_limit", "params": {"max_bytes": 100000}}]"#;
    let up = counting_upstream();
    let proxied = Akit::proxy(mw, up);
    let whole = Akit::held(mw);
    for akit in [&proxied, &whole] {
        for (body, size, want) in [
            (Body::Chunked(1000), 1000, 200),
            (Body::Chunked(100_000), 100_000, 200),
            (Body::Chunked(100_001), 0, 413),
            (Body::Chunked(1 << 20), 0, 413),
            (Body::Sized(100_000), 100_000, 200),
            (Body::Sized(100_001), 0, 413),
        ] {
            let what = match &body {
                Body::Chunked(n) => format!("{n} bytes chunked"),
                Body::Sized(n) => format!("{n} bytes"),
                _ => String::new(),
            };
            let r = exchange(akit.addr, "POST", "/", &[], body);
            assert_eq!(r.status, want, "{what}");
            if want == 200 {
                assert_eq!(r.body, size.to_string(), "{what}: the handler saw");
            }
        }
    }
    // A limit past the listener's: the listener's holds.
    let wide = Akit::proxy(
        r#"[{"module": "body_limit", "params": {"max_bytes": 1000000000}}]"#,
        up,
    );
    assert_eq!(
        exchange(wide.addr, "POST", "/", &[], Body::Chunked(300_000)).body,
        "300000"
    );
}

/// What a 413 refusing a body that streams does to its HTTP/1.1 connection, as fit's: the rest
/// of the body is not read, and the connection closes after the answer. Through the proxy
/// (whose body streams), a body that declares a length past the limit and one that grows past
/// it (refused by akio's `Context::limit_body`, answered by the proxy).
#[test]
fn body_limit_closes_http1() {
    let mw = r#"[{"module": "body_limit", "params": {"max_bytes": 100000}}]"#;
    let proxied = Akit::proxy(mw, counting_upstream());
    for (framing, what) in [
        ("Content-Length: 1000000", "declared"),
        ("Transfer-Encoding: chunked", "growing"),
    ] {
        let s = TcpStream::connect(proxied.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut w = s.try_clone().unwrap();
        let chunked = framing.starts_with("Transfer");
        let writer = thread::spawn(move || {
            w.write_all(format!("POST / HTTP/1.1\r\nHost: x\r\n{framing}\r\n\r\n").as_bytes())?;
            // 200 KiB, past the limit, in 16 KiB pieces: what the server does not read waits.
            for _ in 0..(200 << 10) / (16 << 10) {
                if chunked {
                    w.write_all(format!("{:x}\r\n", 16 << 10).as_bytes())?;
                }
                w.write_all(&[b'x'; 16 << 10])?;
                if chunked {
                    w.write_all(b"\r\n")?;
                }
            }
            Ok::<(), std::io::Error>(())
        });
        let mut r = BufReader::new(s);
        let resp = read_response(&mut r);
        assert_eq!(resp.status, 413, "{what}");
        // fit's words: the middleware's for a declared length, the proxy's for one that grew.
        assert!(
            resp.body.contains("Request Entity Too Large"),
            "{what}: {}",
            resp.body
        );
        // Closed, and not after reading the rest: the read ends (or is reset), it does not
        // time out.
        let start = Instant::now();
        let mut rest = Vec::new();
        let _ = r.read_to_end(&mut rest);
        assert!(rest.is_empty(), "{what}: more after the answer");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{what}: not closed"
        );
        let _ = writer.join();
    }
}

/// The client engine of the h2c requests.
fn clients() -> Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        let e = Engine::builder().client_threads(2).build().unwrap();
        e.start().unwrap();
        Arc::new(e)
    })
    .clone()
}

/// A POST over h2c (prior knowledge) of `size` bytes, declaring `length` when given, written
/// in pieces after its head: the status and the body the server answered with.
fn h2_post(addr: SocketAddr, length: Option<u64>, size: usize) -> (u16, String) {
    let c = H2Client::dial_blocking(&clients(), addr, None, H2Options::default()).unwrap();
    let mut req = ClientRequest::new(Method::Post, &format!("http://{addr}/"));
    if let Some(n) = length {
        req.headers.append("content-length", n.to_string());
    }
    let (tx, rx) = channel();
    let (mut status, mut got) = (0, Vec::new());
    let up = c.open(req, move |e| match e {
        H2Event::Head { status: s, .. } => status = s,
        H2Event::Data(d) => got.extend_from_slice(d),
        H2Event::Trailers(_) => {}
        H2Event::End | H2Event::Error(_) => {
            let _ = tx.send((status, std::mem::take(&mut got)));
        }
    });
    let piece = vec![b'x'; 16 << 10];
    let mut left = size;
    while left > 0 {
        let n = left.min(piece.len());
        // Refused, the stream takes no more.
        if !up.write(&piece[..n]) {
            break;
        }
        left -= n;
    }
    up.finish();
    let (status, body) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    c.close();
    (status, String::from_utf8(body).unwrap())
}

/// fit's `TestBodyOverNetwork` over h2c: bodies under, at and over the limit, of no declared
/// length (growing past it) and declaring theirs, to a handler that takes the body whole and to
/// the proxy, which streams it.
#[test]
fn body_limit_h2() {
    let mw = r#"[{"module": "body_limit", "params": {"max_bytes": 100000}}]"#;
    let up = counting_upstream();
    let proxied = Akit::proxy(mw, up);
    let whole = Akit::held(mw);
    for (akit, name) in [(&proxied, "proxy"), (&whole, "whole")] {
        for (length, size, want) in [
            (None, 1000, 200),
            (None, 100_000, 200),
            (None, 100_001, 413),
            (None, 1 << 20, 413),
            (Some(100_000), 100_000, 200),
            (Some(100_001), 100_001, 413),
            (Some(1 << 20), 1000, 413),
        ] {
            let what = format!("{name}: {size} bytes, length {length:?}");
            let (status, body) = h2_post(akit.addr, length, size);
            assert_eq!(status, want, "{what}");
            if want == 200 {
                assert_eq!(body, size.to_string(), "{what}: the handler saw");
            } else {
                assert!(body.contains("Request Entity Too Large"), "{what}: {body}");
            }
        }
    }
}

// fit's TestParamErrors.
#[test]
fn param_errors() {
    for (name, params, want) in [
        ("rate_limit", r#"{"rate": 0}"#, "0 is below 0.001"),
        ("rate_limit", r#"{}"#, "rate: missing required field"),
        (
            "rate_limit",
            r#"{"rate": 1, "key": "cookie:a"}"#,
            r#""cookie:a" is not ip, route or header:Name"#,
        ),
        (
            "rate_limit",
            r#"{"rate": 1, "key": "header:"}"#,
            r#""header:" is not ip, route or header:Name"#,
        ),
        (
            "rate_limit",
            r#"{"rate": 1, "key": "header:a b"}"#,
            r#""header:a b" is not ip, route or header:Name"#,
        ),
        (
            "rate_limit",
            r#"{"rate": 1, "trusted_proxies": ["x"]}"#,
            r#""x" is not an IP address or a CIDR prefix"#,
        ),
        (
            "rate_limit",
            r#"{"rate": 1, "status": 200}"#,
            "200 is not an error status",
        ),
        ("rate_limit", r#"{"rate": 1, "burst": -1}"#, "burst"),
        (
            "rate_limit",
            r#"{"rate": 1, "ipv6_prefix": 129}"#,
            "ipv6_prefix",
        ),
        ("rate_limit", r#"{"rate": 1, "max_keys": -1}"#, "max_keys"),
        ("rate_limit", r#"{"rate": "1"}"#, "rate: expected a number"),
        (
            "rate_limit",
            r#"{"rate": 1, "rates": 2}"#,
            "rates: unknown field",
        ),
        ("concurrency_limit", r#"{}"#, "max"),
        ("concurrency_limit", r#"{"max": 0}"#, "max"),
        (
            "concurrency_limit",
            r#"{"max": 1, "status": 302}"#,
            "302 is not an error status",
        ),
        ("body_limit", r#"{}"#, "max_bytes"),
        ("body_limit", r#"{"max_bytes": -1}"#, "max_bytes"),
    ] {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
                "routes": [{{"middleware": [{{"module": "{name}", "params": {params}}}],
                             "handler": {{"module": "respond"}}}}]}}"#
        );
        let errs = match akit_config::from_json(&json, None) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        };
        assert!(
            errs.contains(want),
            "{name} {params}: {errs:?}, want {want:?}"
        );
    }
    // Without params, the required field is named.
    for (name, field) in [
        ("rate_limit", "rate"),
        ("concurrency_limit", "max"),
        ("body_limit", "max_bytes"),
    ] {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
                "routes": [{{"middleware": [{{"module": "{name}"}}], "handler": {{"module": "respond"}}}}]}}"#
        );
        let errs = akit_config::from_json(&json, None).unwrap_err().to_string();
        assert!(
            errs.contains(&format!("params.{field}: missing required field")),
            "{name}: {errs}"
        );
    }
}

// The params as akit reads them: fit's defaults for what is left out.
#[test]
fn params_read() {
    let read = |params: &str| {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}],
                "routes": [{{"middleware": [{{"module": "rate_limit", "params": {params}}}], "handler": {{"module": "respond"}}}}]}}"#
        );
        let c = akit_config::from_json(&json, None).unwrap();
        match c.routes[0].middleware[0].clone() {
            akit_config::Handler::RateLimit(r) => r,
            h => panic!("{h:?}"),
        }
    };
    let r = read(r#"{"rate": 2.5}"#);
    assert_eq!(
        (r.burst, r.key, r.ipv6_prefix, r.max_keys, r.status),
        (3, akit_config::RateKey::Ip, 64, 100_000, 429)
    );
    let r = read(
        r#"{"rate": 0.5, "burst": 0, "key": "route", "ipv6_prefix": 0, "max_keys": 0, "status": 0}"#,
    );
    assert_eq!(
        (r.burst, r.key, r.ipv6_prefix, r.max_keys, r.status),
        (1, akit_config::RateKey::Route, 64, 100_000, 429)
    );
    let r = read(r#"{"rate": 1, "key": "header:x-api-key", "ipv6_prefix": 128}"#);
    assert_eq!(
        (r.key, r.ipv6_prefix),
        (akit_config::RateKey::Header("x-api-key".into()), 128)
    );
}
