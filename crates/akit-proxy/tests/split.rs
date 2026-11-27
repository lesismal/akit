//! fit's split tests for its proxy (`handler/proxy/split_test.go`), ported for I46: requests
//! spread over `upstreams` by weight; a `failover` that takes them while the chosen upstream has
//! no address in rotation; a `mirror` that gets a copy of the requests whose bodies arrive whole,
//! a share of them by `percent`, never slows them, and is held to [`akit_config::MAX_MIRRORED`]
//! copies under way; and the upstreams they name checked when the handler is provisioned. On the
//! backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
//!
//! The upstreams are plain threads on std sockets.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

/// fit's `splitConfig`: upstreams `a` (out of rotation for a minute after one failure), `b` and
/// `c` at the addresses given, and a route proxying with `params`. `c`, the mirror, has 32
/// connections at most: copies past them wait in its pool, which keeps a held mirror's sockets
/// within a test host's 1024 descriptors.
fn config(params: &str, a: SocketAddr, b: SocketAddr, c: SocketAddr) -> String {
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "upstreams": [{{"name": "a", "addresses": ["{a}"], "max_fails": 1, "fail_timeout": "1m"}},
                {{"name": "b", "addresses": ["{b}"]}},
                {{"name": "c", "addresses": ["{c}"], "max_conns": 32}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {params}}}}}]
}}"#
    )
}

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

/// akit serving `json`, as `akit run` does.
struct Akit {
    engine: Arc<Engine>,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Akit {
    fn start(json: &str) -> Akit {
        let config = akit_config::from_json(json, None).unwrap();
        let registry = Registry::<HandlerParams>::new(&[akit_proxy::MODULES]).unwrap();
        let slot = EngineSlot::new();
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
}

impl Drop for Akit {
    fn drop(&mut self) {
        akit_listener::shutdown(&self.engine, Duration::from_secs(5));
    }
}

fn read_head(r: &mut impl BufRead) -> Option<String> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        head.push_str(&line);
        if line == "\r\n" {
            return Some(head);
        }
    }
}

/// A head's field `name`, in any case.
fn field<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

/// Reads a request's body as its head frames it (by length, or chunked); false when the
/// connection ended first.
fn read_request_body(r: &mut impl BufRead, head: &str) -> bool {
    if let Some(n) = field(head, "content-length").and_then(|v| v.parse::<u64>().ok()) {
        return std::io::copy(&mut r.take(n), &mut std::io::sink()).ok() == Some(n);
    }
    if !field(head, "transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        return true;
    }
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return false;
        }
        let Ok(n) = u64::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16) else {
            return false;
        };
        if n == 0 {
            // The trailers, to the empty line.
            return read_head(r).is_some();
        }
        if std::io::copy(&mut r.take(n), &mut std::io::sink()).ok() != Some(n) {
            return false;
        }
        let mut crlf = String::new();
        if r.read_line(&mut crlf).unwrap_or(0) == 0 {
            return false;
        }
    }
}

/// Serves each connection accepted on `l` with `f`, on a thread of its own.
fn serve(l: TcpListener, f: impl Fn(TcpStream) + Send + Sync + 'static) {
    let f = Arc::new(f);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let f = f.clone();
            thread::spawn(move || f(s));
        }
    });
}

/// fit's `countingServer`: answers every request with `status` and `body`, after `delay`,
/// counting them.
fn counting(status: u16, body: &'static str, delay: Duration) -> (SocketAddr, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    serve(l, move |s| {
        let mut w = s.try_clone().unwrap();
        let mut r = BufReader::new(s);
        while let Some(head) = read_head(&mut r) {
            if !read_request_body(&mut r, &head) {
                return;
            }
            h.fetch_add(1, Ordering::SeqCst);
            thread::sleep(delay);
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if w.write_all(reply.as_bytes()).is_err() {
                return;
            }
        }
    });
    (addr, hits)
}

/// fit's `refused`: an address nothing listens on.
fn refused() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

