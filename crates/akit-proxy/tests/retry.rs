//! fit's retry tests for its proxy (`handler/proxy/retry_test.go`), ported for I45: a request
//! whose connection could not be made goes to the next address whatever its method; one whose
//! request reached the upstream is sent again only when its method is idempotent, up to
//! `attempts`; `statuses` retry idempotent requests only; retries stop at the budget; a body that
//! streams is never sent again; the route's `timeout` bounds the attempts together; and the
//! sticky cookie names the address that answered. On the backend `AKIO_TEST_BACKEND` names
//! (epoll by default, `uring`).
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
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, UringConfig};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

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

/// fit's `retryConfig`: the upstream `app` at `addrs` with the extra `fields`, and a route to the
/// proxy with the extra `params`.
fn config(addrs: &[SocketAddr], fields: &str, params: &str) -> String {
    let addrs: Vec<String> = addrs.iter().map(|a| format!("\"{a}\"")).collect();
    format!(
        r#"{{
  "listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
  "upstreams": [{{"name": "app", "addresses": [{}]{fields}}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"{params}}}}}}}]
}}"#,
        addrs.join(", ")
    )
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

/// fit's `resetter`: reads each request whole, then closes its connection without an answer,
/// counting the requests.
fn resetter() -> (SocketAddr, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    serve(l, move |s| {
        let mut r = BufReader::new(s);
        let Some(head) = read_head(&mut r) else {
            return;
        };
        if read_request_body(&mut r, &head) {
            h.fetch_add(1, Ordering::SeqCst);
        }
    });
    (addr, hits)
}

/// A response: its status, head and body (whole, or dechunked when short).
struct Got {
    status: u16,
    head: String,
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
    Got {
        status,
        head: head.to_owned(),
        body,
    }
}

/// fit's TestRetryConnectFailure: a request whose connection could not be made goes to the next
/// address, whatever its method.
#[test]
fn connect_failure() {
    let (ok, hits) = counting(200, "ok", Duration::ZERO);
    // Every request needs its retry: the budget allows one each.
    let akit = Akit::start(&config(
        &[refused(), ok],
        r#", "max_fails": 0"#,
        r#", "retry": {"budget_ratio": 1}"#,
    ));
    for i in 0..10 {
        for method in ["GET", "POST"] {
            let got = send(akit.addr, method, b"payload", "");
            assert_eq!((got.status, got.body.as_str()), (200, "ok"), "{method} {i}");
        }
    }
    assert_eq!(hits.load(Ordering::SeqCst), 20);
}

/// fit's TestRetryNotResentOnceSent: a request of a method that is not idempotent, whose body
/// reached the upstream, is not sent again; an idempotent one is, up to `attempts`.
#[test]
fn not_resent_once_sent() {
    let (addr, hits) = resetter();
    let akit = Akit::start(&config(
        &[addr],
        r#", "max_fails": 0"#,
        r#", "retry": {"attempts": 3}"#,
    ));
    assert_eq!(send(akit.addr, "POST", b"charge the card", "").status, 502);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the POST was sent again");
    hits.store(0, Ordering::SeqCst);
    assert_eq!(
        send(akit.addr, "PUT", b"the same each time", "").status,
        502
    );
    // The first attempt and three more.
    assert_eq!(hits.load(Ordering::SeqCst), 4);
}

/// I45's "a body that cannot be replayed": one that streams (past 64 KiB) is sent once, its
/// method idempotent or not, its connection failing after it was sent.
#[test]
fn streamed_body_is_not_replayed() {
    let (addr, hits) = resetter();
    let akit = Akit::start(&config(
        &[addr],
        r#", "max_fails": 0"#,
        r#", "retry": {"attempts": 3}"#,
    ));
    let body = vec![b'x'; 200 << 10];
    assert_eq!(send(akit.addr, "PUT", &body, "").status, 502);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the streamed PUT was sent again"
    );
}