/// A response: its status, head and body (whole, or dechunked when short).
struct Got {
    status: u16,
    body: String,
}

/// fit's `do`: an HTTP/1.1 request of `method` with `body` (sent by its length), read in full.
fn send(addr: SocketAddr, method: &str, body: &[u8], fields: &str) -> Got {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "{method} / HTTP/1.1\r\nHost: x\r\n{fields}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    // The upstream may have answered (or akit refused) before the body is all sent.
    let _ = s.write_all(body);
    let mut text = Vec::new();
    let _ = BufReader::new(s).read_to_end(&mut text);
    let text = String::from_utf8_lossy(&text).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("no response head");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let body = if field(head, "transfer-encoding").is_some() {
        body.split("\r\n").nth(1).unwrap_or("").to_owned()
    } else {
        body.to_owned()
    };
    Got { status, body }
}

/// A GET, its status and body.
fn get(addr: SocketAddr) -> (u16, String) {
    let got = send(addr, "GET", b"", "");
    (got.status, got.body)
}

/// fit's TestSplitWeights: 3 to 1 over 800 requests.
#[test]
fn weights() {
    let (a, a_hits) = counting(200, "a", Duration::ZERO);
    let (b, b_hits) = counting(200, "b", Duration::ZERO);
    let (c, _) = counting(200, "c", Duration::ZERO);
    let akit = Akit::start(&config(
        r#"{"upstreams": [{"upstream": "a", "weight": 3}, {"upstream": "b", "weight": 1}]}"#,
        a,
        b,
        c,
    ));
    const N: usize = 800;
    for _ in 0..N {
        assert_eq!(get(akit.addr).0, 200);
    }
    // 600 and 200 expected; the bounds are about five deviations out.
    let (ah, bh) = (a_hits.load(Ordering::SeqCst), b_hits.load(Ordering::SeqCst));
    assert!(
        ah + bh == N && (540..=660).contains(&ah),
        "a {ah}, b {bh} of {N}"
    );
}

/// fit's TestSplitFailover: the first request fails and takes a's one address out of rotation
/// for a minute (max_fails 1); the rest go to the failover.
#[test]
fn failover() {
    let (b, b_hits) = counting(200, "backup", Duration::ZERO);
    let (c, _) = counting(200, "c", Duration::ZERO);
    let akit = Akit::start(&config(
        r#"{"upstream": "a", "failover": "b"}"#,
        refused(),
        b,
        c,
    ));
    assert_eq!(get(akit.addr).0, 502, "first");
    for i in 0..5 {
        assert_eq!(get(akit.addr), (200, "backup".to_owned()), "{i}");
    }
    assert_eq!(b_hits.load(Ordering::SeqCst), 5, "the failover's");
}

/// The failover stands in for an upstream of `upstreams` too, and only for it.
#[test]
fn failover_of_a_split() {
    let (b, b_hits) = counting(200, "b", Duration::ZERO);
    let (c, c_hits) = counting(200, "c", Duration::ZERO);
    let akit = Akit::start(&config(
        r#"{"upstreams": [{"upstream": "a", "weight": 1}, {"upstream": "b", "weight": 1}],
            "failover": "c"}"#,
        refused(),
        b,
        c,
    ));
    // Until a has failed once, about half go to it and fail.
    let mut failed = 0;
    while failed == 0 {
        if get(akit.addr).0 == 502 {
            failed += 1;
        }
    }
    let (b0, c0) = (b_hits.load(Ordering::SeqCst), c_hits.load(Ordering::SeqCst));
    for i in 0..200 {
        assert_eq!(get(akit.addr).0, 200, "{i}");
    }
    let (bh, ch) = (
        b_hits.load(Ordering::SeqCst) - b0,
        c_hits.load(Ordering::SeqCst) - c0,
    );
    assert_eq!(bh + ch, 200);
    assert!((60..=140).contains(&ch), "b {bh}, c {ch}");
}

/// fit's `mirrorServer`: records the requests it gets (method, target, `X-Test` and body); a
/// held one answers none of them until released.
struct MirrorServer {
    addr: SocketAddr,
    got: Arc<Mutex<Vec<String>>>,
    count: Arc<AtomicUsize>,
    released: Arc<(Mutex<bool>, Condvar)>,
}

impl MirrorServer {
    fn start(hold: bool) -> MirrorServer {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let got = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let (g, n, r) = (got.clone(), count.clone(), released.clone());
        serve(l, move |s| {
            let mut w = s.try_clone().unwrap();
            let mut rd = BufReader::new(s);
            while let Some(head) = read_head(&mut rd) {
                let n_body = field(&head, "content-length")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let mut body = Vec::new();
                if (&mut rd).take(n_body).read_to_end(&mut body).is_err() {
                    return;
                }
                let line = head.lines().next().unwrap_or("");
                let mut parts = line.split(' ');
                let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                g.lock().unwrap().push(format!(
                    "{method} {target} {} {}",
                    field(&head, "x-test").unwrap_or(""),
                    String::from_utf8_lossy(&body)
                ));
                n.fetch_add(1, Ordering::SeqCst);
                if hold {
                    let (m, cv) = &*r;
                    let mut done = m.lock().unwrap();
                    while !*done {
                        done = cv.wait(done).unwrap();
                    }
                }
                if w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nmirror")
                    .is_err()
                {
                    return;
                }
            }
        });
        MirrorServer {
            addr,
            got,
            count,
            released,
        }
    }