/// fit's TestRetryStatuses: `statuses` retry idempotent requests only, on the next address.
#[test]
fn statuses() {
    let (bad, bad_hits) = counting(503, "busy", Duration::ZERO);
    let (good, good_hits) = counting(200, "ok", Duration::ZERO);
    let akit = Akit::start(&config(
        &[bad, good],
        "",
        r#", "retry": {"statuses": [503]}"#,
    ));
    for i in 0..10 {
        let got = send(akit.addr, "GET", b"", "");
        assert_eq!((got.status, got.body.as_str()), (200, "ok"), "GET {i}");
    }
    let (b, g) = (
        bad_hits.load(Ordering::SeqCst),
        good_hits.load(Ordering::SeqCst),
    );
    // Every GET ends at the 200 address, and some went to the 503 one first and were sent
    // again. How many did depends on the balancer's turns, one per CPU slot (I36), so on where
    // the I/O threads run: at least one, not a share.
    assert!(g == 10 && b >= 1, "503 address {b}, 200 address {g}");
    bad_hits.store(0, Ordering::SeqCst);
    let mut n503 = 0;
    for _ in 0..10 {
        if send(akit.addr, "POST", b"x", "").status == 503 {
            n503 += 1;
        }
    }
    let b = bad_hits.load(Ordering::SeqCst);
    assert!(
        b == n503 && n503 > 0,
        "POST: {n503} answered 503, the 503 address saw {b}"
    );
}

/// fit's TestRetryBudget: retries stop at the budget, a fifth of the requests, with none a
/// second besides.
#[test]
fn budget() {
    let (bad, hits) = counting(503, "busy", Duration::ZERO);
    let akit = Akit::start(&config(
        &[bad],
        "",
        r#", "retry": {"statuses": [503], "attempts": 2, "budget_ratio": 0.2, "budget_min_per_second": 0}"#,
    ));
    const REQUESTS: usize = 50;
    for _ in 0..REQUESTS {
        assert_eq!(send(akit.addr, "GET", b"", "").status, 503);
    }
    let retries = hits.load(Ordering::SeqCst) - REQUESTS;
    assert!(
        (5..=10).contains(&retries),
        "{retries} retries for {REQUESTS} requests, want at most 10"
    );
}

/// The route's `timeout` bounds the attempts together: an attempt answered 503 after 300ms is
/// sent again with what is left of 500ms, which runs out before the second answer: 504, soon
/// after the timeout, and no third attempt.
#[test]
fn route_timeout_spans_the_attempts() {
    let (slow, hits) = counting(503, "busy", Duration::from_millis(300));
    let akit = Akit::start(&config(
        &[slow],
        "",
        r#", "timeout": "500ms", "retry": {"statuses": [503], "attempts": 5}"#,
    ));
    let start = Instant::now();
    let got = send(akit.addr, "GET", b"", "");
    let took = start.elapsed();
    assert_eq!(got.status, 504, "{}", got.head);
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(2),
        "{took:?}"
    );
    // Give a third attempt, were there one, time to arrive.
    thread::sleep(Duration::from_millis(400));
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

/// A request sent again goes with the sticky cookie of the address that answered it, as fit's
/// retry sets it again: the cookie it gets names the live address, so that the requests that
/// send it go there with no retry.
#[test]
fn sticky_cookie_names_the_address_that_answered() {
    let (ok, hits) = counting(200, "ok", Duration::ZERO);
    let akit = Akit::start(&config(
        &[refused(), ok],
        r#", "max_fails": 0, "sticky_cookie": {}"#,
        r#", "retry": {"budget_ratio": 1}"#,
    ));
    // Each in turn: one of the two goes to the refused address first.
    let mut cookie = None;
    for _ in 0..2 {
        let got = send(akit.addr, "GET", b"", "");
        assert_eq!(got.status, 200, "{}", got.head);
        let set = field(&got.head, "set-cookie").expect("no sticky cookie");
        let c = set.split(';').next().unwrap().to_owned();
        assert!(cookie.as_ref().is_none_or(|p| *p == c), "{cookie:?} {c}");
        cookie = Some(c);
    }
    let cookie = cookie.unwrap();
    for _ in 0..6 {
        let got = send(akit.addr, "GET", b"", &format!("Cookie: {cookie}\r\n"));
        assert_eq!(got.status, 200, "{}", got.head);
        assert!(field(&got.head, "set-cookie").is_none(), "{}", got.head);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 8);
}

/// Without `retry`, a request whose connection could not be made is answered 502.
#[test]
fn no_retry_without_the_param() {
    let akit = Akit::start(&config(&[refused()], r#", "max_fails": 0"#, ""));
    assert_eq!(send(akit.addr, "GET", b"", "").status, 502);
}