    fn release(&self) {
        let (m, cv) = &*self.released;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Waits up to 5 seconds for the count to stop growing (and be at least `least`).
    fn settle(&self, least: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last = usize::MAX;
        while Instant::now() < deadline {
            let n = self.count.load(Ordering::SeqCst);
            if n >= least && n == last {
                break;
            }
            last = n;
            thread::sleep(Duration::from_millis(200));
        }
        self.count.load(Ordering::SeqCst)
    }
}

impl Drop for MirrorServer {
    fn drop(&mut self) {
        self.release();
    }
}

/// fit's TestMirror: the request is answered by its upstream, and its copy reaches the mirror
/// with its method, target, fields and body.
#[test]
fn mirror() {
    let (a, a_hits) = counting(200, "main", Duration::ZERO);
    let m = MirrorServer::start(false);
    let akit = Akit::start(&config(
        r#"{"upstream": "a", "mirror": {"upstream": "c"}}"#,
        a,
        refused(),
        m.addr,
    ));
    let mut s = TcpStream::connect(akit.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(
        b"POST /p?q=1 HTTP/1.1\r\nHost: x\r\nX-Test: t\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
    )
    .unwrap();
    let mut text = String::new();
    BufReader::new(s).read_to_string(&mut text).unwrap();
    assert!(
        text.starts_with("HTTP/1.1 200") && text.ends_with("\r\n\r\nmain"),
        "{text}"
    );
    assert_eq!(a_hits.load(Ordering::SeqCst), 1);
    m.settle(1);
    assert_eq!(*m.got.lock().unwrap(), ["POST /p?q=1 t payload"]);
}

/// A request whose body streams (past 64 KiB) is not copied: only its upstream reads it.
#[test]
fn streamed_body_is_not_mirrored() {
    let (a, a_hits) = counting(200, "main", Duration::ZERO);
    let m = MirrorServer::start(false);
    let akit = Akit::start(&config(
        r#"{"upstream": "a", "mirror": {"upstream": "c"}}"#,
        a,
        refused(),
        m.addr,
    ));
    let got = send(akit.addr, "POST", &vec![b'x'; 100 << 10], "");
    assert_eq!((got.status, got.body.as_str()), (200, "main"));
    // A small one after it is copied: the mirror has had its chance at the first by then.
    assert_eq!(send(akit.addr, "POST", b"small", "").status, 200);
    m.settle(1);
    assert_eq!(a_hits.load(Ordering::SeqCst), 2);
    assert_eq!(*m.got.lock().unwrap(), ["POST /  small"]);
}

/// fit's TestMirrorDoesNotSlow: requests through a mirror that answers none of its copies while
/// they are under way are answered all the same, and the copies stop at the bound.
#[test]
fn mirror_does_not_slow() {
    let (a, _) = counting(200, "main", Duration::ZERO);
    let m = MirrorServer::start(true);
    let akit = Akit::start(&config(
        r#"{"upstream": "a", "mirror": {"upstream": "c"}}"#,
        a,
        refused(),
        m.addr,
    ));
    // More requests than the bound, 32 at a time (macOS resets connects past its listen queue).
    // The copies outlive their requests, so they pile up to the bound all the same.
    let next = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..32)
        .map(|_| {
            let next = next.clone();
            let addr = akit.addr;
            thread::spawn(move || {
                while next.fetch_add(1, Ordering::SeqCst) < 400 {
                    assert_eq!(get(addr).0, 200);
                }
            })
        })
        .collect();
    // Every request answered while the mirror holds every copy it got: none waited for its
    // copy, or its client would have timed out.
    for w in workers {
        w.join().unwrap();
    }
    // The mirror has the copies its 32 connections carry; the rest wait in its pool, under way
    // all the same. None has ended, so no slot was given back: once released, the mirror gets
    // exactly the bound's copies, and none of the other 144 requests'.
    let held = m.settle(1);
    assert!((1..=32).contains(&held), "the mirror holds {held} copies");
    m.release();
    let n = m.settle(akit_config::MAX_MIRRORED);
    assert_eq!(
        n,
        akit_config::MAX_MIRRORED,
        "the mirror got {n} copies of 400 requests"
    );
}

/// fit's TestMirrorPercent: about a quarter of 400 copied.
#[test]
fn mirror_percent() {
    let (a, _) = counting(200, "main", Duration::ZERO);
    let m = MirrorServer::start(false);
    let akit = Akit::start(&config(
        r#"{"upstream": "a", "mirror": {"upstream": "c", "percent": 25}}"#,
        a,
        refused(),
        m.addr,
    ));
    for _ in 0..400 {
        assert_eq!(get(akit.addr).0, 200);
    }
    let n = m.settle(1);
    assert!((60..=140).contains(&n), "{n} of 400 copied, want about 100");
}

/// A mirror that cannot be reached costs the requests nothing.
#[test]
fn mirror_refused() {
    let (a, a_hits) = counting(200, "main", Duration::ZERO);
    let (b, _) = counting(200, "b", Duration::ZERO);
    let akit = Akit::start(&config(
        r#"{"upstream": "a", "mirror": {"upstream": "c"}}"#,
        a,
        b,
        refused(),
    ));
    for _ in 0..20 {
        assert_eq!(get(akit.addr), (200, "main".to_owned()));
    }
    assert_eq!(a_hits.load(Ordering::SeqCst), 20);
}

/// fit's TestSplitParams, for what is checked once the upstreams are known: the names.
#[test]
fn params() {
    let registry = Registry::<HandlerParams>::new(&[akit_proxy::MODULES]).unwrap();
    let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
    for (params, want) in [
        (
            r#"{"upstreams": [{"upstream": "a", "weight": 1}, {"upstream": "nope", "weight": 1}]}"#,
            r#"routes[0].handler.params.upstreams[1].upstream: there is no upstream named "nope""#,
        ),
        (
            r#"{"upstream": "a", "failover": "nope"}"#,
            r#"routes[0].handler.params.failover: there is no upstream named "nope""#,
        ),
        (
            r#"{"upstream": "a", "mirror": {"upstream": "nope"}}"#,
            r#"routes[0].handler.params.mirror.upstream: there is no upstream named "nope""#,
        ),
        (
            r#"{"upstream": "nope"}"#,
            r#"routes[0].handler.params.upstream: there is no upstream named "nope""#,
        ),
    ] {
        let c = akit_config::from_json(&config(params, addr, addr, addr), None).unwrap();
        let e = compile(&c, &registry, &Env::new(EngineSlot::new()))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains(want), "{params}: {e}");
    }
}
